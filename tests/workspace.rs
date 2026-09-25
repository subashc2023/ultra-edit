use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::json;
use tempfile::TempDir;
use ultra_edit::storage::Storage;
use ultra_edit::workspace::{
    EditResult, Evidence, REPLAY_NOTICE, receipt_report, repair_request_id, resolve_ids,
    undo_request_id,
};
use ultra_edit::*;

fn setup(text: &str) -> (TempDir, Workspace, Snapshot) {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("file.txt"), text).unwrap();
    let workspace = Workspace::open(dir.path()).unwrap();
    let snapshot = workspace.read("file.txt").unwrap();
    (dir, workspace, snapshot)
}

fn change(id: &str, old: &str, new: &str) -> Change {
    Change {
        id: id.into(),
        target: Target::Exact {
            old: old.into(),
            scope: None,
        },
        text: new.into(),
    }
}

fn request(id: &str, snapshot: &Snapshot, changes: Vec<Change>) -> EditRequest {
    EditRequest {
        request_id: id.into(),
        files: vec![FileRequest {
            base: snapshot.id.clone(),
            changes,
        }],
    }
}

fn completed(result: EditResult) -> Receipt {
    match result {
        EditResult::Completed { receipt, .. } => receipt,
        other => panic!("Expected a receipt, got {other:?}"),
    }
}

/// The receipt, whether it was replayed, and the report.
fn outcome(result: EditResult) -> (Receipt, bool, String) {
    match result {
        EditResult::Completed {
            receipt,
            replayed,
            report,
        } => (receipt, replayed, report),
        other => panic!("Expected a receipt, got {other:?}"),
    }
}

fn stored_request(workspace: &Workspace, reference: &str) -> EditRequest {
    match workspace.evidence(reference).unwrap() {
        Evidence::Plan(plan) => plan.request,
        Evidence::Draft(draft) => draft.request,
        other => panic!("Expected a plan or draft, got {other:?}"),
    }
}

fn change_ids(request: &EditRequest) -> Vec<&str> {
    request
        .files
        .iter()
        .flat_map(|file| file.changes.iter().map(|change| change.id.as_str()))
        .collect()
}

fn journal_count(dir: &TempDir) -> usize {
    fs::read_dir(dir.path().join(".ultra-edit/journals"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .count()
}

#[test]
fn preview_commits_exact_candidate_and_retries_survive_restart() {
    let (dir, workspace, snapshot) = setup("\u{feff}one\r\ntwo\nthree  ");
    let request = request("edit-1", &snapshot, vec![change("one", "one", "$& NEW")]);
    let preview = workspace.prepare(request.clone()).unwrap();
    assert!(preview.ready);
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        snapshot.text
    );
    let Evidence::Plan(plan) = workspace.evidence(&preview.reference).unwrap() else {
        panic!()
    };
    assert_eq!(plan.files[0].output, "\u{feff}$& NEW\r\ntwo\nthree  ");
    let receipt = workspace.commit(&preview.reference).unwrap();
    assert_eq!(receipt.commit, CommitStatus::Committed);
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        plan.files[0].output
    );
    fs::write(dir.path().join("file.txt"), "later work").unwrap();
    drop(workspace);
    let reopened = Workspace::open(dir.path()).unwrap();
    let repeated_preview = reopened.prepare(request.clone()).unwrap();
    assert_eq!(repeated_preview.receipt.as_ref(), Some(&receipt));
    assert!(repeated_preview.replayed);
    assert!(
        repeated_preview
            .report
            .starts_with(&format!("{REPLAY_NOTICE}\ncommitted:"))
    );
    assert_eq!(completed(reopened.edit(request).unwrap()), receipt);
    assert_eq!(reopened.receipt("edit-1").unwrap(), Some(receipt));
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "later work"
    );
}

#[test]
fn stale_preview_cannot_retarget_an_identical_match_elsewhere() {
    let (dir, workspace, snapshot) = setup("intended: old\nother: stay\n");
    let preview = workspace
        .prepare(request(
            "preview",
            &snapshot,
            vec![change("c", "old", "NEW")],
        ))
        .unwrap();
    let current = "intended: changed\nother: old\n";
    fs::write(dir.path().join("file.txt"), current).unwrap();
    let receipt = workspace.commit(&preview.reference).unwrap();
    assert_eq!(receipt.commit, CommitStatus::NotCommitted);
    assert!(receipt.files.iter().all(|file| file.changes_applied == 0));
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        current
    );
}

