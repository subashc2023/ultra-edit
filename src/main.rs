use std::env;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Deserialize;
use serde_json::{Value, json};
use ultra_edit::workspace::EditResult;
use ultra_edit::{Change, CommitStatus, Error, Preparation, Workspace};

const HELP: &str = "ultra-edit 0.1.0
Usage: ultra-edit [--root WORKSPACE] COMMAND [ARGS]

  read PATH                  Persist a complete UTF-8 snapshot and return range refs
  read-range PATH FIRST LAST  Read inclusive line bodies with editable range refs
  search PATH QUERY          Find literal text and return exact editable match refs
  prepare                    Read an edit request from stdin; persist a preview
  edit                       Read an edit request from stdin; prepare and commit
  repair                     Read {reference,request_id,changes} from stdin
  commit PLAN                Commit the recorded candidate, conditional on its base
  receipt REQUEST_ID         Retrieve the full recorded persistence outcome
  inspect PLAN               Capture journal and original/intended/current file evidence
  reconcile                  Read {inspection,decision,note} from stdin; accept current state
  get REFERENCE              Retrieve a full snapshot, plan, draft, or inspection
  diff PLAN                  Retrieve the full before/after diff (JSON string)
  undo PLAN NEW_REQUEST_ID    Conditionally restore confirmed committed files

Output is JSON. Exit codes: 0 successful read/preview/commit/reconciliation, 2 rejected/error,
3 commit not fully confirmed. References and receipts live in WORKSPACE/.ultra-edit.
See README.md for request schemas, preservation policy, and initial limitations.
";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairRequest {
    reference: String,
    request_id: String,
    changes: Vec<Change>,
}

fn main() -> ExitCode {
    let result = run();
    let (value, status) = match result {
        Ok(None) => return ExitCode::SUCCESS,
        Ok(Some(output)) => output,
        Err(error) => {
            let status = if error.commit.is_some() { 3 } else { 2 };
            (
                json!({ "error": { "code": error.code, "message": error.message }, "commit": error.commit, "request_id": error.request_id, "plan_id": error.plan_id }),
                status,
            )
        }
    };
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    if serde_json::to_writer_pretty(&mut writer, &value).is_err() || writeln!(writer).is_err() {
        return ExitCode::from(2);
    }
    ExitCode::from(status)
}

fn run() -> Result<Option<(Value, u8)>, Error> {
    let mut args = env::args_os().skip(1).collect::<Vec<_>>();
    if args
        .first()
        .is_none_or(|arg| arg == "--help" || arg == "-h")
    {
        print!("{HELP}");
        return Ok(None);
    }
    if args[0] == "--version" {
        println!("ultra-edit {}", env!("CARGO_PKG_VERSION"));
        return Ok(None);
    }
    let root = if args[0] == "--root" {
        if args.len() < 3 {
            return Err(usage("--root requires a workspace and command"));
        }
        let root = PathBuf::from(&args[1]);
        args.drain(..2);
        root
    } else {
        env::current_dir()?
    };
    let command = args
        .remove(0)
        .into_string()
        .map_err(|_| usage("Command must be Unicode"))?;
    let expected = match command.as_str() {
        "read" | "commit" | "receipt" | "inspect" | "get" | "diff" => 1,
        "undo" | "search" => 2,
        "read-range" => 3,
        "prepare" | "edit" | "repair" | "reconcile" => 0,
        _ => return Err(usage("Unknown command; run --help")),
    };
    if args.len() != expected {
        return Err(usage("Unexpected or missing command arguments; run --help"));
    }
    let workspace = Workspace::open(root)?;
    let argument = |index: usize| {
        args[index]
            .to_str()
            .ok_or_else(|| usage("Command arguments must be Unicode"))
    };
    let output = match command.as_str() {
        "read" => (
            serde_json::to_value(workspace.read(PathBuf::from(&args[0]))?)?,
            0,
        ),
        "read-range" => (
            serde_json::to_value(workspace.read_range(
                PathBuf::from(&args[0]),
                line_number(argument(1)?)?,
                line_number(argument(2)?)?,
            )?)?,
            0,
        ),
        "search" => (
            serde_json::to_value(workspace.search(PathBuf::from(&args[0]), argument(1)?)?)?,
            0,
        ),
        "prepare" => preparation(workspace.prepare(input()?)?),
        "edit" => edited(workspace.edit(input()?)?),
        "repair" => {
            let input: RepairRequest = input()?;
            preparation(workspace.repair(&input.reference, &input.request_id, input.changes)?)
        }
        "commit" => completed(workspace.commit(argument(0)?)?),
        "receipt" => (serde_json::to_value(workspace.receipt(argument(0)?)?)?, 0),
        "inspect" => (serde_json::to_value(workspace.inspect(argument(0)?)?)?, 0),
        "reconcile" => (serde_json::to_value(workspace.reconcile(input()?)?)?, 0),
        "get" => (serde_json::to_value(workspace.evidence(argument(0)?)?)?, 0),
        "diff" => (json!({ "diff": workspace.diff(argument(0)?)? }), 0),
        "undo" => edited(workspace.undo(argument(0)?, argument(1)?)?),
        _ => return Err(usage("Unknown command")),
    };
    Ok(Some(output))
}

fn usage(message: &str) -> Error {
    Error::new("USAGE", message)
}

fn line_number(value: &str) -> Result<usize, Error> {
    value
        .parse()
        .map_err(|_| usage("Line numbers must be positive integers"))
}

fn input<T: serde::de::DeserializeOwned>() -> Result<T, Error> {
    const MAX_INPUT: u64 = 16 * 1024 * 1024;
    let mut bytes = Vec::new();
    io::stdin().take(MAX_INPUT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INPUT {
        return Err(Error::new("INPUT_TOO_LARGE", "JSON request exceeds 16 MiB"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn preparation(preparation: Preparation) -> (Value, u8) {
    if let Some(receipt) = preparation.receipt {
        return completed(receipt);
    }
    let code = if preparation.ready { 0 } else { 2 };
    let diagnostic_summary = preparation.diagnostics.iter().take(6).map(|diagnostic| {
        // Full strings/counts remain in the draft; inline diagnostics cannot grow with source size.
        json!({
            "code": diagnostic.code.chars().take(80).collect::<String>(),
            "change_id": diagnostic.change_id.as_ref().map(|id| id.chars().take(80).collect::<String>()),
            "message": diagnostic.message.chars().take(240).collect::<String>(),
            "expected": diagnostic.expected,
            "actual": diagnostic.actual,
        })
    }).collect::<Vec<_>>();
    (
        json!({
            "reference": preparation.reference,
            "ready": preparation.ready,
            "diagnostic_count": preparation.diagnostics.len(),
            "diagnostics": diagnostic_summary,
            "report": preparation.report,
        }),
        code,
    )
}

fn edited(result: EditResult) -> (Value, u8) {
    match result {
        EditResult::Rejected { preparation: value } => preparation(value),
        EditResult::Completed { receipt, .. } => completed(receipt),
    }
}

fn completed(receipt: ultra_edit::Receipt) -> (Value, u8) {
    let code = if receipt.commit == CommitStatus::Committed {
        0
    } else {
        3
    };
    (
        json!({
            "request_id": receipt.request_id,
            "plan_id": receipt.plan_id,
            "commit": receipt.commit,
            "report": ultra_edit::report::receipt(&receipt, 60, 6_000),
        }),
        code,
    )
}
