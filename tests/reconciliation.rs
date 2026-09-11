use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use ultra_edit::storage::Storage;
use ultra_edit::workspace::{EditResult, Evidence};
use ultra_edit::*;

fn prepare(workspace: &Workspace, paths: &[&str], request_id: &str) -> PreparedPlan {
    let files = paths
        .iter()
        .enumerate()
        .map(|(index, path)| FileRequest {
            base: workspace.read(path).unwrap().id,
            changes: vec![Change {
                id: format!("change-{index}"),
                target: Target::Exact {
                    old: "before".into(),
                    scope: None,
                },
                text: "after".into(),
            }],
        })
        .collect();
    let preview = workspace
        .prepare(EditRequest {
            request_id: request_id.into(),
            files,
        })
        .unwrap();
    assert!(preview.ready, "{preview:?}");
    let Evidence::Plan(plan) = workspace.evidence(&preview.reference).unwrap() else {
        panic!("Expected a prepared plan")
    };
    plan
}

fn journal_path(root: &Path, plan: &PreparedPlan) -> PathBuf {
    root.join(".ultra-edit/journals")
        .join(format!("{}.jsonl", plan.id))
}

fn record(payload: Value) -> Vec<u8> {
    let checksum = digest(&serde_json::to_vec(&payload).unwrap());
    let mut bytes = serde_json::to_vec(&json!({"checksum": checksum, "payload": payload})).unwrap();
    bytes.push(b'\n');
    bytes
}

fn mark_uncertain(root: &Path, plan: &PreparedPlan) {
    // A commit indexes its plan as uncertain before its first write; the index is what
    // later mutations scan, so a crash fixture needs its marker as well as its journal.
    let uncertain = root.join(".ultra-edit/uncertain");
    fs::create_dir_all(&uncertain).unwrap();
    File::create_new(uncertain.join(&plan.id)).unwrap();
}

fn interrupted(root: &Path, plan: &PreparedPlan) -> Vec<u8> {
    // A durable intent without an outcome models either side of an interrupted
    // target replacement; the target's present bytes cannot distinguish them.
    mark_uncertain(root, plan);
    let mut bytes = record(json!({
        "event": "begin", "plan_id": plan.id,
        "plan_digest": digest(&serde_json::to_vec(plan).unwrap()),
        "file_count": plan.files.len(),
    }));
    bytes.extend(record(json!({"event": "intent", "index": 0})));
    let mut journal = File::create_new(journal_path(root, plan)).unwrap();
    journal.write_all(&bytes).unwrap();
    journal.sync_all().unwrap();
    bytes
}

fn fixture() -> (TempDir, Workspace, PreparedPlan, Vec<u8>) {
    let root = TempDir::new().unwrap();
    fs::write(
        root.path().join("file.txt"),
        "\u{feff}before\r\n\tkeep  \nfinal",
    )
    .unwrap();
    fs::write(root.path().join("second.txt"), "before").unwrap();
    let workspace = Workspace::open(root.path()).unwrap();
    let plan = prepare(&workspace, &["file.txt", "second.txt"], "interrupted");
    let journal = interrupted(root.path(), &plan);
    (root, workspace, plan, journal)
}

fn accept(inspection: &Inspection) -> ReconciliationRequest {
    ReconciliationRequest {
        inspection: inspection.id.clone(),
        decision: ReconciliationDecision::AcceptCurrent,
        note: "Reviewed current files and retained journal; accept current state.".into(),
    }
}

