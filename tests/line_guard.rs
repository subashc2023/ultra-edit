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
        error.message.contains("then line 3 too"),
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
    assert!(error.message.contains("if copied"), "{}", error.message);
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

/// Equal lines above the anchor let several runs match; the longest is named,
/// so either remedy the message gives commits the same bytes.
#[test]
fn equal_lines_above_are_named_as_the_longest_run() {
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
            .contains("keeps lines 2-3 and new repeats them first"),
        "{}",
        error.message
    );
    assert!(error.message.contains("lines:[2,3]"), "{}", error.message);
    let dropped = outcome(&base, insert(3, Some(&expect), "    sim.check()"));
    assert_eq!(dropped.unwrap(), wanted);
    let replaced = outcome(&base, ends([2, 3], Some(tick), Some(tick), &new));
    assert_eq!(replaced.unwrap(), wanted);
    // An `expect` naming the anchor alone does not shorten the run: dropping
    // one tick would leave text that restates the anchor again.
    let error = rejected(&base, insert(3, Some(tick), &new));
    assert!(
        error.message.contains("keeps lines 2-3"),
        "{}",
        error.message
    );
    let dropped = outcome(&base, insert(3, Some(tick), "    sim.check()"));
    assert_eq!(dropped.unwrap(), wanted);
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
            .contains("if copied, send this new as lines:[1,450]"),
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

/// Each line's visible characters are counted once per file, so long lines
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

