use std::env;
use std::fs;
use std::io::{self, IsTerminal};

use ultra_edit::report;
use ultra_edit::workspace::EditResult;
use ultra_edit::{Change, CommitStatus, EditRequest, FileRequest, Target, Workspace};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut color_mode = "auto";
    for argument in env::args().skip(1) {
        color_mode = match argument.as_str() {
            "--color=auto" => "auto",
            "--color=always" => "always",
            "--color=never" => "never",
            "--help" | "-h" => {
                println!("Usage: cargo run --example walkthrough -- [--color=auto|always|never]");
                println!("Auto uses color on terminals unless NO_COLOR is nonempty.");
                return Ok(());
            }
            _ => return Err("Unknown argument; use --color=auto|always|never or --help".into()),
        };
    }
    let color = match color_mode {
        "always" => true,
        "never" => false,
        _ => {
            io::stdout().is_terminal()
                && env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
        }
    };
    println!("{}", report::terminal("ULTRA Edit walkthrough", color));
    println!("Two edits to a disposable retry.rs, followed by a retry and an undo.");

    let directory = tempfile::tempdir()?;
    let path = directory.path().join("retry.rs");
    let original = b"const RETRIES: u32 = 2;\r\nconst DELAY_MS: u32 = 100;\r\n";
    let edited = b"const RETRIES: u32 = 3;\r\nconst DELAY_MS: u32 = 250;\r\n";
    fs::write(&path, original)?;
    let workspace = Workspace::open(directory.path())?;
    let snapshot = workspace.read("retry.rs")?;
    let request = EditRequest {
        request_id: "example-edit".into(),
        files: vec![FileRequest {
            base: snapshot.id,
            changes: vec![
                Change {
                    id: "retries".into(),
                    target: Target::Exact {
                        old: "RETRIES: u32 = 2".into(),
                        scope: None,
                    },
                    text: "RETRIES: u32 = 3".into(),
                },
                Change {
                    id: "delay".into(),
                    target: Target::Span { span: "r2".into() },
                    text: "const DELAY_MS: u32 = 250;".into(),
                },
            ],
        }],
    };
    println!("\n{}", report::terminal("1 / 4  Preview", color));
    let preview = workspace.prepare(request.clone())?;
    println!("{}", report::terminal(&preview.report, color));
    if !preview.ready {
        return Err("Example planning failed".into());
    }
    if fs::read(&path)? != original {
        return Err("Preview unexpectedly changed the file".into());
    }
    println!("\n{}", report::terminal("2 / 4  Commit", color));
    let committed = workspace.commit(&preview.reference)?;
    println!(
        "{}",
        report::terminal(&report::receipt(&committed, 12, 1_500), color)
    );
    if committed.commit != CommitStatus::Committed || fs::read(&path)? != edited {
        return Err("Commit did not confirm the expected file bytes".into());
    }
    println!("Verified both edits and preserved CRLF line endings.");

    println!(
        "\n{}",
        report::terminal("3 / 4  Retry the same request", color)
    );
    let EditResult::Completed {
        receipt: replay, ..
    } = workspace.edit(request)?
    else {
        return Err("Retry unexpectedly rejected".into());
    };
    if replay != committed || fs::read(&path)? != edited {
        return Err("Retry did not retain the original receipt and committed bytes".into());
    }
    println!("Returned the original receipt; file bytes are unchanged.");

    println!("\n{}", report::terminal("4 / 4  Undo", color));
    let EditResult::Completed { receipt, report } =
        workspace.undo(&preview.reference, "example-undo")?
    else {
        return Err("Undo unexpectedly rejected".into());
    };
    println!("{}", report::terminal(&report, color));
    if receipt.commit != CommitStatus::Committed || fs::read(&path)? != original {
        return Err("Undo did not confirm restoration of the original bytes".into());
    }
    println!("Verified retry.rs exactly matches its original bytes.");
    println!("\n{}", report::terminal("All four checks passed.", color));
    Ok(())
}
