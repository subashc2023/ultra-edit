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

#[test]
fn focused_read_and_search_issue_editable_references_across_processes() {
    let root = TempDir::new().unwrap();
    let original = "\u{feff}private header\r\nlet value = 1;\nprivate footer";
    fs::write(root.path().join("source.txt"), original).unwrap();
    let (code, selected) = run(root.path(), &["read-range", "source.txt", "2", "2"], None);
    assert_eq!(code, 0, "{selected}");
    assert_eq!(selected["text"], "let value = 1;");
    assert_eq!(selected["total_lines"], 3);
    assert!(!selected.to_string().contains("private"));
    let request = json!({"request_id":"range-edit","files":[{
        "base":selected["snapshot"],
        "changes":[{"id":"value","target":{"kind":"span","span":"selection"},"text":"let value = 2;"}]
    }]});
    let (code, committed) = run(root.path(), &["edit"], Some(&request));
    assert_eq!(code, 0, "{committed}");
    assert_eq!(committed["commit"], "committed");
    assert_eq!(
        fs::read_to_string(root.path().join("source.txt")).unwrap(),
        "\u{feff}private header\r\nlet value = 2;\nprivate footer"
    );
    let (code, found) = run(root.path(), &["search", "source.txt", "value = 2"], None);
    assert_eq!(code, 0, "{found}");
    assert_eq!(found["total_matches"], 1);
    assert_eq!(found["matches"][0]["span"]["line"], 2);
    assert_eq!(found["matches"][0]["before"], "let ");
    assert_eq!(found["matches"][0]["after"], ";");
    assert!(!found.to_string().contains("private"));
    let request = json!({"request_id":"search-edit","files":[{
        "base":found["snapshot"],
        "changes":[{"id":"value","target":{"kind":"span","span":found["matches"][0]["span"]["id"]},"text":"value = 3"}]
    }]});
    let (code, committed) = run(root.path(), &["edit"], Some(&request));
    assert_eq!(code, 0, "{committed}");
    assert_eq!(
        fs::read_to_string(root.path().join("source.txt")).unwrap(),
        "\u{feff}private header\r\nlet value = 3;\nprivate footer"
    );
    let (code, evidence) = run(
        root.path(),
        &["get", selected["snapshot"].as_str().unwrap()],
        None,
    );
    assert_eq!(code, 0, "{evidence}");
    assert_eq!(evidence["value"]["text"], original);
}

#[test]
fn malformed_focused_read_arguments_are_rejected_without_mutation() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("source.txt"), "x\n").unwrap();
    for args in [
        vec!["read-range", "source.txt", "1"],
        vec!["read-range", "source.txt", "one", "1"],
        vec!["read-range", "source.txt", "-1", "1"],
        vec!["read-range", "source.txt", "0", "1"],
        vec!["read-range", "source.txt", "1", "2"],
        vec![
            "read-range",
            "source.txt",
            "1",
            "999999999999999999999999999999",
        ],
        vec!["search", "source.txt"],
        vec!["search", "source.txt", ""],
    ] {
        let (code, rejected) = run(root.path(), &args, None);
        assert_eq!(code, 2, "{args:?}: {rejected}");
        assert!(rejected["error"]["code"].is_string());
        assert_eq!(
            fs::read_to_string(root.path().join("source.txt")).unwrap(),
            "x\n"
        );
    }
    let (code, missing) = run(root.path(), &["search", "source.txt", "absent"], None);
    assert_eq!(code, 0, "{missing}");
    assert_eq!(missing["total_matches"], 0);
    assert_eq!(missing["matches"], json!([]));
}