/// Lines are trimmed once per file, so insertions that start with blank lines
/// beside long whitespace-only lines stay cheap.
#[test]
fn blank_led_insertions_beside_whitespace_lines_are_checked_quickly() {
    let blank = " ".repeat(1_000);
    let text = format!("{}value = 1\n", format!("{blank}\n").repeat(200));
    let base = snapshot("full.txt".into(), text);
    let new = format!("{}value = 2", "\n".repeat(199));
    let changes: Vec<Change> = (0..1_000)
        .map(|index| {
            let mut change = insert(200, None, &new);
            change.id = format!("i{index}");
            change
        })
        .collect();
    let started = Instant::now();
    let _ = compile(&request(&base, changes), &bases(&base));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

/// Changes apply to the original file, so restated lines that other changes
/// delete whole appear once, and the batch commits.
#[test]
fn restated_lines_that_other_changes_delete_whole_appear_once() {
    let anchor = "    tax_table=resolve(region),";
    let base = spanless(&format!("totals = compute(\n{anchor}\n)\n"));
    let insertion = || Change {
        id: "i".into(),
        target: Target::Insert {
            after: 2,
            expect: Some(anchor.into()),
        },
        text: format!("{anchor}\n    rounding=1,"),
    };
    let wanted = format!("totals = compute(\n{anchor}\n    rounding=1,\n)\n");
    let mut deleted = ends([2, 2], Some(anchor), None, "");
    deleted.id = "d".into();
    let plan = compile(&request(&base, vec![deleted, insertion()]), &bases(&base));
    assert_eq!(plan.unwrap().files[0].output, wanted);
    let whole = parsed(json!({"id":"o","old":format!("{anchor}\n"),"new":""}));
    let plan = compile(&request(&base, vec![whole, insertion()]), &bases(&base));
    assert_eq!(plan.unwrap().files[0].output, wanted);
}

/// Other changes that edit restated lines in place apply to the original
/// lines, which stay, so the restated copy is dropped from `new`; following
/// that commits every change, a counted rename's other matches included.
#[test]
fn restated_lines_edited_in_place_are_dropped_from_new() {
    let follow = |text: &str,
                  changes: Vec<serde_json::Value>,
                  wanted_advice: &str,
                  followed: Vec<serde_json::Value>,
                  wanted: &str| {
        let base = spanless(text);
        let errors = compile(
            &request(&base, changes.into_iter().map(parsed).collect()),
            &bases(&base),
        )
        .unwrap_err();
        assert_eq!(errors[0].code, "INSERT_REPEATS_LINE", "{errors:?}");
        assert!(
            errors[0].message.contains(wanted_advice),
            "{}",
            errors[0].message
        );
        let plan = compile(
            &request(&base, followed.into_iter().map(parsed).collect()),
            &bases(&base),
        );
        assert_eq!(plan.unwrap().files[0].output, wanted);
    };
    let anchor = "    tax_table=resolve(region),";
    let call = format!("totals = compute(\n{anchor}\n)\n");
    let token = json!({"id":"o","old":"resolve(region)","new":"resolve(country)"});
    follow(
        &call,
        vec![
            token.clone(),
            json!({"id":"i","after":2,"expect":anchor,"new":format!("{anchor}\n    rounding=1,")}),
        ],
        "drop it from new",
        vec![
            token.clone(),
            json!({"id":"i","after":2,"expect":anchor,"new":"    rounding=1,"}),
        ],
        "totals = compute(\n    tax_table=resolve(country),\n    rounding=1,\n)\n",
    );
    // An insertion that only repeats an edited line is dropped, not emptied.
    follow(
        &call,
        vec![
            token.clone(),
            json!({"id":"i","after":2,"expect":anchor,"new":anchor}),
        ],
        "drop this insertion",
        vec![token],
        "totals = compute(\n    tax_table=resolve(country),\n)\n",
    );
    // A counted rename keeps its matches on other lines.
    let load = "def load(path):\n    cfg = read_config(path)\n    cfg.validate()\n    return cfg\n";
    let rename = json!({"id":"rename","old":"cfg","new":"config","count":3});
    follow(
        load,
        vec![
            rename.clone(),
            json!({"id":"i","after":2,"expect":"    cfg = read_config(path)","new":"    cfg = read_config(path)\n    config.normalize()"}),
        ],
        "drop it from new",
        vec![
            rename,
            json!({"id":"i","after":2,"expect":"    cfg = read_config(path)","new":"    config.normalize()"}),
        ],
        "def load(path):\n    config = read_config(path)\n    config.normalize()\n    config.validate()\n    return config\n",
    );
    // An edited line copied below the anchor: the new line goes below it.
    let connect = "def connect(host):\n    timeout = 30\n    retries = 3\n    return Client(host, timeout, retries)\n";
    let edit = json!({"id":"e","old":"retries = 3","new":"retries = 5"});
    follow(
        connect,
        vec![
            edit.clone(),
            json!({"id":"i","after":2,"expect":"    timeout = 30","new":"    timeout = 30\n    retries = 3\n    backoff = retries * 2"}),
        ],
        "drop lines 2-3 from new and insert after:3, with line 3 as expect",
        vec![
            edit,
            json!({"id":"i","after":3,"expect":"    retries = 3","new":"    backoff = retries * 2"}),
        ],
        "def connect(host):\n    timeout = 30\n    retries = 5\n    backoff = retries * 2\n    return Client(host, timeout, retries)\n",
    );
    // An insertion above the restated line does not stop the in-place advice.
    let fetch = "class Service:\n    def fetch(self, url):\n        response = requests.get(url)\n        return response.json()\n";
    let log = json!({"id":"log","after":2,"expect":"    def fetch(self, url):","new":"        log.info(\"fetching %s\", url)"});
    let timeout = json!({"id":"t","old":"requests.get(url)","new":"requests.get(url, timeout=10)"});
    follow(
        fetch,
        vec![
            log.clone(),
            timeout.clone(),
            json!({"id":"c","after":3,"expect":"        response = requests.get(url)","new":"        response = requests.get(url)\n        response.raise_for_status()"}),
        ],
        "drop it from new",
        vec![
            log,
            timeout,
            json!({"id":"c","after":3,"expect":"        response = requests.get(url)","new":"        response.raise_for_status()"}),
        ],
        "class Service:\n    def fetch(self, url):\n        log.info(\"fetching %s\", url)\n        response = requests.get(url, timeout=10)\n        response.raise_for_status()\n        return response.json()\n",
    );
}

/// Any other touch on restated or copied lines leaves which lines end up where
/// to the caller: the message names one `lines` change covering every change
/// that touches or borders those lines, and sending it commits exactly what
/// it says.
#[test]
fn restated_lines_that_other_changes_touch_are_merged_into_one_change() {
    let refused = |text: &str, changes: Vec<serde_json::Value>, range: &str| {
        let base = spanless(text);
        let changes = changes.into_iter().map(parsed).collect();
        let errors = compile(&request(&base, changes), &bases(&base)).unwrap_err();
        assert_eq!(errors[0].code, "INSERT_REPEATS_LINE", "{errors:?}");
        assert!(
            errors[0]
                .message
                .contains(&format!("send them as one change, lines:{range}")),
            "{}",
            errors[0].message
        );
        assert!(!errors[0].conflicts.is_empty());
        base
    };
    let anchor = "    tax_table=resolve(region),";
    let call = format!("totals = compute(\n{anchor}\n)\n");
    let insertion =
        json!({"id":"i","after":2,"expect":anchor,"new":format!("{anchor}\n    rounding=1,")});
    // A cut that leaves only the indentation, and a rewrite of the lines
    // around it, may drop the line or rewrite it.
    for (other, range) in [
        (
            json!({"id":"o","old":"tax_table=resolve(region),","new":""}),
            "[2,2]",
        ),
        (
            json!({"id":"o","old":format!("totals = compute(\n{anchor}"),"new":"totals = compute_all("}),
            "[1,2]",
        ),
    ] {
        refused(&call, vec![other, insertion.clone()], range);
    }
    // Removing a wrapped argument keeps the start of the statement.
    refused(
        "def run(data):\n    result = compute(data,\n                     verbose=True)\n    return result\n",
        vec![
            json!({"id":"o","old":",\n                     verbose=True","new":""}),
            json!({"id":"i","after":3,"expect":["    result = compute(data,","                     verbose=True)"],
                   "new":"    result = compute(data,\n                     verbose=True)\n    log(result)"}),
        ],
        "[2,3]",
    );
    // A deletion wider than the restated line, with a copied line below.
    let imports = "import os\nimport sys\nimport json\n\ndef main():\n    pass\n";
    let base = refused(
        imports,
        vec![
            json!({"id":"d","lines":[1,2],"expect":["import os","import sys"],"new":""}),
            json!({"id":"i","after":2,"expect":"import sys","new":"import sys\nimport json\nimport re"}),
        ],
        "[1,3]",
    );
    let one = parsed(
        json!({"id":"m","lines":[1,3],"expect":["import os","import json"],"new":"import sys\nimport json\nimport re"}),
    );
    assert_eq!(
        outcome(&base, one).unwrap(),
        "import sys\nimport json\nimport re\n\ndef main():\n    pass\n"
    );
    // A deleted anchor whose copied line below stays.
    let connect =
        "def connect(host):\n    timeout = 30\n    retries = 3\n    return Client(host)\n";
    refused(
        connect,
        vec![
            json!({"id":"d","lines":[2,2],"expect":"    timeout = 30","new":""}),
            json!({"id":"i","after":2,"expect":"    timeout = 30","new":"    timeout = 30\n    retries = 3\n    backoff = 2.0"}),
        ],
        "[2,3]",
    );
    // Deleting a final line without a line ending takes the line before's
    // ending, so the range reaches it, and an insertion at the range's edge
    // is inside it too.
    let steps = "steps:\n  - run: make build\n  - run: make test\n  - run: make deploy";
    let base = refused(
        steps,
        vec![
            json!({"id":"drop","lines":[4,4],"expect":"  - run: make deploy","new":""}),
            json!({"id":"cut","lines":[2,2],"expect":"  - run: make build","new":""}),
            json!({"id":"top","after":1,"expect":"steps:","new":"  - run: make setup"}),
            json!({"id":"i","after":2,"expect":"  - run: make build","new":"  - run: make build\n  - run: make test\n  - run: make lint"}),
        ],
        "[2,4]",
    );
    let one = vec![parsed(
        json!({"id":"m","lines":[2,4],"expect":["  - run: make build","  - run: make deploy"],"new":"  - run: make setup\n  - run: make test\n  - run: make lint"}),
    )];
    let plan = compile(&request(&base, one), &bases(&base));
    assert_eq!(
        plan.unwrap().files[0].output,
        "steps:\n  - run: make setup\n  - run: make test\n  - run: make lint"
    );
}

/// A thousand insertions beside long lines read each line once, not once per
/// insertion.
#[test]
fn many_insertions_beside_long_lines_read_each_line_once() {
    let row = "x".repeat(40_000);
    let base = snapshot("full.txt".into(), format!("{row}\n").repeat(201));
    let new = vec!["y = 1"; 200].join("\n");
    let changes: Vec<Change> = (0..999)
        .map(|index| {
            let mut change = insert(201, None, &new);
            change.id = format!("i{index}");
            change
        })
        .collect();
    let started = Instant::now();
    let _ = compile(&request(&base, changes), &bases(&base));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

/// Judging other changes beside a thousand restating insertions reads each
/// long whitespace line once per file.
#[test]
fn other_changes_beside_many_restating_insertions_are_judged_quickly() {
    let blank = " ".repeat(20_000);
    let text = format!("{}value = 1234\n", format!("{blank}\n").repeat(199));
    let base = snapshot("full.txt".into(), text);
    let new = format!("{}value = 1234\nvalue = 5678", "\n".repeat(199));
    let mut changes: Vec<Change> = (0..999)
        .map(|index| {
            let mut change = insert(200, None, &new);
            change.id = format!("i{index}");
            change
        })
        .collect();
    changes.push(parsed(
        json!({"id":"e","old":"value = 1234","new":"value = 4321"}),
    ));
    let started = Instant::now();
    let _ = compile(&request(&base, changes), &bases(&base));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}
