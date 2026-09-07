use std::collections::BTreeMap;

use ultra_edit::compiler::{compile, snapshot};
use ultra_edit::{Change, EditRequest, FileRequest, Snapshot, Span, Target};

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
    }
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