#[test]
fn pruning_blocks_unresolved_outcomes_and_retains_all_reconciled_evidence() {
    let (root, workspace, plan, journal) = fixture();
    let unused = workspace.read("second.txt").unwrap();
    let unused_path = root
        .path()
        .join(".ultra-edit/snapshots")
        .join(format!("{}.json", unused.id));
    assert_eq!(
        workspace
            .prune_snapshots(Duration::ZERO, true)
            .unwrap_err()
            .code,
        "RECONCILIATION_REQUIRED"
    );
    assert!(unused_path.exists());
    let inspection = workspace.inspect(&plan.id).unwrap();
    let reconciliation = workspace.reconcile(accept(&inspection)).unwrap();
    let pruned = workspace.prune_snapshots(Duration::ZERO, true).unwrap();
    assert_eq!(pruned.removed_snapshots, 1);
    assert!(!unused_path.exists());
    for file in &plan.files {
        assert!(workspace.evidence(&file.base.id).is_ok());
    }
    let Evidence::Inspection(retained) = workspace.evidence(&inspection.id).unwrap() else {
        panic!()
    };
    assert_eq!(*retained, inspection);
    assert_eq!(
        workspace.inspect(&plan.id).unwrap().reconciliation,
        Some(reconciliation)
    );
    assert_eq!(fs::read(journal_path(root.path(), &plan)).unwrap(), journal);
    assert_eq!(
        workspace.receipt("interrupted").unwrap().unwrap().commit,
        CommitStatus::OutcomeUnknown
    );
}

fn assert_fresh_edit(workspace: &Workspace, request_id: &str) {
    let plan = prepare(workspace, &["second.txt"], request_id);
    assert_eq!(
        workspace.commit(&plan.id).unwrap().commit,
        CommitStatus::Committed
    );
}

#[test]
fn inspection_and_resolution_preserve_history_and_allow_fresh_edits_after_restart() {
    for current in [
        "\u{feff}before\r\n\tkeep  \nfinal",
        "\u{feff}after\r\n\tkeep  \nfinal",
        "external work\r\n",
    ] {
        let (root, workspace, plan, journal) = fixture();
        fs::write(root.path().join("file.txt"), current).unwrap();
        let original = workspace.receipt("interrupted").unwrap().unwrap();
        let inspection = workspace.inspect(&plan.id).unwrap();
        assert_eq!(inspection.plan, plan);
        assert_eq!(inspection.journal.as_bytes(), journal);
        assert_eq!(inspection.journal_digest, digest(&journal));
        assert_eq!(inspection.receipt.as_ref(), Some(&original));
        assert!(inspection.receipt_error.is_none());
        assert!(inspection.reconciliation.is_none());
        assert_eq!(inspection.files.len(), 2);
        assert_eq!(inspection.files[0].path, plan.files[0].base.path);
        let ObservedState::File {
            digest: actual,
            content,
        } = &inspection.files[0].state
        else {
            panic!("Expected current file bytes")
        };
        assert_eq!(*actual, digest(current.as_bytes()));
        assert_eq!(content.as_bytes(), current.as_bytes());
        assert_eq!(original.commit, CommitStatus::OutcomeUnknown);
        assert_eq!(original.files[0].status, FileStatus::OutcomeUnknown);
        assert_eq!(original.files[1].status, FileStatus::NotCommitted);
        assert_eq!(original.undo, None);

        let resolution = workspace.reconcile(accept(&inspection)).unwrap();
        assert_eq!(resolution.plan_id, plan.id);
        assert_eq!(
            resolution.plan_digest,
            digest(&serde_json::to_vec(&plan).unwrap())
        );
        assert_eq!(resolution.journal_digest, digest(&journal));
        assert_eq!(resolution.request, accept(&inspection));
        drop(workspace);
        let workspace = Workspace::open(root.path()).unwrap();
        assert_eq!(
            workspace.receipt("interrupted").unwrap(),
            Some(original.clone())
        );
        assert_eq!(workspace.commit(&plan.id).unwrap(), original);
        assert_eq!(
            workspace
                .retry(&plan.id, "retry-reconciled")
                .unwrap_err()
                .code,
            "RETRY_CLOSED"
        );
        assert_eq!(
            workspace.prepare(plan.request.clone()).unwrap().receipt,
            Some(original.clone())
        );
        let EditResult::Completed { receipt, .. } = workspace.edit(plan.request.clone()).unwrap()
        else {
            panic!("Expected historical receipt")
        };
        assert_eq!(receipt, original);
        assert_eq!(
            workspace.undo(&plan.id, "undo-old").unwrap_err().code,
            "OUTCOME_UNKNOWN"
        );
        assert_eq!(
            workspace
                .repair(
                    &plan.id,
                    "repair-old",
                    plan.request.files[0].changes.clone()
                )
                .unwrap_err()
                .code,
            "REPAIR_CLOSED"
        );
        assert_eq!(
            workspace.inspect(&plan.id).unwrap().reconciliation,
            Some(resolution)
        );
        assert_fresh_edit(&workspace, "fresh-edit");
        assert_eq!(
            fs::read(root.path().join("file.txt")).unwrap(),
            current.as_bytes()
        );
        assert_eq!(fs::read(journal_path(root.path(), &plan)).unwrap(), journal);
    }
}

