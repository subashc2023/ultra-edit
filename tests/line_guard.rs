//! Line-guard boundaries: overlapping heads and tails, empty last lines, capped
//! match counts, and suggested ranges that can be followed.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::json;
use ultra_edit::compiler::{compile, snapshot};
use ultra_edit::{Change, Diagnostic, EditRequest, FileRequest, Snapshot, Target};

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

/// A base that discloses no line, as a path file's base.
fn spanless(text: &str) -> Snapshot {
    let mut base = snapshot("spanless.txt".into(), text.into());
    base.spans.clear();
    base
}

fn outcome(base: &Snapshot, change: Change) -> Result<String, Vec<Diagnostic>> {
    compile(&request(base, vec![change]), &bases(base)).map(|plan| plan.files[0].output.clone())
}

fn rejected(base: &Snapshot, change: Change) -> Diagnostic {
    let mut errors = outcome(base, change).expect_err("the change should be refused");
    assert_eq!(errors.len(), 1, "{errors:?}");
    errors.remove(0)
}

fn ends(lines: [usize; 2], head: Option<&str>, tail: Option<&str>, text: &str) -> Change {
    Change {
        id: "c".into(),
        target: Target::Lines {
            lines,
            expect: head.map(str::to_owned),
            expect_last: tail.map(str::to_owned),
        },
        text: text.into(),
    }
}

fn insert(after: usize, expect: Option<&str>, text: &str) -> Change {
    Change {
        id: "c".into(),
        target: Target::Insert {
            after,
            expect: expect.map(str::to_owned),
        },
        text: text.into(),
    }
}

fn parsed(value: serde_json::Value) -> Change {
    serde_json::from_value(value).unwrap()
}

/// An array `expect` lists the range's lines; an empty last element is an empty
/// last line, as it is in the pair form, not dropped by the join.
#[test]
fn an_array_expect_keeps_an_empty_last_line() {
    // Every line of [1,3], the last one blank, as a path file's caller gives it.
    let base = spanless("fn a() {}\n}\n\nfn b() {}\n");
    let change = parsed(json!({"id":"c","lines":[1,3],"expect":["fn a() {}","}",""],"new":""}));
    assert_eq!(outcome(&base, change).unwrap(), "fn b() {}\n");
    // A disclosed base: the array says line 3 is empty, but it holds `cccc`.
    let full = snapshot("full.txt".into(), "aaaa\nbbbb\ncccc\n".into());
    let change = parsed(json!({"id":"c","lines":[1,3],"expect":["aaaa","bbbb",""],"new":"x"}));
    assert_eq!(rejected(&full, change).code, "EXPECTED_TEXT_MISMATCH");
}

/// A guard is strong when it is long, since repeated lines such as `port = 8080`
/// are what line numbers tell apart, or when it matches one place; a short guard
/// that matches elsewhere too is weak.
#[test]
fn a_guard_must_be_long_or_match_one_place() {
    let unique = spanless("abcd\nefgh ijkl\n");
    for change in [
        ends([1, 1], Some("abcd"), None, "x"),
        ends([1, 1], Some("abcd"), Some("abcd"), "x"),
    ] {
        assert_eq!(outcome(&unique, change).unwrap(), "x\nefgh ijkl\n");
    }
    let repeated = spanless("abcd\nabcd\nefgh ijkl\n");
    for change in [
        ends([1, 1], Some("abcd"), None, "x"),
        ends([1, 1], Some("abcd"), Some("abcd"), "x"),
    ] {
        assert_eq!(rejected(&repeated, change).code, "LINE_GUARD_WEAK");
    }
    let services = spanless(&"[service]\nport = 8080\n\n".repeat(4));
    assert_eq!(
        outcome(
            &services,
            ends([5, 5], Some("port = 8080"), None, "port = 9090")
        )
        .unwrap(),
        "[service]\nport = 8080\n\n[service]\nport = 9090\n\n".to_owned()
            + &"[service]\nport = 8080\n\n".repeat(2)
    );
}

/// Past five places, a line's count is given as more than five.
#[test]
fn a_head_found_more_than_five_times_is_not_counted_as_five() {
    let base = spanless(&("zzzz zzzz\n".repeat(8) + "other line here\n"));
    let error = rejected(&base, ends([9, 9], Some("zzzz zzzz"), None, "x"));
    assert!(
        !error.message.contains("it occurs 5 times"),
        "{}",
        error.message
    );
}