#[test]
fn preflight_retry_preserves_history_candidates_and_request_identity_after_restart() {
    let (dir, workspace, snapshot) = setup("one");
    fs::write(dir.path().join("second.txt"), "one").unwrap();
    let second = workspace.read("second.txt").unwrap();
    let mut original_request = request("original", &snapshot, vec![change("first", "one", "two")]);
    original_request.files.push(FileRequest {
        base: second.id,
        changes: vec![change("second", "one", "two")],
    });
    let preview = workspace.prepare(original_request.clone()).unwrap();
    assert_eq!(
        workspace
            .retry(&preview.reference, "too-early")
            .unwrap_err()
            .code,
        "RETRY_CLOSED"
    );
    let target = dir.path().join("second.txt");
    let original_permissions = fs::metadata(&target).unwrap().permissions();
    let mut readonly = original_permissions.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&target, readonly).unwrap();
    let failed = workspace.commit(&preview.reference).unwrap();
    fs::set_permissions(&target, original_permissions).unwrap();
    assert_eq!(failed.commit, CommitStatus::NotCommitted);
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "one"
    );
    let original_journal = fs::read(
        dir.path()
            .join(".ultra-edit/journals")
            .join(format!("{}.jsonl", preview.reference)),
    )
    .unwrap();
    drop(workspace);
    let workspace = Workspace::open(dir.path()).unwrap();
    let retried = completed(workspace.retry(&preview.reference, "retry").unwrap());
    assert_eq!(retried.commit, CommitStatus::Committed);
    assert_ne!(retried.plan_id, failed.plan_id);
    let Evidence::Plan(original) = workspace.evidence(&preview.reference).unwrap() else {
        panic!()
    };
    let Evidence::Plan(attempt) = workspace.evidence(&retried.plan_id).unwrap() else {
        panic!()
    };
    assert_eq!(attempt.files, original.files);
    assert_eq!(attempt.request.files, original.request.files);
    assert_eq!(attempt.request.request_id, "retry");
    assert_eq!(workspace.commit(&preview.reference).unwrap(), failed);
    assert_eq!(completed(workspace.edit(original_request).unwrap()), failed);
    assert_eq!(workspace.receipt("original").unwrap(), Some(failed));
    assert_eq!(
        fs::read(
            dir.path()
                .join(".ultra-edit/journals")
                .join(format!("{}.jsonl", preview.reference))
        )
        .unwrap(),
        original_journal
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "two");
    fs::write(&target, "later work").unwrap();
    drop(workspace);
    let workspace = Workspace::open(dir.path()).unwrap();
    assert_eq!(
        completed(workspace.retry(&preview.reference, "retry").unwrap()),
        retried
    );
    assert_eq!(workspace.receipt("retry").unwrap(), Some(retried.clone()));
    assert_eq!(fs::read_to_string(&target).unwrap(), "later work");
    assert_eq!(
        workspace.retry(&retried.plan_id, "retry").unwrap_err().code,
        "REQUEST_ID_REUSED"
    );
    assert_eq!(
        workspace
            .retry(&retried.plan_id, "retry-committed")
            .unwrap_err()
            .code,
        "RETRY_CLOSED"
    );
    assert_eq!(
        workspace
            .retry(&preview.reference, "original")
            .unwrap_err()
            .code,
        "REQUEST_ID_REUSED"
    );
}

#[test]
fn preflight_retry_rechecks_the_original_bytes_without_retargeting() {
    let (dir, workspace, snapshot) = setup("one");
    let preview = workspace
        .prepare(request(
            "original",
            &snapshot,
            vec![change("c", "one", "two")],
        ))
        .unwrap();
    let target = dir.path().join("file.txt");
    fs::write(&target, "one elsewhere").unwrap();
    assert_eq!(
        workspace.commit(&preview.reference).unwrap().commit,
        CommitStatus::NotCommitted
    );
    let failed_retry = completed(workspace.retry(&preview.reference, "retry-stale").unwrap());
    assert_eq!(failed_retry.commit, CommitStatus::NotCommitted);
    assert!(
        failed_retry.files[0]
            .error
            .as_deref()
            .unwrap()
            .contains("STALE_SNAPSHOT")
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "one elsewhere");
    fs::write(&target, "one").unwrap();
    assert_eq!(
        completed(workspace.retry(&preview.reference, "retry-stale").unwrap()),
        failed_retry
    );
    assert_eq!(
        completed(
            workspace
                .retry(&failed_retry.plan_id, "retry-restored")
                .unwrap()
        )
        .commit,
        CommitStatus::Committed
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "two");
}

#[test]
fn failed_batch_keeps_every_target_unchanged_and_repair_retains_positions() {
    let (dir, workspace, snapshot) = setup("one two three");
    fs::write(dir.path().join("second.txt"), "four five").unwrap();
    let second = workspace.read("second.txt").unwrap();
    let mut request = request(
        "draft",
        &snapshot,
        vec![
            change("first", "one", "ONE"),
            change("second", "missing", "TWO"),
        ],
    );
    request.files.push(FileRequest {
        base: second.id,
        changes: vec![change("third", "also missing", "FOUR")],
    });
    let failed = workspace.prepare(request).unwrap();
    assert!(!failed.ready);
    assert!(
        failed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.change_id.as_deref() == Some("second"))
    );
    assert!(
        failed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.change_id.as_deref() == Some("third"))
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "one two three"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("second.txt")).unwrap(),
        "four five"
    );
    let repaired = workspace
        .repair(
            &failed.reference,
            "repair",
            vec![
                change("second", "two", "TWO"),
                change("third", "four", "FOUR"),
            ],
        )
        .unwrap();
    assert!(repaired.ready);
    let Evidence::Plan(plan) = workspace.evidence(&repaired.reference).unwrap() else {
        panic!()
    };
    assert_eq!(
        plan.request.files[0]
            .changes
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(
        workspace.commit(&repaired.reference).unwrap().commit,
        CommitStatus::Committed
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "ONE TWO three"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("second.txt")).unwrap(),
        "FOUR five"
    );
}

