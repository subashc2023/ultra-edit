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
        },
        text: "x".into(),
    };
    let errors = compile(&request(&base, vec![exact]), &bases(&base)).unwrap_err();
    assert_eq!(errors[0].code, "TARGET_AMBIGUOUS");
    assert_eq!(errors[0].expected, Some(1));
    assert_eq!(errors[0].actual, Some(6));
    assert_eq!(
        errors[0].message,
        "Expected 1 occurrence(s), found 6 overlapping starts (4 non-overlapping); inspect the snapshot and choose an explicit span or narrower scope"
    );

    let all = Change {
        id: "all".into(),
        target: Target::All {
            old: "aa".into(),
            scope: "r4".into(),
            expected: 4,
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
            scope: "r0".into(),
            expected: 2,
        },
        text: "x".into(),
    };
    let errors = compile(&request(&base, vec![change.clone()]), &bases(&base)).unwrap_err();
    assert_eq!(errors[0].code, "EXPECTED_COUNT_MISMATCH");
    assert_eq!(errors[0].expected, Some(2));
    assert_eq!(errors[0].actual, Some(1));
    change.target = Target::All {
        old: "aa".into(),
        scope: "r0".into(),
        expected: 1,
    };
    let plan = compile(&request(&base, vec![change.clone()]), &bases(&base)).unwrap();
    assert_eq!(plan.files[0].output, "xa");
    assert_eq!(plan.files[0].replacements.len(), 1);
    change.target = Target::All {
        old: "a".into(),
        scope: "r0".into(),
        expected: 2,
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
                scope: "r2".into(),
                expected,
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
            scope: "r0".into(),
            expected: 2,
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
        (
            "a\r\nb\r\n",
            "a",
            "a\nextra\0",
            "a\nextra\0\r\nb\r\n",
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
            scope: "r2".into(),
            expected: 2,
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
        "Exact search text must not be empty; for insertion, replace adjacent text with itself plus the insertion, or use a returned zero-width span"
    );

    let repaired = Change {
        id: "insert".into(),
        target: Target::Exact {
            old: "first".into(),
            scope: Some("r1".into()),
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
                base: "missing".into(),
                changes: vec![exact("duplicate", "", "x")],
            },
            FileRequest {
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
                            scope: "".into(),
                            expected: 0,
                        },
                        text: "x".into(),
                    },
                ],
            },
            FileRequest {
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
                base: first.id.clone(),
                changes: vec![exact("a", "a", "x")],
            },
            FileRequest {
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
                base: first.id.clone(),
                changes: (0..MAX_CHANGES / 2 + 1)
                    .map(|index| exact(&format!("a{index}"), "a", ""))
                    .collect(),
            },
            FileRequest {
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
                scope: "r0".into(),
                expected: MAX_REPLACEMENTS / 2 + usize::from(index == 0),
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
            scope: "r0".into(),
            expected: MAX_REPLACEMENTS,
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
            scope: "r0".into(),
            expected: 3,
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
                scope: "r0".into(),
                expected,
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
                        assert!(result.is_ok(), "{text:?}, {needle:?}");
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
    let base = snapshot(
        "crlf.txt".into(),
        "a\r\n  b = 1;\r\n  c = 2;\r\nd\r\n".into(),
    );
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
        "Expected 1 occurrence(s), found 0; inspect the snapshot and choose an explicit span or narrower scope"
    );
    let error = rejected(&base, scoped("x", "  x = 1", "r3"));
    let third = candidate(CandidateKind::Whitespace, (3, 3), "\tx = 1");
    assert_eq!(error.candidates, std::slice::from_ref(&third));

    let all = Change {
        id: "all".into(),
        target: Target::All {
            old: "  x = 1".into(),
            scope: "r3".into(),
            expected: 1,
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
        assert!(
            error
                .message
                .ends_with("choose an explicit span or narrower scope")
        );
    }
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
            scope: "r0".into(),
            expected: 3,
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
