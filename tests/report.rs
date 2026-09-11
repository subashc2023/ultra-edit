use ultra_edit::model::{
    CommitStatus, EditRequest, FileOutcome, FileStatus, PreparedFile, PreparedPlan, Receipt,
    Replacement, Snapshot,
};
use ultra_edit::report;

fn plan(before: &str, replacements: Vec<Replacement>) -> PreparedPlan {
    let mut output = String::new();
    let mut previous = 0;
    for replacement in &replacements {
        output.push_str(&before[previous..replacement.start]);
        output.push_str(&replacement.text);
        previous = replacement.end;
    }
    output.push_str(&before[previous..]);
    PreparedPlan {
        id: "p-example".into(),
        request: EditRequest {
            request_id: "edit-example".into(),
            files: Vec::new(),
        },
        files: vec![PreparedFile {
            base: Snapshot {
                id: "s-example".into(),
                path: "source.txt".into(),
                digest: String::new(),
                text: before.into(),
                spans: Vec::new(),
            },
            output,
            replacements,
            change_ids: vec!["change-example".into()],
        }],
        warnings: Vec::new(),
    }
}

fn replacement(start: usize, end: usize, text: &str) -> Replacement {
    Replacement {
        start,
        end,
        text: text.into(),
        change_id: "change-example".into(),
    }
}

fn outcome(status: FileStatus, changes_applied: usize) -> FileOutcome {
    FileOutcome {
        path: "source.txt".into(),
        before: "s-example".into(),
        intended_digest: String::new(),
        after: None,
        status,
        changes_applied,
        error: (status != FileStatus::Committed).then(|| "write interrupted".into()),
    }
}

fn receipt(commit: CommitStatus, files: Vec<FileOutcome>) -> Receipt {
    Receipt {
        request_id: "edit-example".into(),
        plan_id: "p-example".into(),
        commit,
        files,
        validation: "not_requested".into(),
        undo: Some("u-example".into()),
        warnings: Vec::new(),
    }
}

#[test]
fn deletion_appears_in_preview_and_complete_diff() {
    let plan = plan("keep\ndelete\n", vec![replacement(5, 12, "")]);
    let preview = report::preview(&plan, 20, 1000);
    assert!(preview.contains("- \"delete\\n\""), "{preview}");
    assert!(preview.contains("+ \"\""), "{preview}");
    assert!(preview.contains("plan: p-example"), "{preview}");
    let diff = report::diff(&plan);
    assert!(diff.contains("@@ -1,2 +1,1 @@\n"), "{diff}");
    assert!(diff.contains("-delete\n"), "{diff}");
    assert!(!diff.contains("+delete\n"), "{diff}");
}

#[test]
fn distant_replacements_get_separate_excerpts_without_untouched_middle() {
    let before = format!("first{}last", "\nuntouched middle".repeat(1000));
    let plan = plan(
        &before,
        vec![
            replacement(0, 5, "FIRST"),
            replacement(before.len() - 4, before.len(), "LAST"),
        ],
    );
    let preview = report::preview(&plan, 12, 600);
    assert!(preview.contains("- \"first\""), "{preview}");
    assert!(preview.contains("- \"last\""), "{preview}");
    assert!(preview.contains("+ \"FIRST\""), "{preview}");
    assert!(preview.contains("+ \"LAST\""), "{preview}");
    assert!(!preview.contains("untouched middle"), "{preview}");
    assert_eq!(preview.matches("bytes ").count(), 2);
}

#[test]
fn long_unicode_lines_are_elided_without_starving_later_regions() {
    let long = "界".repeat(10_000);
    let before = format!("{long}tail");
    let plan = plan(
        &before,
        vec![
            replacement(0, long.len(), &"🌍".repeat(10_000)),
            replacement(long.len(), before.len(), "end"),
        ],
    );
    let preview = report::preview(&plan, 10, 600);
    assert!(preview.contains('…'), "{preview}");
    assert!(preview.contains("- \"tail\""), "{preview}");
    assert!(preview.contains("+ \"end\""), "{preview}");
    assert!(preview.chars().count() <= 600);
    assert!(preview.lines().count() <= 10);
}

#[test]
fn both_reports_obey_zero_tiny_and_normal_budgets() {
    let before = "🌍\n".repeat(1000);
    let plan = plan(
        &before,
        vec![replacement(0, before.len(), &"界".repeat(1000))],
    );
    let mut receipt = receipt(
        CommitStatus::Committed,
        vec![outcome(FileStatus::Committed, 1)],
    );
    receipt.files[0].path = "long\npath🌍".repeat(1000);
    for lines in [0, 1, 2, 3, 4, 10, 40] {
        for chars in [0, 1, 2, 10, 80, 160, 400] {
            let preview = report::preview(&plan, lines, chars);
            assert!(!preview.contains("- \"\""), "{lines}/{chars}: {preview}");
            assert!(!preview.contains("+ \"\""), "{lines}/{chars}: {preview}");
            for output in [preview, report::receipt(&receipt, lines, chars)] {
                assert!(output.chars().count() <= chars, "{lines}/{chars}: {output}");
                assert!(output.lines().count() <= lines, "{lines}/{chars}: {output}");
            }
        }
    }
}

