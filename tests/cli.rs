use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;
use ultra_edit::workspace::REPLAY_NOTICE;

/// What repeating the command that returned `original` prints: the same recorded
/// result, flagged, with the replay notice leading its report.
fn replay_of(original: &Value) -> Value {
    assert!(original.get("replayed").is_none(), "{original}");
    let mut replay = original.clone();
    replay["replayed"] = json!(true);
    replay["report"] = json!(format!(
        "{REPLAY_NOTICE}\n{}",
        original["report"].as_str().unwrap()
    ));
    replay
}

#[test]
fn help_and_version_report_the_package_version() {
    let expected = format!("ultra-edit {}", env!("CARGO_PKG_VERSION"));

    for arguments in [vec![], vec!["--help"], vec!["-h"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_ultra-edit"))
            .args(arguments)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().lines().next(),
            Some(expected.as_str())
        );
    }

    let output = Command::new(env!("CARGO_BIN_EXE_ultra-edit"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), expected);
}

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
    let edit = json!({"request_id":"cli-edit","files":[{"base":snapshot["snapshot"],"changes":[{"id":"one","target":{"kind":"span","span":"r1"},"text":"const n = 2;"}]}]});
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
    assert_eq!(replay, replay_of(&committed));
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

#[cfg(windows)]
#[test]
fn canonical_windows_paths_are_display_only_across_cli_responses() {
    let root = TempDir::new().unwrap();
    let target = root.path().join("file.txt");
    fs::write(&target, "old\r\nkeep\r\n").unwrap();
    let canonical = fs::canonicalize(&target)
        .unwrap()
        .into_os_string()
        .into_string()
        .unwrap();
    let displayed = ultra_edit::report::path_for_display(&canonical).into_owned();

    let (code, snapshot) = run(root.path(), &["read", "file.txt"], None);
    assert_eq!(code, 0, "{snapshot}");
    assert_eq!(snapshot["path"], displayed);
    let request = json!({
        "request_id": "windows-display",
        "files": [{
            "base": snapshot["snapshot"],
            "changes": [{
                "id": "mixed-endings",
                "target": {"kind": "exact", "old": "old"},
                "text": "new\nextra",
            }],
        }],
    });
    let (code, prepared) = run(root.path(), &["prepare"], Some(&request));
    assert_eq!(code, 0, "{prepared}");
    assert_eq!(prepared["warnings"][0]["file"], displayed);
    let plan_id = prepared["reference"].as_str().unwrap();

    let (code, diff) = run(root.path(), &["diff", plan_id], None);
    assert_eq!(code, 0, "{diff}");
    assert!(
        diff["diff"]
            .as_str()
            .unwrap()
            .starts_with(&format!("--- {:?}\n", format!("a/{displayed}"))),
        "{diff}"
    );
    let (code, evidence) = run(root.path(), &["get", plan_id], None);
    assert_eq!(code, 0, "{evidence}");
    assert_eq!(evidence["value"]["files"][0]["base"]["path"], displayed);
    assert_eq!(evidence["value"]["warnings"][0]["file"], displayed);

    let (code, committed) = run(root.path(), &["commit", plan_id], None);
    assert_eq!(code, 0, "{committed}");
    assert_eq!(committed["warnings"][0]["file"], displayed);
    let (code, receipt) = run(root.path(), &["receipt", "windows-display"], None);
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["files"][0]["path"], displayed);
    assert_eq!(receipt["warnings"][0]["file"], displayed);

    let storage = ultra_edit::storage::Storage::open(root.path()).unwrap();
    let stored: ultra_edit::PreparedPlan = storage.get("plans", plan_id).unwrap();
    assert_eq!(stored.files[0].base.path, canonical);
    assert_eq!(stored.warnings[0].file.as_deref(), Some(canonical.as_str()));
}