#[test]
fn stale_file_or_journal_observations_cannot_be_acknowledged() {
    for change_journal in [false, true] {
        let (root, workspace, plan, _) = fixture();
        let inspection = workspace.inspect(&plan.id).unwrap();
        if change_journal {
            fs::OpenOptions::new()
                .append(true)
                .open(journal_path(root.path(), &plan))
                .unwrap()
                .write_all(b"{\"checksum\":\"torn")
                .unwrap();
        } else {
            fs::write(root.path().join("file.txt"), "new external work").unwrap();
        }
        assert_eq!(
            workspace.reconcile(accept(&inspection)).unwrap_err().code,
            "STALE_INSPECTION"
        );
        let fresh = workspace.inspect(&plan.id).unwrap();
        assert!(fresh.reconciliation.is_none());
        workspace.reconcile(accept(&fresh)).unwrap();
        assert_fresh_edit(&workspace, "fresh-after-reinspection");
    }
}

#[test]
fn recorded_replacement_uncertainty_can_be_resolved_without_changing_its_receipt() {
    let (root, workspace, plan, mut journal) = fixture();
    journal.extend(record(json!({
        "event": "outcome", "index": 0, "status": "outcome_unknown",
        "error": "REPLACEMENT_UNCERTAIN: persistence operation returned an error",
    })));
    journal.extend(record(json!({
        "event": "outcome", "index": 1, "status": "not_committed",
        "error": "NOT_ATTEMPTED: an earlier persistence operation failed",
    })));
    journal.extend(record(json!({"event": "finished"})));
    fs::write(journal_path(root.path(), &plan), &journal).unwrap();
    fs::write(root.path().join("file.txt"), &plan.files[0].output).unwrap();
    let original = workspace.receipt("interrupted").unwrap().unwrap();
    let inspection = workspace.inspect(&plan.id).unwrap();
    workspace.reconcile(accept(&inspection)).unwrap();
    assert_eq!(workspace.commit(&plan.id).unwrap(), original);
    assert_eq!(original.commit, CommitStatus::OutcomeUnknown);
    assert_eq!(original.files[0].changes_applied, 0);
    assert_eq!(original.undo, None);
    assert_fresh_edit(&workspace, "fresh-after-recorded-uncertainty");
    assert_eq!(fs::read(journal_path(root.path(), &plan)).unwrap(), journal);
}

#[test]
fn absent_target_reappearing_invalidates_its_inspection() {
    let (root, workspace, plan, _) = fixture();
    let target = root.path().join("file.txt");
    fs::rename(&target, root.path().join("file.saved")).unwrap();
    let inspection = workspace.inspect(&plan.id).unwrap();
    assert_eq!(inspection.files[0].state, ObservedState::Missing);
    fs::write(&target, "recreated externally").unwrap();
    assert_eq!(
        workspace.reconcile(accept(&inspection)).unwrap_err().code,
        "STALE_INSPECTION"
    );
    assert_eq!(fs::read_to_string(target).unwrap(), "recreated externally");
    assert!(
        workspace
            .inspect(&plan.id)
            .unwrap()
            .reconciliation
            .is_none()
    );
}

#[test]
fn repeated_resolution_returns_original_record_without_accepting_different_arguments() {
    let (root, workspace, plan, journal) = fixture();
    let inspection = workspace.inspect(&plan.id).unwrap();
    let request = accept(&inspection);
    let original = workspace.reconcile(request.clone()).unwrap();
    fs::write(root.path().join("file.txt"), "later work").unwrap();
    drop(workspace);
    let workspace = Workspace::open(root.path()).unwrap();
    assert_eq!(workspace.reconcile(request.clone()).unwrap(), original);
    let mut changed = request;
    changed.note = "A different operator decision".into();
    assert_eq!(
        workspace.reconcile(changed).unwrap_err().code,
        "RECONCILIATION_CONFLICT"
    );
    let new_inspection = workspace.inspect(&plan.id).unwrap();
    assert_eq!(
        workspace
            .reconcile(accept(&new_inspection))
            .unwrap_err()
            .code,
        "RECONCILIATION_CONFLICT"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "later work"
    );
    assert_eq!(fs::read(journal_path(root.path(), &plan)).unwrap(), journal);
}