#[test]
fn unknown_outcomes_are_not_counted_as_applied() {
    let receipt = receipt(
        CommitStatus::OutcomeUnknown,
        vec![
            outcome(FileStatus::Committed, 2),
            outcome(FileStatus::OutcomeUnknown, 0),
            outcome(FileStatus::NotCommitted, 0),
        ],
    );
    let output = report::receipt(&receipt, 10, 1000);
    assert!(
        output
            .starts_with("outcome_unknown: 2 confirmed change IDs, 1/3 files committed, 1 unknown"),
        "{output}"
    );
    assert!(
        output.contains("outcome_unknown; change IDs: unknown"),
        "{output}"
    );
    assert!(output.contains("not_committed; change IDs: 0"), "{output}");
    assert!(output.contains("error: write interrupted"), "{output}");
}

#[test]
fn partial_receipt_retains_status_counts_and_reference_before_long_paths() {
    let mut receipt = receipt(
        CommitStatus::Partial,
        vec![
            outcome(FileStatus::Committed, 3),
            outcome(FileStatus::NotCommitted, 0),
        ],
    );
    receipt.files[0].path = "long-path".repeat(10_000);
    let output = report::receipt(&receipt, 1, 160);
    assert!(
        output.starts_with("partial: 3 confirmed change IDs, 1/2 files committed, 0 unknown"),
        "{output}"
    );
    assert!(output.contains("receipt: edit-example"), "{output}");
    assert!(!output.contains("long-path"), "{output}");
}

#[test]
fn diff_preserves_bom_crlf_and_missing_final_newline() {
    let plan = plan("\u{feff}old\r\nend", vec![replacement(3, 6, "new")]);
    let output = report::diff(&plan);
    assert_eq!(
        output,
        "--- \"a/source.txt\"\n+++ \"b/source.txt\"\n@@ -1,2 +1,2 @@\n-\u{feff}old\r\n+\u{feff}new\r\n end\n\\ No newline at end of file\n"
    );
    let preview = report::preview(&plan, 10, 400);
    assert!(preview.contains("- \"old\""), "{preview}");
    let bom = plan_with_all_text_replaced("\u{feff}old\r\n");
    assert!(report::preview(&bom, 10, 500).contains("\\u{feff}old\\r\\n"));
}

fn plan_with_all_text_replaced(before: &str) -> PreparedPlan {
    plan(before, vec![replacement(0, before.len(), "")])
}

#[test]
fn full_diff_includes_complete_long_deletions_and_empty_file_ranges() {
    let before = "界".repeat(10_000);
    let output = report::diff(&plan_with_all_text_replaced(&before));
    assert!(output.contains("@@ -1,1 +0,0 @@\n"));
    assert!(output.contains(&format!("-{before}\n\\ No newline at end of file\n")));
    let output = report::diff(&plan("", vec![replacement(0, 0, "new\n")]));
    assert!(output.contains("@@ -0,0 +1,1 @@\n+new\n"));
}

fn apply_diff(before: &str, diff: &str) -> String {
    let source: Vec<_> = before.split_inclusive('\n').collect();
    let mut lines = diff.split_inclusive('\n').peekable();
    let mut output = String::new();
    let (mut old_cursor, mut new_cursor) = (0, 0);
    if diff.is_empty() {
        return before.into();
    }
    assert!(lines.next().unwrap().starts_with("--- "));
    assert!(lines.next().unwrap().starts_with("+++ "));
    while let Some(header) = lines.next() {
        let ranges: Vec<_> = header.split_whitespace().collect();
        assert_eq!(ranges[0], "@@");
        let range = |text: &str| {
            let (start, count) = text[1..].split_once(',').unwrap();
            let count: usize = count.parse().unwrap();
            (
                start.parse::<usize>().unwrap() - usize::from(count != 0),
                count,
            )
        };
        let (old_start, old_count) = range(ranges[1]);
        let (new_start, new_count) = range(ranges[2]);
        for line in &source[old_cursor..old_start] {
            output.push_str(line);
            new_cursor += 1;
        }
        old_cursor = old_start;
        assert_eq!(new_cursor, new_start, "{diff}");
        while lines.peek().is_some_and(|line| !line.starts_with("@@ ")) {
            let line = lines.next().unwrap();
            let mut content = &line[1..];
            if lines.peek() == Some(&"\\ No newline at end of file\n") {
                lines.next();
                content = content.strip_suffix('\n').unwrap();
            }
            match line.as_bytes()[0] {
                b' ' | b'-' => {
                    assert_eq!(source[old_cursor], content, "{diff}");
                    old_cursor += 1;
                }
                b'+' => {}
                _ => panic!("Invalid diff line: {line:?}"),
            }
            if !line.starts_with('-') {
                output.push_str(content);
                new_cursor += 1;
            }
        }
        assert_eq!(old_cursor, old_start + old_count, "{diff}");
        assert_eq!(new_cursor, new_start + new_count, "{diff}");
    }
    for line in &source[old_cursor..] {
        output.push_str(line);
    }
    output
}

