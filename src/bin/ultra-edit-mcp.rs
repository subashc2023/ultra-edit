use std::env;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

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
use tokio::sync::mpsc;
use tokio_util::{
    codec::{FramedWrite, LinesCodec},
    sync::CancellationToken,
};
use ultra_edit::{Workspace, mcp::McpServer};

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const CLAUDE_CONTEXT: &str = include_str!("../../plugin/claude-code/instructions.md");

fn main() -> ExitCode {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() == 1 && (arguments[0] == "--help" || arguments[0] == "-h") {
        println!(
            "ultra-edit-mcp {}\nUsage: ultra-edit-mcp --root WORKSPACE\n\
             Or: ultra-edit-mcp --claude-context SessionStart|SubagentStart\n\n\
             Serve MCP over stdio inside one fixed existing workspace.\n\
             JSON-RPC lines are limited to 16 MiB. Protocol output uses stdout; errors use stderr.\n\
             Cancellation or disconnection does not imply rollback; query receipts before retrying.\n\
             --claude-context prints plugin hook JSON and exits without opening a workspace.",
            env!("CARGO_PKG_VERSION")
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
    if arguments.len() != 2 || arguments[0] != "--root" {
        eprintln!(
            "Usage: ultra-edit-mcp --root WORKSPACE (or --claude-context EVENT / --help / --version)"
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

async fn serve(workspace: Workspace) -> Result<(), Box<dyn std::error::Error>> {
    let failed = CancellationToken::new();
    let transport = StdioTransport {
        input: stdin_messages()?,
        pending_error: None,
        output: Arc::new(ProtocolOutput {
            writer: Mutex::new(FramedWrite::new(tokio::io::stdout(), LinesCodec::new())),
            failed: failed.clone(),
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
}

impl ProtocolOutput {
    async fn send(&self, message: impl serde::Serialize) -> Result<(), io::Error> {
        // Protocol errors and SDK replies share this writer. Check failure under
        // the lock so queued sends never reuse a broken output stream.
        let mut writer = self.writer.lock().await;
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
}

struct StdioTransport {
    input: mpsc::Receiver<Result<String, io::Error>>,
    output: Arc<ProtocolOutput>,
    pending_error: Option<BoxFuture<'static, Result<(), io::Error>>>,
}

impl Transport<RoleServer> for StdioTransport {
    type Error = io::Error;

    fn send(
        &mut self,
        message: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let output = Arc::clone(&self.output);
        async move { output.send(message).await }
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
            let line = match select(
                Box::pin(self.output.failed.cancelled()),
                Box::pin(self.input.recv()),
            )
            .await
            {
                Either::Left(_) | Either::Right((None, _)) => return None,
                Either::Right((Some(line), _)) => line,
            };
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    eprintln!("MCP input error: {error}");
                    self.output.failed.cancel();
                    return None;
                }
            };
            let value = serde_json::from_str::<serde_json::Value>(
                line.strip_prefix('\u{feff}').unwrap_or(&line),
            );
            let (code, message, id) = match value {
                Err(_) => (-32700, "Parse error", serde_json::Value::Null),
                Ok(value) => {
                    let has_id = value.get("id").is_some();
                    let id = value
                        .get("id")
                        .filter(|id| id.is_string() || id.is_i64() || id.is_u64())
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    match serde_json::from_value::<RxJsonRpcMessage<RoleServer>>(value) {
                        // The SDK's untagged decoder can reinterpret an invalid
                        // request ID as a custom notification. Never drop that request.
                        Ok(rmcp::model::JsonRpcMessage::Notification(_)) if has_id => {
                            (-32600, "Invalid Request", id)
                        }
                        Ok(message) => return Some(message),
                        Err(_) => (-32600, "Invalid Request", id),
                    }
                }
            };
            let response = serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": code, "message": message},
            });
            let output = Arc::clone(&self.output);
            self.pending_error = Some(Box::pin(async move { output.send(response).await }));
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