/// Whole last lines found above the range are whole lines, not part of one.
#[test]
fn whole_last_lines_above_the_range_are_not_called_part_of_a_line() {
    let base = spanless("end marker line\ncommon header x\nfoo\ncommon header x\nbar\n");
    let error = rejected(
        &base,
        ends(
            [4, 5],
            Some("common header x"),
            Some("end marker line"),
            "x",
        ),
    );
    assert_eq!(error.code, "EXPECTED_TEXT_MISMATCH");
    assert!(!error.message.contains("only part of"), "{}", error.message);
}

/// A suggested range must be one the guard can accept: inside the file, and at
/// least as long as the head and tail.
#[test]
fn suggested_ranges_are_ranges_that_can_be_followed() {
    // The head is at line 5 of 5, so a three-line range cannot have moved there.
    let base = spanless("aaaa bbbb\nx\ny\nz\nhead line here\n");
    let error = rejected(&base, ends([1, 3], Some("head line here"), Some("y"), ""));
    assert!(
        !error.message.contains("use lines [5,7]"),
        "{}",
        error.message
    );
    // The tail found inside the two-line head would give a range shorter than it.
    let base = spanless("head one\nhead two\nmiddle\nlast line\n");
    let error = rejected(
        &base,
        ends([1, 4], Some("head one\nhead two"), Some("head one"), ""),
    );
    assert!(
        !error.message.contains("the range is [1,1]"),
        "{}",
        error.message
    );
}

/// A weak expect is quoted as given: a missing tail adds no trailing space.
#[test]
fn a_weak_expect_is_quoted_without_a_trailing_space() {
    let base = spanless("abcd\nabcd\n");
    let error = rejected(&base, ends([1, 1], Some("abcd"), None, "x"));
    assert_eq!(error.code, "LINE_GUARD_WEAK");
    assert!(
        error
            .message
            .starts_with("expect \"abcd\" is short and matches 2 ranges"),
        "{}",
        error.message
    );
}

/// An insertion has no range to widen, nor a range to repeat in `new`.
#[test]
fn weak_insert_advice_does_not_talk_about_a_range() {
    let base = spanless("ab\ncdef ghij\nab\n");
    let error = rejected(&base, insert(1, Some("ab"), "x"));
    assert_eq!(error.code, "LINE_GUARD_WEAK");
    assert!(
        !error.message.contains("widen the range"),
        "{}",
        error.message
    );
}

/// An `in` past the end of a file with eight-digit line counts stays within 240 chars.
#[test]
fn an_in_past_the_end_stays_within_240_chars() {
    let base = spanless(&"\n".repeat(10_000_000));
    let change = Change {
        id: "c".into(),
        target: Target::Exact {
            old: "ab".into(),
            scope: None,
            lines: Some([9_999_999, usize::MAX]),
        },
        text: "x".into(),
    };
    let error = rejected(&base, change);
    assert_eq!(error.code, "LINE_OUT_OF_RANGE");
    assert!(
        error.message.chars().count() <= 240,
        "{} chars: {}",
        error.message.chars().count(),
        error.message
    );
}

