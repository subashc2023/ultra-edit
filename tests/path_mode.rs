//! Path mode: a file named by `path` is read under the workspace lock as its base, so
//! an edit needs no snapshot call, as with native Edit.

use std::fs;

use tempfile::TempDir;
use ultra_edit::workspace::{EditResult, Evidence};
use ultra_edit::*;

fn setup(files: &[(&str, &str)]) -> (TempDir, Workspace) {
    let dir = TempDir::new().unwrap();
    for (name, text) in files {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let workspace = Workspace::open(dir.path()).unwrap();
    (dir, workspace)
}

fn exact(old: &str, new: &str) -> Change {
    Change {
        id: String::new(),
        target: Target::Exact {
            old: old.into(),
            scope: None,
        },
        text: new.into(),
    }
}

fn at(path: &str, changes: Vec<Change>) -> FileRequest {
    FileRequest {
        path: Some(path.into()),
        base: String::new(),
        changes,
    }
}

fn request(files: Vec<FileRequest>) -> EditRequest {
    EditRequest {
        request_id: String::new(),
        files,
    }
}

fn receipt(result: EditResult) -> (Receipt, bool) {
    match result {
        EditResult::Completed {
            receipt, replayed, ..
        } => (receipt, replayed),
        other => panic!("Expected a receipt, got {other:?}"),
    }
}

fn rejected(result: EditResult) -> Preparation {
    match result {
        EditResult::Rejected { preparation } => preparation,
        other => panic!("Expected a rejection, got {other:?}"),
    }
}

fn codes(preparation: &Preparation) -> Vec<&str> {
    preparation
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect()
}

fn read(dir: &TempDir, name: &str) -> String {
    fs::read_to_string(dir.path().join(name)).unwrap()
}

#[test]
fn a_path_edit_commits_and_records_the_bytes_it_read_as_a_stored_base() {
    let (dir, workspace) = setup(&[("a.txt", "one\ntwo\n"), ("b.txt", "x = 1\n")]);
    let (done, replayed) = receipt(
        workspace
            .edit(request(vec![
                at("a.txt", vec![exact("two", "2")]),
                at("b.txt", vec![exact("1", "2")]),
            ]))
            .unwrap(),
    );
    assert_eq!(done.commit, CommitStatus::Committed);
    assert!(!replayed);
    assert_eq!(read(&dir, "a.txt"), "one\n2\n");
    assert_eq!(read(&dir, "b.txt"), "x = 2\n");
    let Evidence::Plan(plan) = workspace.evidence(&done.plan_id).unwrap() else {
        panic!("expected a plan");
    };
    // Stored plans keep the snapshot-request shape: a base, no path.
    for (file, original) in plan.request.files.iter().zip(["one\ntwo\n", "x = 1\n"]) {
        assert!(file.path.is_none());
        let Evidence::Snapshot(base) = workspace.evidence(&file.base).unwrap() else {
            panic!("expected a snapshot base");
        };
        assert_eq!(base.text, original);
    }
}

#[test]
fn spellings_of_one_path_derive_one_request_and_replay() {
    let (dir, workspace) = setup(&[("a.txt", "one\n")]);
    let absolute = dir.path().join("a.txt").to_str().unwrap().to_owned();
    let first = receipt(
        workspace
            .edit(request(vec![at("a.txt", vec![exact("one", "1")])]))
            .unwrap(),
    );
    for spelling in ["./a.txt", absolute.as_str(), ".//a.txt"] {
        let (again, replayed) = receipt(
            workspace
                .edit(request(vec![at(spelling, vec![exact("one", "1")])]))
                .unwrap(),
        );
        assert!(replayed, "{spelling}");
        assert_eq!(again.request_id, first.0.request_id, "{spelling}");
    }
    assert_eq!(read(&dir, "a.txt"), "1\n");
}

#[test]
fn a_replayed_insertion_is_not_applied_twice_and_warns_when_the_file_changed() {
    let (dir, workspace) = setup(&[("a.txt", "head\n")]);
    let insert = || request(vec![at("a.txt", vec![exact("head\n", "head\nadded\n")])]);
    receipt(workspace.edit(insert()).unwrap());
    let (_, replayed) = receipt(workspace.edit(insert()).unwrap());
    assert!(replayed);
    assert_eq!(read(&dir, "a.txt"), "head\nadded\n");
    fs::write(dir.path().join("a.txt"), "head\nadded\nlater\n").unwrap();
    let (replay, replayed) = receipt(workspace.edit(insert()).unwrap());
    assert!(replayed);
    assert_eq!(read(&dir, "a.txt"), "head\nadded\nlater\n");
    let warning = replay
        .warnings
        .iter()
        .find(|warning| warning.code == "REPLAYED_FILE_CHANGED")
        .expect("a replay over changed bytes warns");
    assert!(warning.file.as_deref().unwrap().ends_with("a.txt"));
}

#[test]
fn a_rejected_path_request_is_not_bound_so_an_identical_resend_after_a_fix_commits() {
    let (dir, workspace) = setup(&[("a.txt", "colour\n")]);
    let edit = || request(vec![at("a.txt", vec![exact("color", "hue")])]);
    let first = rejected(workspace.edit(edit()).unwrap());
    assert_eq!(codes(&first), ["TARGET_NOT_FOUND"]);
    fs::write(dir.path().join("a.txt"), "color\n").unwrap();
    let (done, replayed) = receipt(workspace.edit(edit()).unwrap());
    assert!(!replayed);
    assert_eq!(done.request_id, first.request_id);
    assert_eq!(read(&dir, "a.txt"), "hue\n");
}

#[test]
fn an_explicit_request_id_binds_a_rejected_path_request() {
    let (dir, workspace) = setup(&[("a.txt", "colour\n")]);
    let mut edit = request(vec![at("a.txt", vec![exact("color", "hue")])]);
    edit.request_id = "fix-color".into();
    rejected(workspace.edit(edit.clone()).unwrap());
    fs::write(dir.path().join("a.txt"), "color\n").unwrap();
    let again = rejected(workspace.edit(edit).unwrap());
    assert!(again.replayed);
    assert_eq!(read(&dir, "a.txt"), "color\n");
}

#[test]
fn a_stale_snapshot_base_beside_a_path_file_writes_nothing() {
    let (dir, workspace) = setup(&[("a.txt", "one\n"), ("b.txt", "two\n")]);
    let base = workspace.read("b.txt").unwrap();
    fs::write(dir.path().join("b.txt"), "changed\n").unwrap();
    let outcome = rejected(
        workspace
            .edit(request(vec![
                at("a.txt", vec![exact("one", "1")]),
                FileRequest {
                    path: None,
                    base: base.id,
                    changes: vec![exact("two", "2")],
                },
            ]))
            .unwrap(),
    );
    assert_eq!(codes(&outcome), ["STALE_SNAPSHOT"]);
    assert_eq!(read(&dir, "a.txt"), "one\n");
}

#[test]
fn a_path_given_with_a_base_must_name_that_snapshots_file() {
    let (dir, workspace) = setup(&[("a.txt", "one\n"), ("b.txt", "two\n")]);
    let base = workspace.read("b.txt").unwrap();
    let mismatch = rejected(
        workspace
            .edit(request(vec![FileRequest {
                path: Some("a.txt".into()),
                base: base.id.clone(),
                changes: vec![exact("two", "2")],
            }]))
            .unwrap(),
    );
    assert_eq!(codes(&mismatch), ["SNAPSHOT_PATH_MISMATCH"]);
    receipt(
        workspace
            .edit(request(vec![FileRequest {
                path: Some("b.txt".into()),
                base: base.id,
                changes: vec![exact("two", "2")],
            }]))
            .unwrap(),
    );
    assert_eq!(read(&dir, "b.txt"), "2\n");
}

#[test]
fn unreadable_paths_are_diagnosed_per_file_and_nothing_is_written() {
    let (dir, workspace) = setup(&[("a.txt", "one\n")]);
    fs::create_dir(dir.path().join("sub")).unwrap();
    for (path, code) in [
        ("missing.txt", "TARGET_MISSING"),
        ("../outside.txt", "PATH_OUTSIDE_WORKSPACE"),
        (".ultra-edit", "PATH_OUTSIDE_WORKSPACE"),
        ("sub", "NOT_REGULAR_FILE"),
    ] {
        fs::write(dir.path().parent().unwrap().join("outside.txt"), "x\n").ok();
        let outcome = rejected(
            workspace
                .edit(request(vec![
                    at("a.txt", vec![exact("one", "1")]),
                    at(path, vec![exact("x", "y")]),
                ]))
                .unwrap(),
        );
        assert_eq!(codes(&outcome), [code], "{path}");
        assert_eq!(outcome.diagnostics[0].file.as_deref(), Some(path));
        assert_eq!(read(&dir, "a.txt"), "one\n");
    }
}

#[test]
fn spans_in_a_path_file_need_a_base_except_r0() {
    let (dir, workspace) = setup(&[("a.txt", "a a\n")]);
    let span = Change {
        id: String::new(),
        target: Target::Span {
            span: "r1".into(),
            expect: None,
        },
        text: "b".into(),
    };
    let outcome = rejected(
        workspace
            .edit(request(vec![at("a.txt", vec![span])]))
            .unwrap(),
    );
    assert_eq!(codes(&outcome), ["SPAN_NEEDS_BASE"]);
    let all = Change {
        id: String::new(),
        target: Target::All {
            old: "a".into(),
            scope: "r0".into(),
            expected: 2,
        },
        text: "b".into(),
    };
    receipt(
        workspace
            .edit(request(vec![at("a.txt", vec![all])]))
            .unwrap(),
    );
    assert_eq!(read(&dir, "a.txt"), "b b\n");
}

#[test]
fn a_path_edit_undoes() {
    let (dir, workspace) = setup(&[("a.txt", "one\n")]);
    let (done, _) = receipt(
        workspace
            .edit(request(vec![at("a.txt", vec![exact("one", "1")])]))
            .unwrap(),
    );
    receipt(workspace.undo(&done.plan_id, "").unwrap());
    assert_eq!(read(&dir, "a.txt"), "one\n");
}

#[test]
fn a_file_without_path_or_base_is_rejected() {
    let (_dir, workspace) = setup(&[("a.txt", "one\n")]);
    let outcome = rejected(
        workspace
            .edit(request(vec![FileRequest {
                path: None,
                base: String::new(),
                changes: vec![exact("one", "1")],
            }]))
            .unwrap(),
    );
    assert_eq!(codes(&outcome), ["EMPTY_SNAPSHOT_ID"]);
}

#[test]
fn two_paths_to_one_file_are_an_alias_and_write_nothing() {
    let (dir, workspace) = setup(&[("a.txt", "one\n")]);
    fs::hard_link(dir.path().join("a.txt"), dir.path().join("b.txt")).unwrap();
    let outcome = rejected(
        workspace
            .edit(request(vec![
                at("a.txt", vec![exact("one", "1")]),
                at("b.txt", vec![exact("one", "2")]),
            ]))
            .unwrap(),
    );
    assert_eq!(codes(&outcome), ["TARGET_ALIAS"]);
    assert_eq!(read(&dir, "a.txt"), "one\n");
}
