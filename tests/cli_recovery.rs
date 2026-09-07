use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;
use ultra_edit::storage::Storage;
use ultra_edit::{CommitStatus, FileStatus, PreparedPlan, Receipt, digest, report};

fn start(root: &Path, args: &[&str], input: Option<&Value>) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ultra-edit"))
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    if let Some(input) = input {
        stdin
            .write_all(&serde_json::to_vec(input).unwrap())
            .unwrap();
    }
    drop(stdin);
    child
}

fn finish(child: Child) -> (i32, Value) {
    let output = child.wait_with_output().unwrap();
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stdout)));
    (output.status.code().unwrap(), value)
}

fn run(root: &Path, args: &[&str], input: Option<&Value>) -> (i32, Value) {
    finish(start(root, args, input))
}

fn prepare(root: &Path, files: &[(&str, &str, &str)], request_id: &str) -> (Value, PreparedPlan) {
    let files: Vec<_> = files
        .iter()
        .enumerate()
        .map(|(index, (path, before, after))| {
            fs::write(root.join(path), before).unwrap();
            let (code, snapshot) = run(root, &["read", path], None);
            assert_eq!(code, 0, "{snapshot}");
            json!({
                "base": snapshot["snapshot"],
                "changes": [{
                    "id": format!("change-{index}"),
                    "target": {"kind": "exact", "old": before},
                    "text": after,
                }],
            })
        })
        .collect();
    let request = json!({"request_id": request_id, "files": files});
    let (code, preview) = run(root, &["prepare"], Some(&request));
    assert_eq!(code, 0, "{preview}");
    assert_eq!(preview["ready"], true);
    let (code, evidence) = run(root, &["get", preview["reference"].as_str().unwrap()], None);
    assert_eq!(code, 0, "{evidence}");
    assert_eq!(evidence["kind"], "plan");
    let storage = Storage::open(root).unwrap();
    let plan: PreparedPlan = storage
        .get("plans", preview["reference"].as_str().unwrap())
        .unwrap();
    let mut displayed = serde_json::to_value(&plan).unwrap();
    report::display_paths(&mut displayed);
    assert_eq!(evidence["value"], displayed);
    (request, plan)
}

fn record_interrupted_commit(root: &Path, plan: &PreparedPlan) -> Vec<u8> {
    // This durable wire fixture represents a process crash after recording write
    // intent but before recording whether the target replacement completed.
    let path = root
        .join(".ultra-edit/journals")
        .join(format!("{}.jsonl", plan.id));
    let mut file = File::create_new(path).unwrap();
    let mut bytes = Vec::new();
    for payload in [
        json!({"event": "begin", "plan_id": plan.id, "plan_digest": digest(&serde_json::to_vec(plan).unwrap()), "file_count": plan.files.len()}),
        json!({"event": "intent", "index": 0}),
    ] {
        let checksum = digest(&serde_json::to_vec(&payload).unwrap());
        bytes.extend(
            serde_json::to_vec(&json!({"checksum": checksum, "payload": payload})).unwrap(),
        );
        bytes.push(b'\n');
    }
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    bytes
}

#[test]
fn interrupted_commit_replays_unknown_without_inferring_from_current_bytes() {
    for replacement_happened in [false, true] {
        let root = TempDir::new().unwrap();
        let (request, plan) = prepare(
            root.path(),
            &[("first.txt", "x", "xx"), ("second.txt", "y", "yy")],
            "interrupted-edit",
        );
        let journal = record_interrupted_commit(root.path(), &plan);
        let expected_first = if replacement_happened { "xx" } else { "x" };
        if replacement_happened {
            fs::write(root.path().join("first.txt"), expected_first).unwrap();
        }

        let (code, committed) = run(root.path(), &["commit", &plan.id], None);
        assert_eq!(code, 3, "{committed}");
        assert_eq!(committed["commit"], "outcome_unknown");
        assert_eq!(committed["request_id"], "interrupted-edit");
        assert_eq!(committed["plan_id"], plan.id);
        assert!(
            committed["report"]
                .as_str()
                .unwrap()
                .contains("0 confirmed change IDs")
        );
        for command in ["prepare", "edit"] {
            let (code, replay) = run(root.path(), &[command], Some(&request));
            assert_eq!(code, 3, "{replay}");
            assert_eq!(replay, committed);
        }

        let (code, value) = run(root.path(), &["receipt", "interrupted-edit"], None);
        assert_eq!(code, 0, "{value}");
        let receipt: Receipt = serde_json::from_value(value).unwrap();
        assert_eq!(receipt.commit, CommitStatus::OutcomeUnknown);
        assert_eq!(receipt.files.len(), 2);
        assert_eq!(receipt.files[0].status, FileStatus::OutcomeUnknown);
        assert_eq!(receipt.files[1].status, FileStatus::NotCommitted);
        for (outcome, prepared) in receipt.files.iter().zip(&plan.files) {
            assert_eq!(outcome.path, report::path_for_display(&prepared.base.path));
            assert_eq!(outcome.before, prepared.base.id);
            assert_eq!(outcome.after_digest, digest(prepared.output.as_bytes()));
            assert_eq!(outcome.changes_applied, 0);
            assert!(
                outcome
                    .error
                    .as_deref()
                    .unwrap()
                    .starts_with("INTERRUPTED:")
            );
        }
        assert_eq!(receipt.validation, "not_requested");
        assert_eq!(receipt.undo, None);

        let (code, evidence) = run(root.path(), &["get", &plan.id], None);
        assert_eq!(code, 0, "{evidence}");
        let mut displayed = serde_json::to_value(&plan).unwrap();
        report::display_paths(&mut displayed);
        assert_eq!(evidence["value"], displayed);
        let (code, current) = run(root.path(), &["read", "first.txt"], None);
        assert_eq!(code, 0, "{current}");
        assert_eq!(current["text"], expected_first);
        let fresh_request = json!({
            "request_id": "blocked-edit",
            "files": [{"base": current["snapshot"], "changes": [{
                "id": "blocked", "target": {"kind": "exact", "old": expected_first},
                "text": "must not write",
            }]}],
        });
        let (code, blocked) = run(root.path(), &["edit"], Some(&fresh_request));
        assert_eq!(code, 3, "{blocked}");
        assert_eq!(blocked["error"]["code"], "RECONCILIATION_REQUIRED");
        assert_eq!(blocked["commit"], "not_committed");
        assert_eq!(blocked["request_id"], "blocked-edit");
        assert_eq!(
            fs::read_to_string(root.path().join("first.txt")).unwrap(),
            expected_first
        );
        assert_eq!(
            fs::read_to_string(root.path().join("second.txt")).unwrap(),
            "y"
        );
        assert_eq!(
            fs::read(
                root.path()
                    .join(".ultra-edit/journals")
                    .join(format!("{}.jsonl", plan.id))
            )
            .unwrap(),
            journal
        );
    }
}