#[test]
fn omitted_ids_are_derived_and_replays_are_flagged_across_processes() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("file.txt");
    fs::write(&path, "x x\n").unwrap();
    let (code, snapshot) = run(root.path(), &["read", "file.txt"], None);
    assert_eq!(code, 0, "{snapshot}");
    let ambiguous = json!({"files":[{"base":snapshot["snapshot"],"changes":[
        {"target":{"kind":"exact","old":"x"},"text":"y"}
    ]}]});
    let (code, rejected) = run(root.path(), &["prepare"], Some(&ambiguous));
    assert_eq!(code, 2, "{rejected}");
    assert_eq!(rejected["diagnostics"][0]["change_id"], "1.1");
    // The CLI shows a derived ID so its receipt can be queried later.
    let draft_id = rejected["request_id"].as_str().unwrap();
    assert!(draft_id.starts_with("auto-"), "{rejected}");
    let (code, replayed) = run(root.path(), &["edit"], Some(&ambiguous));
    assert_eq!(code, 2, "{replayed}");
    assert_eq!(replayed, replay_of(&rejected));

    let repair = json!({"reference":rejected["reference"],"changes":[
        {"id":"1.1","target":{"kind":"all","old":"x","scope":"r0","expected":2},"text":"y"}
    ]});
    let (code, repaired) = run(root.path(), &["repair"], Some(&repair));
    assert_eq!(code, 0, "{repaired}");
    assert_eq!(repaired["ready"], true);
    let repair_id = repaired["request_id"].as_str().unwrap();
    assert!(
        repair_id.starts_with("auto-") && repair_id != draft_id,
        "{repaired}"
    );
    let (code, replayed) = run(root.path(), &["repair"], Some(&repair));
    assert_eq!(code, 0, "{replayed}");
    assert_eq!(replayed, replay_of(&repaired));
    let plan = repaired["reference"].as_str().unwrap();

    let (code, committed) = run(root.path(), &["commit", plan], None);
    assert_eq!(code, 0, "{committed}");
    assert!(committed.get("replayed").is_none(), "{committed}");
    let request_id = committed["request_id"].as_str().unwrap();
    assert_eq!(request_id, repair_id, "{committed}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "y y\n");
    let (code, replayed) = run(root.path(), &["commit", plan], None);
    assert_eq!(code, 0, "{replayed}");
    assert_eq!(replayed, replay_of(&committed));
    let (code, receipt) = run(root.path(), &["receipt", request_id], None);
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["plan_id"], plan);

    let (code, usage) = run(root.path(), &["retry", plan], None);
    assert_eq!(code, 2, "{usage}");
    assert_eq!(usage["error"]["code"], "USAGE");
    let (code, undone) = run(root.path(), &["undo", plan], None);
    assert_eq!(code, 0, "{undone}");
    assert!(undone.get("replayed").is_none(), "{undone}");
    assert!(undone["request_id"].as_str().unwrap().starts_with("auto-"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "x x\n");
    // With the committed bytes back, a new undo could succeed; the replay writes nothing.
    fs::write(&path, "y y\n").unwrap();
    let (code, replayed) = run(root.path(), &["undo", plan], None);
    assert_eq!(code, 0, "{replayed}");
    assert_eq!(replayed, replay_of(&undone));
    assert_eq!(fs::read_to_string(&path).unwrap(), "y y\n");
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
    assert!(!selected.to_string().contains("private header"));
    assert!(!selected.to_string().contains("private footer"));
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
    assert!(!found.to_string().contains("private header"));
    assert!(!found.to_string().contains("private footer"));
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

#[test]
fn a_continued_range_edits_distant_lines_under_one_base_across_processes() {
    let help = Command::new(env!("CARGO_BIN_EXE_ultra-edit"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("read-range PATH FIRST LAST [SNAPSHOT]")
    );
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("source.txt"), "first\nmiddle\nlast\n").unwrap();
    let (code, first) = run(root.path(), &["read-range", "source.txt", "1", "1"], None);
    assert_eq!(code, 0, "{first}");
    assert_eq!(first["stale"], false);
    let base = first["snapshot"].as_str().unwrap();
    let (code, last) = run(
        root.path(),
        &["read-range", "source.txt", "3", "3", base],
        None,
    );
    assert_eq!(code, 0, "{last}");
    assert_eq!(last["stale"], false);
    assert_eq!(last["text"], "last");
    assert_eq!(last["spans"], json!(["r1", "r3", "selection"]));
    assert_eq!(last["lines"], json!(["r3 | last"]));
    let continued = last["snapshot"].as_str().unwrap();
    for (args, error) in [
        (
            vec!["read-range", "source.txt", "2", "2", continued, "extra"],
            "USAGE",
        ),
        (
            vec!["read-range", "source.txt", "2", "2", "p-invalid"],
            "INVALID_REFERENCE",
        ),
        (
            vec!["read-range", "source.txt", "2", "4", continued],
            "INVALID_LINE_RANGE",
        ),
    ] {
        let (code, rejected) = run(root.path(), &args, None);
        assert_eq!(code, 2, "{args:?}: {rejected}");
        assert_eq!(rejected["error"]["code"], error, "{args:?}: {rejected}");
    }
    let request = json!({"request_id":"distant-lines","files":[{"base":continued,"changes":[
        {"id":"first","target":{"kind":"span","span":"r1"},"text":"FIRST"},
        {"id":"last","target":{"kind":"span","span":"selection"},"text":"LAST"}
    ]}]});
    let (code, committed) = run(root.path(), &["edit"], Some(&request));
    assert_eq!(code, 0, "{committed}");
    assert_eq!(committed["commit"], "committed");
    assert_eq!(
        fs::read(root.path().join("source.txt")).unwrap(),
        b"FIRST\nmiddle\nLAST\n"
    );
    // The committed edit changed the file, so continuing its old base is stale.
    let (code, stale) = run(
        root.path(),
        &["read-range", "source.txt", "2", "2", continued],
        None,
    );
    assert_eq!(code, 0, "{stale}");
    assert_eq!(stale["stale"], true);
    assert_eq!(stale["text"], "middle");
}

#[test]
fn a_rejected_batch_identifies_the_file_that_failed() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("one.txt"), "one\n").unwrap();
    fs::write(root.path().join("two.txt"), "two\n").unwrap();
    let (code, one) = run(root.path(), &["read-range", "one.txt", "1", "1"], None);
    assert_eq!(code, 0, "{one}");
    let (code, two) = run(root.path(), &["read-range", "two.txt", "1", "1"], None);
    assert_eq!(code, 0, "{two}");
    fs::write(root.path().join("two.txt"), "external change\n").unwrap();
    let request = json!({"request_id":"stale-batch","files":[
        {"base":one["snapshot"],"changes":[
            {"id":"first","target":{"kind":"span","span":"selection"},"text":"ONE"}
        ]},
        {"base":two["snapshot"],"changes":[
            {"id":"second","target":{"kind":"span","span":"selection"},"text":"TWO"}
        ]}
    ]});
    let (code, rejected) = run(root.path(), &["edit"], Some(&request));
    assert_eq!(code, 2, "{rejected}");
    let stale = rejected["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| diagnostic["code"] == "STALE_SNAPSHOT")
        .unwrap_or_else(|| panic!("{rejected}"));
    assert!(
        stale["file"].as_str().unwrap().ends_with("two.txt"),
        "{stale}"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("one.txt")).unwrap(),
        "one\n"
    );
}