#[test]
fn repaired_preview_revalidates_conflicts_and_never_changes_original_plan() {
    let (_dir, workspace, snapshot) = setup("one two");
    let preview = workspace
        .prepare(request(
            "first",
            &snapshot,
            vec![change("a", "one", "ONE"), change("b", "two", "TWO")],
        ))
        .unwrap();
    let repaired = workspace
        .repair(
            &preview.reference,
            "second",
            vec![change("b", "one", "overlap")],
        )
        .unwrap();
    assert!(!repaired.ready);
    assert!(
        repaired
            .diagnostics
            .iter()
            .any(|d| d.code == "OVERLAPPING_CHANGES")
    );
    assert_eq!(
        workspace.commit(&preview.reference).unwrap().commit,
        CommitStatus::Committed
    );
    assert_eq!(
        workspace
            .repair(
                &preview.reference,
                "third",
                vec![change("b", "two", "THREE")]
            )
            .unwrap_err()
            .code,
        "REPAIR_CLOSED"
    );
}

#[test]
fn changed_arguments_under_a_request_id_fail_even_after_restart() {
    let (dir, workspace, snapshot) = setup("one");
    let original = request("same-id", &snapshot, vec![change("a", "one", "two")]);
    workspace.prepare(original.clone()).unwrap();
    drop(workspace);
    let workspace = Workspace::open(dir.path()).unwrap();
    let mut changed = original;
    changed.files[0].changes[0].text = "three".into();
    assert_eq!(
        workspace.edit(changed).unwrap_err().code,
        "REQUEST_ID_REUSED"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "one"
    );
}

#[test]
fn undo_is_conditional_and_idempotent() {
    let (dir, workspace, snapshot) = setup("one");
    let receipt = completed(
        workspace
            .edit(request(
                "forward",
                &snapshot,
                vec![change("a", "one", "two")],
            ))
            .unwrap(),
    );
    let undone = completed(workspace.undo(&receipt.plan_id, "backward").unwrap());
    assert_eq!(undone.commit, CommitStatus::Committed);
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "one"
    );
    fs::write(dir.path().join("file.txt"), "newer work").unwrap();
    assert_eq!(
        completed(workspace.undo(&receipt.plan_id, "backward").unwrap()),
        undone
    );
    let EditResult::Rejected { preparation } =
        workspace.undo(&receipt.plan_id, "other-undo").unwrap()
    else {
        panic!("Expected stale undo rejection")
    };
    let diagnostic = preparation
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "STALE_SNAPSHOT")
        .unwrap();
    assert!(
        diagnostic
            .message
            .contains("Undo requires the recorded after-bytes")
    );
    assert!(
        diagnostic
            .message
            .contains("already be undone or contain newer work")
    );
    assert!(
        !diagnostic
            .message
            .contains("read a fresh snapshot and submit a new request")
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "newer work"
    );
}

#[test]
fn aliases_cannot_overwrite_separate_candidates() {
    let (dir, workspace, first) = setup("one two");
    let alias = workspace.read("./file.txt").unwrap();
    let mut request = request("aliases", &first, vec![change("a", "one", "ONE")]);
    request.files.push(FileRequest {
        base: alias.id,
        changes: vec![change("b", "two", "TWO")],
    });
    assert!(!workspace.prepare(request).unwrap().ready);
    fs::hard_link(dir.path().join("file.txt"), dir.path().join("link.txt")).unwrap();
    let hardlink = workspace.read("link.txt").unwrap();
    let mut request = self::request("hardlinks", &first, vec![change("a", "one", "ONE")]);
    request.files.push(FileRequest {
        base: hardlink.id,
        changes: vec![change("b", "two", "TWO")],
    });
    let rejected = workspace.prepare(request).unwrap();
    assert!(!rejected.ready);
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|d| d.code == "TARGET_ALIAS")
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "one two"
    );
}

#[test]
fn references_are_workspace_local_and_missing_references_never_rebind() {
    let (_first_dir, first, snapshot) = setup("one");
    let (_second_dir, second, _) = setup("one");
    let rejected = second
        .prepare(request(
            "unavailable",
            &snapshot,
            vec![change("a", "one", "two")],
        ))
        .unwrap();
    assert!(!rejected.ready);
    assert!(first.evidence("p123").is_err());
    assert!(first.evidence("p../../file.txt").is_err());
}