#[test]
fn every_uncertain_plan_requires_its_own_resolution() {
    let (root, workspace, first, _) = fixture();
    let second = prepare(&workspace, &["second.txt"], "second-interrupted");
    interrupted(root.path(), &second);
    let inspection = workspace.inspect(&first.id).unwrap();
    workspace.reconcile(accept(&inspection)).unwrap();
    let fresh = prepare(&workspace, &["second.txt"], "fresh-blocked");
    assert_eq!(
        workspace.commit(&fresh.id).unwrap_err().code,
        "RECONCILIATION_REQUIRED"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("second.txt")).unwrap(),
        "before"
    );
    let inspection = workspace.inspect(&second.id).unwrap();
    workspace.reconcile(accept(&inspection)).unwrap();
    assert_eq!(
        workspace.commit(&fresh.id).unwrap().commit,
        CommitStatus::Committed
    );
}

#[test]
fn corrupt_journal_bytes_remain_inspectable_and_historical_errors_survive_resolution() {
    for corruption in [0, 1, 2, 3] {
        let (root, workspace, plan, mut journal) = fixture();
        match corruption {
            0 => journal.clear(),
            1 => journal = b"{\"checksum\":\"torn-header".to_vec(),
            2 => journal.extend_from_slice(
                b"{\"checksum\":\"bad\",\"payload\":{\"event\":\"finished\"}}\n",
            ),
            _ => journal.extend_from_slice(&[0xff, b'\n']),
        }
        fs::write(journal_path(root.path(), &plan), &journal).unwrap();
        let inspection = workspace.inspect(&plan.id).unwrap();
        assert_eq!(inspection.journal.as_bytes(), journal);
        assert_eq!(inspection.journal_digest, digest(&journal));
        assert!(inspection.receipt.is_none());
        assert_eq!(
            inspection.receipt_error.as_ref().unwrap().code,
            "JOURNAL_CORRUPT"
        );
        workspace.reconcile(accept(&inspection)).unwrap();
        drop(workspace);
        let workspace = Workspace::open(root.path()).unwrap();
        for error in [
            workspace.receipt("interrupted").unwrap_err(),
            workspace.commit(&plan.id).unwrap_err(),
            workspace.prepare(plan.request.clone()).unwrap_err(),
            workspace.edit(plan.request.clone()).unwrap_err(),
        ] {
            assert_eq!(error.code, "JOURNAL_CORRUPT");
            assert_eq!(error.commit, Some(CommitStatus::OutcomeUnknown));
        }
        assert_fresh_edit(&workspace, "fresh-after-corruption");
        assert_eq!(fs::read(journal_path(root.path(), &plan)).unwrap(), journal);
    }
}

#[test]
fn changed_or_missing_journal_after_resolution_blocks_replay_and_fresh_mutations() {
    for remove_journal in [false, true] {
        let (root, workspace, plan, _) = fixture();
        let inspection = workspace.inspect(&plan.id).unwrap();
        let request = accept(&inspection);
        workspace.reconcile(request.clone()).unwrap();
        let fresh = prepare(&workspace, &["second.txt"], "fresh-after-tampering");
        let path = journal_path(root.path(), &plan);
        if remove_journal {
            fs::rename(&path, path.with_extension("saved")).unwrap();
        } else {
            fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b"torn")
                .unwrap();
        }
        for error in [
            workspace.receipt("interrupted").unwrap_err(),
            workspace.commit(&plan.id).unwrap_err(),
            workspace.commit(&fresh.id).unwrap_err(),
            workspace.reconcile(request).unwrap_err(),
        ] {
            assert_eq!(error.code, "RECONCILIATION_EVIDENCE_CHANGED");
        }
        assert_eq!(
            fs::read_to_string(root.path().join("second.txt")).unwrap(),
            "before"
        );
    }
}

