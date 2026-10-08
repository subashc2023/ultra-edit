use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::json;
use ultra_edit::compiler::{compile, snapshot};
use ultra_edit::{
    Candidate, CandidateKind, Change, Diagnostic, Draft, EditRequest, FileRequest, Snapshot, Span,
    Target,
};

fn exact(id: &str, old: &str, text: &str) -> Change {
    Change {
        id: id.into(),
        target: Target::Exact {
            old: old.into(),
            scope: None,
            lines: None,
        },
        text: text.into(),
    }
}

fn span_change(id: &str, span: &str, text: &str) -> Change {
    Change {
        id: id.into(),
        target: Target::Span {
            span: span.into(),
            expect: None,
        },
        text: text.into(),
    }
}

fn request(base: &Snapshot, changes: Vec<Change>) -> EditRequest {
    EditRequest {
        request_id: "test".into(),
        files: vec![FileRequest {
            path: None,
            base: base.id.clone(),
            changes,
        }],
    }
}

fn bases(base: &Snapshot) -> BTreeMap<String, Snapshot> {
    BTreeMap::from([(base.id.clone(), base.clone())])
}

fn add_span(base: &mut Snapshot, id: &str, start: usize, end: usize) {
    base.spans.push(Span {
        id: id.into(),
        start,
        end,
        line: 0,
    });
}

fn scoped(id: &str, old: &str, scope: &str) -> Change {
    Change {
        id: id.into(),
        target: Target::Exact {
            old: old.into(),
            scope: Some(scope.into()),
            lines: None,
        },
        text: "replacement".into(),
    }
}

fn guarded(span: &str, expect: &str) -> Change {
    Change {
        id: "guarded".into(),
        target: Target::Span {
            span: span.into(),
            expect: Some(expect.into()),
        },
        text: "replacement".into(),
    }
}

/// Compiles one change that must fail alone and returns its diagnostic.
fn rejected(base: &Snapshot, change: Change) -> Diagnostic {
    let mut errors = compile(&request(base, vec![change]), &bases(base)).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    let error = errors.remove(0);
    assert!(error.message.chars().count() <= 240, "{}", error.message);
    error
}

fn candidate(kind: CandidateKind, lines: (usize, usize), text: &str) -> Candidate {
    Candidate {
        kind,
        line: lines.0,
        end_line: lines.1,
        text: Some(text.into()),
        similarity: None,
    }
}

fn similar(lines: (usize, usize), text: &str, similarity: u8) -> Candidate {
    Candidate {
        similarity: Some(similarity),
        ..candidate(CandidateKind::Similar, lines, text)
    }
}

#[test]
fn snapshots_preserve_bom_line_endings_and_empty_lines() {
    let base = snapshot("mixed.txt".into(), "\u{feff}é\r\n\nlast\r".into());
    let texts: Vec<_> = base
        .spans
        .iter()
        .map(|span| &base.text[span.start..span.end])
        .collect();
    assert_eq!(texts, ["\u{feff}é\r\n\nlast\r", "é", "", "last\r"]);
    assert_eq!(base.spans[1].start, 3);
    assert_eq!(base.spans[1].end, 5);
    assert_eq!(base.spans[2].line, 2);
    assert_eq!(base.digest, ultra_edit::digest(base.text.as_bytes()));
    assert_ne!(base.id, snapshot(base.path.clone(), base.text.clone()).id);

    for text in ["", "\u{feff}"] {
        let empty = snapshot("empty.txt".into(), text.into());
        assert_eq!(empty.spans.len(), 2);
        assert_eq!(empty.spans[1].start, text.len());
        assert_eq!(empty.spans[1].end, text.len());
    }
    let terminated = snapshot("end.txt".into(), "line\n".into());
    assert_eq!(terminated.spans.len(), 2);
}

#[test]
fn independent_changes_search_the_original_snapshot_in_either_order() {
    let base = snapshot("colors.txt".into(), "red blue".into());
    let changes = vec![exact("a", "red", "blue"), exact("b", "blue", "green")];
    let forward = compile(&request(&base, changes.clone()), &bases(&base)).unwrap();
    let reverse = compile(
        &request(&base, changes.into_iter().rev().collect()),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(forward.files[0].output, "blue green");
    assert_eq!(forward.files[0].output, reverse.files[0].output);
    assert_eq!(forward.files[0].replacements, reverse.files[0].replacements);
    assert_eq!(forward.files[0].base, base);

    let dependent = request(
        &base,
        vec![exact("a", "red", "yellow"), exact("b", "yellow", "green")],
    );
    let errors = compile(&dependent, &bases(&base)).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, "TARGET_NOT_FOUND");
    assert_eq!(errors[0].change_id.as_deref(), Some("b"));
}

#[test]
fn exact_ambiguity_counts_overlapping_unicode_matches() {
    for (text, old) in [("aaa", "aa"), ("ééé", "éé")] {
        let base = snapshot("overlap.txt".into(), text.into());
        let errors =
            compile(&request(&base, vec![exact("a", old, "x")]), &bases(&base)).unwrap_err();
        assert_eq!(errors[0].code, "TARGET_AMBIGUOUS");
        assert_eq!(errors[0].expected, Some(1));
        assert_eq!(errors[0].actual, Some(2));
        assert!(
            errors[0]
                .message
                .contains("found 2 overlapping starts (1 non-overlapping)")
        );
    }
}

#[test]
fn exact_ambiguity_reports_replace_all_cardinality() {
    let base = snapshot(
        "overlap.txt".into(),
        "one\ntwo\nthree\naaa aaa aa aa".into(),
    );
    let exact = Change {
        id: "exact".into(),
        target: Target::Exact {
            old: "aa".into(),
            scope: Some("r4".into()),
            lines: None,
        },
        text: "x".into(),
    };
    let errors = compile(&request(&base, vec![exact]), &bases(&base)).unwrap_err();
    assert_eq!(errors[0].code, "TARGET_AMBIGUOUS");
    assert_eq!(errors[0].expected, Some(1));
    assert_eq!(errors[0].actual, Some(6));
    assert_eq!(
        errors[0].message,
        "Expected 1 occurrence(s), found 6 overlapping starts (4 non-overlapping) at line 4 and later; add surrounding text to `old`, or restrict it with \"in\":[4,4]"
    );

    let all = Change {
        id: "all".into(),
        target: Target::All {
            old: "aa".into(),
            scope: Some("r4".into()),
            expected: 4,
            lines: None,
        },
        text: "x".into(),
    };
    let plan = compile(&request(&base, vec![all]), &bases(&base)).unwrap();
    assert_eq!(plan.files[0].output, "one\ntwo\nthree\nxa xa x x");
}

#[test]
fn replace_all_requires_non_overlapping_cardinality() {
    let base = snapshot("all.txt".into(), "aaa".into());
    let mut change = Change {
        id: "a".into(),
        target: Target::All {
            old: "aa".into(),
            scope: Some("r0".into()),
            expected: 2,
            lines: None,
        },
        text: "x".into(),
    };
    let errors = compile(&request(&base, vec![change.clone()]), &bases(&base)).unwrap_err();
    assert_eq!(errors[0].code, "EXPECTED_COUNT_MISMATCH");
    assert_eq!(errors[0].expected, Some(2));
    assert_eq!(errors[0].actual, Some(1));
    change.target = Target::All {
        old: "aa".into(),
        scope: Some("r0".into()),
        expected: 1,
        lines: None,
    };
    let plan = compile(&request(&base, vec![change.clone()]), &bases(&base)).unwrap();
    assert_eq!(plan.files[0].output, "xa");
    assert_eq!(plan.files[0].replacements.len(), 1);
    change.target = Target::All {
        old: "a".into(),
        scope: Some("r0".into()),
        expected: 2,
        lines: None,
    };
    let errors = compile(&request(&base, vec![change]), &bases(&base)).unwrap_err();
    assert_eq!(errors[0].code, "EXPECTED_COUNT_MISMATCH");
    assert_eq!(errors[0].expected, Some(2));
    assert_eq!(errors[0].actual, Some(3));
}

#[test]
fn replace_all_self_overlapping_patterns_apply_left_to_right_within_scope() {
    for (text, old, expected, output) in [
        ("        self.name = name", "  ", 4, "xxxxself.name = name"),
        ("ééééé", "éé", 2, "xxé"),
        ("abababa", "aba", 2, "xbx"),
        ("-----", "--", 2, "xx-"),
        ("/////", "//", 2, "xx/"),
        ("...", "..", 1, "x."),
    ] {
        let base = snapshot("periodic.txt".into(), format!("{text}\n{text}"));
        let change = Change {
            id: "all".into(),
            target: Target::All {
                old: old.into(),
                scope: Some("r2".into()),
                expected,
                lines: None,
            },
            text: "x".into(),
        };
        let plan = compile(&request(&base, vec![change]), &bases(&base)).unwrap();
        assert_eq!(plan.files[0].output, format!("{text}\n{output}"));
        assert_eq!(plan.files[0].replacements.len(), expected);
    }
    let base = snapshot("blank-lines.txt".into(), "\n\n\n\n\n".into());
    let change = Change {
        id: "all".into(),
        target: Target::All {
            old: "\n\n".into(),
            scope: Some("r0".into()),
            expected: 2,
            lines: None,
        },
        text: "\n".into(),
    };
    let plan = compile(&request(&base, vec![change]), &bases(&base)).unwrap();
    assert_eq!(plan.files[0].output, "\n\n\n");
}

#[test]
fn span_expect_checks_original_selected_bytes_without_normalization() {
    let base = snapshot("expected.txt".into(), "\u{feff}é\r\nblue\n".into());
    for expected in ["e\u{301}", "é\r\n", "blue", ""] {
        let change = Change {
            id: "guarded".into(),
            target: Target::Span {
                span: "r1".into(),
                expect: Some(expected.into()),
            },
            text: "replacement".into(),
        };
        let errors = compile(&request(&base, vec![change]), &bases(&base)).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "EXPECTED_TEXT_MISMATCH");
        assert_eq!(errors[0].file.as_deref(), Some("expected.txt"));
        assert_eq!(errors[0].change_id.as_deref(), Some("guarded"));
    }
    let change = Change {
        id: "guarded".into(),
        target: Target::Span {
            span: "r1".into(),
            expect: Some("é".into()),
        },
        text: "new".into(),
    };
    let plan = compile(
        &request(&base, vec![exact("other", "blue", "é"), change]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "\u{feff}new\r\né\n");

    let empty = snapshot("empty.txt".into(), "".into());
    let change = Change {
        id: "insert".into(),
        target: Target::Span {
            span: "r0".into(),
            expect: Some("".into()),
        },
        text: "new".into(),
    };
    let plan = compile(&request(&empty, vec![change]), &bases(&empty)).unwrap();
    assert_eq!(plan.files[0].output, "new");
}

#[test]
fn output_warnings_are_nonblocking_and_inspect_literal_resulting_bytes() {
    for (before, old, replacement, output, codes) in [
        ("a", "a", "b\0b", "b\0b", vec!["NUL_BYTE"]),
        ("a\0", "a", "b", "b\0", vec!["NUL_BYTE"]),
        ("a\0", "\0", "", "a", vec![]),
        // LF-only text is adapted to an all-CRLF file; a text holding CR is literal.
        (
            "a\r\nb\r\n",
            "a",
            "a\nextra\0",
            "a\r\nextra\0\r\nb\r\n",
            vec!["NUL_BYTE", "EOL_ADAPTED"],
        ),
        (
            "a\r\nb\r\n",
            "a",
            "a\nextra\0\r",
            "a\nextra\0\r\r\nb\r\n",
            vec!["NUL_BYTE", "MIXED_LINE_ENDINGS"],
        ),
        ("a\nb\n", "a", "a\r", "a\r\nb\n", vec!["MIXED_LINE_ENDINGS"]),
        ("a\r\nb\n", "a", "A", "A\r\nb\n", vec!["MIXED_LINE_ENDINGS"]),
        ("a\r\nb\n", "\r\n", "\n", "a\nb\n", vec![]),
        ("a\r\nb\r\n", "a", "A", "A\r\nb\r\n", vec![]),
        ("a\nb\n", "a", "A", "A\nb\n", vec![]),
    ] {
        let base = snapshot("warnings.txt".into(), before.into());
        let plan = compile(
            &request(&base, vec![exact("change", old, replacement)]),
            &bases(&base),
        )
        .unwrap();
        assert_eq!(plan.files[0].output.as_bytes(), output.as_bytes());
        assert_eq!(
            plan.warnings
                .iter()
                .map(|warning| warning.code.as_str())
                .collect::<Vec<_>>(),
            codes,
            "{before:?}, {replacement:?}"
        );
        assert!(
            plan.warnings
                .iter()
                .all(|warning| warning.file.as_deref() == Some("warnings.txt"))
        );
    }
}

