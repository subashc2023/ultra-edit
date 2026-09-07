use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

fn run(root: &std::path::Path, args: &[&str], input: Option<&Value>) -> (i32, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ultra-edit"))
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(serde_json::to_string(input).unwrap().as_bytes())
            .unwrap();
    } else {
        drop(child.stdin.take());
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (
        output.status.code().unwrap(),
        serde_json::from_slice(&output.stdout).unwrap(),
    )
}

#[test]
fn read_prepare_commit_receipt_and_undo_work_in_separate_processes() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("file.txt"), "const n = 1;\r\n").unwrap();
    let (code, snapshot) = run(root.path(), &["read", "file.txt"], None);
    assert_eq!(code, 0);
    let edit = json!({"request_id":"cli-edit","files":[{"base":snapshot["id"],"changes":[{"id":"one","target":{"kind":"span","span":"r1"},"text":"const n = 2;"}]}]});
    let (code, preview) = run(root.path(), &["prepare"], Some(&edit));
    assert_eq!(code, 0);
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "const n = 1;\r\n"
    );
    let reference = preview["reference"].as_str().unwrap();
    let (code, committed) = run(root.path(), &["commit", reference], None);
    assert_eq!(code, 0);
    assert_eq!(committed["commit"], "committed");
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "const n = 2;\r\n"
    );
    let (code, replay) = run(root.path(), &["edit"], Some(&edit));
    assert_eq!(code, 0);
    assert_eq!(replay, committed);
    let (code, receipt) = run(root.path(), &["receipt", "cli-edit"], None);
    assert_eq!(code, 0);
    assert_eq!(receipt["files"][0]["changes_applied"], 1);
    let (code, undone) = run(root.path(), &["undo", reference, "cli-undo"], None);
    assert_eq!(code, 0);
    assert_eq!(undone["commit"], "committed");
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "const n = 1;\r\n"
    );
}

#[test]
fn rejects_unknown_fields_instead_of_silently_ignoring_options() {
    let root = TempDir::new().unwrap();
    let (code, result) = run(
        root.path(),
        &["edit"],
        Some(&json!({"request_id":"bad","files":[],"force":true})),
    );
    assert_eq!(code, 2);
    assert_eq!(result["error"]["code"], "INVALID_JSON");
}
