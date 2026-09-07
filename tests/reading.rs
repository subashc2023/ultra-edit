use std::fs;

use tempfile::TempDir;
use ultra_edit::workspace::Evidence;
use ultra_edit::*;

fn setup(text: &str) -> (TempDir, Workspace) {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("file.txt"), text).unwrap();
    let workspace = Workspace::open(dir.path()).unwrap();
    (dir, workspace)
}

fn request(id: &str, base: &str, span: &str, text: &str) -> EditRequest {
    EditRequest {
        request_id: id.into(),
        files: vec![FileRequest {
            base: base.into(),
            changes: vec![Change {
                id: "replace".into(),
                target: Target::Span { span: span.into() },
                text: text.into(),
            }],
        }],
    }
}

#[test]
fn range_references_and_original_bytes_survive_reopening_and_commit() {
    let original = "\u{feff}alpha\r\né\nthird\r\noutside  \r";
    let (dir, workspace) = setup(original);
    let view = workspace.read_range("file.txt", 1, 3).unwrap();
    assert_eq!(view.text, "alpha\r\né\nthird");
    assert_eq!(view.total_lines, 4);
    assert_eq!(view.total_bytes, original.len());
    assert_eq!(view.digest, digest(original.as_bytes()));
    assert_eq!(&original[view.start..view.end], view.text);
    assert_eq!(view.start, 3);
    assert_eq!(
        view.spans
            .iter()
            .map(|span| span.id.as_str())
            .collect::<Vec<_>>(),
        ["r1", "r2", "r3", "selection"]
    );
    drop(workspace);

    let reopened = Workspace::open(dir.path()).unwrap();
    let Evidence::Snapshot(base) = reopened.evidence(&view.snapshot).unwrap() else {
        panic!("Expected the retained snapshot")
    };
    assert_eq!(base.text, original);
    assert_eq!(base.spans, view.spans);
    assert_ne!(
        reopened.read_range("file.txt", 1, 3).unwrap().snapshot,
        view.snapshot
    );
    let preview = reopened
        .prepare(request(
            "range-edit",
            &view.snapshot,
            "selection",
            "NEW\r\né",
        ))
        .unwrap();
    assert!(preview.ready);
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        original
    );
    drop(reopened);

    let committed = Workspace::open(dir.path())
        .unwrap()
        .commit(&preview.reference)
        .unwrap();
    assert_eq!(committed.commit, CommitStatus::Committed);
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "\u{feff}NEW\r\né\r\noutside  \r"
    );
}

#[test]
fn focused_reads_do_not_issue_references_for_undisclosed_source() {
    let (_dir, workspace) = setup("hidden\nshown\nhidden again\n");
    let view = workspace.read_range("file.txt", 2, 2).unwrap();
    assert_eq!(view.text, "shown");
    assert_eq!(view.spans.len(), 2);
    assert_eq!(view.spans[0].id, "r2");
    for span in ["r0", "r1", "r3"] {
        let rejected = workspace
            .prepare(request(span, &view.snapshot, span, "replacement"))
            .unwrap();
        assert!(!rejected.ready);
        assert_eq!(rejected.diagnostics[0].code, "UNKNOWN_SPAN");
    }
    assert!(
        workspace
            .prepare(request("shown", &view.snapshot, "r2", "NEW"))
            .unwrap()
            .ready
    );
}

#[test]
fn unscoped_exact_targets_still_search_the_complete_original_file() {
    let (_dir, workspace) = setup("same\nsame\n");
    let view = workspace.read_range("file.txt", 2, 2).unwrap();
    let mut edit = request("unscoped", &view.snapshot, "selection", "NEW");
    edit.files[0].changes[0].target = Target::Exact {
        old: "same".into(),
        scope: None,
    };
    let rejected = workspace.prepare(edit.clone()).unwrap();
    assert!(!rejected.ready);
    assert_eq!(rejected.diagnostics[0].code, "TARGET_AMBIGUOUS");
    assert_eq!(rejected.diagnostics[0].actual, Some(2));
    edit.request_id = "scoped".into();
    edit.files[0].changes[0].target = Target::Exact {
        old: "same".into(),
        scope: Some("selection".into()),
    };
    assert!(workspace.prepare(edit).unwrap().ready);
}