#[test]
fn near_miss_candidates_appear_in_summaries_and_full_drafts() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("guard.txt"), "first\nsecond\r\nthird\n").unwrap();
    let (code, snapshot) = run(root.path(), &["read", "guard.txt"], None);
    assert_eq!(code, 0, "{snapshot}");
    let request = json!({"request_id":"cli-near-miss","files":[{"base":snapshot["snapshot"],"changes":[
        {"id":"guard","target":{"kind":"span","span":"r1","expect":"second"},"text":"2nd"},
        {"id":"endings","target":{"kind":"exact","old":"second\nthird"},"text":"x"}
    ]}]});
    let (code, prepared) = run(root.path(), &["prepare"], Some(&request));
    assert_eq!(code, 2, "{prepared}");
    let guard = &prepared["diagnostics"][0];
    assert_eq!(guard["code"], "EXPECTED_TEXT_MISMATCH");
    assert_eq!(
        guard["candidates"],
        json!([{"kind":"exact","line":2,"end_line":2,"text":"second"}])
    );
    let message = guard["message"].as_str().unwrap();
    assert!(
        message.starts_with("Span holds \"first\", not expect;"),
        "{message}"
    );
    let endings = &prepared["diagnostics"][1];
    assert_eq!(
        endings["candidates"],
        json!([{"kind":"whitespace","line":2,"end_line":3,"text":"second\r\nthird"}])
    );
    let reference = prepared["reference"].as_str().unwrap();
    let (code, draft) = run(root.path(), &["get", reference], None);
    assert_eq!(code, 0, "{draft}");
    assert_eq!(
        draft["value"]["diagnostics"][0]["candidates"],
        guard["candidates"]
    );
    assert_eq!(
        draft["value"]["diagnostics"][1]["candidates"],
        endings["candidates"]
    );
}

#[test]
fn bounded_reads_pagination_and_pruning_flags_work_across_processes() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("file.txt"), "x\n".repeat(500)).unwrap();
    let (code, rejected) = run(root.path(), &["read", "file.txt"], None);
    assert_eq!(code, 2);
    assert_eq!(rejected["error"]["code"], "READ_TOO_LARGE");
    let (code, full) = run(root.path(), &["read", "file.txt", "1000"], None);
    assert_eq!(code, 0, "{full}");
    assert!(full["snapshot"].is_string());
    assert!(full.get("id").is_none());
    let (code, page) = run(
        root.path(),
        &[
            "search",
            "file.txt",
            "x",
            "480",
            full["snapshot"].as_str().unwrap(),
        ],
        None,
    );
    assert_eq!(code, 0, "{page}");
    assert_eq!(page["matches"][0]["span"]["line"], 481);
    assert!(page["next_offset"].is_null());
    let (code, dry_run) = run(root.path(), &["prune-snapshots", "0"], None);
    assert_eq!(code, 0, "{dry_run}");
    assert_eq!(dry_run["dry_run"], true);
    assert_eq!(dry_run["removed_snapshots"], 0);
    assert_eq!(dry_run["eligible"].as_array().unwrap().len(), 2);
    assert_eq!(
        run(
            root.path(),
            &["get", full["snapshot"].as_str().unwrap()],
            None
        )
        .0,
        0
    );
    assert_eq!(
        run(root.path(), &["prune-snapshots", "0", "--force"], None).0,
        2
    );
    let (code, applied) = run(root.path(), &["prune-snapshots", "0", "--apply"], None);
    assert_eq!(code, 0, "{applied}");
    assert_eq!(applied["removed_snapshots"], 2);
    assert_eq!(
        fs::read(root.path().join("file.txt")).unwrap(),
        "x\n".repeat(500).as_bytes()
    );
}