#[test]
fn optional_span_expect_and_warnings_preserve_legacy_plan_serialization() {
    let target = serde_json::json!({"kind": "span", "span": "r0"});
    let parsed: Target = serde_json::from_value(target.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), target);
    let base = snapshot("legacy.txt".into(), "old".into());
    let plan = compile(
        &request(&base, vec![span_change("change", "r0", "new")]),
        &bases(&base),
    )
    .unwrap();
    let encoded = serde_json::to_value(&plan).unwrap();
    assert!(encoded.get("warnings").is_none());
    let decoded: ultra_edit::PreparedPlan = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(decoded, plan);
    assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
}

#[test]
fn scopes_bound_candidates_and_replacement_text_is_literal() {
    let base = snapshot("scope.txt".into(), "a a\r\na a\n".into());
    let all = Change {
        id: "all".into(),
        target: Target::All {
            old: "a".into(),
            scope: Some("r2".into()),
            expected: 2,
            lines: None,
        },
        text: "é\r\n".into(),
    };
    let plan = compile(&request(&base, vec![all]), &bases(&base)).unwrap();
    assert_eq!(plan.files[0].output, "a a\r\né\r\n é\r\n\n");

    let crossing = Change {
        id: "crossing".into(),
        target: Target::Exact {
            old: "a\r\n".into(),
            scope: Some("r1".into()),
            lines: None,
        },
        text: "x".into(),
    };
    let errors = compile(&request(&base, vec![crossing]), &bases(&base)).unwrap_err();
    assert_eq!(errors[0].code, "TARGET_NOT_FOUND");
    assert_eq!(errors[0].actual, Some(0));
}