#[test]
fn diff_reconstructs_both_sides_with_minimal_edits_for_repeated_unicode_lines() {
    let mut corpus = vec![String::new(), "\u{feff}a\r\n\0\r".into(), "\n\nlast".into()];
    for length in 1..=4 {
        for bits in 0..1 << length {
            let text: String = (0..length)
                .map(|index| {
                    if bits & (1 << index) == 0 {
                        "a\n"
                    } else {
                        "é\r\n"
                    }
                })
                .collect();
            corpus.push(text.strip_suffix('\n').unwrap().into());
            corpus.push(text);
        }
    }
    for before in &corpus {
        for after in &corpus {
            let diff = report::diff(&plan(before, vec![replacement(0, before.len(), after)]));
            assert_eq!(apply_diff(before, &diff), *after, "{before:?} -> {after:?}");
            let left: Vec<_> = before.split_inclusive('\n').collect();
            let right: Vec<_> = after.split_inclusive('\n').collect();
            let mut lcs = vec![vec![0; right.len() + 1]; left.len() + 1];
            for (i, old) in left.iter().enumerate() {
                for (j, new) in right.iter().enumerate() {
                    lcs[i + 1][j + 1] = if old == new {
                        lcs[i][j] + 1
                    } else {
                        lcs[i][j + 1].max(lcs[i + 1][j])
                    };
                }
            }
            let edits = diff
                .split_inclusive('\n')
                .skip(2)
                .filter(|line| line.starts_with(['-', '+']))
                .count();
            assert_eq!(
                edits,
                left.len() + right.len() - 2 * lcs[left.len()][right.len()],
                "{diff}"
            );
        }
    }
}

#[test]
fn diff_separates_distant_changes_with_three_lines_of_context() {
    let before: String = (0..10_000).map(|line| format!("line {line}\n")).collect();
    let first = before.find("line 50\n").unwrap();
    let last = before.find("line 9950\n").unwrap();
    let plan = plan(
        &before,
        vec![
            replacement(first, first + "line 50\n".len(), "changed first\n"),
            replacement(last, last + "line 9950\n".len(), "changed last\n"),
        ],
    );
    let diff = report::diff(&plan);
    assert_eq!(diff.matches("@@ -").count(), 2, "{diff}");
    assert!(diff.contains("@@ -48,7 +48,7 @@\n"), "{diff}");
    assert!(diff.contains("@@ -9948,7 +9948,7 @@\n"), "{diff}");
    assert!(diff.lines().count() <= 20, "{diff}");
    assert!(!diff.contains("line 5000"));
    assert_eq!(apply_diff(&before, &diff), plan.files[0].output);
}

#[test]
fn diff_groups_nearby_insertions_deletions_and_boundary_changes() {
    for (before, after, hunks) in [
        (
            "1\n2\n3\n4\n5\n6\n7\n8\n9\n",
            "first\n1\n2\n3\n4\n5\n6\n7\n8\n9\nlast",
            2,
        ),
        ("1\n2\n3\n4\n5\n", "1\nnew\n2\n3\n5\n", 1),
        ("1\n2\n3\n4\n", "2\n3\n", 1),
        ("same\nlast", "same\nlast\n", 1),
        ("", "new", 1),
        ("old", "", 1),
    ] {
        let diff = report::diff(&plan(before, vec![replacement(0, before.len(), after)]));
        assert_eq!(diff.matches("@@ -").count(), hunks, "{diff}");
        assert_eq!(apply_diff(before, &diff), after);
    }
}

