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
            lines: None,
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
        // Paths are normalized with the platform's separator, so compare components.
        let file = outcome.diagnostics[0].file.as_deref().unwrap();
        assert_eq!(std::path::Path::new(file), std::path::Path::new(path));
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
            scope: Some("r0".into()),
            expected: 2,
            lines: None,
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
fn line_targets_in_a_path_file_need_a_strong_expect() {
    let (dir, workspace) = setup(&[("a.rs", "fn main() {\n    run();\n    serve_forever();\n}\n")]);
    let lines = |range: [usize; 2], expect: Option<&str>| Change {
        id: String::new(),
        target: Target::Lines {
            lines: range,
            expect: expect.map(str::to_owned),
            expect_last: None,
        },
        text: "    start();".into(),
    };
    // A path file was read through another view, so its line numbers may be stale.
    for (change, code) in [
        (lines([2, 2], None), "LINE_GUARD_REQUIRED"),
        (lines([2, 3], Some("    run();")), "LINE_GUARD_REQUIRED"),
        (
            lines([2, 3], Some("    serve_forever();\n}")),
            "EXPECTED_TEXT_MISMATCH",
        ),
    ] {
        let outcome = rejected(
            workspace
                .edit(request(vec![at("a.rs", vec![change])]))
                .unwrap(),
        );
        assert_eq!(codes(&outcome), [code]);
    }
    let change = lines([2, 3], Some("    run();\n    serve_forever();"));
    receipt(
        workspace
            .edit(request(vec![at("a.rs", vec![change])]))
            .unwrap(),
    );
    assert_eq!(read(&dir, "a.rs"), "fn main() {\n    start();\n}\n");
}