#[test]
fn invalid_snapshot_references_are_not_misreported_as_file_paths() {
    let (_dir, workspace, _) = setup("one");
    let reference = r"\\?\C:\looks-like-a-path";
    let rejected = workspace
        .prepare(EditRequest {
            request_id: "invalid-reference".into(),
            files: vec![FileRequest {
                base: reference.into(),
                changes: vec![change("unused", "one", "two")],
            }],
        })
        .unwrap();
    assert!(!rejected.ready);
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "INVALID_SNAPSHOT")
    );
    assert!(
        rejected
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.file.is_none())
    );
    assert_eq!(
        rejected
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.matches(reference).count())
            .sum::<usize>(),
        1
    );

    let Evidence::Draft(draft) = workspace.evidence(&rejected.reference).unwrap() else {
        panic!("Expected rejected draft evidence")
    };
    assert_eq!(draft.request.files[0].base, reference);
    assert_eq!(draft.diagnostics, rejected.diagnostics);

    let two_unknown = workspace
        .prepare(EditRequest {
            request_id: "two-invalid-references".into(),
            files: vec![
                FileRequest {
                    base: "sdoesnotexist000".into(),
                    changes: vec![change("first", "one", "two")],
                },
                FileRequest {
                    base: reference.into(),
                    changes: vec![change("second", "one", "two")],
                },
            ],
        })
        .unwrap();
    assert!(!two_unknown.ready);
    assert_eq!(
        two_unknown
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>(),
        ["INVALID_SNAPSHOT", "INVALID_SNAPSHOT"]
    );
}

#[test]
fn a_blank_request_id_is_rejected_before_anything_is_persisted() {
    let (dir, workspace, snapshot) = setup("one");
    let error = workspace
        .prepare(request("   ", &snapshot, vec![change("c", "one", "two")]))
        .unwrap_err();
    assert_eq!(error.code, "INVALID_REQUEST_ID");
    for kind in ["drafts", "requests"] {
        let directory = dir.path().join(".ultra-edit").join(kind);
        assert!(
            !directory.exists() || fs::read_dir(&directory).unwrap().next().is_none(),
            "{kind} retained state for a blank request ID"
        );
    }
    assert_eq!(
        workspace.receipt("   ").unwrap_err().code,
        "INVALID_REQUEST_ID"
    );
}

#[test]
fn encoding_errors_and_workspace_escape_are_explicit() {
    let (dir, workspace, _) = setup("valid");
    fs::write(dir.path().join("invalid.txt"), [0xff, 0xfe, 0x61, 0x00]).unwrap();
    assert!(workspace.read("invalid.txt").is_err());
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("outside.txt"), "outside").unwrap();
    assert!(workspace.read(outside.path().join("outside.txt")).is_err());
    assert!(workspace.read(".ultra-edit/coordinator.lock").is_err());
}

#[test]
fn pruning_retains_plans_drafts_and_recent_snapshots_and_defaults_to_no_deletions() {
    let (dir, workspace, planned) = setup("one");
    let preview = workspace
        .prepare(request(
            "planned",
            &planned,
            vec![change("c", "one", "two")],
        ))
        .unwrap();
    assert!(preview.ready);
    let drafted = workspace.read("file.txt").unwrap();
    assert!(
        !workspace
            .prepare(request(
                "drafted",
                &drafted,
                vec![change("c", "missing", "two")]
            ))
            .unwrap()
            .ready
    );
    let recent = workspace.read("file.txt").unwrap();
    let snapshot_path = |id: &str| {
        dir.path()
            .join(".ultra-edit/snapshots")
            .join(format!("{id}.json"))
    };
    let recent_path = snapshot_path(&recent.id);
    fs::File::options()
        .write(true)
        .open(&recent_path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(SystemTime::now() + Duration::from_secs(3600)))
        .unwrap();
    fs::write(dir.path().join("unused.txt"), "discarded source").unwrap();
    let unused = workspace.read("unused.txt").unwrap();
    fs::remove_file(dir.path().join("unused.txt")).unwrap();
    let unused_path = snapshot_path(&unused.id);
    let protected_by_age = workspace
        .prune_snapshots(Duration::from_secs(3600), false)
        .unwrap();
    assert!(protected_by_age.eligible.is_empty());
    assert_eq!(protected_by_age.retained_snapshots, 4);
    let dry_run = workspace.prune_snapshots(Duration::ZERO, false).unwrap();
    assert!(dry_run.dry_run);
    assert_eq!(dry_run.eligible.len(), 1);
    assert_eq!(dry_run.eligible[0].snapshot, unused.id);
    assert_eq!(
        dry_run.eligible_bytes,
        fs::metadata(&unused_path).unwrap().len()
    );
    assert_eq!(dry_run.retained_snapshots, 3);
    assert_eq!(dry_run.removed_snapshots, 0);
    assert!(unused_path.exists());
    let pruned = workspace.prune_snapshots(Duration::ZERO, true).unwrap();
    assert!(!pruned.dry_run);
    assert_eq!(pruned.removed_snapshots, 1);
    assert!(!unused_path.exists());
    for retained in [&planned.id, &drafted.id, &recent.id] {
        assert!(snapshot_path(retained).exists());
        assert!(workspace.evidence(retained).is_ok());
    }
    assert_eq!(
        workspace.commit(&preview.reference).unwrap().commit,
        CommitStatus::Committed
    );
    let repeated = workspace.prune_snapshots(Duration::ZERO, true).unwrap();
    assert_eq!(repeated.removed_snapshots, 0);
    assert_eq!(
        workspace.receipt("planned").unwrap().unwrap().commit,
        CommitStatus::Committed
    );
}