#[test]
fn changes_outside_the_disclosed_range_still_invalidate_prepare_and_commit() {
    let original = "outside\nselected\n";
    let changed = "changed outside\nselected\n";
    let (dir, workspace) = setup(original);
    let view = workspace.read_range("file.txt", 2, 2).unwrap();
    fs::write(dir.path().join("file.txt"), changed).unwrap();
    let rejected = workspace
        .prepare(request("stale-read", &view.snapshot, "r2", "NEW"))
        .unwrap();
    assert!(!rejected.ready);
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "STALE_SNAPSHOT")
    );

    fs::write(dir.path().join("file.txt"), original).unwrap();
    let preview = workspace
        .prepare(request("stale-preview", &view.snapshot, "r2", "NEW"))
        .unwrap();
    assert!(preview.ready);
    fs::write(dir.path().join("file.txt"), changed).unwrap();
    let receipt = workspace.commit(&preview.reference).unwrap();
    assert_eq!(receipt.commit, CommitStatus::NotCommitted);
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        changed
    );
}

#[test]
fn empty_and_terminated_files_have_no_phantom_line_and_allow_empty_insertion() {
    for original in ["", "\u{feff}", "\n", "\r\n"] {
        let (dir, workspace) = setup(original);
        let view = workspace.read_range("file.txt", 1, 1).unwrap();
        assert_eq!(view.text, "");
        assert_eq!(view.total_lines, 1);
        assert_eq!(view.start, view.end);
        assert_eq!(
            workspace.read_range("file.txt", 2, 2).unwrap_err().code,
            "INVALID_LINE_RANGE"
        );
        let preview = workspace
            .prepare(request("insert", &view.snapshot, "r1", "é"))
            .unwrap();
        assert!(preview.ready);
        assert_eq!(
            workspace.commit(&preview.reference).unwrap().commit,
            CommitStatus::Committed
        );
        let expected = if original == "\u{feff}" {
            "\u{feff}é".into()
        } else {
            format!("é{original}")
        };
        assert_eq!(
            fs::read_to_string(dir.path().join("file.txt")).unwrap(),
            expected
        );
    }
    let (_dir, workspace) = setup("line\r");
    assert_eq!(
        workspace.read_range("file.txt", 1, 1).unwrap().text,
        "line\r"
    );
}

#[test]
fn focused_range_limits_count_unicode_characters_and_interior_line_endings() {
    let (dir, workspace) = setup(&"x\n".repeat(201));
    assert_eq!(
        workspace
            .read_range("file.txt", 1, 200)
            .unwrap()
            .spans
            .len(),
        201
    );
    assert_eq!(
        workspace.read_range("file.txt", 1, 201).unwrap_err().code,
        "READ_TOO_LARGE"
    );
    for (source, accepted) in [
        ("é".repeat(6_000), true),
        ("é".repeat(6_001), false),
        (format!("{}\r\nx", "é".repeat(5_997)), true),
        (format!("{}\r\nx", "é".repeat(5_998)), false),
    ] {
        fs::write(dir.path().join("file.txt"), &source).unwrap();
        let last = if source.contains('\n') { 2 } else { 1 };
        let result = workspace.read_range("file.txt", 1, last);
        if accepted {
            assert_eq!(result.unwrap().text, source);
        } else {
            assert_eq!(result.unwrap_err().code, "READ_TOO_LARGE");
            assert_eq!(workspace.read("file.txt").unwrap().text, source);
        }
    }
}

#[test]
fn invalid_and_extreme_line_ranges_are_rejected() {
    let (_dir, workspace) = setup("one\ntwo\n");
    for (first, last, code) in [
        (0, 1, "INVALID_LINE_RANGE"),
        (1, 0, "INVALID_LINE_RANGE"),
        (2, 1, "INVALID_LINE_RANGE"),
        (1, 3, "INVALID_LINE_RANGE"),
        (3, 3, "INVALID_LINE_RANGE"),
        (usize::MAX, usize::MAX, "INVALID_LINE_RANGE"),
        (1, usize::MAX, "READ_TOO_LARGE"),
    ] {
        assert_eq!(
            workspace
                .read_range("file.txt", first, last)
                .unwrap_err()
                .code,
            code
        );
    }
}