/// Boundary cases that behave as intended.
#[test]
fn line_guard_boundaries_behave_as_intended() {
    // CRLF and mixed files compare line-wise without line endings.
    for (text, head, tail, output) in [
        (
            "fn keep() {}\r\nfn remove_me() {\r\n    one();\r\n}\r\nrest\r\n",
            "fn remove_me() {",
            "}\r\n",
            "fn keep() {}\r\nrest\r\n",
        ),
        (
            "fn keep() {}\nfn remove_me() {\r\n    one();\n}\r\nrest\n",
            "fn remove_me() {\n",
            "}",
            "fn keep() {}\nrest\n",
        ),
    ] {
        let base = spanless(text);
        assert_eq!(
            outcome(&base, ends([2, 4], Some(head), Some(tail), "")).unwrap(),
            output
        );
    }
    let bom = spanless("\u{feff}fn remove_me() {\n}\nrest\n");
    assert_eq!(
        outcome(&bom, ends([1, 2], Some("fn remove_me() {"), Some("}"), "")).unwrap(),
        "\u{feff}rest\n"
    );
    // Lines removed inside the range: the tail is found earlier, and the advised
    // range commits the intended deletion.
    let base = spanless("fn keep() {}\nfn remove_me() {\n}\nfn tail() {}\nmore\n");
    let error = rejected(&base, ends([2, 4], Some("fn remove_me() {"), Some("}"), ""));
    assert!(
        error
            .message
            .contains("it is at line 3, so the range is [2,3]")
    );
    assert_eq!(
        outcome(&base, ends([2, 3], Some("fn remove_me() {"), Some("}"), "")).unwrap(),
        "fn keep() {}\nfn tail() {}\nmore\n"
    );
    // A range ending at the phantom line clamps, and the tail is checked at line T.
    let base = spanless("fn keep() {}\nfn remove_me() {\n}\n");
    assert_eq!(
        outcome(&base, ends([2, 4], Some("fn remove_me() {"), Some("}"), "")).unwrap(),
        "fn keep() {}\n"
    );
    // A disclosed base still checks a given tail, and needs no head.
    let text = "fn keep() {}\nfn other() {}\nfn remove_me() {\n    step_one();\n}\nfn tail() {}\n";
    let full = snapshot("full.txt".into(), text.into());
    assert_eq!(
        rejected(
            &full,
            ends([3, 5], Some("fn remove_me() {"), Some("nope"), "")
        )
        .code,
        "EXPECTED_TEXT_MISMATCH"
    );
    assert!(outcome(&full, ends([3, 5], None, Some("}"), "")).is_ok());
    // A tail alone is not enough for an undisclosed range.
    let base = spanless(text);
    assert_eq!(
        rejected(&base, ends([3, 5], None, Some("}"), "")).code,
        "LINE_GUARD_REQUIRED"
    );
    // after:0 takes an empty expect, and only that.
    assert!(outcome(&base, insert(0, Some(""), "x")).is_ok());
    assert!(outcome(&base, insert(0, Some("\n"), "x")).is_ok());
    assert_eq!(
        rejected(&base, insert(0, Some("fn keep() {}"), "x")).code,
        "EXPECTED_TEXT_MISMATCH"
    );
}

/// An insertion that restates the line it follows would hold that line twice, as
/// a benchmark session did when it wrote a replacement as `after`.
#[test]
fn an_insertion_that_restates_its_anchor_line_is_refused() {
    let text = "totals = compute(\n    tax_table=resolve(region),\n)\n";
    let base = spanless(text);
    let anchor = "    tax_table=resolve(region),";
    let error = rejected(
        &base,
        insert(
            2,
            Some(anchor),
            &format!("{anchor}\n    rounding=\"half_even\","),
        ),
    );
    assert_eq!(error.code, "INSERT_REPEATS_LINE");
    assert!(error.message.contains("lines:[2,2]"), "{}", error.message);
    // Without the restated line it inserts.
    assert_eq!(
        outcome(
            &base,
            insert(2, Some(anchor), "    rounding=\"half_even\",")
        )
        .unwrap(),
        "totals = compute(\n    tax_table=resolve(region),\n    rounding=\"half_even\",\n)\n"
    );
    // A deliberate duplicate replaces the line with itself twice.
    let twice = ends([2, 2], Some(anchor), None, &format!("{anchor}\n{anchor}"));
    assert_eq!(
        outcome(&base, twice).unwrap(),
        format!("totals = compute(\n{anchor}\n{anchor}\n)\n")
    );
    // LF text restating a CRLF line is caught after line-ending adaptation.
    let crlf = spanless(&text.replace('\n', "\r\n"));
    assert_eq!(
        rejected(&crlf, insert(2, Some(anchor), &format!("{anchor}\nx"))).code,
        "INSERT_REPEATS_LINE"
    );
    // A short line such as `)` may follow itself.
    assert_eq!(
        outcome(&base, insert(3, Some(")"), ")")).unwrap(),
        format!("{text})\n")
    );
}