#[test]
fn pruning_validates_retained_state_and_all_candidates_before_deleting_anything() {
    for corrupt_kind in [
        "requests",
        "plans",
        "drafts",
        "inspections",
        "snapshots",
        "journals",
        // Blob collection scans every object directory, including kinds it does not know.
        "custom",
    ] {
        let (dir, workspace, snapshot) = setup("one");
        let preview = workspace
            .prepare(request(
                "original",
                &snapshot,
                vec![change("c", "one", "two")],
            ))
            .unwrap();
        let unused = workspace.read("file.txt").unwrap();
        let candidate_path = dir
            .path()
            .join(".ultra-edit/snapshots")
            .join(format!("{}.json", unused.id));
        let corrupt_directory = dir.path().join(".ultra-edit").join(corrupt_kind);
        fs::create_dir_all(&corrupt_directory).unwrap();
        let extension = if corrupt_kind == "journals" {
            "jsonl"
        } else {
            "json"
        };
        fs::write(
            corrupt_directory.join(format!("zcorrupt.{extension}")),
            "invalid retained state\n",
        )
        .unwrap();
        assert!(
            workspace.prune_snapshots(Duration::ZERO, true).is_err(),
            "{corrupt_kind}"
        );
        assert!(
            candidate_path.exists(),
            "must not delete before finding {corrupt_kind} corruption"
        );
        assert!(workspace.evidence(&preview.reference).is_ok());
    }
}

/// Over the blob threshold, with one `before` line.
fn large(tag: &str) -> String {
    let mut text = String::from("before\n");
    for line in 0..300 {
        text.push_str(&format!("{tag} line {line:04} stays unchanged\n"));
    }
    text
}

fn blob_path(root: &Path, text: &str) -> PathBuf {
    root.join(".ultra-edit/blobs").join(digest(text.as_bytes()))
}

#[test]
fn pruning_removes_unreferenced_and_orphaned_blobs_and_keeps_referenced_ones() {
    let kept = large("kept");
    let (dir, workspace, _) = setup(&kept);
    let planned = workspace.read_range("file.txt", 1, 1).unwrap();
    let preview = workspace
        .prepare(EditRequest {
            request_id: "planned".into(),
            files: vec![FileRequest {
                base: planned.snapshot.clone(),
                changes: vec![Change {
                    id: "c".into(),
                    target: Target::Span {
                        span: "r1".into(),
                        expect: Some("before".into()),
                    },
                    text: "after".into(),
                }],
            }],
        })
        .unwrap();
    assert!(preview.ready);
    let output = kept.replacen("before", "after", 1);
    let discarded = large("discarded");
    fs::write(dir.path().join("other.txt"), &discarded).unwrap();
    workspace.read_range("other.txt", 1, 1).unwrap();
    // An object publication interrupted after its blob was written leaves an orphan;
    // one interrupted while staging leaves a temporary file, which is not a blob.
    let orphan = large("orphan");
    fs::write(blob_path(dir.path(), &orphan), &orphan).unwrap();
    let staged = dir.path().join(".ultra-edit/blobs/.ultra-edit-staged");
    fs::write(&staged, "partial").unwrap();
    let paths = |texts: &[&str]| {
        texts
            .iter()
            .map(|text| blob_path(dir.path(), text))
            .collect::<Vec<_>>()
    };

    let by_age = workspace
        .prune_snapshots(Duration::from_secs(3600), false)
        .unwrap();
    assert!(by_age.eligible.is_empty());
    assert_eq!(by_age.reclaimable_blobs, 1);
    assert_eq!(by_age.reclaimable_blob_bytes, orphan.len() as u64);
    assert_eq!(by_age.retained_blobs, 3);

    let dry_run = workspace.prune_snapshots(Duration::ZERO, false).unwrap();
    assert_eq!(dry_run.eligible.len(), 2);
    assert_eq!(dry_run.reclaimable_blobs, 2);
    assert_eq!(
        dry_run.reclaimable_blob_bytes,
        (discarded.len() + orphan.len()) as u64
    );
    assert_eq!(dry_run.retained_blobs, 2);
    assert_eq!(dry_run.removed_blobs, 0);
    for path in paths(&[&kept, &output, &discarded, &orphan]) {
        assert!(path.exists(), "{}", path.display());
    }

    let pruned = workspace.prune_snapshots(Duration::ZERO, true).unwrap();
    assert_eq!(pruned.removed_snapshots, 2);
    assert_eq!(pruned.reclaimable_blobs, 2);
    assert_eq!(pruned.removed_blobs, 2);
    assert_eq!(pruned.retained_blobs, 2);
    for path in paths(&[&discarded, &orphan]) {
        assert!(!path.exists(), "{}", path.display());
    }
    for path in paths(&[&kept, &output]) {
        assert!(path.exists(), "{}", path.display());
    }
    assert!(staged.exists());
    let Evidence::Snapshot(snapshot) = workspace.evidence(&planned.snapshot).unwrap() else {
        panic!("Expected snapshot evidence")
    };
    assert_eq!(snapshot.text, kept);
    let receipt = workspace.commit(&preview.reference).unwrap();
    assert_eq!(receipt.commit, CommitStatus::Committed);
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        output
    );
    let repeated = workspace.prune_snapshots(Duration::ZERO, true).unwrap();
    assert_eq!(repeated.reclaimable_blobs, 0);
    assert_eq!(repeated.removed_blobs, 0);
    let after = receipt.files[0].after.clone().unwrap();
    let Evidence::Snapshot(snapshot) = workspace.evidence(&after).unwrap() else {
        panic!("Expected snapshot evidence")
    };
    assert_eq!(snapshot.text, output);
}