#[test]
fn search_counts_overlapping_matches_but_only_disclosed_spans_can_be_selected() {
    let original = "a".repeat(25);
    let (dir, workspace) = setup(&original);
    let result = workspace.search("file.txt", "aa").unwrap();
    assert_eq!(result.query, "aa");
    assert_eq!(result.total_matches, 24);
    assert_eq!(result.omitted_matches, 4);
    assert_eq!(result.matches.len(), 20);
    for (index, hit) in result.matches.iter().enumerate() {
        assert_eq!(hit.span.id, format!("m{}", index + 1));
        assert_eq!(
            (hit.span.start, hit.span.end, hit.span.line),
            (index, index + 2, 1)
        );
        assert_eq!(&original[hit.span.start..hit.span.end], result.query);
    }
    for span in ["r0", "r1", "selection", "m21"] {
        let rejected = workspace
            .prepare(request(span, &result.snapshot, span, "X"))
            .unwrap();
        assert!(!rejected.ready);
        assert_eq!(rejected.diagnostics[0].code, "UNKNOWN_SPAN");
    }
    drop(workspace);

    let reopened = Workspace::open(dir.path()).unwrap();
    let Evidence::Snapshot(base) = reopened.evidence(&result.snapshot).unwrap() else {
        panic!("Expected the retained search snapshot")
    };
    assert_eq!(base.text, original);
    assert_eq!(
        base.spans,
        result
            .matches
            .iter()
            .map(|hit| hit.span.clone())
            .collect::<Vec<_>>()
    );
    let preview = reopened
        .prepare(request("chosen-overlap", &result.snapshot, "m2", "X"))
        .unwrap();
    assert!(preview.ready);
    drop(reopened);
    assert_eq!(
        Workspace::open(dir.path())
            .unwrap()
            .commit(&preview.reference)
            .unwrap()
            .commit,
        CommitStatus::Committed
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        format!("aX{}", "a".repeat(22))
    );
}

#[test]
fn search_preserves_multiline_unicode_matches_and_bounds_context_in_characters() {
    let query = "needle\r\nnext";
    let original = format!(
        "omitted\n{}{query}{}\r\noutside",
        "é".repeat(90),
        "漢".repeat(90)
    );
    let (_dir, workspace) = setup(&original);
    let result = workspace.search("file.txt", query).unwrap();
    assert_eq!(result.total_matches, 1);
    assert_eq!(result.omitted_matches, 0);
    let hit = &result.matches[0];
    assert_eq!(hit.span.line, 2);
    assert_eq!(hit.span.start, "omitted\n".len() + 90 * "é".len());
    assert_eq!(&original[hit.span.start..hit.span.end], query);
    assert_eq!(hit.before, "é".repeat(80));
    assert_eq!(hit.after, "漢".repeat(80));

    let result = workspace.search("file.txt", "omitted").unwrap();
    assert_eq!(result.matches[0].before, "");
    assert_eq!(result.matches[0].after, "");
    let result = workspace.search("file.txt", "outside").unwrap();
    assert_eq!(result.matches[0].span.line, 4);
    assert_eq!(result.matches[0].before, "");
    assert_eq!(result.matches[0].after, "");
}

#[test]
fn search_validates_query_limits_and_issues_no_spans_when_nothing_matches() {
    let query = "é".repeat(1_000);
    let (_dir, workspace) = setup(&query);
    assert_eq!(
        workspace.search("file.txt", &query).unwrap().total_matches,
        1
    );
    for invalid in ["".to_owned(), "é".repeat(1_001)] {
        assert_eq!(
            workspace.search("file.txt", &invalid).unwrap_err().code,
            "INVALID_QUERY"
        );
    }
    for source in ["", "\u{feff}", "different"] {
        let (_dir, workspace) = setup(source);
        let result = workspace.search("file.txt", "missing").unwrap();
        assert_eq!((result.total_matches, result.omitted_matches), (0, 0));
        assert!(result.matches.is_empty());
        let Evidence::Snapshot(base) = workspace.evidence(&result.snapshot).unwrap() else {
            panic!("Expected a snapshot even without matches")
        };
        assert_eq!(base.text, source);
        assert!(base.spans.is_empty());
    }
}

#[test]
fn focused_reads_and_searches_keep_workspace_and_encoding_checks() {
    let (dir, workspace) = setup("valid");
    fs::write(dir.path().join("invalid.txt"), [0xff, 0xfe]).unwrap();
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("outside.txt"), "outside").unwrap();
    for (path, code) in [
        (dir.path().join("invalid.txt"), "UNSUPPORTED_ENCODING"),
        (outside.path().join("outside.txt"), "PATH_OUTSIDE_WORKSPACE"),
        (
            dir.path().join(".ultra-edit/coordinator.lock"),
            "PATH_OUTSIDE_WORKSPACE",
        ),
    ] {
        assert_eq!(workspace.read_range(&path, 1, 1).unwrap_err().code, code);
        assert_eq!(workspace.search(&path, "a").unwrap_err().code, code);
    }
}