#[test]
fn line_replacement_preserves_every_undeclared_byte() {
    let base = snapshot(
        "bytes.md".into(),
        "\u{feff}\tfirst  \r\n\tsecond  \nthird\r\nlast".into(),
    );
    let plan = compile(
        &request(&base, vec![span_change("line", "r2", "new\r\nliteral  ")]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(
        plan.files[0].output.as_bytes(),
        "\u{feff}\tfirst  \r\nnew\r\nliteral  \nthird\r\nlast".as_bytes()
    );
    let first = compile(
        &request(&base, vec![span_change("first", "r1", "x")]),
        &bases(&base),
    )
    .unwrap();
    assert!(first.files[0].output.starts_with("\u{feff}x\r\n"));
    let whole = compile(
        &request(&base, vec![span_change("whole", "r0", "x")]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(whole.files[0].output, "x");
}

#[test]
fn insertions_are_explicit_and_boundary_conflicts_are_rejected() {
    let mut base = snapshot("insertion.txt".into(), "abc".into());
    add_span(&mut base, "body", 1, 2);
    for point in [1, 2] {
        let mut current = base.clone();
        add_span(&mut current, "point", point, point);
        let errors = compile(
            &request(
                &current,
                vec![
                    span_change("body", "body", "B"),
                    span_change("insert", "point", "!"),
                ],
            ),
            &bases(&current),
        )
        .unwrap_err();
        assert_eq!(errors[0].code, "OVERLAPPING_CHANGES");
    }
    add_span(&mut base, "point", 0, 0);
    let errors = compile(
        &request(
            &base,
            vec![
                span_change("a", "point", "A"),
                span_change("b", "point", "B"),
            ],
        ),
        &bases(&base),
    )
    .unwrap_err();
    assert_eq!(errors[0].code, "OVERLAPPING_CHANGES");
    add_span(&mut base, "end", 3, 3);
    let plan = compile(
        &request(
            &base,
            vec![
                span_change("end", "end", "$"),
                span_change("start", "point", "^"),
            ],
        ),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "^abc$");
    let empty = snapshot("empty.txt".into(), "".into());
    assert_eq!(
        compile(
            &request(&empty, vec![span_change("new", "r1", "text")]),
            &bases(&empty)
        )
        .unwrap()
        .files[0]
            .output,
        "text"
    );
}

#[test]
fn empty_exact_target_gives_reachable_insertion_advice() {
    let base = snapshot("insertion.txt".into(), "first\r\nsecond".into());
    let rejected = request(&base, vec![exact("insert", "", "inserted")]);
    let errors = compile(&rejected, &bases(&base)).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, "EMPTY_TARGET");
    assert_eq!(
        errors[0].message,
        "Exact search text must not be empty; to insert, use {\"after\":n,\"new\":...}, or replace adjacent text with itself plus the insertion"
    );

    let repaired = Change {
        id: "insert".into(),
        target: Target::Exact {
            old: "first".into(),
            scope: Some("r1".into()),
            lines: None,
        },
        text: "first\r\ninserted".into(),
    };
    let plan = compile(&request(&base, vec![repaired]), &bases(&base)).unwrap();
    assert_eq!(plan.files[0].output, "first\r\ninserted\r\nsecond");
}

#[test]
fn adjacent_replacements_are_valid_and_every_conflict_is_reported() {
    let mut base = snapshot("ranges.txt".into(), "abcd".into());
    add_span(&mut base, "left", 0, 2);
    add_span(&mut base, "right", 2, 4);
    let plan = compile(
        &request(
            &base,
            vec![
                span_change("right", "right", "R"),
                span_change("left", "left", "L"),
            ],
        ),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "LR");
    let errors = compile(
        &request(
            &base,
            vec![
                span_change("whole", "r0", ""),
                span_change("left", "left", "L"),
                span_change("right", "right", "R"),
            ],
        ),
        &bases(&base),
    )
    .unwrap_err();
    assert_eq!(errors.len(), 2);
    assert!(
        errors
            .iter()
            .all(|error| error.code == "OVERLAPPING_CHANGES")
    );
}

#[test]
fn all_safely_discoverable_request_errors_are_collected_across_files() {
    let base = snapshot("known.txt".into(), "a a".into());
    let request = EditRequest {
        request_id: "".into(),
        files: vec![
            FileRequest {
                path: None,
                base: "missing".into(),
                changes: vec![exact("duplicate", "", "x")],
            },
            FileRequest {
                path: None,
                base: base.id.clone(),
                changes: vec![
                    exact("duplicate", "a", "x"),
                    exact("missing", "z", "x"),
                    span_change("span", "absent", "x"),
                    span_change("", "", "x"),
                    Change {
                        id: "all".into(),
                        target: Target::All {
                            old: "".into(),
                            scope: Some("".into()),
                            expected: 0,
                            lines: None,
                        },
                        text: "x".into(),
                    },
                ],
            },
            FileRequest {
                path: None,
                base: "".into(),
                changes: vec![],
            },
        ],
    };
    let errors = compile(&request, &bases(&base)).unwrap_err();
    let codes: Vec<_> = errors.iter().map(|error| error.code.as_str()).collect();
    for code in [
        "EMPTY_REQUEST_ID",
        "EMPTY_TARGET",
        "DUPLICATE_CHANGE_ID",
        "EMPTY_CHANGE_ID",
        "EMPTY_SPAN_ID",
        "INVALID_EXPECTED_COUNT",
        "EMPTY_SNAPSHOT_ID",
        "EMPTY_CHANGES",
        "UNKNOWN_SNAPSHOT",
        "TARGET_AMBIGUOUS",
        "TARGET_NOT_FOUND",
        "UNKNOWN_SPAN",
    ] {
        assert!(codes.contains(&code), "Missing {code} in {codes:?}");
    }
    let empty = EditRequest {
        request_id: "empty".into(),
        files: vec![],
    };
    assert_eq!(
        compile(&empty, &BTreeMap::new()).unwrap_err()[0].code,
        "EMPTY_FILES"
    );
}

#[test]
fn distinct_snapshots_of_one_path_cannot_produce_competing_plans() {
    let first = snapshot("same.txt".into(), "a".into());
    let second = snapshot("same.txt".into(), "b".into());
    let snapshots = BTreeMap::from([
        (first.id.clone(), first.clone()),
        (second.id.clone(), second.clone()),
    ]);
    let request = EditRequest {
        request_id: "duplicate-path".into(),
        files: vec![
            FileRequest {
                path: None,
                base: first.id.clone(),
                changes: vec![exact("a", "a", "x")],
            },
            FileRequest {
                path: None,
                base: second.id.clone(),
                changes: vec![exact("b", "b", "y")],
            },
        ],
    };
    let errors = compile(&request, &snapshots).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, "DUPLICATE_TARGET_PATH");
}

#[test]
fn public_snapshot_inputs_reject_corrupt_identity_digest_and_spans() {
    let original = snapshot("unicode.txt".into(), "éx".into());
    for (start, end) in [(0, 1), (1, 2), (3, 2), (0, 4), (usize::MAX, usize::MAX)] {
        let mut base = original.clone();
        add_span(&mut base, "invalid", start, end);
        let errors =
            compile(&request(&base, vec![exact("a", "x", "y")]), &bases(&base)).unwrap_err();
        assert_eq!(errors[0].code, "INVALID_SPAN");
    }
    let mut base = original.clone();
    base.text.push('!');
    assert_eq!(
        compile(&request(&base, vec![exact("a", "x", "y")]), &bases(&base)).unwrap_err()[0].code,
        "SNAPSHOT_DIGEST_MISMATCH"
    );
    let mut base = original.clone();
    add_span(&mut base, "r0", 0, 0);
    add_span(&mut base, "", 0, 0);
    let errors = compile(&request(&base, vec![exact("a", "x", "y")]), &bases(&base)).unwrap_err();
    assert_eq!(errors.len(), 2);
    assert!(errors.iter().all(|error| error.code == "INVALID_SPAN_ID"));
    let req = request(&original, vec![exact("a", "x", "y")]);
    let mut base = original.clone();
    base.id = "different".into();
    base.path.clear();
    let snapshots = BTreeMap::from([(original.id, base)]);
    assert_eq!(
        compile(&req, &snapshots).unwrap_err()[0].code,
        "INVALID_SNAPSHOT"
    );
}

#[test]
fn independent_spans_commute_and_preserve_untouched_bytes_exhaustively() {
    for length in 2..=5 {
        for bits in 0..(1 << length) {
            let text: String = (0..length)
                .map(|index| if bits & (1 << index) == 0 { 'a' } else { 'é' })
                .collect();
            let offsets: Vec<_> = text
                .char_indices()
                .map(|(offset, _)| offset)
                .chain([text.len()])
                .collect();
            for a in 0..length {
                for b in a + 1..=length {
                    for c in b..length {
                        for d in c + 1..=length {
                            let mut base = snapshot("property.txt".into(), text.clone());
                            let [a, b, c, d] = [offsets[a], offsets[b], offsets[c], offsets[d]];
                            add_span(&mut base, "left", a, b);
                            add_span(&mut base, "right", c, d);
                            let changes = vec![
                                span_change("left", "left", "\r\n左"),
                                span_change("right", "right", ""),
                            ];
                            let forward =
                                compile(&request(&base, changes.clone()), &bases(&base)).unwrap();
                            let reverse = compile(
                                &request(&base, changes.into_iter().rev().collect()),
                                &bases(&base),
                            )
                            .unwrap();
                            let expected =
                                format!("{}\r\n左{}{}", &text[..a], &text[b..c], &text[d..]);
                            assert_eq!(forward.files[0].output, expected);
                            assert_eq!(reverse.files[0].output, expected);
                            assert_eq!(
                                forward.files[0].replacements,
                                reverse.files[0].replacements
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn request_and_replacement_limits_apply_across_file_entries() {
    use ultra_edit::compiler::{MAX_CHANGES, MAX_REPLACEMENTS};
    let first = snapshot("first.txt".into(), "a".repeat(MAX_REPLACEMENTS / 2 + 1));
    let second = snapshot("second.txt".into(), "a".repeat(MAX_REPLACEMENTS / 2));
    let snapshots = BTreeMap::from([
        (first.id.clone(), first.clone()),
        (second.id.clone(), second.clone()),
    ]);
    let mut req = EditRequest {
        request_id: "limited".into(),
        files: vec![
            FileRequest {
                path: None,
                base: first.id.clone(),
                changes: (0..MAX_CHANGES / 2 + 1)
                    .map(|index| exact(&format!("a{index}"), "a", ""))
                    .collect(),
            },
            FileRequest {
                path: None,
                base: second.id.clone(),
                changes: (0..MAX_CHANGES / 2)
                    .map(|index| exact(&format!("b{index}"), "a", ""))
                    .collect(),
            },
        ],
    };
    let errors = compile(&req, &snapshots).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, "RESOURCE_LIMIT");
    for (index, file) in req.files.iter_mut().enumerate() {
        file.changes = vec![Change {
            id: format!("all{index}"),
            target: Target::All {
                old: "a".into(),
                scope: Some("r0".into()),
                expected: MAX_REPLACEMENTS / 2 + usize::from(index == 0),
                lines: None,
            },
            text: "".into(),
        }];
    }
    let errors = compile(&req, &snapshots).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, "RESOURCE_LIMIT");
    assert_eq!(errors[0].change_id.as_deref(), Some("all1"));
    let limit = snapshot("limit.txt".into(), "a".repeat(MAX_REPLACEMENTS));
    let change = Change {
        id: "all".into(),
        target: Target::All {
            old: "a".into(),
            scope: Some("r0".into()),
            expected: MAX_REPLACEMENTS,
            lines: None,
        },
        text: "".into(),
    };
    let plan = compile(&request(&limit, vec![change]), &bases(&limit)).unwrap();
    assert_eq!(plan.files[0].replacements.len(), MAX_REPLACEMENTS);
    assert_eq!(plan.files[0].output, "");
}

#[test]
fn overlap_diagnostics_and_output_expansion_are_bounded() {
    use ultra_edit::compiler::{MAX_OVERLAP_DIAGNOSTICS, MAX_TEXT_BYTES};
    let base = snapshot("conflicts.txt".into(), "a".into());
    let changes = (0..20)
        .map(|index| span_change(&format!("change{index}"), "r0", "b"))
        .collect();
    let errors = compile(&request(&base, changes), &bases(&base)).unwrap_err();
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.code == "OVERLAPPING_CHANGES")
            .count(),
        MAX_OVERLAP_DIAGNOSTICS
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.code == "RESOURCE_LIMIT")
            .count(),
        1
    );

    let base = snapshot("expansion.txt".into(), "aaa".into());
    let change = Change {
        id: "all".into(),
        target: Target::All {
            old: "a".into(),
            scope: Some("r0".into()),
            expected: 3,
            lines: None,
        },
        text: "b".repeat(MAX_TEXT_BYTES / 2),
    };
    assert_eq!(
        compile(&request(&base, vec![change]), &bases(&base)).unwrap_err()[0].code,
        "RESOURCE_LIMIT"
    );
    let mut base = snapshot("large.txt".into(), "a".repeat(MAX_TEXT_BYTES));
    add_span(&mut base, "end", MAX_TEXT_BYTES, MAX_TEXT_BYTES);
    assert_eq!(
        compile(
            &request(&base, vec![span_change("append", "end", "!")]),
            &bases(&base)
        )
        .unwrap_err()[0]
            .code,
        "RESOURCE_LIMIT"
    );
}

#[test]
fn overlap_counts_remain_exact_beyond_position_retention_limit() {
    let base = snapshot("repetitive.txt".into(), "a".repeat(16_384));
    let errors = compile(
        &request(&base, vec![exact("count", &"a".repeat(4_096), "")]),
        &bases(&base),
    )
    .unwrap_err();
    assert_eq!(errors[0].code, "TARGET_AMBIGUOUS");
    assert_eq!(errors[0].actual, Some(12_289));
}

#[test]
fn replace_all_counts_beyond_the_retained_position_budget() {
    use ultra_edit::compiler::MAX_REPLACEMENTS;
    let base = snapshot(
        "repetitive.txt".into(),
        "a".repeat(MAX_REPLACEMENTS * 2 + 2),
    );
    for expected in [1, MAX_REPLACEMENTS, MAX_REPLACEMENTS + 1] {
        let change = Change {
            id: "all".into(),
            target: Target::All {
                old: "aa".into(),
                scope: Some("r0".into()),
                expected,
                lines: None,
            },
            text: "".into(),
        };
        let errors = compile(&request(&base, vec![change]), &bases(&base)).unwrap_err();
        if expected <= MAX_REPLACEMENTS {
            assert_eq!(errors[0].code, "EXPECTED_COUNT_MISMATCH");
            assert_eq!(errors[0].actual, Some(MAX_REPLACEMENTS + 1));
        } else {
            assert_eq!(errors[0].code, "RESOURCE_LIMIT");
        }
    }
}

#[test]
fn exact_match_counts_agree_with_exhaustive_unicode_search() {
    for length in 0..=5 {
        for bits in 0..(1 << length) {
            let text: String = (0..length)
                .map(|index| if bits & (1 << index) == 0 { 'a' } else { 'é' })
                .collect();
            let base = snapshot("matching.txt".into(), text.clone());
            for needle_length in 1..=3 {
                for needle_bits in 0..(1 << needle_length) {
                    let needle: String = (0..needle_length)
                        .map(|index| {
                            if needle_bits & (1 << index) == 0 {
                                'a'
                            } else {
                                'é'
                            }
                        })
                        .collect();
                    let count = text
                        .char_indices()
                        .filter(|(offset, _)| text[*offset..].starts_with(&needle))
                        .count();
                    let result = compile(
                        &request(&base, vec![exact("match", &needle, "x")]),
                        &bases(&base),
                    );
                    if count == 1 {
                        // A single match inside an ASCII word is found, then refused.
                        if let Err(errors) = result {
                            assert_eq!(errors[0].code, "OLD_INSIDE_WORD", "{text:?}, {needle:?}");
                        }
                    } else {
                        assert_eq!(
                            result.unwrap_err()[0].actual,
                            Some(count),
                            "{text:?}, {needle:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn whitespace_candidates_carry_exact_source_text_and_lines() {
    let base = snapshot("tabs.rs".into(), "fn main() {\n\tlet x = 1;\n}\n".into());
    let error = rejected(&base, exact("tabs", "    let x = 1;", "    let x = 2;"));
    assert_eq!(error.code, "TARGET_NOT_FOUND");
    assert_eq!((error.expected, error.actual), (Some(1), Some(0)));
    let tab = candidate(CandidateKind::Whitespace, (2, 2), "\tlet x = 1;");
    assert_eq!(error.candidates, [tab]);
    assert_eq!(
        error.message,
        "Expected 1 occurrence(s), found 0; a candidate at line 2 differs only in whitespace. Copy its exact text into `old` (ultra_edit_repair can replace just this change)."
    );
    let copied = exact("tabs", "\tlet x = 1;", "\tlet x = 2;");
    let plan = compile(&request(&base, vec![copied]), &bases(&base)).unwrap();
    assert_eq!(plan.files[0].output, "fn main() {\n\tlet x = 2;\n}\n");

    // A multi-line LF needle finds CRLF source, including its final line ending.
    // The file's last line ends in LF, so the needle is not adapted to CRLF.
    let base = snapshot("crlf.txt".into(), "a\r\n  b = 1;\r\n  c = 2;\r\nd\n".into());
    let error = rejected(&base, exact("crlf", "  b = 1;\n  c = 2;\n", "x"));
    let crlf = candidate(
        CandidateKind::Whitespace,
        (2, 3),
        "  b = 1;\r\n  c = 2;\r\n",
    );
    assert_eq!(error.candidates, [crlf]);
    assert!(
        error
            .message
            .contains("a candidate at lines 2-3 differs only")
    );

    // Trailing spaces on either side, and characters beyond ASCII after a BOM.
    let base = snapshot("trailing.txt".into(), "let x = 1;   \nlet y = 2;\n".into());
    let error = rejected(&base, exact("trailing", "let x = 1;\nlet y = 2;", "x"));
    let spaces = candidate(
        CandidateKind::Whitespace,
        (1, 2),
        "let x = 1;   \nlet y = 2;",
    );
    assert_eq!(error.candidates, [spaces]);
    let base = snapshot("trailing.txt".into(), "let x = 1;\nlet y = 2;\n".into());
    let error = rejected(&base, exact("trailing", "let x = 1; \t\nlet y = 2;", "x"));
    let bare = candidate(CandidateKind::Whitespace, (1, 2), "let x = 1;\nlet y = 2;");
    assert_eq!(error.candidates, [bare]);
    let base = snapshot("bom.txt".into(), "\u{feff}\tnamé = \"ü\";\n".into());
    let error = rejected(&base, exact("bom", "  namé  =  \"ü\";", "x"));
    let unicode = candidate(CandidateKind::Whitespace, (1, 1), "\tnamé = \"ü\";");
    assert_eq!(error.candidates, [unicode]);
}

#[test]
fn candidates_stay_within_the_target_scope() {
    let base = snapshot("scope.txt".into(), "\tx = 1\ny = 2\n\tx = 1\n".into());
    let error = rejected(&base, exact("x", "  x = 1", "x"));
    let lines: Vec<_> = error.candidates.iter().map(|found| found.line).collect();
    assert_eq!(lines, [1, 3]);
    assert!(
        error
            .message
            .contains("2 candidates, first at line 1, differ only in whitespace")
    );

    let error = rejected(&base, scoped("x", "  x = 1", "r2"));
    assert!(error.candidates.is_empty());
    assert_eq!(
        error.message,
        "Expected 1 occurrence(s), found 0 and nothing similar; read the file again and copy `old` exactly from it"
    );
    let error = rejected(&base, scoped("x", "  x = 1", "r3"));
    let third = candidate(CandidateKind::Whitespace, (3, 3), "\tx = 1");
    assert_eq!(error.candidates, std::slice::from_ref(&third));

    let all = Change {
        id: "all".into(),
        target: Target::All {
            old: "  x = 1".into(),
            scope: Some("r3".into()),
            expected: 1,
            lines: None,
        },
        text: "x".into(),
    };
    let error = rejected(&base, all);
    assert_eq!(error.code, "TARGET_NOT_FOUND");
    assert_eq!(error.candidates, [third]);

    // A span narrower than its line keeps the candidate inside the span.
    let mut base = snapshot("partial.txt".into(), "let  total = compute();\n".into());
    add_span(&mut base, "name", 0, 11);
    let error = rejected(&base, scoped("name", "let total", "name"));
    let partial = candidate(CandidateKind::Whitespace, (1, 1), "let  total");
    assert_eq!(error.candidates, [partial]);
}

#[test]
fn similar_candidates_catch_changed_operators_and_names_but_not_unrelated_text() {
    let source = "function check(request) {\n    if (request.role !== \"admin\") {\n        deny();\n    }\n    let total = compute(items);\n}\n";
    let base = snapshot("check.js".into(), source.into());
    let error = rejected(
        &base,
        exact("op", "    if (request.role === \"admin\") {", "x"),
    );
    let operator = similar((2, 2), "    if (request.role !== \"admin\") {", 96);
    assert_eq!(error.candidates, [operator]);
    assert_eq!(
        error.message,
        "Expected 1 occurrence(s), found 0; a 96% similar candidate is at line 2. Verify it, then copy its exact text into `old` (ultra_edit_repair can replace just this change)."
    );
    // A fragment is matched by the corresponding fragment, never a partial word.
    let error = rejected(&base, exact("fragment", "role === \"admin\"", "x"));
    assert_eq!(
        error.candidates,
        [similar((2, 2), "role !== \"admin\"", 93)]
    );

    let error = rejected(&base, exact("name", "let sum = compute(items);", "x"));
    assert_eq!(
        error.candidates,
        [similar((5, 5), "let total = compute(items);", 81)]
    );

    for unrelated in [
        "fn unrelated_function() -> Result<(), Error> {}",
        "SELECT name FROM users WHERE id = 7;",
        "Z",
    ] {
        let error = rejected(&base, exact("unrelated", unrelated, "x"));
        assert!(error.candidates.is_empty(), "{unrelated}: {error:?}");
        assert!(error.message.ends_with("copy `old` exactly from it"));
    }
}

#[test]
fn similar_candidates_need_more_than_a_shared_shape() {
    let source = "fn lookup(key: &str) -> Result<Value, Error> {\n    let x = foo(a, b);\n    return Err(Error::Timeout);\n    let cache = registry.lookup(key);\n}\n";
    let base = snapshot("lookup.rs".into(), source.into());
    // A short line is 72% like another of its shape, and this one shares only
    // `Error` of its two content words; neither is suggested.
    for unrelated in ["    let y = bar(a, c);", "    return Err(Error::NotFound);"] {
        let error = rejected(&base, exact("unrelated", unrelated, "x"));
        assert!(error.candidates.is_empty(), "{unrelated}: {error:?}");
    }
    let typo = rejected(&base, exact("typo", "    return Err(Error::Timout);", "x"));
    assert_eq!(
        typo.candidates,
        [similar((3, 3), "    return Err(Error::Timeout);", 96)]
    );
    let short = rejected(&base, exact("short", "let x = fo(a, b);", "x"));
    assert_eq!(
        short.candidates,
        [similar((2, 2), "let x = foo(a, b);", 94)]
    );
    let renamed = rejected(
        &base,
        exact("renamed", "    let store = registry.lookup(key);", "x"),
    );
    assert_eq!(
        renamed.candidates,
        [similar((4, 4), "    let cache = registry.lookup(key);", 87)]
    );
    // Below 80%, a line that keeps most of its content words still passes.
    let added = "    let cache = registry.lookup(key, 30, true);";
    let error = rejected(&base, exact("added", added, "x"));
    assert_eq!(
        error.candidates,
        [similar((4, 4), "    let cache = registry.lookup(key);", 76)]
    );
    // Several lines carry their content into the score, which alone decides.
    let block = "    return Err(Error::NotFound);\n    let entry = registry.lookup(name);";
    let error = rejected(&base, exact("block", block, "x"));
    let lines = "    return Err(Error::Timeout);\n    let cache = registry.lookup(key);";
    assert_eq!(error.candidates, [similar((3, 4), lines, 76)]);
}

#[test]
fn expectation_mismatch_quotes_the_span_and_finds_the_expected_text() {
    let base = snapshot(
        "expect.txt".into(),
        "first\tline \"one\"\nsecond\n\tthird line\n".into(),
    );
    let error = rejected(&base, guarded("r1", "second"));
    assert_eq!(error.code, "EXPECTED_TEXT_MISMATCH");
    let second = candidate(CandidateKind::Exact, (2, 2), "second");
    assert_eq!(error.candidates, [second]);
    assert_eq!(
        error.message,
        r#"Span holds "first\tline \"one\"", not expect; the expected text is at line 2. Copy its exact text into `old` (ultra_edit_repair can replace just this change)."#
    );

    // An expectation differing only in whitespace finds its source spelling.
    let error = rejected(&base, guarded("r1", "    third line"));
    let third = candidate(CandidateKind::Whitespace, (3, 3), "\tthird line");
    assert_eq!(error.candidates, [third]);

    // A long selection is clipped on one line; nothing resembles `expect`.
    let base = snapshot("long.txt".into(), format!("{}\nend\n", "a".repeat(100)));
    let error = rejected(&base, guarded("r1", "zzz"));
    assert!(error.candidates.is_empty());
    assert_eq!(
        error.message,
        format!(
            "Span holds \"{}\"…, not expect; inspect the original snapshot and choose the intended span",
            "a".repeat(60)
        )
    );
}

#[test]
fn long_candidates_omit_text_instead_of_clipping() {
    for (repeat, kept) in [(1_999, true), (2_000, false)] {
        // The tab plus the body is one character more than the body.
        let body = "é".repeat(repeat);
        let base = snapshot("long.txt".into(), format!("start\n\t{body}\nend\n"));
        let error = rejected(&base, exact("long", &format!("    {body}"), "x"));
        let found = &error.candidates[0];
        assert_eq!(
            (found.kind, found.line, found.end_line),
            (CandidateKind::Whitespace, 2, 2)
        );
        assert_eq!(found.text.clone(), kept.then(|| format!("\t{body}")));
        let advice = if kept {
            "Copy its"
        } else {
            "Read it and copy its"
        };
        assert!(error.message.contains(advice), "{}", error.message);
    }
}

#[test]
fn candidate_order_is_deterministic() {
    let base = snapshot("order.txt".into(), "\tx = 1\n".repeat(5));
    let error = rejected(&base, exact("x", "  x = 1", "x"));
    let lines: Vec<_> = error.candidates.iter().map(|found| found.line).collect();
    assert_eq!(lines, [1, 2, 3]);

    // Similar candidates run from best score, then by position.
    let source = "let alpha = 2;\nlet alpha = 12;\nlet alpha = 3;\nlet alpha = 4;\n";
    let base = snapshot("similar.txt".into(), source.into());
    let error = rejected(&base, exact("alpha", "let alpha = 1;", "x"));
    let expected = [
        similar((2, 2), "let alpha = 12;", 93),
        similar((1, 1), "let alpha = 2;", 92),
        similar((3, 3), "let alpha = 3;", 92),
    ];
    assert_eq!(error.candidates, expected);
    assert!(
        error
            .message
            .contains("3 similar candidates, best 93% at line 2")
    );
    for _ in 0..3 {
        assert_eq!(
            rejected(&base, exact("alpha", "let alpha = 1;", "x")),
            error
        );
    }
}

#[test]
fn counted_matches_explain_themselves_without_candidates() {
    let base = snapshot("counts.txt".into(), "\tx = 1\nx = 1\n x  = 1\n".into());
    let error = rejected(&base, exact("dup", "x = 1", "y"));
    assert_eq!(error.code, "TARGET_AMBIGUOUS");
    assert!(error.candidates.is_empty());
    let all = Change {
        id: "all".into(),
        target: Target::All {
            old: "x = 1".into(),
            scope: Some("r0".into()),
            expected: 3,
            lines: None,
        },
        text: "y".into(),
    };
    let error = rejected(&base, all);
    assert_eq!(error.code, "EXPECTED_COUNT_MISMATCH");
    assert_eq!(error.actual, Some(2));
    assert!(error.candidates.is_empty());
}

#[test]
fn candidate_search_stays_fast_on_large_inputs() {
    let source: String = (0..100_000)
        .map(|line| format!("    let value_{line} = compute(input_{line}, {line});\n"))
        .collect();
    let base = snapshot("large.rs".into(), source);
    let started = Instant::now();
    // Near the end, so every tier scans the whole file before the similar tier ranks it.
    let near = "    let value_99999 = compute(input_99998, 99999);";
    let error = rejected(&base, exact("near", near, "x"));
    let elapsed = started.elapsed();
    assert_eq!(
        error.candidates[0],
        similar(
            (100_000, 100_000),
            "    let value_99999 = compute(input_99999, 99999);",
            97
        )
    );
    // Generous for unoptimized builds on shared runners; quadratic work over
    // 100,000 lines would take far longer.
    assert!(elapsed < Duration::from_secs(20), "{elapsed:?}");
}

#[test]
fn diagnostics_without_candidates_keep_their_serialized_form() {
    let legacy = json!({
        "id": "d1",
        "request": {"request_id": "legacy", "files": []},
        "diagnostics": [{
            "file": "a.txt", "change_id": "c", "code": "TARGET_NOT_FOUND",
            "message": "Expected 1 occurrence(s), found 0; inspect the snapshot and choose an explicit span or narrower scope",
            "expected": 1, "actual": 0, "conflicts": [],
        }],
    });
    let draft: Draft = serde_json::from_value(legacy.clone()).unwrap();
    assert!(draft.diagnostics[0].candidates.is_empty());
    assert_eq!(serde_json::to_value(&draft).unwrap(), legacy);

    let base = snapshot("tabs.rs".into(), "if (a === b) {\n\tgo();\n}\n".into());
    let whitespace = rejected(&base, exact("tab", "    go();", "x"));
    let similar = rejected(&base, exact("op", "if (a !== b) {", "x"));
    let encoded = serde_json::to_value([&whitespace, &similar]).unwrap();
    assert_eq!(
        encoded[0]["candidates"],
        json!([{"kind": "whitespace", "line": 2, "end_line": 2, "text": "\tgo();"}])
    );
    assert_eq!(
        encoded[1]["candidates"],
        json!([{"kind": "similar", "line": 1, "end_line": 1, "text": "if (a === b) {", "similarity": 92}])
    );
    let decoded: [Diagnostic; 2] = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, [whitespace, similar]);
}

#[test]
fn a_scope_that_misses_the_text_points_to_where_it_is() {
    // An off-by-one scope must not suggest a similar line inside the scope when
    // the exact text sits just outside it.
    let base = snapshot(
        "main.rs".into(),
        "fn main() {\n    let y = 2;\n    let x = 1;\n}\n".into(),
    );
    let error = rejected(&base, scoped("x", "    let x = 1;", "r2"));
    assert_eq!(error.code, "TARGET_NOT_FOUND");
    assert_eq!(
        error.candidates,
        [candidate(CandidateKind::Exact, (3, 3), "    let x = 1;")]
    );
    assert!(
        error.message.contains("outside it, first at line 3"),
        "{}",
        error.message
    );
}

#[test]
fn candidate_searches_per_request_are_capped() {
    let base = snapshot("cap.txt".into(), "\tvalue = 1\n".into());
    let changes = (0..10)
        .map(|index| exact(&format!("c{index}"), "  value = 1", "x"))
        .collect();
    let errors = compile(&request(&base, changes), &bases(&base)).unwrap_err();
    assert_eq!(errors.len(), 10);
    let searched: Vec<_> = errors
        .iter()
        .map(|error| !error.candidates.is_empty())
        .collect();
    let cap = ultra_edit::compiler::MAX_CANDIDATE_SEARCHES;
    assert!(searched[..cap].iter().all(|found| *found), "{searched:?}");
    assert!(searched[cap..].iter().all(|found| !found), "{searched:?}");
}

/// A base like a range read continued with more ranges: only the listed lines are
/// disclosed, and `selection` covers the last range.
fn ranged(text: &str, ranges: &[(usize, usize)]) -> Snapshot {
    let mut base = snapshot("ranged.txt".into(), text.into());
    let lines = base.spans.clone();
    base.spans.clear();
    for &(first, last) in ranges {
        base.spans.extend(lines[first..=last].iter().cloned());
    }
    let &(first, last) = ranges.last().unwrap();
    base.spans.push(Span {
        id: "selection".into(),
        start: lines[first].start,
        end: lines[last].end,
        line: first,
    });
    base
}

fn numbered(lines: usize) -> String {
    (1..=lines).map(|line| format!("line {line}\n")).collect()
}

#[test]
fn span_ranges_replace_from_the_first_body_to_the_last_like_selection() {
    let base = snapshot("full.txt".into(), "a\r\nb\r\nc\r\nd\r\n".into());
    let plan = compile(
        &request(&base, vec![span_change("range", "r2..r3", "B\r\nC")]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "a\r\nB\r\nC\r\nd\r\n");
    assert_eq!(
        (
            plan.files[0].replacements[0].start,
            plan.files[0].replacements[0].end
        ),
        (3, 7)
    );
    // `""` blanks the lines rather than deleting them, and a one-line range is that line.
    for (span, output) in [
        ("r2..r3", "a\r\n\r\nd\r\n"),
        ("r4..r4", "a\r\nb\r\nc\r\n\r\n"),
    ] {
        let plan = compile(
            &request(&base, vec![span_change("blank", span, "")]),
            &bases(&base),
        )
        .unwrap();
        assert_eq!(plan.files[0].output, output, "{span}");
    }
    // A range keeps byte-exact `expect`, after LF-only text is adapted to an
    // all-CRLF file.
    let error = compile(
        &request(&base, vec![guarded("r1..r2", "a \nb")]),
        &bases(&base),
    )
    .unwrap_err();
    assert_eq!(error[0].code, "EXPECTED_TEXT_MISMATCH");
    let plan = compile(
        &request(&base, vec![guarded("r1..r2", "a\nb")]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.warnings[0].code, "EOL_ADAPTED");
    let mixed = snapshot("mixed.txt".into(), "a\r\nb\r\nc\n".into());
    let error = compile(
        &request(&mixed, vec![guarded("r1..r2", "a\nb")]),
        &bases(&mixed),
    )
    .unwrap_err();
    assert_eq!(error[0].code, "EXPECTED_TEXT_MISMATCH");
    let plan = compile(
        &request(&base, vec![guarded("r1..r2", "a\r\nb")]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "replacement\r\nc\r\nd\r\n");
}

#[test]
fn span_ranges_need_every_line_disclosed() {
    let text = numbered(2_000);
    let base = ranged(&text, &[(146, 150), (1875, 1879)]);
    for (span, output) in [
        ("r146..r150", "line 145\nX\nline 151\n"),
        ("r1875..r1879", "line 1874\nX\nline 1880\n"),
        ("r147..r147", "line 146\nX\nline 148\n"),
    ] {
        let plan = compile(
            &request(&base, vec![span_change("range", span, "X")]),
            &bases(&base),
        )
        .unwrap();
        assert!(plan.files[0].output.contains(output), "{span}");
    }
    for span in ["r145..r150", "r146..r151", "r146..r1879", "r1..r2000"] {
        let error = rejected(&base, span_change("b", span, "X"));
        assert_eq!(error.code, "UNKNOWN_SPAN");
        assert_eq!(
            error.message,
            format!(
                "Span {span} needs every line disclosed; this base discloses lines 146-150, 1875-1879; selection = lines 1875-1879. Continue this snapshot to read the rest, or use \"lines\" with expect"
            )
        );
    }
    let error = rejected(&base, span_change("b", "r150..r146", "X"));
    assert_eq!(
        error.message,
        "Span r150..r146 is reversed; write r146..r150. This base discloses lines 146-150, 1875-1879; selection = lines 1875-1879"
    );
}

#[test]
fn span_ranges_scope_exact_and_all_targets() {
    let base = snapshot("scope.txt".into(), "x\nx\nx\nx\n".into());
    let scoped_range = scoped("one", "x", "r2..r2");
    let plan = compile(&request(&base, vec![scoped_range]), &bases(&base)).unwrap();
    assert_eq!(plan.files[0].output, "x\nreplacement\nx\nx\n");
    let all = Change {
        id: "all".into(),
        target: Target::All {
            old: "x".into(),
            scope: Some("r2..r3".into()),
            expected: 2,
            lines: None,
        },
        text: "y".into(),
    };
    let plan = compile(&request(&base, vec![all]), &bases(&base)).unwrap();
    assert_eq!(plan.files[0].output, "x\ny\ny\nx\n");
    let error = rejected(&base, scoped("b", "x", "r2..r3"));
    assert_eq!(error.code, "TARGET_AMBIGUOUS");
    assert!(
        error.message.contains("at lines 2, 3;"),
        "{}",
        error.message
    );
}

#[test]
fn unknown_spans_teach_what_the_base_discloses() {
    let text = numbered(2_000);
    let base = ranged(&text, &[(146, 150), (1875, 1879)]);
    let disclosed = "lines 146-150, 1875-1879; selection = lines 1875-1879";
    for (span, message) in [
        (
            "r12",
            format!("Span r12 is not disclosed by this base, which discloses {disclosed}"),
        ),
        (
            "r0",
            format!("Span r0 is not disclosed by this base, which discloses {disclosed}"),
        ),
        (
            "m1",
            format!("Span m1 is not disclosed by this base, which discloses {disclosed}"),
        ),
        (
            "whole",
            format!("Span whole is not disclosed by this base, which discloses {disclosed}"),
        ),
    ] {
        assert_eq!(
            rejected(&base, span_change("b", span, "X")).message,
            message
        );
    }
    for span in [
        "146-150",
        "L146",
        "r146-r150",
        "lines 146..150",
        "r0146..r150",
    ] {
        let error = rejected(&base, span_change("b", span, "X"));
        assert_eq!(error.code, "UNKNOWN_SPAN");
        assert_eq!(
            error.message,
            format!(
                "{span:?} is not a span ID; use r146, r146..r150, selection, m1, or \"lines\":[146,150]. This base discloses {disclosed}"
            )
        );
    }
    // A long guessed ID is clipped, and a long disclosure list ends in an ellipsis.
    let scattered: Vec<_> = (1..=60).map(|line| (line * 30, line * 30)).collect();
    let base = ranged(&text, &scattered);
    let error = rejected(&base, span_change("b", &"r9".repeat(40), "X"));
    assert!(
        error
            .message
            .starts_with("\"r9r9r9r9r9r9r9r9r9r9r9r9…\" is not a span ID")
    );
    assert!(error.message.ends_with(", …"), "{}", error.message);
    // Every UNKNOWN_SPAN message fits what clients display.
    for span in [
        "r9".repeat(40),
        format!("r{}..r1", usize::MAX),
        format!("r1..r{}", usize::MAX),
        format!("m{}", usize::MAX),
    ] {
        let error = rejected(&base, span_change("b", &span, "X"));
        assert!(error.message.contains(", …"), "{}", error.message);
        assert!(error.message.chars().count() <= 240, "{}", error.message);
    }
    // A whole-file snapshot says so.
    let full = snapshot("full.txt".into(), "a\nb\n".into());
    assert_eq!(
        rejected(&full, span_change("b", "selection", "X")).message,
        "Span selection is not disclosed by this base, which discloses r0 (whole file); lines 1-2"
    );
}

#[test]
fn ambiguous_exact_targets_name_the_first_five_match_lines() {
    let base = snapshot(
        "ambiguous.txt".into(),
        "x = 1\nskip\nx = 1; x = 1\nx = 1\n\nx = 1\nx = 1\nx = 1\n".into(),
    );
    let error = rejected(&base, exact("b", "x = 1", "y"));
    assert_eq!(error.code, "TARGET_AMBIGUOUS");
    assert_eq!(error.actual, Some(7));
    assert_eq!(
        error.message,
        "Expected 1 occurrence(s), found 7 overlapping starts (7 non-overlapping) at lines 1, 3, 4, 6 and later; add surrounding text to `old`, or restrict it with \"in\":[1,1]"
    );
    let base = snapshot("two.txt".into(), "a\nkey\nb\nkey\n".into());
    assert!(
        rejected(&base, exact("b", "key", "y"))
            .message
            .contains("(2 non-overlapping) at lines 2, 4;")
    );
}

#[test]
fn an_lf_target_on_a_mixed_eol_file_gets_a_line_ending_hint() {
    // One LF line keeps the file from being all CRLF, so `old` stays literal.
    let base = snapshot(
        "crlf.ps1".into(),
        "param()\r\nif ($x) {\r\n    Write-Host 1\r\n}\r\n# lf\n".into(),
    );
    let error = rejected(&base, exact("b", "if ($x) {\n    Write-Host 1\n}", "y"));
    assert_eq!(error.code, "TARGET_NOT_FOUND");
    assert_eq!(
        error.message,
        "Expected 1 occurrence(s), found 0; a candidate at lines 2-4 differs only in its CRLF line endings. Copy its exact text, \\r\\n included, into `old` (ultra_edit_repair can replace just this change)."
    );
    assert_eq!(
        error.candidates[0].text.as_deref(),
        Some("if ($x) {\r\n    Write-Host 1\r\n}")
    );
}

fn edge_warnings(text: &str, changes: Vec<Change>) -> (String, Vec<Diagnostic>) {
    let base = snapshot("edge.py".into(), text.into());
    let plan = compile(&request(&base, changes), &bases(&base)).unwrap();
    let warnings = plan
        .warnings
        .into_iter()
        .filter(|warning| warning.code == "WHITESPACE_EDGE")
        .collect();
    (plan.files[0].output.clone(), warnings)
}

#[test]
fn dropped_edge_whitespace_that_joins_text_warns_without_blocking() {
    // The benchmark case: the trailing space of `old` was left out of `new`.
    let text = "def f(line):\n    prorated = _round_cents(line.unit_price * Decimal(line.qty))\n";
    let (output, warnings) = edge_warnings(
        text,
        vec![exact(
            "1.1",
            "    prorated = _round_cents(line.unit_price * ",
            "    prorated = _quantize(line.unit_price *",
        )],
    );
    assert!(output.contains("_quantize(line.unit_price *Decimal(line.qty))"));
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].change_id.as_deref(), Some("1.1"));
    assert_eq!(warnings[0].file.as_deref(), Some("edge.py"));
    assert_eq!(
        warnings[0].message,
        "Change 1.1: `old` ends in whitespace `new` drops, joining what follows; line 2 now reads \"…  prorated = _quantize(line.unit_price *Decimal(line.qty))\""
    );

    let (_, warnings) = edge_warnings("a = b\nc = d\n", vec![exact("join", "b\n", "b")]);
    assert_eq!(
        warnings[0].message,
        "Change join: `old` ends in whitespace `new` drops, joining what follows; line 1 now reads \"a = bc = d\""
    );
    let (_, warnings) = edge_warnings("x+ y\n", vec![exact("lead", " y", "z")]);
    assert_eq!(warnings.len(), 0, "the first visible characters differ");
    let (_, warnings) = edge_warnings("x+ y\n", vec![exact("lead", " y", "y")]);
    assert_eq!(
        warnings[0].message,
        "Change lead: `old` starts with whitespace `new` drops, joining what precedes; line 1 now reads \"x+y\""
    );
    // A dropped line ending joins indented text too.
    let (_, warnings) = edge_warnings("if a:\n    b\n", vec![exact("nl", "a:\n", "a:")]);
    assert_eq!(warnings.len(), 1);
}

#[test]
fn added_edge_whitespace_beside_whitespace_warns() {
    let (output, warnings) = edge_warnings("f(a, b)\n", vec![exact("double", "a,", "a, ")]);
    assert_eq!(output, "f(a,  b)\n");
    assert_eq!(
        warnings[0].message,
        "Change double: `new` ends in spaces or tabs `old` lacks, before whitespace or a line end; line 1 now reads \"f(a,  b)\""
    );
    let (_, warnings) = edge_warnings("x = 1\r\ny = 2\r\n", vec![exact("eol", "x = 1", "x = 1 ")]);
    assert!(
        warnings[0].message.ends_with("line 1 now reads \"x = 1 \""),
        "{}",
        warnings[0].message
    );
    let (_, warnings) = edge_warnings("f(a, b)\n", vec![exact("lead", "b)", " b)")]);
    assert!(
        warnings[0]
            .message
            .contains("after more whitespace; line 1 now reads \"f(a,  b)\"")
    );
}

#[test]
fn ordinary_edits_carry_no_edge_warning() {
    for (text, old, new) in [
        ("a = 1\n", "a = 1", "a = 2"),
        ("a = 1\n", "a = 1\n", "a = 2\n"),
        // Deleting a line, adding lines, and re-indenting are deliberate.
        ("a\nb\nc\n", "b\n", ""),
        ("a\nb\n", "a\n", "a\nx\n"),
        ("a\nb\n", "b", "    b"),
        ("a\n\nb\n", "a\n", "a"),
        // Whitespace-only targets have no visible edge.
        ("a  b\n", "  ", " "),
        // The dropped whitespace sits beside more whitespace.
        ("a b  c\n", "b ", "B"),
        // Nothing follows the target.
        ("f(x) ", "x) ", "y)"),
        ("\u{feff} a\n", " a", "a"),
    ] {
        let (_, warnings) = edge_warnings(text, vec![exact("ok", old, new)]);
        assert!(warnings.is_empty(), "{old:?} -> {new:?}: {warnings:?}");
    }
    let (_, warnings) = edge_warnings("a b\n", vec![span_change("span", "r1", "ab")]);
    assert!(warnings.is_empty());
}

#[test]
fn edge_warnings_are_once_per_change_and_bounded() {
    let all = Change {
        id: "all".into(),
        target: Target::All {
            old: "x ".into(),
            scope: Some("r0".into()),
            expected: 3,
            lines: None,
        },
        text: "x".into(),
    };
    let (output, warnings) = edge_warnings("x a\nx b\nx c\n", vec![all]);
    assert_eq!(output, "xa\nxb\nxc\n");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.ends_with("line 1 now reads \"xa\""));

    let text: String = (0..20).map(|index| format!("k{index}= v\n")).collect();
    let changes = (0..20)
        .map(|index| {
            exact(
                &format!("c{index}"),
                &format!("k{index}= "),
                &format!("k{index}="),
            )
        })
        .collect();
    let (_, warnings) = edge_warnings(&text, changes);
    assert_eq!(warnings.len(), 8);
    // Long lines, escapes and IDs still fit what clients display.
    let long = format!("{} = \t\u{1}{}\n", "é".repeat(500), "\"".repeat(500));
    let id = "i".repeat(300);
    let (_, long_warnings) = edge_warnings(&long, vec![exact(&id, "= \t", "=")]);
    assert_eq!(long_warnings.len(), 1);
    let message = &long_warnings[0].message;
    assert!(message.chars().count() <= 240, "{message}");
    assert!(message.ends_with("\"…"), "{message}");
    assert!(
        warnings[7].message.contains("line 8 now reads \"k7=v\""),
        "{}",
        warnings[7].message
    );
}

#[test]
fn byte_warnings_precede_edge_warnings_so_compact_summaries_keep_them() {
    // Eight edge warnings in the first file must not push the second file's
    // MIXED_LINE_ENDINGS, or the first file's NUL_BYTE, out of the six warnings
    // a compact response shows.
    let text: String = (0..8).map(|index| format!("k{index}= v\n")).collect();
    let mut first = snapshot("first.txt".into(), format!("{text}\0"));
    first.id = "s1".into();
    let mut second = snapshot("second.txt".into(), "a\r\nb\n".into());
    second.id = "s2".into();
    let changes = (0..8)
        .map(|index| {
            exact(
                &format!("c{index}"),
                &format!("k{index}= "),
                &format!("k{index}="),
            )
        })
        .collect();
    let request = EditRequest {
        request_id: "order".into(),
        files: vec![
            FileRequest {
                path: None,
                base: first.id.clone(),
                changes,
            },
            FileRequest {
                path: None,
                base: second.id.clone(),
                changes: vec![exact("eol", "a", "A")],
            },
        ],
    };
    let snapshots = BTreeMap::from([
        (first.id.clone(), first.clone()),
        (second.id.clone(), second.clone()),
    ]);
    let plan = compile(&request, &snapshots).unwrap();
    let codes: Vec<_> = plan
        .warnings
        .iter()
        .map(|warning| warning.code.as_str())
        .collect();
    let mut expected = vec!["NUL_BYTE", "MIXED_LINE_ENDINGS"];
    expected.extend(["WHITESPACE_EDGE"; 8]);
    assert_eq!(codes, expected);
    let shown = ultra_edit::report::warning_summaries(&plan.warnings);
    assert_eq!(shown[1]["code"], "MIXED_LINE_ENDINGS");
    assert_eq!(shown[1]["file"], "second.txt");
}

#[test]
fn span_ranges_past_the_last_line_say_so_instead_of_asking_for_more_reads() {
    let base = snapshot("short.txt".into(), "a\nb\nc\n".into());
    let error = rejected(&base, span_change("b", "r2..r9", "X"));
    assert_eq!(error.code, "UNKNOWN_SPAN");
    assert_eq!(
        error.message,
        "Span r2..r9 runs past the file's last line, 3; this base discloses r0 (whole file); lines 1-3"
    );
    // A range base points past the end the same way, not at another read.
    let text = numbered(10);
    let ranged_base = ranged(&text, &[(8, 10)]);
    assert_eq!(
        rejected(&ranged_base, span_change("b", "r9..r11", "X")).message,
        "Span r9..r11 runs past the file's last line, 10; this base discloses lines 8-10; selection = lines 8-10"
    );
    // The bound holds for the longest IDs and disclosure lists.
    let scattered: Vec<_> = (1..=10).map(|line| (line, line)).collect();
    let error = rejected(
        &ranged(&numbered(10), &scattered),
        span_change("b", &format!("r1..r{}", usize::MAX), "X"),
    );
    assert!(error.message.chars().count() <= 240, "{}", error.message);
}

#[test]
fn edge_warnings_judge_neighbours_in_the_output_not_the_original() {
    // Each change alone would join text, but together the space survives.
    let (output, warnings) = edge_warnings(
        "a b\n",
        vec![exact("left", "a ", "a"), exact("right", "b", " b")],
    );
    assert_eq!(output, "a b\n");
    assert!(warnings.is_empty(), "{warnings:?}");
    // The neighbour a dropped space now joins is another change's text.
    let (output, warnings) = edge_warnings(
        "x = a  b\n",
        vec![exact("left", "a ", "a"), exact("right", " b", "c")],
    );
    assert_eq!(output, "x = ac\n");
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].message.ends_with("line 1 now reads \"x = ac\""),
        "{}",
        warnings[0].message
    );
}

#[test]
fn a_span_range_over_inconsistent_line_offsets_is_rejected_not_sliced() {
    // A tampered snapshot whose line 2 lies before line 1 must not reach a
    // reversed byte slice.
    let mut base = snapshot("tampered.txt".into(), "aaaa\nbb\n".into());
    base.spans[1].start = 5;
    base.spans[1].end = 7;
    base.spans[2].start = 0;
    base.spans[2].end = 2;
    let error = rejected(&base, span_change("b", "r1..r2", "X"));
    assert_eq!(error.code, "UNKNOWN_SPAN");
}

fn lines_change(id: &str, lines: [usize; 2], expect: Option<&str>, text: &str) -> Change {
    Change {
        id: id.into(),
        target: Target::Lines {
            lines,
            expect: expect.map(str::to_owned),
            expect_last: None,
        },
        text: text.into(),
    }
}

fn insert(id: &str, after: usize, expect: Option<&str>, text: &str) -> Change {
    Change {
        id: id.into(),
        target: Target::Insert {
            after,
            expect: expect.map(str::to_owned),
        },
        text: text.into(),
    }
}

/// Plans `changes` against a full snapshot of `text`, which discloses every line.
fn output(text: &str, changes: Vec<Change>) -> String {
    let base = snapshot("lines.txt".into(), text.into());
    let plan = compile(&request(&base, changes), &bases(&base))
        .unwrap_or_else(|errors| panic!("{text:?}: {errors:?}"));
    plan.files[0].output.clone()
}

/// A base that discloses no line, like a receipt's after-snapshot.
fn spanless(text: &str) -> Snapshot {
    let mut base = snapshot("spanless.txt".into(), text.into());
    base.spans.clear();
    base
}

fn settings(lines: usize, eol: &str) -> String {
    (1..=lines)
        .map(|line| format!("    setting_{line} = {line}{eol}"))
        .collect()
}

#[test]
fn lines_replace_whole_lines_and_inherit_the_last_terminator() {
    for (text, lines, new, expected) in [
        ("a\nb\nc\n", [2, 2], "B", "a\nB\nc\n"),
        ("a\nb\nc\n", [2, 2], "B1\nB2", "a\nB1\nB2\nc\n"),
        ("a\r\nb\r\nc\r\n", [2, 2], "B", "a\r\nB\r\nc\r\n"),
        ("a\r\nb\r\nc\r\n", [2, 3], "B\r\nC", "a\r\nB\r\nC\r\n"),
        // No final newline stays absent, and mixed endings keep each line's own.
        ("a\nb", [2, 2], "B", "a\nB"),
        ("a\r\nb\nc", [1, 2], "X", "X\nc"),
        // A trailing line feed in `new` is written literally.
        ("a\nb", [2, 2], "B\n", "a\nB\n"),
        ("a\nb\nc\n", [1, 1], "X\n\n", "X\n\nb\nc\n"),
        ("\u{feff}a\nb\n", [1, 1], "A", "\u{feff}A\nb\n"),
        ("", [1, 1], "new", "new"),
    ] {
        assert_eq!(
            output(text, vec![lines_change("c", lines, None, new)]),
            expected,
            "{text:?} {lines:?} {new:?}"
        );
    }
}

#[test]
fn empty_lines_text_deletes_and_keeps_the_final_newline_state() {
    for (text, lines, expected) in [
        ("a\nb\nc\n", [2, 2], "a\nc\n"),
        ("a\nb\nc\n", [3, 3], "a\nb\n"),
        ("a\nb\nc\n", [1, 3], ""),
        // Deleting a last line without a terminator takes the one before it.
        ("a\nb\nc", [3, 3], "a\nb"),
        ("a\nb\nc", [2, 3], "a"),
        ("a\nb\nc", [1, 3], ""),
        ("a\r\nb\r\nc", [3, 3], "a\r\nb"),
        ("\u{feff}a\nb\n", [1, 1], "\u{feff}b\n"),
        ("\u{feff}a\nb", [1, 2], "\u{feff}"),
    ] {
        assert_eq!(
            output(text, vec![lines_change("c", lines, None, "")]),
            expected,
            "{text:?} {lines:?}"
        );
    }
}

#[test]
fn after_inserts_whole_lines_with_the_files_line_endings() {
    for (text, after, new, expected) in [
        ("a\nb\n", 0, "x", "x\na\nb\n"),
        ("a\nb\n", 1, "x", "a\nx\nb\n"),
        ("a\nb\n", 1, "x\n", "a\nx\nb\n"),
        ("a\nb\n", 2, "x\ny", "a\nb\nx\ny\n"),
        // Native Read's empty line after a final line ending stands for the last line.
        ("a\nb\n", 3, "x", "a\nb\nx\n"),
        ("a\r\nb\r\n", 1, "x", "a\r\nx\r\nb\r\n"),
        // At an end without a newline, the file's first terminator separates the
        // insertion, and the file still lacks a final newline.
        ("a\nb", 2, "x", "a\nb\nx"),
        ("a\r\nb", 2, "x", "a\r\nb\r\nx"),
        ("a", 1, "x", "a\nx"),
        ("a", 0, "x", "x\na"),
        ("\u{feff}a\n", 0, "x", "\u{feff}x\na\n"),
        // An empty file receives the text as given.
        ("", 0, "x", "x"),
        ("", 1, "x\n", "x\n"),
        ("\u{feff}", 0, "x", "\u{feff}x"),
    ] {
        assert_eq!(
            output(text, vec![insert("c", after, None, new)]),
            expected,
            "{text:?} after {after} {new:?}"
        );
    }
    let base = snapshot("empty.txt".into(), "a\n".into());
    let error = rejected(&base, insert("c", 1, None, ""));
    assert_eq!(error.code, "EMPTY_INSERTION");
}

#[test]
fn lines_past_the_end_explain_the_phantom_line() {
    // [2,3] on a two-line file with a final newline clamps to [2,2].
    assert_eq!(
        output("a\nb\n", vec![lines_change("c", [2, 3], None, "B")]),
        "a\nB\n"
    );
    let base = snapshot("end.txt".into(), "a\nb\n".into());
    for change in [
        lines_change("c", [3, 3], None, "x"),
        lines_change("c", [2, 4], None, "x"),
        insert("c", 4, None, "x"),
    ] {
        let error = rejected(&base, change);
        assert_eq!(error.code, "LINE_OUT_OF_RANGE");
        assert_eq!(error.expected, Some(2));
        assert!(
            error.message.ends_with(
                "the file has 2 lines; native Read also shows an empty line 3 after the final line ending, which holds no text; use after:2 to append"
            ),
            "{}",
            error.message
        );
    }
    let base = snapshot("open.txt".into(), "a\nb".into());
    let error = rejected(&base, lines_change("c", [2, 3], None, "x"));
    assert_eq!(
        (error.code.as_str(), error.actual),
        ("LINE_OUT_OF_RANGE", Some(3))
    );
    assert_eq!(
        error.message,
        "Line 3 is past the end: the file has 2 lines and no final line ending; use after:2 to append"
    );
    // Malformed verbose ranges are reported once, without a range check.
    let error = rejected(&base, lines_change("c", [0, 1], None, "x"));
    assert_eq!(error.code, "INVALID_LINE_RANGE");
    assert_eq!(error.message, "Lines [0,1] must satisfy 1 <= first <= last");
}

fn ends_change(id: &str, lines: [usize; 2], ends: [&str; 2], text: &str) -> Change {
    Change {
        id: id.into(),
        target: Target::Lines {
            lines,
            expect: Some(ends[0].into()),
            expect_last: Some(ends[1].into()),
        },
        text: text.into(),
    }
}

#[test]
fn line_expect_compares_whole_lines_ignoring_line_endings() {
    for eol in ["\n", "\r\n"] {
        let text = settings(12, eol);
        let base = spanless(&text);
        for change in [
            lines_change(
                "c",
                [5, 6],
                Some("    setting_5 = 5\n    setting_6 = 6"),
                "    done",
            ),
            lines_change(
                "c",
                [5, 6],
                Some("    setting_5 = 5\r\n    setting_6 = 6\r\n"),
                "    done",
            ),
            ends_change(
                "c",
                [5, 6],
                ["    setting_5 = 5", "    setting_6 = 6\n"],
                "    done",
            ),
        ] {
            let id = format!("{:?}", change.target);
            let plan = compile(&request(&base, vec![change]), &bases(&base))
                .unwrap_or_else(|errors| panic!("{id}: {errors:?}"));
            assert_eq!(
                plan.files[0].output,
                text.replace(
                    &format!("    setting_5 = 5{eol}    setting_6 = 6{eol}"),
                    &format!("    done{eol}")
                )
            );
        }
        // A base that disclosed the lines takes `expect` as a prefix of the range.
        let full = snapshot("full.txt".into(), text.clone());
        let change = lines_change("c", [5, 6], Some("    setting_5 = 5"), "    done");
        assert!(compile(&request(&full, vec![change]), &bases(&full)).is_ok());
        let plan = compile(
            &request(
                &base,
                vec![insert("c", 7, Some("    setting_7 = 7"), "    added")],
            ),
            &bases(&base),
        )
        .unwrap();
        assert!(
            plan.files[0]
                .output
                .contains(&format!("setting_7 = 7{eol}    added{eol}    setting_8"))
        );
    }
    // A shifted line number points to where the expected lines are.
    let base = spanless(&settings(12, "\n"));
    let error = rejected(
        &base,
        lines_change("c", [5, 5], Some("    setting_6 = 6"), "x"),
    );
    assert_eq!(error.code, "EXPECTED_TEXT_MISMATCH");
    assert_eq!(
        error.candidates,
        [candidate(CandidateKind::Exact, (6, 6), "    setting_6 = 6")]
    );
    assert_eq!(
        error.message,
        "Line 5 holds \"    setting_5 = 5\", not expect; it is at line 6, so use lines [6,6] if the whole range moved (ultra_edit_repair can replace just this change)."
    );
    // The new range for a moved head keeps the range's length.
    let error = rejected(
        &base,
        ends_change("c", [4, 6], ["    setting_5 = 5", "    setting_7 = 7"], "x"),
    );
    assert!(
        error
            .message
            .contains("it is at line 5, so use lines [5,7] if the whole range moved"),
        "{}",
        error.message
    );
    // A line added inside the range moves its end, which the last lines catch.
    let mut grown: Vec<String> = settings(12, "\n").lines().map(str::to_owned).collect();
    grown.insert(6, "    extra = 0".into());
    let grown = spanless(&(grown.join("\n") + "\n"));
    let error = rejected(
        &grown,
        ends_change("c", [5, 8], ["    setting_5 = 5", "    setting_8 = 8"], ""),
    );
    assert_eq!(error.code, "EXPECTED_TEXT_MISMATCH");
    assert_eq!(
        error.message,
        "Line 8 holds \"    setting_7 = 7\", not the expected last lines; it is at line 9, so the range is [5,9] (ultra_edit_repair can replace just this change)."
    );
    assert_eq!(
        error.candidates,
        [candidate(CandidateKind::Exact, (9, 9), "    setting_8 = 8")]
    );
    // Whitespace is compared exactly; a near miss asks for a corrected expect.
    let error = rejected(
        &base,
        lines_change(
            "c",
            [5, 6],
            Some("    setting_5 = 5 \n    setting_6 = 6"),
            "x",
        ),
    );
    assert_eq!(error.code, "EXPECTED_TEXT_MISMATCH");
    assert!(
        error.message.starts_with("Lines 5-6 hold \""),
        "{}",
        error.message
    );
    assert!(error.message.contains("into expect"), "{}", error.message);
    // Part of a line is not a line, and is not answered with its own line number.
    let error = rejected(&base, lines_change("c", [5, 5], Some("    setting_5"), "x"));
    assert_eq!(
        error.message,
        "Line 5 holds \"    setting_5 = 5\", not expect; expect compares whole lines, and its text is only part of line 5; give whole lines (ultra_edit_repair can replace just this change)."
    );
    // The lines above an insertion guard it; more lines than exist is explained.
    let error = rejected(
        &base,
        insert(
            "c",
            2,
            Some("    setting_1 = 1\n    setting_2 = 2\n    setting_3 = 3"),
            "x",
        ),
    );
    assert_eq!(
        error.message,
        "expect has 3 lines but only 2 precede the insertion; it gives the lines ending at after"
    );
    let error = rejected(
        &base,
        lines_change(
            "c",
            [5, 5],
            Some("    setting_5 = 5\n    setting_6 = 6"),
            "x",
        ),
    );
    assert_eq!(error.message, "expect has 2 lines but lines [5,5] have 1");
}

#[test]
fn undisclosed_lines_need_an_expect_that_reaches_the_last_line() {
    let text = "fn a() {\n    one();\n}\n}\nfn b() {}\n}\n}\n";
    let base = spanless(text);
    for (change, code, message) in [
        (
            lines_change("c", [3, 4], None, ""),
            "LINE_GUARD_REQUIRED",
            "Lines 3-4 were not disclosed by this base, so the numbers may be stale; give expect as [first line, last line] with their current text, or every line",
        ),
        (
            lines_change("c", [3, 4], Some("}"), ""),
            "LINE_GUARD_REQUIRED",
            "Lines 3-4 were not disclosed by this base, so the numbers may be stale; expect checks only the first 1, missing a shift inside the range: give it as [first line, last line], or every line",
        ),
        (
            insert("c", 2, None, "x"),
            "LINE_GUARD_REQUIRED",
            "Line 2 was not disclosed by this base, so the numbers may be stale; add expect with line 2's current text",
        ),
        (
            ends_change("c", [3, 4], ["}", "}"], ""),
            "LINE_GUARD_WEAK",
            "expect \"} }\" is short and matches 2 ranges in this file, from line 3, so a stale number could pick another; widen the range by a neighbouring line, repeating it in `new` and `expect`, or use `old`",
        ),
        (
            insert("c", 0, Some("fn a() {"), "x"),
            "EXPECTED_TEXT_MISMATCH",
            "after:0 inserts at the top of the file, so no line precedes it for expect to guard; omit expect",
        ),
    ] {
        let error = rejected(&base, change);
        assert_eq!(
            (error.code.as_str(), error.message.as_str()),
            (code, message)
        );
    }
    let error = rejected(&base, insert("c", 3, Some("}\n"), "x"));
    assert_eq!(error.code, "LINE_GUARD_WEAK");
    assert!(error.message.contains("extend it with the lines above"));
    // A short line that occurs once guards as well as a long one, as with `old`.
    for changes in [
        vec![lines_change("c", [2, 2], Some("    one();"), "    two();")],
        vec![
            insert("top", 0, None, "// top"),
            insert("c", 3, Some("fn a() {\n    one();\n}"), "x"),
        ],
        vec![insert("top", 0, Some(""), "// top")],
        vec![ends_change("c", [1, 2], ["fn a() {", "    one();"], "")],
    ] {
        compile(&request(&base, changes), &bases(&base)).unwrap();
    }
    // A base that disclosed the lines needs no expect, and an ambiguous one is fine.
    let full = snapshot("full.txt".into(), text.into());
    for change in [
        lines_change("c", [3, 4], None, ""),
        lines_change("c", [3, 4], Some("}"), ""),
    ] {
        let plan = compile(&request(&full, vec![change]), &bases(&full)).unwrap();
        assert_eq!(
            plan.files[0].output,
            "fn a() {\n    one();\nfn b() {}\n}\n}\n"
        );
    }
    // An empty file has no line to guard: the advice says how to insert, and its
    // one empty line is a guard that matches once.
    let empty = spanless("");
    let error = rejected(&empty, lines_change("c", [1, 1], None, "x"));
    assert!(
        error
            .message
            .contains("the file is empty, so insert with after:0"),
        "{}",
        error.message
    );
    compile(
        &request(&empty, vec![insert("c", 1, Some(""), "x")]),
        &bases(&empty),
    )
    .unwrap();
}

#[test]
fn a_ranged_base_guards_only_the_lines_it_disclosed() {
    let base = ranged(&numbered(300), &[(146, 150)]);
    for change in [
        lines_change("c", [146, 150], None, "x"),
        lines_change("c", [150, 150], None, ""),
        insert("c", 150, None, "x"),
        insert("c", 0, None, "x"),
    ] {
        let id = format!("{:?}", change.target);
        compile(&request(&base, vec![change]), &bases(&base))
            .unwrap_or_else(|errors| panic!("{id}: {errors:?}"));
    }
    for change in [
        lines_change("c", [145, 146], None, "x"),
        lines_change("c", [150, 151], None, "x"),
        insert("c", 151, None, "x"),
    ] {
        assert_eq!(rejected(&base, change).code, "LINE_GUARD_REQUIRED");
    }
}

#[test]
fn anchors_sharing_context_they_both_keep_merge() {
    let text = "log() { printf '%s' \"$(date)\" \"$*\"; }\n";
    let base = snapshot("deploy.sh".into(), text.into());
    // The two `old` texts share `"$(date`, and both new texts keep it.
    let plan = compile(
        &request(
            &base,
            vec![
                exact("a", "printf '%s' \"$(date", "printf '[%s] %s' \"$(date"),
                exact("b", "\"$(date)\" \"$*\"", "\"$(date)\" \"$ENV\" \"$*\""),
            ],
        ),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(
        plan.files[0].output,
        "log() { printf '[%s] %s' \"$(date)\" \"$ENV\" \"$*\"; }\n"
    );
    // A change that rewrites the shared bytes still conflicts.
    let errors = compile(
        &request(
            &base,
            vec![
                exact("a", "printf '%s' \"$(date", "printf '%s' \"$(when"),
                exact("b", "\"$(date)\" \"$*\"", "\"$(date)\" \"$ENV\" \"$*\""),
            ],
        ),
        &bases(&base),
    )
    .unwrap_err();
    assert_eq!(errors[0].code, "OVERLAPPING_CHANGES");
    // So does one change contained in another.
    let errors = compile(
        &request(
            &base,
            vec![
                exact("a", "printf '%s' \"$(date)\"", "printf '%s' \"$(date)\""),
                exact("b", "'%s'", "'%s'"),
            ],
        ),
        &bases(&base),
    )
    .unwrap_err();
    assert_eq!(errors[0].code, "OVERLAPPING_CHANGES");
}

#[test]
fn a_range_ending_in_blank_lines_may_guard_the_text_before_them() {
    let text =
        "intro\n\n## Legacy export\n\nbody\nposition must skip the owner column.\n\n## Next\n";
    let base = snapshot("doc.md".into(), text.into());
    let range = |lines: [usize; 2], head: &str, tail: &str| Change {
        id: "c".into(),
        target: Target::Lines {
            lines,
            expect: Some(head.into()),
            expect_last: Some(tail.into()),
        },
        text: String::new(),
    };
    // Lines 3-7 end in a blank line; the tail names line 6, before it.
    let plan = compile(
        &request(
            &base,
            vec![range(
                [3, 7],
                "## Legacy export",
                "position must skip the owner column.",
            )],
        ),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "intro\n\n## Next\n");
    for change in [
        // The tail must be the last text in the range, not text further up.
        range([3, 7], "## Legacy export", "body"),
        // Text past the range's end is not skipped to.
        range([3, 6], "## Legacy export", "## Next"),
        // A stale range whose head moved still fails on the head.
        range(
            [2, 7],
            "## Legacy export",
            "position must skip the owner column.",
        ),
    ] {
        let errors = compile(&request(&base, vec![change]), &bases(&base)).unwrap_err();
        assert_eq!(errors[0].code, "EXPECTED_TEXT_MISMATCH", "{errors:?}");
    }
    // A short guard that may recur cannot stand before the blank lines.
    let short = snapshot("short.txt".into(), "a\n}\n\nb\n}\n\n".into());
    let errors = compile(
        &request(&short, vec![range([4, 6], "b", "}")]),
        &bases(&short),
    )
    .unwrap_err();
    assert_eq!(errors[0].code, "EXPECTED_TEXT_MISMATCH", "{errors:?}");
}

#[test]
fn an_insertion_beside_a_deletion_applies_in_either_order() {
    let text = "1\n2\n3\n4\n";
    // Deleting line 2 and inserting after it puts the new text where it was.
    assert_eq!(
        output(
            text,
            vec![
                insert("a", 2, None, "two"),
                lines_change("b", [2, 2], None, ""),
            ]
        ),
        "1\ntwo\n3\n4\n"
    );
    assert_eq!(
        output(
            text,
            vec![
                lines_change("a", [2, 3], None, ""),
                insert("b", 1, None, "x"),
            ]
        ),
        "1\nx\n4\n"
    );
    // An insertion inside the deleted lines, or beside replaced text, conflicts.
    let base = snapshot("conflict.txt".into(), text.into());
    for changes in [
        vec![
            lines_change("a", [2, 3], None, ""),
            insert("b", 2, None, "x"),
        ],
        vec![
            lines_change("a", [2, 2], None, "B"),
            insert("b", 2, None, "x"),
        ],
    ] {
        let errors = compile(&request(&base, changes), &bases(&base)).unwrap_err();
        assert_eq!(errors[0].code, "OVERLAPPING_CHANGES");
    }
}

#[test]
fn line_targets_touching_one_another_conflict_with_a_combining_hint() {
    let text = "1\n2\n3\n4\n5\n6\n";
    assert_eq!(
        output(
            text,
            vec![
                lines_change("a", [1, 3], None, "A"),
                lines_change("b", [4, 6], None, "B"),
            ]
        ),
        "A\nB\n"
    );
    let base = snapshot("conflict.txt".into(), text.into());
    let conflict = |changes: Vec<Change>| {
        let errors = compile(&request(&base, changes), &bases(&base)).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].code, "OVERLAPPING_CHANGES");
        errors[0].message.clone()
    };
    assert!(
        conflict(vec![
            insert("a", 3, None, "x"),
            lines_change("b", [4, 6], None, "B"),
        ])
        .ends_with(
            "overlap; combine them into one lines change [4,6] whose new text includes the insertion"
        )
    );
    assert!(
        conflict(vec![
            lines_change("a", [2, 3], None, "x"),
            lines_change("b", [3, 4], None, "B"),
        ])
        .ends_with("overlap; combine them into one lines change [2,4]")
    );
    assert!(
        conflict(vec![insert("a", 2, None, "x"), insert("b", 2, None, "y")])
            .ends_with("overlap; combine them into one after:2 change")
    );
    // Replacing the line before a last line deleted without a terminator overlaps.
    let base = snapshot("open.txt".into(), "a\nb\nc".into());
    let errors = compile(
        &request(
            &base,
            vec![
                lines_change("a", [2, 2], None, "B"),
                lines_change("b", [3, 3], None, ""),
            ],
        ),
        &bases(&base),
    )
    .unwrap_err();
    assert_eq!(
        errors[0].message,
        "Changes a (2..4) and b (3..5) overlap; combine them into one lines change [2,3] (deleting a last line that has no line ending also takes the one before it)"
    );
}

#[test]
fn in_restricts_old_to_whole_lines() {
    let text = "x = 1\ny = 2\nx = 1\n";
    let base = snapshot("in.txt".into(), text.into());
    let error = rejected(&base, exact("c", "x = 1", "x = 9"));
    assert!(error.message.ends_with("or restrict it with \"in\":[1,1]"));
    let scoped = |lines: [usize; 2]| Change {
        id: "c".into(),
        target: Target::Exact {
            old: "x = 1".into(),
            scope: None,
            lines: Some(lines),
        },
        text: "x = 9".into(),
    };
    assert_eq!(output(text, vec![scoped([3, 3])]), "x = 1\ny = 2\nx = 9\n");
    // The last line's terminator is in scope, and the phantom line clamps.
    assert_eq!(output(text, vec![scoped([2, 4])]), "x = 1\ny = 2\nx = 9\n");
    let error = rejected(&base, scoped([2, 2]));
    assert_eq!(error.code, "TARGET_NOT_FOUND");
    assert_eq!(error.candidates[0].kind, CandidateKind::Exact);
    let error = rejected(&base, scoped([5, 6]));
    assert_eq!(error.code, "LINE_OUT_OF_RANGE");
    // A search scope past the end is not advice to append.
    assert!(
        error
            .message
            .ends_with("end `in` by line 3 or drop it to search the whole file"),
        "{}",
        error.message
    );
    // A final line feed in `old` ends its line, so the hint holds that line alone.
    let error = rejected(
        &snapshot("in.txt".into(), "x = 1\nx = 1\ny = 2\n".into()),
        exact("c", "x = 1\n", "x = 9\n"),
    );
    assert!(
        error.message.ends_with("or restrict it with \"in\":[1,1]"),
        "{}",
        error.message
    );
    let all = Change {
        id: "c".into(),
        target: Target::All {
            old: "1".into(),
            scope: None,
            expected: 2,
            lines: Some([1, 3]),
        },
        text: "7".into(),
    };
    assert_eq!(output(text, vec![all]), "x = 7\ny = 2\nx = 7\n");
    // A scope-less replace-all counts the whole file.
    let all = Change {
        id: "c".into(),
        target: Target::All {
            old: "x".into(),
            scope: None,
            expected: 2,
            lines: None,
        },
        text: "z".into(),
    };
    assert_eq!(output(text, vec![all]), "z = 1\ny = 2\nz = 1\n");
    let both = Change {
        id: "c".into(),
        target: Target::Exact {
            old: "x = 1".into(),
            scope: Some("r1".into()),
            lines: Some([1, 1]),
        },
        text: "x".into(),
    };
    assert_eq!(rejected(&base, both).code, "CONFLICTING_SCOPE");
}

#[test]
fn lf_text_is_adapted_to_an_all_crlf_file_and_says_so() {
    let text = "a\r\nb\r\nc\r\n";
    let base = snapshot("crlf.txt".into(), text.into());
    let plan = compile(
        &request(&base, vec![exact("c", "a\nb", "A\nB")]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "A\r\nB\r\nc\r\n");
    assert_eq!(plan.files[0].replacements[0].text, "A\r\nB");
    // The request keeps the text as written, so its derived ID is unchanged.
    assert_eq!(plan.request.files[0].changes[0].text, "A\nB");
    assert_eq!(plan.warnings.len(), 1);
    assert_eq!(plan.warnings[0].code, "EOL_ADAPTED");
    assert_eq!(plan.warnings[0].change_id.as_deref(), Some("c"));
    assert_eq!(
        plan.warnings[0].message,
        "Change c: this file ends every line with CRLF, so LF in its text was matched and written as CRLF; text holding a \\r stays literal"
    );
    for (changes, expected) in [
        // Only `old` holds LF.
        (vec![exact("c", "a\nb", "AB")], "AB\r\nc\r\n"),
        // Lines text is adapted before its terminator is inherited.
        (
            vec![lines_change("c", [1, 1], None, "x\ny")],
            "x\r\ny\r\nb\r\nc\r\n",
        ),
        (vec![insert("c", 3, None, "d\n")], "a\r\nb\r\nc\r\nd\r\n"),
        (vec![guarded("r1..r2", "a\nb")], "replacement\r\nc\r\n"),
    ] {
        let plan = compile(&request(&base, changes), &bases(&base)).unwrap();
        assert_eq!(plan.files[0].output, expected);
        assert_eq!(plan.warnings[0].code, "EOL_ADAPTED");
    }
    // Text holding a CR, and text without LF, stay literal.
    let plan = compile(
        &request(&base, vec![exact("c", "a\r\nb", "A\nB\r")]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "A\nB\r\r\nc\r\n");
    assert_eq!(plan.warnings[0].code, "MIXED_LINE_ENDINGS");
    assert_eq!(plan.warnings.len(), 1);
    let plan = compile(&request(&base, vec![exact("c", "b", "B")]), &bases(&base)).unwrap();
    assert!(plan.warnings.is_empty());
    // Mixed files, a lone CR, or no line ending at all keep every text literal.
    for text in ["a\r\nb\nc\r\n", "a\r\nb\rc\r\n", "a b c"] {
        let base = snapshot("literal.txt".into(), text.into());
        let plan = compile(
            &request(&base, vec![exact("c", "b", "x\ny")]),
            &bases(&base),
        )
        .unwrap();
        assert!(plan.files[0].output.contains("x\ny"), "{text:?}");
        assert!(
            plan.warnings
                .iter()
                .all(|warning| warning.code != "EOL_ADAPTED"),
            "{text:?}"
        );
        if text.contains('\n') {
            assert_eq!(
                rejected(&base, exact("c", "a\nb", "x")).code,
                "TARGET_NOT_FOUND"
            );
        }
    }
    // A literal compile, as undo uses, writes LF as given.
    let plan = ultra_edit::compiler::compile_with(
        &request(&base, vec![span_change("undo", "r0", "a\nb\n")]),
        &bases(&base),
        ultra_edit::compiler::Eol::Literal,
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "a\nb\n");
    assert!(plan.warnings.is_empty());
}

#[test]
fn an_eol_warning_names_a_bounded_number_of_changes() {
    let base = snapshot("crlf.txt".into(), "a\r\nb\r\nc\r\nd\r\ne\r\n".into());
    let long = "x".repeat(60);
    let changes: Vec<_> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|old| exact(&format!("{long}{old}"), old, &format!("{old}\n{old}")))
        .collect();
    let plan = compile(&request(&base, changes), &bases(&base)).unwrap();
    assert_eq!(
        plan.files[0].output,
        "a\r\na\r\nb\r\nb\r\nc\r\nc\r\nd\r\nd\r\ne\r\ne\r\n"
    );
    let warning = &plan.warnings[0];
    assert_eq!(warning.code, "EOL_ADAPTED");
    assert!(
        warning.message.starts_with("Changes xxxxxxxx"),
        "{}",
        warning.message
    );
    assert!(
        warning.message.contains(", 2 more: "),
        "{}",
        warning.message
    );
    assert!(
        warning.message.chars().count() <= 240,
        "{}",
        warning.message
    );
}

#[test]
fn a_change_whose_old_holds_a_cr_stays_literal_so_crlf_converts_to_lf() {
    let base = snapshot("crlf.txt".into(), "a\r\nb\r\n".into());
    let count = |old: &str, text: &str, expected| Change {
        id: "c".into(),
        target: Target::All {
            old: old.into(),
            scope: None,
            expected,
            lines: None,
        },
        text: text.into(),
    };
    for (change, output) in [
        (count("\r\n", "\n", 2), "a\nb\n"),
        (exact("c", "a\r\nb", "a\nb"), "a\nb\r\n"),
    ] {
        let plan = compile(&request(&base, vec![change]), &bases(&base)).unwrap();
        assert_eq!(plan.files[0].output, output);
        assert!(
            plan.warnings
                .iter()
                .all(|warning| warning.code != "EOL_ADAPTED"),
            "{:?}",
            plan.warnings
        );
    }
    // Text copied from a view that hides the CR is still adapted.
    let plan = compile(
        &request(&base, vec![exact("c", "a\nb", "A\nB")]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "A\r\nB\r\n");
    assert_eq!(plan.warnings[0].code, "EOL_ADAPTED");
}

/// A single `old` that starts or ends inside an ASCII word matched a longer name
/// or number, which text written without reading the file does by accident.
#[test]
fn a_single_old_inside_a_word_is_refused_unless_counted() {
    let output = |text: &str, change: Change| {
        let base = snapshot("w.py".into(), text.into());
        compile(&request(&base, vec![change]), &bases(&base))
            .map(|plan| plan.files[0].output.clone())
    };
    let refused = |text: &str, old: &str, word: &str| {
        let errors = output(text, exact("w", old, "x")).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].code, "OLD_INSIDE_WORD", "{old:?} in {text:?}");
        assert!(
            errors[0].message.contains(&format!("`{word}` at line 2")),
            "{}",
            errors[0].message
        );
        assert!(
            errors[0].message.contains("\"count\":1"),
            "{}",
            errors[0].message
        );
    };
    refused("x\nmax_retries = 20\n", "retries = 2", "max_retries");
    refused("x\ntimeout = 300\n", "timeout = 30", "300");
    refused(
        "x\nTEMPLATE_DEBUG = True\n",
        "DEBUG = True",
        "TEMPLATE_DEBUG",
    );
    refused("x\nversion 11.4.2\n", "1.4.2", "11");
    // `count`, even 1, takes part of a word on purpose.
    let counted = Change {
        id: "w".into(),
        target: Target::All {
            old: "retries = 2".into(),
            scope: None,
            expected: 1,
            lines: None,
        },
        text: "retries = 3".into(),
    };
    assert_eq!(
        output("x\nmax_retries = 20\n", counted).unwrap(),
        "x\nmax_retries = 30\n"
    );
    // Whole words, punctuation edges, and non-ASCII neighbours are not words here.
    for (text, old) in [
        ("cfg = load(cfg)\n", "load(cfg)"),
        ("a.retries = 2;\n", "retries = 2"),
        ("Hauptstraße\n", "Hauptstra"),
        ("ルートを保存\n", "ルート"),
        ("é1.4.2\n", "1.4.2"),
    ] {
        assert!(
            output(text, exact("w", old, "y")).is_ok(),
            "{old:?} in {text:?}"
        );
    }
}