#[test]
fn pruning_keeps_blobs_of_any_object_kind_and_refuses_missing_blobs() {
    let (dir, workspace, _) = setup("small");
    let custom = large("custom");
    let storage = Storage::open(dir.path()).unwrap();
    {
        let _lock = storage.lock().unwrap();
        storage
            .put("custom", "c1", &json!({ "text": custom }))
            .unwrap();
    }
    let pruned = workspace.prune_snapshots(Duration::ZERO, true).unwrap();
    assert_eq!(pruned.removed_snapshots, 1);
    assert_eq!(pruned.retained_blobs, 1);
    assert_eq!(pruned.removed_blobs, 0);
    assert!(blob_path(dir.path(), &custom).exists());

    let other = large("other");
    fs::write(dir.path().join("other.txt"), &other).unwrap();
    let unused = workspace.read_range("other.txt", 1, 1).unwrap();
    let unused_path = dir
        .path()
        .join(".ultra-edit/snapshots")
        .join(format!("{}.json", unused.snapshot));
    fs::remove_file(blob_path(dir.path(), &custom)).unwrap();
    for apply in [false, true] {
        let error = workspace
            .prune_snapshots(Duration::ZERO, apply)
            .unwrap_err();
        assert_eq!(error.code, "STORE_CORRUPT");
        assert!(error.message.contains(&digest(custom.as_bytes())));
    }
    assert!(unused_path.exists());
    assert!(blob_path(dir.path(), &other).exists());
}

#[test]
fn an_omitted_request_id_is_stable_and_every_argument_changes_it() {
    let (_dir, workspace, snapshot) = setup("one two");
    let automatic = request("", &snapshot, vec![change("a", "one", "ONE")]);
    let resolved = resolve_ids(automatic.clone()).unwrap();
    let id = resolved.request_id.clone();
    assert_eq!(id.len(), 37, "{id}");
    assert!(id.starts_with("auto-"), "{id}");
    assert!(
        id[5..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{id}"
    );
    assert_eq!(resolve_ids(automatic.clone()).unwrap(), resolved);
    assert_eq!(resolve_ids(resolved.clone()).unwrap(), resolved);

    let other_base = workspace.read("file.txt").unwrap();
    let mut scoped = automatic.clone();
    scoped.files[0].changes[0].target = Target::Exact {
        old: "one".into(),
        scope: Some("r1".into()),
    };
    let variants = [
        request("", &other_base, vec![change("a", "one", "ONE")]),
        request("", &snapshot, vec![change("b", "one", "ONE")]),
        request("", &snapshot, vec![change("a", "two", "ONE")]),
        request("", &snapshot, vec![change("a", "one", "One")]),
        request(
            "",
            &snapshot,
            vec![change("a", "one", "ONE"), change("b", "two", "TWO")],
        ),
        scoped,
    ];
    let mut ids = BTreeSet::from([id.clone()]);
    for variant in variants {
        let derived = resolve_ids(variant).unwrap().request_id;
        assert!(ids.insert(derived.clone()), "{derived} was derived twice");
    }
    // Each operation derives under its own domain.
    for derived in [
        repair_request_id("", &id, &[]).unwrap(),
        undo_request_id("", &id).unwrap(),
    ] {
        assert!(derived.starts_with("auto-"), "{derived}");
        assert!(ids.insert(derived.clone()), "{derived} collided");
    }
    assert_eq!(
        resolve_ids(request("explicit", &snapshot, vec![]))
            .unwrap()
            .request_id,
        "explicit"
    );

    let preview = workspace.prepare(automatic.clone()).unwrap();
    assert_eq!(stored_request(&workspace, &preview.reference), resolved);
    let receipt = completed(workspace.edit(automatic).unwrap());
    assert_eq!(receipt.request_id, id);
    assert_eq!(workspace.receipt(&id).unwrap(), Some(receipt));
}

#[test]
fn an_identical_automatic_request_replays_without_writing_again() {
    let (dir, workspace, snapshot) = setup("one");
    let path = dir.path().join("file.txt");
    let automatic = request("", &snapshot, vec![change("", "one", "two")]);
    let (receipt, replayed, _) = outcome(workspace.edit(automatic.clone()).unwrap());
    assert!(!replayed);
    assert_eq!(receipt.commit, CommitStatus::Committed);
    assert_eq!(fs::read_to_string(&path).unwrap(), "two");
    // Restoring the base bytes would let a new attempt succeed; a replay must not write.
    fs::write(&path, "one").unwrap();
    let (replay, replayed, report) = outcome(workspace.edit(automatic.clone()).unwrap());
    assert!(replayed);
    assert_eq!(replay, receipt);
    assert_eq!(
        report,
        format!("{REPLAY_NOTICE}\n{}", receipt_report(&receipt, false))
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "one");
    assert_eq!(journal_count(&dir), 1);
    drop(workspace);
    let workspace = Workspace::open(dir.path()).unwrap();
    let preparation = workspace.prepare(automatic).unwrap();
    assert!(preparation.replayed);
    assert_eq!(preparation.receipt, Some(receipt));
    assert_eq!(fs::read_to_string(&path).unwrap(), "one");
    assert_eq!(journal_count(&dir), 1);
}

#[test]
fn different_automatic_requests_never_reuse_a_request_id() {
    let (dir, workspace, snapshot) = setup("one two");
    let first = completed(
        workspace
            .edit(request("", &snapshot, vec![change("", "one", "ONE")]))
            .unwrap(),
    );
    let fresh = workspace.read("file.txt").unwrap();
    let second = completed(
        workspace
            .edit(request("", &fresh, vec![change("", "two", "TWO")]))
            .unwrap(),
    );
    assert_eq!(second.commit, CommitStatus::Committed);
    assert_ne!(first.request_id, second.request_id);
    // Different arguments on the first base get their own binding and a fresh verdict.
    let EditResult::Rejected { preparation } = workspace
        .edit(request("", &snapshot, vec![change("", "two", "2")]))
        .unwrap()
    else {
        panic!("Expected a stale rejection")
    };
    assert!(!preparation.replayed);
    assert!(
        preparation
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "STALE_SNAPSHOT")
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "ONE TWO"
    );
}

