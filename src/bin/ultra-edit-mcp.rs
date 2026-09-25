use std::collections::HashSet;
use std::env;
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, PoisonError};

use futures::{
    SinkExt,
    future::{BoxFuture, Either, select},
    lock::Mutex,
};
use rmcp::{
    RoleServer, ServiceExt,
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::Transport,
};
use tokio::sync::{Notify, mpsc};
use tokio_util::{
    codec::{FramedWrite, LinesCodec},
    sync::CancellationToken,
};
use ultra_edit::{Workspace, mcp::McpServer, shell_guard};

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const INVALID_TOOL_CALL: &str =
    "Invalid params: tools/call requires {name, arguments?} with object arguments";
const CLAUDE_CONTEXT: &str = include_str!("../../plugin/claude-code/instructions.md");

fn main() -> ExitCode {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() == 1 && (arguments[0] == "--help" || arguments[0] == "-h") {
        println!(
            "ultra-edit-mcp {}\nUsage: ultra-edit-mcp --root WORKSPACE\n\
             Or: ultra-edit-mcp --claude-context SessionStart|SubagentStart\n\
             Or: ultra-edit-mcp --claude-hook PreToolUse\n\n\
             Serve MCP over stdio inside one fixed existing workspace.\n\
             JSON-RPC lines are limited to 16 MiB. Protocol output uses stdout; errors use stderr.\n\
             Cancellation or disconnection does not imply rollback; query receipts before retrying.\n\
             --claude-context prints plugin hook JSON and exits without opening a workspace.\n\
             --claude-hook PreToolUse reads Claude Code hook JSON from stdin and denies Bash\n\
             commands that write file content through the shell (heredoc or echo redirection,\n\
             inline interpreter writes, sed -i). It allows anything it cannot parse.\n\
             {}=allow disables the guard.",
            env!("CARGO_PKG_VERSION"),
            shell_guard::ESCAPE_HATCH
        );
        return ExitCode::SUCCESS;
    }
    if arguments.len() == 1 && arguments[0] == "--version" {
        println!("ultra-edit-mcp {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if arguments.len() == 2 && arguments[0] == "--claude-context" {
        let event = match arguments[1].to_str() {
            Some(event @ ("SessionStart" | "SubagentStart")) => event,
            _ => {
                eprintln!("--claude-context requires SessionStart or SubagentStart");
                return ExitCode::from(2);
            }
        };
        let output = serde_json::json!({"hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": CLAUDE_CONTEXT,
        }});
        let mut stdout = io::stdout().lock();
        if serde_json::to_writer(&mut stdout, &output).is_err() || writeln!(stdout).is_err() {
            eprintln!("Could not write Claude Code hook context");
            return ExitCode::from(2);
        }
        return ExitCode::SUCCESS;
    }
    if arguments.len() == 2 && arguments[0] == "--claude-hook" {
        if arguments[1] != "PreToolUse" {
            eprintln!("--claude-hook requires PreToolUse");
            return ExitCode::from(2);
        }
        guard_bash_writes();
        return ExitCode::SUCCESS;
    }
    if arguments.len() != 2 || arguments[0] != "--root" {
        eprintln!(
            "Usage: ultra-edit-mcp --root WORKSPACE (or --claude-context EVENT / --claude-hook EVENT / --help / --version)"
        );
        return ExitCode::from(2);
    }
    let workspace = match Workspace::open(PathBuf::from(&arguments[1])) {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Could not start MCP runtime: {error}");
            return ExitCode::from(2);
        }
    };
    match runtime.block_on(serve(workspace)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("MCP server error: {error}");
            ExitCode::from(2)
        }
    }
}