#[test]
fn binary_and_missing_current_targets_are_retained_without_inventing_text_or_writes() {
    let (root, workspace, plan, journal) = fixture();
    let binary = [0xff, 0xfe, 0, b'\r', b'\n'];
    fs::write(root.path().join("file.txt"), binary).unwrap();
    fs::rename(
        root.path().join("second.txt"),
        root.path().join("second.saved"),
    )
    .unwrap();
    let inspection = workspace.inspect(&plan.id).unwrap();
    let ObservedState::File {
        digest: actual,
        content: ByteContent::Binary { bytes },
    } = &inspection.files[0].state
    else {
        panic!("Expected exact binary evidence")
    };
    assert_eq!(*actual, digest(&binary));
    assert_eq!(bytes, &binary);
    assert_eq!(inspection.files[1].state, ObservedState::Missing);
    workspace.reconcile(accept(&inspection)).unwrap();
    fs::write(root.path().join("fresh.txt"), "before").unwrap();
    let fresh = prepare(&workspace, &["fresh.txt"], "fresh-unrelated");
    assert_eq!(
        workspace.commit(&fresh.id).unwrap().commit,
        CommitStatus::Committed
    );
    assert_eq!(fs::read(root.path().join("file.txt")).unwrap(), binary);
    assert!(!root.path().join("second.txt").exists());
    assert_eq!(fs::read(journal_path(root.path(), &plan)).unwrap(), journal);
}

#[test]
fn unavailable_current_evidence_cannot_be_resolved() {
    for directory in [false, true] {
        let (root, workspace, plan, _) = fixture();
        let path = root.path().join("file.txt");
        if directory {
            fs::rename(&path, root.path().join("file.saved")).unwrap();
            fs::create_dir(&path).unwrap();
        } else {
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(16 * 1024 * 1024 + 1)
                .unwrap();
        }
        let inspection = workspace.inspect(&plan.id).unwrap();
        assert!(matches!(
            inspection.files[0].state,
            ObservedState::Unavailable { .. }
        ));
        assert_eq!(
            workspace.reconcile(accept(&inspection)).unwrap_err().code,
            "INSPECTION_INCOMPLETE"
        );
        assert!(
            workspace
                .inspect(&plan.id)
                .unwrap()
                .reconciliation
                .is_none()
        );
    }
}

#[cfg(unix)]
#[test]
fn redirected_targets_and_missing_targets_under_redirected_parents_are_unavailable() {
    for missing in [false, true] {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        fs::write(root.path().join("nested/file.txt"), "before").unwrap();
        if !missing {
            fs::write(outside.path().join("file.txt"), "outside bytes").unwrap();
        }
        let workspace = Workspace::open(root.path()).unwrap();
        let plan = prepare(&workspace, &["nested/file.txt"], "redirected");
        interrupted(root.path(), &plan);
        fs::rename(root.path().join("nested"), root.path().join("saved")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("nested")).unwrap();
        let inspection = workspace.inspect(&plan.id).unwrap();
        assert!(matches!(
            inspection.files[0].state,
            ObservedState::Unavailable { .. }
        ));
        assert_eq!(
            workspace.reconcile(accept(&inspection)).unwrap_err().code,
            "INSPECTION_INCOMPLETE"
        );
    }
}

#[test]
fn known_outcomes_and_invalid_operator_notes_cannot_be_reconciled() {
    let (root, workspace, plan, _) = fixture();
    let inspection = workspace.inspect(&plan.id).unwrap();
    for note in [
        "".to_string(),
        " \r\n\t".to_string(),
        "x".repeat(1024 * 1024),
    ] {
        let mut request = accept(&inspection);
        request.note = note;
        assert_eq!(
            workspace.reconcile(request).unwrap_err().code,
            "INVALID_RECONCILIATION"
        );
    }
    workspace.reconcile(accept(&inspection)).unwrap();
    let known = prepare(&workspace, &["second.txt"], "known");
    assert_eq!(
        workspace.inspect(&known.id).unwrap_err().code,
        "NO_COMMIT_ATTEMPT"
    );
    workspace.commit(&known.id).unwrap();
    let inspection = workspace.inspect(&known.id).unwrap();
    assert_eq!(
        workspace.reconcile(accept(&inspection)).unwrap_err().code,
        "NOT_UNCERTAIN"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("second.txt")).unwrap(),
        "after"
    );
}

