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
                target: Target::Span {
                    span: span.into(),
                    expect: None,
                },
                text: text.into(),
            }],
        }],
    }
}

#[test]
fn full_reads_require_an_exact_byte_count_to_disclose_large_files() {
    let source = "é".repeat(12_001);
    let (dir, workspace) = setup(&source);
    let error = workspace.read("file.txt").unwrap_err();
    assert_eq!(error.code, "READ_TOO_LARGE");
    assert!(error.message.contains("24002 bytes and 1 lines"), "{error}");
    assert!(error.message.contains("line range or search"));
    assert!(error.message.contains("expected_bytes=24002"));
    assert!(!dir.path().join(".ultra-edit/snapshots").exists());
    assert_eq!(
        workspace
            .read_with_expected_bytes("file.txt", Some(source.len() - 1))
            .unwrap_err()
            .code,
        "READ_SIZE_CHANGED"
    );
    let base = workspace
        .read_with_expected_bytes("file.txt", Some(source.len()))
        .unwrap();
    assert_eq!(base.text, source);
    assert_eq!(base.spans[0].id, "r0");
    assert_eq!(base.spans[0].end, source.len());

    fs::write(dir.path().join("file.txt"), "é".repeat(12_000)).unwrap();
    assert_eq!(workspace.read("file.txt").unwrap().text.len(), 24_000);
    assert_eq!(
        workspace
            .read_with_expected_bytes("file.txt", Some(source.len()))
            .unwrap_err()
            .code,
        "READ_SIZE_CHANGED"
    );
}

#[test]
fn full_reads_bound_line_spans_before_persisting_a_snapshot() {
    let source = "x\n".repeat(401);
    let (dir, workspace) = setup(&source);
    let error = workspace.read("file.txt").unwrap_err();
    assert_eq!(error.code, "READ_TOO_LARGE");
    // The rejected limit is the line count, not the byte count.
    assert!(error.message.contains("802 bytes and 401 lines"), "{error}");
    assert!(!dir.path().join(".ultra-edit/snapshots").exists());
    let base = workspace
        .read_with_expected_bytes("file.txt", Some(source.len()))
        .unwrap();
    assert_eq!(base.text, source);
    assert_eq!(base.spans.len(), 402);
    fs::write(dir.path().join("file.txt"), "x\n".repeat(400)).unwrap();
    assert_eq!(workspace.read("file.txt").unwrap().spans.len(), 401);
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
    assert_eq!(view.spans, ["r1..r3", "selection"]);
    assert_eq!(view.lines, ["r1 | alpha", "r2 | é", "r3 | third"]);
    drop(workspace);

    let reopened = Workspace::open(dir.path()).unwrap();
    let Evidence::Snapshot(base) = reopened.evidence(&view.snapshot).unwrap() else {
        panic!("Expected the retained snapshot")
    };
    assert_eq!(base.text, original);
    // Stored evidence keeps the byte offsets the response summarizes away.
    assert_eq!(
        base.spans
            .iter()
            .map(|span| (span.id.as_str(), span.start, span.end))
            .collect::<Vec<_>>(),
        [
            ("r1", 3, 8),
            ("r2", 10, 12),
            ("r3", 13, 18),
            ("selection", 3, 18)
        ]
    );
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
    assert_eq!(view.spans, ["r2", "selection"]);
    assert_eq!(view.lines, ["r2 | shown"]);
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
        assert_eq!(view.spans, ["r1", "selection"]);
        assert_eq!(view.lines, ["r1 | "]);
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
    let view = workspace.read_range("file.txt", 1, 200).unwrap();
    assert_eq!(view.spans, ["r1..r200", "selection"]);
    assert_eq!(view.lines.len(), 200);
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
            let error = result.unwrap_err();
            assert_eq!(error.code, "READ_TOO_LARGE");
            // A plain full read is not offered: it can exceed its own limits.
            assert!(!error.message.contains("use a full read"), "{error}");
            assert!(
                error
                    .message
                    .contains(&format!("expected_bytes={}", source.len())),
                "{error}"
            );
            assert_eq!(workspace.read("file.txt").unwrap().text, source);
        }
    }
}