/// Answers a `PreToolUse` hook: prints a deny decision for a Bash command that
/// writes file content through the shell, and nothing otherwise. Every failure
/// allows the call, because a broken guard must never block a session.
fn guard_bash_writes() {
    let mut input = Vec::new();
    // Read before deciding so the host never writes into a closed pipe.
    let limit = MAX_MESSAGE_BYTES as u64 + 1;
    let read = io::stdin().lock().take(limit).read_to_end(&mut input);
    if read.is_err()
        || input.len() > MAX_MESSAGE_BYTES
        || env::var_os(shell_guard::ESCAPE_HATCH).is_some_and(|value| value == "allow")
    {
        return;
    }
    let Ok(event) = serde_json::from_slice::<serde_json::Value>(&input) else {
        return;
    };
    if event["tool_name"] != "Bash" {
        return;
    }
    let Some(command) = event["tool_input"]["command"].as_str() else {
        return;
    };
    // A classifier bug must not turn into a hook error on every Bash call.
    let Some(finding) = std::panic::catch_unwind(|| shell_guard::classify(command))
        .ok()
        .flatten()
    else {
        return;
    };
    let output = serde_json::json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": shell_guard::deny_reason(&finding),
    }});
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &output).is_ok() {
        let _ = writeln!(stdout);
    }
}

async fn serve(workspace: Workspace) -> Result<(), Box<dyn std::error::Error>> {
    let failed = CancellationToken::new();
    let transport = StdioTransport {
        input: stdin_messages()?,
        pending_error: None,
        ended: None,
        initialized: false,
        output: Arc::new(ProtocolOutput {
            writer: Mutex::new(FramedWrite::new(tokio::io::stdout(), LinesCodec::new())),
            failed: failed.clone(),
            in_flight: std::sync::Mutex::new(InFlight::default()),
            answered: Notify::new(),
        }),
    };
    McpServer::new(workspace)
        .serve(transport)
        .await?
        .waiting()
        .await?;
    if failed.is_cancelled() {
        return Err(io::Error::other("MCP transport failed; see stderr").into());
    }
    Ok(())
}

struct ProtocolOutput {
    writer: Mutex<FramedWrite<tokio::io::Stdout, LinesCodec>>,
    failed: CancellationToken,
    // Request IDs the SDK still owes a reply for: a second request reusing one
    // is refused instead of losing a reply, and input that ends while one is
    // outstanding is answered instead of dropped.
    in_flight: std::sync::Mutex<InFlight>,
    answered: Notify,
}

#[derive(Default)]
struct InFlight {
    ids: HashSet<serde_json::Value>,
    // Replies whose IDs are free again but whose bytes are still being written.
    writing: usize,
}

impl ProtocolOutput {
    async fn send(&self, message: impl serde::Serialize) -> Result<(), io::Error> {
        let mut writer = self.writer.lock().await;
        self.write(&mut writer, message).await
    }

    /// Protocol errors and SDK replies share the writer. Checking failure while
    /// the caller holds it keeps queued sends from reusing a broken stream.
    async fn write(
        &self,
        writer: &mut FramedWrite<tokio::io::Stdout, LinesCodec>,
        message: impl serde::Serialize,
    ) -> Result<(), io::Error> {
        if self.failed.is_cancelled() {
            return Err(io::Error::other("MCP transport has failed"));
        }
        let result = match serde_json::to_string(&message) {
            Ok(line) => writer.send(line).await.map_err(io::Error::other),
            Err(error) => Err(io::Error::other(error)),
        };
        if let Err(error) = &result {
            eprintln!("MCP output error: {error}");
            self.failed.cancel();
        }
        result
    }

    /// Sends an SDK reply and frees the request ID it answers. The ID is freed
    /// under the writer lock before the write, because a client may reuse it the
    /// moment it reads the reply; a drain still waits until those bytes are out.
    async fn reply(&self, message: TxJsonRpcMessage<RoleServer>) -> Result<(), io::Error> {
        let mut writer = self.writer.lock().await;
        let _writing = answered_id(&message).and_then(|id| Writing::start(self, &id));
        self.write(&mut writer, message).await
    }

    /// Records a request ID as awaiting a reply; false means it already is.
    fn accept(&self, id: &serde_json::Value) -> bool {
        self.in_flight().ids.insert(id.clone())
    }

    fn release(&self, id: &serde_json::Value) {
        self.in_flight().ids.remove(id);
        self.answered.notify_one();
    }

    fn awaits_reply(&self) -> bool {
        let in_flight = self.in_flight();
        !in_flight.ids.is_empty() || in_flight.writing > 0
    }

    fn in_flight(&self) -> std::sync::MutexGuard<'_, InFlight> {
        // These IDs are bookkeeping: a poisoned set must not stop the transport
        // from answering the requests it still holds.
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Holds a drain open while a reply whose ID is already free is written, and
/// releases it even if that write is dropped.
struct Writing<'a>(&'a ProtocolOutput);