/// A restatement may end at the anchor or have lines below it that look the
/// same, and may differ from the file only in trailing whitespace; the advice it
/// gets can be followed either way.
#[test]
fn restated_runs_are_named_whole_and_their_advice_commits_cleanly() {
    let (two, three) = (
        "    tax_table=resolve(region),",
        "    discount=lookup(customer),",
    );
    let text = format!("def f(\n{two}\n{three}\n)\n");
    let base = spanless(&text);
    let new = format!("{two}\n{three}\n    rounding=1,");
    let wanted = format!("def f(\n{two}\n{three}\n    rounding=1,\n)\n");
    // Lines 2-3 restated after line 2, as a replacement of [2,3] written as `after`.
    let error = rejected(&base, insert(2, Some(two), &new));
    assert_eq!(error.code, "INSERT_REPEATS_LINE");
    assert!(error.message.contains("keeps line 2"), "{}", error.message);
    assert!(
        error.message.contains("equal line 3 too"),
        "{}",
        error.message
    );
    assert!(error.message.contains("lines:[2,3]"), "{}", error.message);
    let replaced = ends([2, 3], Some(two), Some(three), &new);
    assert_eq!(outcome(&base, replaced).unwrap(), wanted);
    // The guarded lines copied from a multi-line expect, ending at the anchor.
    let error = rejected(&base, insert(3, Some(&format!("{two}\n{three}")), &new));
    assert_eq!(error.code, "INSERT_REPEATS_LINE");
    assert!(
        error.message.contains("keeps lines 2-3"),
        "{}",
        error.message
    );
    assert!(error.message.contains("lines:[2,3]"), "{}", error.message);
    let after = insert(3, Some(three), "    rounding=1,");
    assert_eq!(outcome(&base, after).unwrap(), wanted);
    // A restatement without the line's trailing spaces is still a restatement.
    let spaced = spanless(&format!("a\n{two}  \nb\n"));
    let error = rejected(&spaced, insert(2, Some(&format!("{two}  ")), two));
    assert_eq!(error.code, "INSERT_REPEATS_LINE");
}

/// A new sibling shaped like the next block matches the lines below the anchor;
/// dropping only the restated line keeps the next block's decorator.
#[test]
fn lines_below_the_anchor_that_look_the_same_are_offered_as_new() {
    let text = "class User:\n    @property\n    def name(self):\n        return self._name\n\n    @property\n    def age(self):\n        return self._age\n";
    let base = spanless(text);
    let anchor = "        return self._name";
    let sibling = "\n\n    @property\n    def email(self):\n        return self._email";
    let error = rejected(
        &base,
        insert(4, Some(anchor), &format!("{anchor}{sibling}")),
    );
    assert_eq!(error.code, "INSERT_REPEATS_LINE");
    assert!(error.message.contains("If copied"), "{}", error.message);
    assert!(
        error.message.contains("if new, drop line 4 from new"),
        "{}",
        error.message
    );
    let output = outcome(&base, insert(4, Some(anchor), &sibling[1..])).unwrap();
    assert!(
        output.contains("\n    @property\n    def email"),
        "{output}"
    );
    assert_eq!(output.matches("@property").count(), 3, "{output}");
}

/// Text that only repeats the kept lines is told how to duplicate on purpose.
#[test]
fn a_bare_restatement_is_told_how_to_duplicate() {
    let line = "    sim.advance_tick()";
    let base = spanless(&format!("def test():\n{line}\n    assert sim.ok()\n"));
    let error = rejected(&base, insert(2, Some(line), line));
    assert_eq!(error.code, "INSERT_REPEATS_LINE");
    assert!(
        error.message.contains("new only repeats line 2"),
        "{}",
        error.message
    );
    let twice = ends([2, 2], Some(line), None, &format!("{line}\n{line}"));
    assert_eq!(
        outcome(&base, twice).unwrap(),
        format!("def test():\n{line}\n{line}\n    assert sim.ok()\n")
    );
}