#[test]
fn omitted_change_ids_become_their_one_based_positions() {
    let (dir, workspace, first) = setup("one two");
    fs::write(dir.path().join("second.txt"), "three").unwrap();
    let second = workspace.read("second.txt").unwrap();
    let mut automatic = request(
        "",
        &first,
        vec![change("", "one", "ONE"), change("", "two", "TWO")],
    );
    automatic.files.push(FileRequest {
        base: second.id.clone(),
        changes: vec![change("", "three", "THREE")],
    });
    let preview = workspace.prepare(automatic).unwrap();
    assert!(preview.ready);
    let Evidence::Plan(plan) = workspace.evidence(&preview.reference).unwrap() else {
        panic!("Expected a plan")
    };
    assert_eq!(change_ids(&plan.request), ["1.1", "1.2", "2.1"]);
    assert_eq!(plan.files[0].change_ids, ["1.1", "1.2"]);
    assert_eq!(plan.files[1].change_ids, ["2.1"]);
    // Explicit IDs are kept beside derived ones.
    let mixed = workspace
        .prepare(request(
            "",
            &second,
            vec![change("mine", "th", "TH"), change("", "ree", "REE")],
        ))
        .unwrap();
    assert!(mixed.ready);
    assert_eq!(
        change_ids(&stored_request(&workspace, &mixed.reference)),
        ["mine", "1.2"]
    );
}

#[test]
fn an_explicit_change_id_that_equals_a_derived_one_is_a_duplicate() {
    let (_dir, workspace, snapshot) = setup("one two");
    let rejected = workspace
        .prepare(request(
            "",
            &snapshot,
            vec![change("1.2", "one", "ONE"), change("", "two", "TWO")],
        ))
        .unwrap();
    assert!(!rejected.ready);
    let duplicates = rejected
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "DUPLICATE_CHANGE_ID")
        .collect::<Vec<_>>();
    assert_eq!(duplicates.len(), 1, "{:?}", rejected.diagnostics);
    assert_eq!(duplicates[0].change_id.as_deref(), Some("1.2"));
}

