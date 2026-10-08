//! Line-guard boundaries: overlapping heads and tails, empty last lines, capped
//! match counts, and suggested ranges that can be followed.

use std::collections::BTreeMap;

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