/// The run search is bounded, so a long repetitive insertion stays fast.
#[test]
fn a_long_repetitive_insertion_is_checked_quickly() {
    let rows = 50_000;
    let text = format!(
        "{}    end_of_table_marker,\ntail\n",
        "    0,\n".repeat(rows)
    );
    let base = spanless(&text);
    let new = vec!["    0,"; rows].join("\n");
    let started = std::time::Instant::now();
    let _ = outcome(
        &base,
        insert(rows + 1, Some("    end_of_table_marker,"), &new),
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

/// Changes apply to the original file, so a restated line that another change
/// deletes appears once, and the batch commits; a rewrite would leave both the
/// rewritten line and its restated original, and is refused.
#[test]
fn a_restated_line_is_allowed_only_when_the_batch_deletes_it() {
    let anchor = "    tax_table=resolve(region),";
    let text = format!("totals = compute(\n{anchor}\n)\n");
    let base = spanless(&text);
    let insert = |id: &str| Change {
        id: id.into(),
        target: Target::Insert {
            after: 2,
            expect: Some(anchor.into()),
        },
        text: format!("{anchor}\n    rounding=1,"),
    };
    let wanted = format!("totals = compute(\n{anchor}\n    rounding=1,\n)\n");
    let mut deleted = ends([2, 2], Some(anchor), None, "");
    deleted.id = "d".into();
    let plan = compile(&request(&base, vec![deleted, insert("i")]), &bases(&base));
    assert_eq!(plan.unwrap().files[0].output, wanted);
    let whole = parsed(json!({"id":"o","old":format!("{anchor}\n"),"new":""}));
    let plan = compile(&request(&base, vec![whole, insert("i")]), &bases(&base));
    assert_eq!(plan.unwrap().files[0].output, wanted);
    // A rewrite, however its `old` is cut, would leave both versions.
    for (old, new) in [
        (anchor, "    tax_table=resolve(country),"),
        ("resolve(region)", "resolve(country)"),
        (anchor, "tax_table=resolve(region),"),
    ] {
        let other = parsed(json!({"id":"o","old":old,"new":new}));
        let errors = compile(&request(&base, vec![other, insert("i")]), &bases(&base)).unwrap_err();
        assert_eq!(errors[0].code, "INSERT_REPEATS_LINE", "{errors:?}");
        assert!(
            errors[0].message.contains("another change edits"),
            "{}",
            errors[0].message
        );
    }
}

/// Equal lines above the anchor let several runs match; the one `expect` spans
/// is named, so either remedy the message gives commits the intended bytes.
#[test]
fn equal_lines_above_are_named_as_expect_spans_them() {
    let tick = "    sim.advance_tick()";
    let base = spanless(&format!(
        "def test():\n{tick}\n{tick}\n    assert sim.ok()\n"
    ));
    let expect = format!("{tick}\n{tick}");
    let new = format!("{tick}\n{tick}\n    sim.check()");
    let wanted = format!("def test():\n{tick}\n{tick}\n    sim.check()\n    assert sim.ok()\n");
    let error = rejected(&base, insert(3, Some(&expect), &new));
    assert!(
        error
            .message
            .contains("keeps lines 2-3, and new starts with their text"),
        "{}",
        error.message
    );
    assert!(error.message.contains("lines:[2,3]"), "{}", error.message);
    let dropped = outcome(&base, insert(3, Some(&expect), "    sim.check()"));
    assert_eq!(dropped.unwrap(), wanted);
    let replaced = outcome(&base, ends([2, 3], Some(tick), Some(tick), &new));
    assert_eq!(replaced.unwrap(), wanted);
    // Restating the anchor alone with one line more: `expect` names the anchor.
    let error = rejected(&base, insert(3, Some(tick), &new));
    assert!(error.message.contains("keeps line 3"), "{}", error.message);
}

/// Lines below the anchor are compared to the end of the copy, so the
/// replacement the message offers leaves no line twice.
#[test]
fn a_long_copy_is_named_through_its_end() {
    let rows: Vec<String> = (1..=500)
        .map(|number| format!("row_{number:04} = value_{number:04}"))
        .collect();
    let base = snapshot("full.txt".into(), rows.join("\n") + "\n");
    let new = format!("{}\nnew_row = 1", rows[..450].join("\n"));
    let error = rejected(&base, insert(1, None, &new));
    assert!(
        error
            .message
            .contains("If copied, send this new as lines:[1,450]"),
        "{}",
        error.message
    );
    let replaced = outcome(&base, ends([1, 450], None, None, &new)).unwrap();
    assert_eq!(replaced.matches("row_0401 = ").count(), 1);
    assert_eq!(replaced.matches("new_row = 1").count(), 1);
    let error = rejected(&base, insert(1, None, &rows[..401].join("\n")));
    assert!(
        error.message.contains("only repeats lines 1-401"),
        "{}",
        error.message
    );
}

/// Visible characters are counted only for runs that match, so long lines
/// above an anchor and a thousand restating insertions stay cheap.
#[test]
fn restating_checks_stay_quick_beside_long_lines_and_many_insertions() {
    let row = "x".repeat(40_000);
    let base = snapshot("full.txt".into(), format!("{row}\n").repeat(201));
    let started = Instant::now();
    assert!(outcome(&base, insert(201, None, &vec!["y = 1"; 200].join("\n"))).is_ok());
    let rows: Vec<String> = (1..=9_000)
        .map(|number| format!("value_{number:05} = item"))
        .collect();
    let base = snapshot("full.txt".into(), rows.join("\n") + "\n");
    let changes: Vec<Change> = (200..1_199)
        .map(|anchor| {
            let mut change = insert(
                anchor,
                None,
                &format!(
                    "{}\nadded_{anchor} = 1",
                    rows[anchor - 200..anchor].join("\n")
                ),
            );
            change.id = format!("i{anchor}");
            change
        })
        .collect();
    let errors = compile(&request(&base, changes), &bases(&base)).unwrap_err();
    assert_eq!(errors.len(), 999);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}