impl<'a> Writing<'a> {
    fn start(output: &'a ProtocolOutput, id: &serde_json::Value) -> Option<Self> {
        let mut in_flight = output.in_flight();
        if !in_flight.ids.remove(id) {
            return None;
        }
        in_flight.writing += 1;
        Some(Self(output))
    }
}

impl Drop for Writing<'_> {
    fn drop(&mut self) {
        self.0.in_flight().writing -= 1;
        self.0.answered.notify_one();
    }
}

/// The request ID a reply releases, if it carries one.
fn answered_id(message: &TxJsonRpcMessage<RoleServer>) -> Option<serde_json::Value> {
    match message {
        rmcp::model::JsonRpcMessage::Response(response) => {
            Some(response.id.clone().into_json_value())
        }
        rmcp::model::JsonRpcMessage::Error(error) => {
            error.id.clone().map(|id| id.into_json_value())
        }
        _ => None,
    }
}

/// Whether a `tools/call` request names a tool and, if it passes arguments at
/// all, passes them as an object.
fn tool_call_envelope_is_valid(value: &serde_json::Value) -> bool {
    let Some(params) = value.get("params").and_then(serde_json::Value::as_object) else {
        return false;
    };
    params.get("name").is_some_and(serde_json::Value::is_string)
        && params
            .get("arguments")
            .is_none_or(serde_json::Value::is_object)
}

/// Why stdin stopped producing messages. The SDK may drop a `receive` future
/// mid-drain, and a closed channel cannot report its reason twice.
enum InputEnd {
    Closed,
    Failed(io::Error),
}

/// The JSON-RPC error a rejected line is answered with.
struct Rejection {
    code: i32,
    message: &'static str,
    id: serde_json::Value,
}

struct StdioTransport {
    input: mpsc::Receiver<Result<String, io::Error>>,
    output: Arc<ProtocolOutput>,
    pending_error: Option<BoxFuture<'static, Result<(), io::Error>>>,
    ended: Option<InputEnd>,
    initialized: bool,
}

impl StdioTransport {
    /// Decodes one line for the SDK. `Ok(None)` means the line has no reply to
    /// send and is dropped; `Err` carries the error to answer it with.
    fn accept(&mut self, line: &str) -> Result<Option<RxJsonRpcMessage<RoleServer>>, Rejection> {
        let rejected = |code, message, id| Err(Rejection { code, message, id });
        let Ok(value) = serde_json::from_str::<serde_json::Value>(
            line.strip_prefix('\u{feff}').unwrap_or(line),
        ) else {
            return rejected(-32700, "Parse error", serde_json::Value::Null);
        };
        let has_id = value.get("id").is_some();
        let id = value
            .get("id")
            .filter(|id| id.is_string() || id.is_i64() || id.is_u64())
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        // Decoding consumes the value, so keep what the checks below need.
        let method = value
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let envelope_is_valid = method != "tools/call" || tool_call_envelope_is_valid(&value);
        let cancelled = (method == "notifications/cancelled")
            .then(|| value.pointer("/params/requestId").cloned())
            .flatten();
        let message = match serde_json::from_value::<RxJsonRpcMessage<RoleServer>>(value) {
            // The SDK's untagged decoder can reinterpret an invalid request ID
            // as a custom notification. Never drop that request.
            Ok(rmcp::model::JsonRpcMessage::Notification(_)) if has_id => {
                return rejected(-32600, "Invalid Request", id);
            }
            Ok(message) => message,
            // A malformed tools/call envelope may not decode at all. Report the
            // envelope instead of a bare decode failure.
            Err(_) if !envelope_is_valid && !id.is_null() => {
                return rejected(-32602, INVALID_TOOL_CALL, id);
            }
            Err(_) => return rejected(-32600, "Invalid Request", id),
        };
        let is_request = matches!(message, rmcp::model::JsonRpcMessage::Request(_));
        if !self.initialized {
            // The SDK answers a pre-initialize ping, but treats every other
            // pre-initialize message as a fatal handshake fault and exits.
            match method.as_str() {
                _ if !is_request => return Ok(None),
                "initialize" => self.initialized = true,
                "ping" => (),
                _ => {
                    return rejected(-32002, "Server not initialized; send initialize first", id);
                }
            }
        }
        if is_request && !envelope_is_valid {
            // The SDK decodes a malformed tools/call as a custom request and
            // answers it as an unknown method, which hides the real fault.
            return rejected(-32602, INVALID_TOOL_CALL, id);
        }
        if let Some(id) = cancelled {
            // The SDK drops a cancelled request's response, so stop waiting for
            // a reply that will never be written.
            self.output.release(&id);
        }
        if is_request && !self.output.accept(&id) {
            return rejected(-32600, "Duplicate in-flight request id", id);
        }
        Ok(Some(message))
    }

