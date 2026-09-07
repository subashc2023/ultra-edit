use std::fs;

use tempfile::TempDir;
use ultra_edit::workspace::{EditResult, Evidence};
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
    assert!(repeated_preview.report.starts_with("committed:"));
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
    assert!(matches!(
        workspace.undo(&receipt.plan_id, "other-undo").unwrap(),
        EditResult::Rejected { .. }
    ));
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
fn encoding_errors_and_workspace_escape_are_explicit() {
    let (dir, workspace, _) = setup("valid");
    fs::write(dir.path().join("invalid.txt"), [0xff, 0xfe, 0x61, 0x00]).unwrap();
    assert!(workspace.read("invalid.txt").is_err());
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("outside.txt"), "outside").unwrap();
    assert!(workspace.read(outside.path().join("outside.txt")).is_err());
    assert!(workspace.read(".ultra-edit/coordinator.lock").is_err());
}