#[test]
fn repair_and_undo_derive_request_ids_and_an_identical_undo_replays() {
    let (dir, workspace, snapshot) = setup("one two");
    let path = dir.path().join("file.txt");
    let draft = workspace
        .prepare(request(
            "",
            &snapshot,
            vec![change("", "one", "ONE"), change("", "missing", "TWO")],
        ))
        .unwrap();
    assert!(!draft.ready);
    let error = workspace
        .repair(&draft.reference, "", vec![change("", "two", "TWO")])
        .unwrap_err();
    assert_eq!(error.code, "INVALID_REPAIR");
    assert!(error.message.contains("ID of the change it replaces"));

    let corrections = vec![change("1.2", "two", "TWO")];
    let repaired = workspace
        .repair(&draft.reference, "", corrections.clone())
        .unwrap();
    assert!(repaired.ready);
    assert!(!repaired.replayed);
    let repair_id = repair_request_id("", &draft.reference, &corrections).unwrap();
    assert!(repair_id.starts_with("auto-"));
    assert_eq!(
        stored_request(&workspace, &repaired.reference).request_id,
        repair_id
    );
    assert_ne!(
        stored_request(&workspace, &draft.reference).request_id,
        repair_id
    );
    let repeated = workspace.repair(&draft.reference, "", corrections).unwrap();
    assert!(repeated.replayed);
    assert_eq!(repeated.reference, repaired.reference);
    assert_eq!(
        repeated.report,
        format!("{REPLAY_NOTICE}\n{}", repaired.report)
    );

    let (committed, replayed) = workspace.commit_or_replay(&repaired.reference).unwrap();
    assert!(!replayed);
    assert_eq!(fs::read_to_string(&path).unwrap(), "ONE TWO");
    let (undone, replayed, _) = outcome(workspace.undo(&repaired.reference, "").unwrap());
    assert!(!replayed);
    assert_eq!(undone.commit, CommitStatus::Committed);
    assert_eq!(
        undone.request_id,
        undo_request_id("", &repaired.reference).unwrap()
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "one two");
    // With the committed bytes back, a new undo could succeed; the replay writes nothing.
    fs::write(&path, "ONE TWO").unwrap();
    let (replay, replayed, report) = outcome(workspace.undo(&repaired.reference, "").unwrap());
    assert!(replayed);
    assert_eq!(replay, undone);
    assert!(report.starts_with(REPLAY_NOTICE));
    assert_eq!(fs::read_to_string(&path).unwrap(), "ONE TWO");
    assert_eq!(workspace.receipt(&undone.request_id).unwrap(), Some(undone));
    assert_eq!(
        workspace.commit_or_replay(&repaired.reference).unwrap(),
        (committed, true)
    );
}

#[test]
fn retry_still_requires_an_explicit_request_id() {
    let (dir, workspace, snapshot) = setup("one");
    let preview = workspace
        .prepare(request("", &snapshot, vec![change("", "one", "two")]))
        .unwrap();
    fs::write(dir.path().join("file.txt"), "changed").unwrap();
    assert_eq!(
        workspace.commit(&preview.reference).unwrap().commit,
        CommitStatus::NotCommitted
    );
    fs::write(dir.path().join("file.txt"), "one").unwrap();
    let error = workspace.retry(&preview.reference, "").unwrap_err();
    assert_eq!(error.code, "INVALID_REQUEST_ID");
    assert!(error.message.contains("explicit request ID"), "{error}");
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "one"
    );
    let (retried, replayed, _) = outcome(workspace.retry(&preview.reference, "retry").unwrap());
    assert!(!replayed);
    assert_eq!(retried.commit, CommitStatus::Committed);
    let (replay, replayed, _) = outcome(workspace.retry(&preview.reference, "retry").unwrap());
    assert!(replayed);
    assert_eq!(replay, retried);
}

#[test]
fn an_edit_that_commits_a_prepared_plan_is_not_a_replay() {
    let (dir, workspace, snapshot) = setup("one");
    let automatic = request("", &snapshot, vec![change("", "one", "two")]);
    let preview = workspace.prepare(automatic.clone()).unwrap();
    assert!(preview.ready);
    assert!(!preview.replayed);
    let repeated = workspace.prepare(automatic.clone()).unwrap();
    assert!(repeated.replayed);
    assert_eq!(repeated.reference, preview.reference);
    assert_eq!(
        repeated.report,
        format!("{REPLAY_NOTICE}\n{}", preview.report)
    );
    let (receipt, replayed, report) = outcome(workspace.edit(automatic.clone()).unwrap());
    assert!(!replayed, "committing the stored plan is a new attempt");
    assert!(!report.contains(REPLAY_NOTICE));
    assert_eq!(receipt.plan_id, preview.reference);
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "two"
    );
    let (replay, replayed, _) = outcome(workspace.edit(automatic).unwrap());
    assert!(replayed);
    assert_eq!(replay, receipt);
}

#[test]
fn a_replay_notice_counts_against_the_report_bounds() {
    let receipt = Receipt {
        request_id: "bounded".into(),
        plan_id: "p0".into(),
        commit: CommitStatus::Committed,
        files: (0..100)
            .map(|index| FileOutcome {
                path: format!("/workspace/{}{index}.txt", "long-directory/".repeat(8)),
                before: "s0".into(),
                intended_digest: "0".into(),
                after: None,
                status: FileStatus::Committed,
                changes_applied: 1,
                error: None,
            })
            .collect(),
        validation: "not_requested".into(),
        undo: None,
        warnings: vec![],
    };
    let fresh = receipt_report(&receipt, false);
    assert_eq!(fresh, report::receipt(&receipt, 60, 6_000));
    let replayed = receipt_report(&receipt, true);
    for report in [&fresh, &replayed] {
        assert_eq!(report.lines().count(), 60, "{report}");
        assert!(report.chars().count() <= 6_000, "{report}");
    }
    assert_eq!(replayed.lines().next(), Some(REPLAY_NOTICE));
    assert_eq!(replayed.lines().nth(1), fresh.lines().next());
}