#[test]
fn unpublished_resolution_and_failed_publication_keep_mutations_blocked() {
    let (root, workspace, plan, journal) = fixture();
    let inspection = workspace.inspect(&plan.id).unwrap();
    let request = accept(&inspection);
    let directory = root.path().join(".ultra-edit/reconciliations");
    // An interrupted temporary write is not a published resolution.
    fs::write(directory.join(".ultra-edit-unpublished"), b"torn object").unwrap();
    drop(workspace);
    let workspace = Workspace::open(root.path()).unwrap();
    let fresh = prepare(&workspace, &["second.txt"], "after-unpublished-resolution");
    assert_eq!(
        workspace.commit(&fresh.id).unwrap_err().code,
        "RECONCILIATION_REQUIRED"
    );

    let destination = directory.join(format!("{}.json", plan.id));
    fs::create_dir(&destination).unwrap();
    assert_eq!(
        workspace.reconcile(request.clone()).unwrap_err().code,
        "UNSAFE_STATE_PATH"
    );
    assert!(workspace.commit(&fresh.id).is_err());
    assert_eq!(fs::read(journal_path(root.path(), &plan)).unwrap(), journal);
    assert_eq!(
        fs::read_to_string(root.path().join("second.txt")).unwrap(),
        "before"
    );

    fs::rename(&destination, destination.with_extension("saved")).unwrap();
    let resolution = workspace.reconcile(request.clone()).unwrap();
    drop(workspace);
    let workspace = Workspace::open(root.path()).unwrap();
    assert_eq!(workspace.reconcile(request).unwrap(), resolution);
    assert_eq!(
        workspace.commit(&fresh.id).unwrap().commit,
        CommitStatus::Committed
    );
}

#[test]
fn damaged_resolution_or_missing_inspection_prevents_further_mutations() {
    for damage_resolution in [false, true] {
        let (root, workspace, plan, journal) = fixture();
        let inspection = workspace.inspect(&plan.id).unwrap();
        let request = accept(&inspection);
        workspace.reconcile(request.clone()).unwrap();
        let fresh = prepare(&workspace, &["second.txt"], "after-damaged-resolution");
        if damage_resolution {
            fs::write(
                root.path()
                    .join(".ultra-edit/reconciliations")
                    .join(format!("{}.json", plan.id)),
                b"{\"checksum\":\"torn",
            )
            .unwrap();
        } else {
            let path = root
                .path()
                .join(".ultra-edit/inspections")
                .join(format!("{}.json", inspection.id));
            fs::rename(&path, path.with_extension("saved")).unwrap();
        }
        drop(workspace);
        let workspace = Workspace::open(root.path()).unwrap();
        let expected = if damage_resolution {
            "STORE_CORRUPT"
        } else {
            "REFERENCE_NOT_FOUND"
        };
        for error in [
            workspace.receipt("interrupted").unwrap_err(),
            workspace.commit(&plan.id).unwrap_err(),
            workspace.commit(&fresh.id).unwrap_err(),
            workspace.reconcile(request).unwrap_err(),
        ] {
            assert_eq!(error.code, expected);
        }
        assert_eq!(
            fs::read_to_string(root.path().join("second.txt")).unwrap(),
            "before"
        );
        assert_eq!(fs::read(journal_path(root.path(), &plan)).unwrap(), journal);
    }
}