#[test]
fn a_line_added_inside_a_range_since_the_read_is_caught_at_its_end() {
    let text = "fn keep() {}\nfn other() {}\nfn remove_me() {\n    step_one();\n    step_two();\n}\nfn tail() {}\n";
    let (dir, workspace) = setup(&[("a.rs", text)]);
    // An earlier edit adds a line inside lines 3-6, as the caller read them.
    receipt(
        workspace
            .edit(request(vec![at(
                "a.rs",
                vec![exact(
                    "    step_two();",
                    "    step_two();\n    step_three();",
                )],
            )]))
            .unwrap(),
    );
    let delete = Change {
        id: String::new(),
        target: Target::Lines {
            lines: [3, 6],
            expect: Some("fn remove_me() {".into()),
            expect_last: Some("}".into()),
        },
        text: String::new(),
    };
    let outcome = rejected(
        workspace
            .edit(request(vec![at("a.rs", vec![delete.clone()])]))
            .unwrap(),
    );
    assert_eq!(codes(&outcome), ["EXPECTED_TEXT_MISMATCH"]);
    assert!(
        outcome.diagnostics[0]
            .message
            .contains("it is at line 7, so the range is [3,7]"),
        "{}",
        outcome.diagnostics[0].message
    );
    let mut delete = delete;
    delete.target = Target::Lines {
        lines: [3, 7],
        expect: Some("fn remove_me() {".into()),
        expect_last: Some("}".into()),
    };
    receipt(
        workspace
            .edit(request(vec![at("a.rs", vec![delete])]))
            .unwrap(),
    );
    assert_eq!(
        read(&dir, "a.rs"),
        "fn keep() {}\nfn other() {}\nfn tail() {}\n"
    );
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
fn an_undo_is_never_repaired_into_new_text() {
    let (dir, workspace) = setup(&[("a.txt", "a\nb\n")]);
    let count = Change {
        id: String::new(),
        target: Target::All {
            old: "\n".into(),
            scope: None,
            expected: 2,
            lines: None,
        },
        text: "\r\n".into(),
    };
    let (done, _) = receipt(
        workspace
            .edit(request(vec![at("a.txt", vec![count])]))
            .unwrap(),
    );
    assert_eq!(read(&dir, "a.txt"), "a\r\nb\r\n");
    // The file changes, so the undo is refused and its draft retained.
    fs::write(dir.path().join("a.txt"), "x\r\n").unwrap();
    let draft = rejected(workspace.undo(&done.plan_id, "").unwrap());
    fs::write(dir.path().join("a.txt"), "a\r\nb\r\n").unwrap();
    let restore = Change {
        id: "undo-1".into(),
        target: Target::Span {
            span: "r0".into(),
            expect: None,
        },
        text: "a\nb\n".into(),
    };
    // A repair would plan text, here adapted to CRLF, instead of the recorded bytes.
    let error = workspace
        .repair(&draft.reference, "", vec![restore])
        .unwrap_err();
    assert_eq!(error.code, "INVALID_REFERENCE");
    receipt(workspace.undo(&done.plan_id, "undo-again").unwrap());
    assert_eq!(read(&dir, "a.txt"), "a\nb\n");
}

#[test]
fn one_file_named_twice_or_an_empty_span_is_reported_once() {
    let (dir, workspace) = setup(&[("a.txt", "one\ntwo\n")]);
    let outcome = rejected(
        workspace
            .edit(request(vec![
                at("a.txt", vec![exact("one", "1")]),
                at("./a.txt", vec![exact("two", "2")]),
            ]))
            .unwrap(),
    );
    assert_eq!(codes(&outcome), ["DUPLICATE_TARGET_PATH"]);
    assert!(
        outcome.diagnostics[0]
            .message
            .contains("list each file once")
    );
    let empty = Change {
        id: String::new(),
        target: Target::Exact {
            old: "one".into(),
            scope: Some(String::new()),
            lines: None,
        },
        text: "1".into(),
    };
    let outcome = rejected(
        workspace
            .edit(request(vec![at("a.txt", vec![empty])]))
            .unwrap(),
    );
    assert_eq!(codes(&outcome), ["EMPTY_SPAN_ID"]);
    assert_eq!(read(&dir, "a.txt"), "one\ntwo\n");
}

#[test]
fn a_missing_path_that_ends_like_a_workspace_file_names_it() {
    let (dir, workspace) = setup(&[("docs/a.md", "one\n")]);
    // A directory left out of an absolute path: the root's parent instead of the root.
    let parent = dir.path().parent().unwrap().join("docs").join("a.md");
    let outcome = rejected(
        workspace
            .edit(request(vec![at(
                parent.to_str().unwrap(),
                vec![exact("one", "1")],
            )]))
            .unwrap(),
    );
    assert_eq!(codes(&outcome), ["TARGET_MISSING"]);
    let message = &outcome.diagnostics[0].message;
    let near = std::fs::canonicalize(dir.path().join("docs").join("a.md")).unwrap();
    assert!(
        message.ends_with(&format!(
            "; did you mean {}?",
            ultra_edit::report::path_for_display(&near.to_string_lossy())
        )),
        "{message}"
    );
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

/// The workspace root is stored canonical; a path spelled through the root as the
/// caller gave it (here through a symlink, as macOS temp directories are) must still
/// derive the same request ID, or a resend would apply an insertion twice.
#[cfg(unix)]
#[test]
fn a_path_spelled_through_a_symlinked_root_replays() {
    let real = TempDir::new().unwrap();
    fs::write(real.path().join("a.txt"), "head\n").unwrap();
    let links = TempDir::new().unwrap();
    let root = links.path().join("project");
    std::os::unix::fs::symlink(real.path(), &root).unwrap();
    let workspace = Workspace::open(&root).unwrap();
    let insert = |path: &str| request(vec![at(path, vec![exact("head\n", "head\nadded\n")])]);
    receipt(workspace.edit(insert("a.txt")).unwrap());
    for spelling in [
        root.join("a.txt"),
        real.path().canonicalize().unwrap().join("a.txt"),
    ] {
        let (_, replayed) = receipt(workspace.edit(insert(spelling.to_str().unwrap())).unwrap());
        assert!(replayed, "{}", spelling.display());
    }
    assert_eq!(read(&real, "a.txt"), "head\nadded\n");
}
