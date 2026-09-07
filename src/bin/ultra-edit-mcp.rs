use std::env;
use std::future::ready;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use futures::StreamExt;
use rmcp::{
    RoleServer, ServiceExt,
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::{async_rw::JsonRpcMessageCodec, sink_stream::SinkStreamTransport},
};
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
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
    // Validate each bounded line strictly: the SDK codec can silently consume a
    // malformed notification and stall later messages already in its buffer.
    let input = FramedRead::new(
        tokio::io::stdin(),
        LinesCodec::new_with_max_length(MAX_MESSAGE_BYTES),
    )
    .scan((), |(), result| {
        let message = result.map_err(io::Error::other).and_then(|line| {
            serde_json::from_str::<RxJsonRpcMessage<RoleServer>>(
                line.strip_prefix('\u{feff}').unwrap_or(&line),
            )
            .map_err(io::Error::other)
        });
        ready(match message {
            Ok(message) => Some(message),
            Err(error) => {
                eprintln!("MCP input error: {error}");
                None
            }
        })
    });
    let output = FramedWrite::new(
        tokio::io::stdout(),
        JsonRpcMessageCodec::<TxJsonRpcMessage<RoleServer>>::new(),
    );
    McpServer::new(workspace)
        .serve(SinkStreamTransport::new(output, input))
        .await?
        .waiting()
        .await?;
    Ok(())
}