#[test]
fn diff_complexity_fallback_remains_complete_and_trims_common_outer_lines() {
    let prefix = "shared prefix\n".repeat(20);
    let suffix = "shared suffix\n".repeat(20);
    for count in [600, 100_001] {
        let before = format!("{prefix}{}{suffix}", "old\n".repeat(count));
        let after = format!("{prefix}{}{suffix}", "new\n".repeat(count));
        let diff = report::diff(&plan(&before, vec![replacement(0, before.len(), &after)]));
        assert_eq!(diff.matches("@@ -").count(), 1);
        assert_eq!(diff.matches(" shared prefix\n").count(), 3);
        assert_eq!(diff.matches(" shared suffix\n").count(), 3);
        assert_eq!(apply_diff(&before, &diff), after);
    }
}

#[test]
fn preview_distinguishes_change_ids_from_replacement_regions() {
    let plan = plan("a a", vec![replacement(0, 1, "b"), replacement(2, 3, "b")]);
    let preview = report::preview(&plan, 20, 1000);
    assert!(preview.contains("1 change ID"), "{preview}");
    assert!(preview.contains("2 replacement regions"), "{preview}");
}

#[test]
fn human_paths_are_readable_without_changing_evidence() {
    for (path, displayed) in [
        (r"\\?\C:\Users\meeet\retry.rs", r"C:\Users\meeet\retry.rs"),
        (r"\\?\UNC\server\share\retry.rs", r"\\server\share\retry.rs"),
        (
            r"\\?\Volume{example}\retry.rs",
            r"\\?\Volume{example}\retry.rs",
        ),
        (r"\\?\UNC\server", r"\\?\UNC\server"),
        (r"/tmp/literal\name.rs", r"/tmp/literal\name.rs"),
        (
            "C:\\odd\nname\t\u{1b}\u{feff}.rs",
            r#""C:\\odd\nname\t\u{1b}\u{feff}.rs""#,
        ),
        (r"/tmp/a\nb.rs", r"/tmp/a\nb.rs"),
        ("/tmp/a\nb.rs", r#""/tmp/a\nb.rs""#),
    ] {
        let mut plan = plan_with_all_text_replaced("before");
        plan.files[0].base.path = path.into();
        let mut receipt = receipt(
            CommitStatus::Committed,
            vec![outcome(FileStatus::Committed, 1)],
        );
        receipt.files[0].path = path.into();
        for output in [
            report::preview(&plan, 20, 2_000),
            report::receipt(&receipt, 20, 2_000),
        ] {
            assert!(output.contains(&format!("File: {displayed}")), "{output}");
            assert!(!output.contains('\u{1b}'));
        }
        assert_eq!(plan.files[0].base.path, path);
        assert_eq!(receipt.files[0].path, path);
        let path = report::path_for_display(path);
        assert!(report::diff(&plan).starts_with(&format!("--- {:?}\n", format!("a/{path}"))));
    }
}

#[test]
fn structured_path_projection_changes_only_filesystem_path_fields() {
    let mut value = serde_json::json!({
        "path": r"\\?\C:\Users\meeet\source.rs",
        "nested": {
            "file": r"\\?\UNC\server\share\source.rs",
            "message": r"keep \\?\C:\Users\meeet\source.rs",
            "base": r"\\?\C:\Users\meeet\source.rs",
        },
        "items": [
            {"path": r"\\?\Volume{example}\source.rs"},
            {"file": null},
        ],
    });
    report::display_paths(&mut value);
    assert_eq!(value["path"], r"C:\Users\meeet\source.rs");
    assert_eq!(value["nested"]["file"], r"\\server\share\source.rs");
    assert_eq!(
        value["nested"]["message"],
        r"keep \\?\C:\Users\meeet\source.rs"
    );
    assert_eq!(value["nested"]["base"], r"\\?\C:\Users\meeet\source.rs");
    assert_eq!(value["items"][0]["path"], r"\\?\Volume{example}\source.rs");
    assert!(value["items"][1]["file"].is_null());
}

#[test]
fn terminal_colors_are_explicit_and_preserve_bounded_printable_text() {
    let plan = plan("old\u{1b}[31m\n", vec![replacement(0, 4, "new\t")]);
    let plain = report::preview(&plan, 10, 500);
    assert!(!plain.contains('\u{1b}'));
    assert_eq!(report::terminal(&plain, false), plain);
    let colored = report::terminal(&plain, true);
    assert!(colored.contains("\u{1b}[31m    - \""), "{colored}");
    assert!(colored.contains("\u{1b}[32m    + \""), "{colored}");
    let unstyled = [
        "\u{1b}[31m",
        "\u{1b}[32m",
        "\u{1b}[1;36m",
        "\u{1b}[1m",
        "\u{1b}[0m",
    ]
    .iter()
    .fold(colored, |output, style| output.replace(style, ""));
    assert_eq!(unstyled, plain);
    assert!(unstyled.chars().count() <= 500);
    assert_eq!(report::terminal("", true), "");
}