    /// Waits for the replies the SDK still owes, then reports a fatal input
    /// error. The SDK may drop this future at any await, so each wake rechecks.
    async fn drain(&mut self) {
        while !self.output.failed.is_cancelled() {
            let answered = self.output.answered.notified();
            if !self.output.awaits_reply() {
                break;
            }
            answered.await;
        }
        // Cancelling `failed` is what makes the run exit unsuccessfully, but it
        // also stops `send`, so it has to wait for the drain.
        if let Some(InputEnd::Failed(error)) = self.ended.take() {
            eprintln!("MCP input error: {error}");
            self.output.failed.cancel();
        }
    }
}

impl Transport<RoleServer> for StdioTransport {
    type Error = io::Error;

    fn send(
        &mut self,
        message: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let output = Arc::clone(&self.output);
        async move { output.reply(message).await }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            // The SDK may drop a receive future while another event is ready.
            // Retain an in-flight error response so it is neither lost nor resent.
            if let Some(response) = &mut self.pending_error {
                if response.await.is_err() {
                    return None;
                }
                self.pending_error = None;
            }
            if self.ended.is_some() {
                self.drain().await;
                return None;
            }
            let line = match select(
                Box::pin(self.output.failed.cancelled()),
                Box::pin(self.input.recv()),
            )
            .await
            {
                // Broken output can no longer answer anything still pending.
                Either::Left(_) => return None,
                Either::Right((None, _)) => {
                    self.ended = Some(InputEnd::Closed);
                    continue;
                }
                Either::Right((Some(line), _)) => line,
            };
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    self.ended = Some(InputEnd::Failed(error));
                    continue;
                }
            };
            match self.accept(&line) {
                Ok(Some(message)) => return Some(message),
                Ok(None) => (),
                Err(Rejection { code, message, id }) => {
                    let response = serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": code, "message": message},
                    });
                    let output = Arc::clone(&self.output);
                    self.pending_error = Some(Box::pin(async move { output.send(response).await }));
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.input.close();
        Ok(())
    }
}

fn stdin_messages() -> Result<mpsc::Receiver<Result<String, io::Error>>, io::Error> {
    let (sender, receiver) = mpsc::channel(1);
    // Tokio's stdin uses a blocking worker that runtime shutdown must join. A
    // detached reader lets broken stdout terminate with stdin still open, while
    // the runtime continues to wait for actual workspace workers to finish.
    std::thread::Builder::new()
        .name("ultra-edit-stdin".into())
        .spawn(move || {
            let stdin = io::stdin();
            let mut input = stdin.lock();
            loop {
                match read_message(&mut input) {
                    Ok(Some(line)) => {
                        if sender.blocking_send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = sender.blocking_send(Err(error));
                        break;
                    }
                }
            }
        })?;
    Ok(receiver)
}

fn read_message(input: &mut impl BufRead) -> Result<Option<String>, io::Error> {
    let mut bytes = Vec::new();
    loop {
        let buffer = input.fill_buf()?;
        if buffer.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            break;
        }
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let count = newline.unwrap_or(buffer.len());
        if count > MAX_MESSAGE_BYTES - bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MCP line exceeds 16 MiB",
            ));
        }
        let needed = bytes.len() + count;
        if needed > bytes.capacity() {
            let capacity = needed
                .max(bytes.capacity().saturating_mul(2))
                .min(MAX_MESSAGE_BYTES);
            bytes.reserve_exact(capacity - bytes.len());
        }
        bytes.extend_from_slice(&buffer[..count]);
        input.consume(count + usize::from(newline.is_some()));
        if newline.is_some() {
            break;
        }
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