#[test]
fn reads_summarize_references_and_list_lines_without_byte_offsets() {
    let (_dir, workspace) = setup("alpha\n\nbravo\ncharlie\n");
    let view = workspace.read_range("file.txt", 1, 3).unwrap();
    assert_eq!(view.spans, ["r1..r3", "selection"]);
    assert_eq!(view.lines, ["r1 | alpha", "r2 | ", "r3 | bravo"]);
    let single = workspace.read_range("file.txt", 4, 4).unwrap();
    assert_eq!(single.spans, ["r4", "selection"]);
    assert_eq!(single.lines, ["r4 | charlie"]);
    let full = FullRead::from(workspace.read("file.txt").unwrap());
    assert_eq!(full.spans, ["r0", "r1..r4"]);
    assert_eq!(
        full.lines,
        ["r1 | alpha", "r2 | ", "r3 | bravo", "r4 | charlie"]
    );
    let serialized = serde_json::to_string(&full).unwrap();
    assert!(!serialized.contains("\"start\""), "{serialized}");

    let (_dir, workspace) = setup("");
    let empty = FullRead::from(workspace.read("file.txt").unwrap());
    assert_eq!(empty.spans, ["r0", "r1"]);
    assert_eq!(empty.lines, ["r1 | "]);
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
        (2, 900, "INVALID_LINE_RANGE"),
        (usize::MAX, usize::MAX, "INVALID_LINE_RANGE"),
        (1, usize::MAX, "INVALID_LINE_RANGE"),
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
fn search_pages_accumulate_every_disclosed_match_into_the_latest_snapshot() {
    let original = "a".repeat(25);
    let (dir, workspace) = setup(&original);
    let first = workspace.search("file.txt", "aa").unwrap();
    let second = workspace
        .search_page(
            "file.txt",
            "aa",
            first.next_offset.unwrap(),
            Some(&first.snapshot),
        )
        .unwrap();
    assert_eq!((second.offset, second.next_offset), (20, None));
    assert_eq!((second.total_matches, second.omitted_matches), (24, 20));
    assert_eq!(second.matches.len(), 4);
    assert_eq!(second.digest, first.digest);
    assert!(!second.stale);
    assert_ne!(second.snapshot, first.snapshot);
    for (index, hit) in second.matches.iter().enumerate() {
        assert_eq!(hit.span.id, format!("m{}", index + 21));
        assert_eq!((hit.span.start, hit.span.end), (index + 20, index + 22));
    }
    // The page lists only its own matches; its snapshot retains all 24.
    let Evidence::Snapshot(base) = workspace.evidence(&second.snapshot).unwrap() else {
        panic!("Expected the continued snapshot")
    };
    assert_eq!(
        base.spans
            .iter()
            .map(|span| span.id.clone())
            .collect::<Vec<_>>(),
        (1..=24)
            .map(|ordinal| format!("m{ordinal}"))
            .collect::<Vec<_>>()
    );
    let mut edit = request("both-pages", &second.snapshot, "m1", "X");
    edit.files[0].changes.push(Change {
        id: "last".into(),
        target: Target::Span {
            span: "m24".into(),
            expect: Some("aa".into()),
        },
        text: "Y".into(),
    });
    let preview = workspace.prepare(edit).unwrap();
    assert!(preview.ready, "{:?}", preview.diagnostics);
    assert_eq!(
        workspace.commit(&preview.reference).unwrap().commit,
        CommitStatus::Committed
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        format!("X{}Y", "a".repeat(21))
    );
}

#[test]
fn continued_pages_keep_line_references_and_drop_another_query_matches() {
    let (_dir, workspace) = setup("alpha beta\nalpha gamma\n");
    let view = workspace.read_range("file.txt", 1, 2).unwrap();
    let first = workspace
        .search_page("file.txt", "alpha", 0, Some(&view.snapshot))
        .unwrap();
    assert!(!first.stale);
    let repeated = workspace
        .search_page("file.txt", "alpha", 0, Some(&first.snapshot))
        .unwrap();
    let other = workspace
        .search_page("file.txt", "beta", 0, Some(&first.snapshot))
        .unwrap();
    let disclosed = |reference: &str| match workspace.evidence(reference).unwrap() {
        Evidence::Snapshot(base) => base
            .spans
            .iter()
            .map(|span| span.id.clone())
            .collect::<Vec<_>>(),
        evidence => panic!("Expected a snapshot, got {evidence:?}"),
    };
    assert_eq!(
        disclosed(&first.snapshot),
        ["r1", "r2", "selection", "m1", "m2"]
    );
    // Re-requesting a page must not duplicate the references it already granted.
    assert_eq!(disclosed(&repeated.snapshot), disclosed(&first.snapshot));
    assert_eq!(disclosed(&other.snapshot), ["r1", "r2", "selection", "m1"]);
    assert!(
        workspace
            .prepare(request("repeated-page", &repeated.snapshot, "m2", "X"))
            .unwrap()
            .ready
    );
    assert!(
        workspace
            .prepare(request("lines-kept", &other.snapshot, "r2", "X"))
            .unwrap()
            .ready
    );
}

#[test]
fn search_pagination_keeps_snapshot_bytes_and_lines_after_source_changes() {
    let original = (1..=45)
        .map(|line| format!("line {line}: é\r\n"))
        .collect::<String>();
    let (dir, workspace) = setup(&original);
    let first = workspace.search("file.txt", "é").unwrap();
    assert!(!first.stale);
    fs::write(dir.path().join("file.txt"), "changed: é").unwrap();
    drop(workspace);
    let reopened = Workspace::open(dir.path()).unwrap();
    let second = reopened
        .search_page("./file.txt", "é", 20, Some(&first.snapshot))
        .unwrap();
    assert_eq!((second.total_matches, second.next_offset), (45, Some(40)));
    assert_eq!(second.digest, first.digest);
    // Continuing retained source after an external change cannot be edited.
    assert!(second.stale);
    assert_eq!(second.matches[0].span.line, 21);
    assert_eq!(second.matches[19].span.line, 40);
    let last = reopened
        .search_page("file.txt", "é", 40, Some(&second.snapshot))
        .unwrap();
    assert_eq!((last.matches.len(), last.next_offset), (5, None));
    assert_eq!(last.matches[4].span.line, 45);
    let rejected = reopened
        .prepare(request("stale-page", &last.snapshot, "m45", "X"))
        .unwrap();
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "STALE_SNAPSHOT")
    );
    let fresh = reopened.search_page("file.txt", "é", 0, None).unwrap();
    assert_eq!(fresh.total_matches, 1);
    assert!(!fresh.stale);
    assert_ne!(fresh.digest, first.digest);

    let end = reopened
        .search_page("file.txt", "é", 45, Some(&first.snapshot))
        .unwrap();
    assert!(end.matches.is_empty());
    assert_eq!(end.next_offset, None);
    for offset in [46, usize::MAX] {
        assert_eq!(
            reopened
                .search_page("file.txt", "é", offset, Some(&first.snapshot))
                .unwrap_err()
                .code,
            "INVALID_SEARCH_OFFSET"
        );
    }
    fs::write(dir.path().join("other.txt"), &original).unwrap();
    assert_eq!(
        reopened
            .search_page("other.txt", "é", 20, Some(&first.snapshot))
            .unwrap_err()
            .code,
        "SNAPSHOT_PATH_MISMATCH"
    );
    assert_eq!(
        reopened
            .search_page("file.txt", "é", 20, Some("p-invalid"))
            .unwrap_err()
            .code,
        "INVALID_REFERENCE"
    );
}

#[test]
fn search_counts_overlapping_matches_but_only_disclosed_spans_can_be_selected() {
    let original = "a".repeat(25);
    let (dir, workspace) = setup(&original);
    let result = workspace.search("file.txt", "aa").unwrap();
    assert_eq!(result.query, "aa");
    assert_eq!(result.total_matches, 24);
    assert_eq!(result.omitted_matches, 4);
    assert_eq!(result.offset, 0);
    assert_eq!(result.next_offset, Some(20));
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