#[test]
fn aggregate_current_evidence_is_bounded_when_external_files_grow() {
    let root = TempDir::new().unwrap();
    let paths = ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"];
    for path in paths {
        fs::write(root.path().join(path), "before").unwrap();
    }
    let workspace = Workspace::open(root.path()).unwrap();
    let plan = prepare(&workspace, &paths, "external-growth");
    interrupted(root.path(), &plan);
    let bytes = vec![b'x'; 16 * 1024 * 1024];
    for path in paths {
        fs::write(root.path().join(path), &bytes).unwrap();
    }
    let inspection = workspace.inspect(&plan.id).unwrap();
    assert!(
        inspection.files[..4]
            .iter()
            .all(|file| matches!(file.state, ObservedState::File { .. }))
    );
    assert!(
        matches!(&inspection.files[4].state, ObservedState::Unavailable { code, .. } if code == "RESOURCE_LIMIT")
    );
    assert_eq!(
        workspace.reconcile(accept(&inspection)).unwrap_err().code,
        "INSPECTION_INCOMPLETE"
    );
}

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

#[test]
fn cli_requires_an_explicit_supported_decision_and_rejects_unknown_options() {
    let (root, workspace, plan, journal) = fixture();
    let inspection = workspace.inspect(&plan.id).unwrap();
    let valid = serde_json::to_value(accept(&inspection)).unwrap();
    for field in ["missing_decision", "unsupported_decision", "unknown_option"] {
        let mut request = valid.clone();
        match field {
            "missing_decision" => {
                request.as_object_mut().unwrap().remove("decision");
            }
            "unsupported_decision" => request["decision"] = json!("declare_committed"),
            _ => request["force"] = json!(true),
        }
        let (code, error) = finish(start(root.path(), &["reconcile"], Some(&request)));
        assert_eq!(code, 2, "{error}");
        assert_eq!(error["error"]["code"], "INVALID_JSON");
        assert!(
            workspace
                .inspect(&plan.id)
                .unwrap()
                .reconciliation
                .is_none()
        );
    }
    assert_eq!(fs::read(journal_path(root.path(), &plan)).unwrap(), journal);
    let (code, resolution) = finish(start(root.path(), &["reconcile"], Some(&valid)));
    assert_eq!(code, 0, "{resolution}");
}

#[test]
fn cli_inspection_and_concurrent_resolution_persist_one_operator_decision() {
    let (root, workspace, plan, journal) = fixture();
    drop(workspace);
    let (code, value) = finish(start(root.path(), &["inspect", &plan.id], None));
    assert_eq!(code, 0, "{value}");
    let inspection: Inspection = serde_json::from_value(value.clone()).unwrap();
    let (code, evidence) = finish(start(root.path(), &["get", &inspection.id], None));
    assert_eq!(code, 0, "{evidence}");
    assert_eq!(evidence["kind"], "inspection");
    assert_eq!(evidence["value"], value);
    let request = serde_json::to_value(accept(&inspection)).unwrap();
    let mut competing = request.clone();
    competing["note"] = json!("Another operator reviewed the same state.");
    let storage = Storage::open(root.path()).unwrap();
    let lock = storage.lock().unwrap();
    let first = start(root.path(), &["reconcile"], Some(&request));
    let second = start(root.path(), &["reconcile"], Some(&competing));
    drop(lock);
    let results = [finish(first), finish(second)];
    let successes: Vec<_> = results.iter().filter(|(code, _)| *code == 0).collect();
    assert_eq!(successes.len(), 1, "{results:?}");
    let errors: Vec<_> = results.iter().filter(|(code, _)| *code != 0).collect();
    assert_eq!(errors.len(), 1, "{results:?}");
    assert_eq!(errors[0].1["error"]["code"], "RECONCILIATION_CONFLICT");
    let original: Reconciliation = serde_json::from_value(successes[0].1.clone()).unwrap();
    fs::write(root.path().join("file.txt"), "later external edit").unwrap();
    let exact_retry = serde_json::to_value(&original.request).unwrap();
    let (code, retry) = finish(start(root.path(), &["reconcile"], Some(&exact_retry)));
    assert_eq!(code, 0, "{retry}");
    assert_eq!(retry, successes[0].1);
    let (code, old) = finish(start(root.path(), &["commit", &plan.id], None));
    assert_eq!(code, 3, "{old}");
    assert_eq!(old["commit"], "outcome_unknown");
    let workspace = Workspace::open(root.path()).unwrap();
    assert_fresh_edit(&workspace, "fresh-after-cli");
    assert_eq!(fs::read(journal_path(root.path(), &plan)).unwrap(), journal);
}