#[test]
fn corrupt_journal_replays_preserve_uncertainty_and_evidence_references() {
    let root = TempDir::new().unwrap();
    let (request, plan) = prepare(root.path(), &[("file.txt", "x", "xx")], "corrupt-edit");
    let mut journal = record_interrupted_commit(root.path(), &plan);
    journal.extend_from_slice(b"{\"checksum\":\"bad\",\"payload\":{\"event\":\"finished\"}}\n");
    let path = root
        .path()
        .join(".ultra-edit/journals")
        .join(format!("{}.jsonl", plan.id));
    fs::write(&path, &journal).unwrap();
    for (code, error) in [
        run(root.path(), &["commit", &plan.id], None),
        run(root.path(), &["prepare"], Some(&request)),
        run(root.path(), &["edit"], Some(&request)),
        run(root.path(), &["receipt", "corrupt-edit"], None),
    ] {
        assert_eq!(code, 3, "{error}");
        assert_eq!(error["error"]["code"], "JOURNAL_CORRUPT");
        assert_eq!(error["commit"], "outcome_unknown");
        assert_eq!(error["request_id"], "corrupt-edit");
        assert_eq!(error["plan_id"], plan.id);
    }
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "x"
    );
    assert_eq!(fs::read(path).unwrap(), journal);
}

#[test]
fn unavailable_prior_journal_cannot_be_reported_as_a_known_non_commit() {
    let root = TempDir::new().unwrap();
    let (_, plan) = prepare(root.path(), &[("file.txt", "x", "xx")], "unavailable-edit");
    let (code, _) = run(root.path(), &["commit", &plan.id], None);
    assert_eq!(code, 0);
    let journal = root
        .path()
        .join(".ultra-edit/journals")
        .join(format!("{}.jsonl", plan.id));
    fs::rename(&journal, journal.with_extension("saved")).unwrap();
    fs::create_dir(&journal).unwrap();
    let (code, error) = run(root.path(), &["commit", &plan.id], None);
    assert_eq!(code, 3, "{error}");
    assert_eq!(error["error"]["code"], "UNSAFE_STATE_PATH");
    assert_eq!(error["commit"], "outcome_unknown");
    assert_eq!(error["request_id"], "unavailable-edit");
    assert_eq!(error["plan_id"], plan.id);
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "xx"
    );
}

#[test]
fn concurrent_commit_processes_return_one_recorded_outcome() {
    let root = TempDir::new().unwrap();
    let (request, plan) = prepare(root.path(), &[("file.txt", "x", "xx")], "concurrent-edit");
    let storage = Storage::open(root.path()).unwrap();
    let lock = storage.lock().unwrap();
    let first = start(root.path(), &["commit", &plan.id], None);
    let second = start(root.path(), &["commit", &plan.id], None);
    drop(lock);
    let (first_code, first) = finish(first);
    let (second_code, second) = finish(second);
    assert_eq!(first_code, 0, "{first}");
    assert_eq!(second_code, 0, "{second}");
    assert_eq!(first, second);
    assert_eq!(first["commit"], "committed");
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "xx"
    );
    let (code, receipt) = run(root.path(), &["receipt", "concurrent-edit"], None);
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["files"][0]["changes_applied"], 1);
    assert_eq!(receipt["files"][0]["status"], "committed");
    let (code, replay) = run(root.path(), &["edit"], Some(&request));
    assert_eq!(code, 0, "{replay}");
    assert_eq!(replay, first);
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "xx"
    );
}
