use crate::model::{CommitStatus, FileStatus, PreparedPlan, Receipt};
use std::fmt::Write;

/// Show literal replacement spans, with control characters and BOMs escaped.
/// Excerpts are bounded independently so a long line cannot consume later regions.
/// The plan reference retrieves the complete evidence, including elided source.
pub fn preview(plan: &PreparedPlan, max_lines: usize, max_chars: usize) -> String {
    let mut report = BoundedReport::new(max_lines, max_chars);
    let regions: usize = plan.files.iter().map(|file| file.replacements.len()).sum();
    let changes: usize = plan.files.iter().map(|file| file.change_ids.len()).sum();
    report.push(
        &format!(
            "Prepared: {}, {}, {}; plan: {}",
            count(plan.files.len(), "file"),
            count(changes, "change ID"),
            count(regions, "replacement region"),
            escaped(&plan.id, max_chars)
        ),
        max_chars,
    );
    warnings(&mut report, &plan.warnings);
    let available_lines = report.remaining_lines();
    let total_lines = regions.saturating_mul(3)
        + plan
            .files
            .iter()
            .filter(|file| !file.replacements.is_empty())
            .count();
    let capacity = available_lines.saturating_sub(usize::from(total_lines > available_lines));
    let mut shown = 0;
    let mut slots = 0;
    'files: for file in &plan.files {
        for index in 0..file.replacements.len() {
            let needed = 3 + usize::from(index == 0);
            if slots + needed > capacity {
                break 'files;
            }
            slots += needed;
            shown += 1;
        }
    }
    let omitted = regions - shown;
    let width = report.detail_width(slots + usize::from(omitted != 0));
    if width < 12 && regions != 0 {
        report.push("… region details available by plan reference", max_chars);
        return report.output;
    }
    for file in &plan.files {
        let included = shown.min(file.replacements.len());
        if included == 0 {
            continue;
        }
        report.push(
            &format!(
                "  File: {}",
                display_path(&file.base.path, width.saturating_sub(8))
            ),
            width,
        );
        for replacement in file.replacements.iter().take(included) {
            report.push(
                &format!(
                    "    {} (bytes {}..{})",
                    escaped(&replacement.change_id, width / 2),
                    replacement.start,
                    replacement.end,
                ),
                width,
            );
            let before = &file.base.text[replacement.start..replacement.end];
            let excerpt_width = width.saturating_sub(8);
            report.push(
                &format!("    - \"{}\"", escaped(before, excerpt_width)),
                width,
            );
            report.push(
                &format!("    + \"{}\"", escaped(&replacement.text, excerpt_width)),
                width,
            );
        }
        shown -= included;
    }
    if omitted != 0 {
        report.push(
            &format!("… {omitted} regions omitted; use plan reference"),
            width,
        );
    }
    report.output
}

/// Summarize persistence outcomes, counting only confirmed committed changes.
/// The request ID is the receipt retrieval reference. Unknown outcomes are never
/// counted as applied or described as not committed.
pub fn receipt(receipt: &Receipt, max_lines: usize, max_chars: usize) -> String {
    let mut report = BoundedReport::new(max_lines, max_chars);
    let committed = receipt
        .files
        .iter()
        .filter(|file| file.status == FileStatus::Committed);
    let (files, changes) = committed.fold((0, 0), |(files, changes), file| {
        (files + 1, changes + file.changes_applied)
    });
    let unknown = receipt
        .files
        .iter()
        .filter(|file| file.status == FileStatus::OutcomeUnknown)
        .count();
    report.push(
        &format!(
            "{}: {}, {files}/{} files committed, {unknown} unknown; receipt: {}",
            commit_status(receipt.commit),
            count(changes, "confirmed change ID"),
            receipt.files.len(),
            escaped(&receipt.request_id, max_chars)
        ),
        max_chars,
    );
    warnings(&mut report, &receipt.warnings);
    let slots = (receipt.files.len().saturating_mul(2) + 3).min(report.remaining_lines());
    let width = report.detail_width(slots);
    if width < 8 {
        return report.output;
    }
    for (label, value) in [
        ("Plan", receipt.plan_id.as_str()),
        ("Undo", receipt.undo.as_deref().unwrap_or("none")),
    ] {
        report.push(&format!("  {label}: {}", escaped(value, width)), width);
    }
    let failures = receipt
        .files
        .iter()
        .filter(|file| file.status != FileStatus::Committed);
    let successes = receipt
        .files
        .iter()
        .filter(|file| file.status == FileStatus::Committed);
    for file in failures.chain(successes) {
        let changes = match file.status {
            FileStatus::Committed => file.changes_applied.to_string(),
            FileStatus::NotCommitted => "0".into(),
            FileStatus::OutcomeUnknown => "unknown".into(),
        };
        let error = file
            .error
            .as_deref()
            .map(|error| format!("; error: {}", escaped(error, width)))
            .unwrap_or_default();
        if !report.push(
            &format!(
                "  {}; change IDs: {changes}{error}",
                file_status(file.status)
            ),
            width,
        ) {
            break;
        }
        report.push(
            &format!(
                "    File: {}",
                display_path(&file.path, width.saturating_sub(10))
            ),
            width,
        );
    }
    report.output
}

fn warnings(report: &mut BoundedReport, warnings: &[crate::Diagnostic]) {
    for warning in warnings.iter().take(3) {
        report.push(
            &format!(
                "  Warning {}: {}",
                escaped(&warning.code, 80),
                escaped(&warning.message, 240)
            ),
            340,
        );
    }
    if warnings.len() > 3 {
        report.push("  Further warnings available in plan/receipt evidence", 80);
    }
}

/// Add ANSI styling to a human report only when explicitly enabled by its host.
/// The printable text and line count are unchanged; escape sequences add bytes.
/// JSON callers should continue using `preview` and `receipt` directly.
pub fn terminal(report: &str, color: bool) -> String {
    if !color || report.is_empty() {
        return report.to_owned();
    }
    let mut output = String::new();
    for (index, line) in report.split('\n').enumerate() {
        if index != 0 {
            output.push('\n');
        }
        let text = line.trim_start();
        let style = if text.starts_with("- \"") {
            "\x1b[31m"
        } else if text.starts_with("+ \"") {
            "\x1b[32m"
        } else if index == 0 {
            "\x1b[1;36m"
        } else if text.starts_with("File: ") {
            "\x1b[1m"
        } else {
            ""
        };
        output.push_str(style);
        output.push_str(line);
        if !style.is_empty() {
            output.push_str("\x1b[0m");
        }
    }
    output
}

/// Produce a deterministic line diff with three context lines per hunk. Source
/// UTF-8, BOMs, and CRLF bytes remain literal; unterminated lines receive the
/// standard no-final-newline marker. Paths use Rust debug quoting.
pub fn diff(plan: &PreparedPlan) -> String {
    let mut output = String::new();
    for file in &plan.files {
        if file.base.text == file.output {
            continue;
        }
        let before = &file.base.text;
        let after = &file.output;
        let before_count = before.split_inclusive('\n').count();
        let after_count = after.split_inclusive('\n').count();
        let changes = line_changes(before, after, before_count, after_count);
        writeln!(output, "--- {:?}", format!("a/{}", file.base.path)).unwrap();
        writeln!(output, "+++ {:?}", format!("b/{}", file.base.path)).unwrap();
        let mut before_lines = before.split_inclusive('\n');
        let mut after_lines = after.split_inclusive('\n');
        let (mut before_cursor, mut after_cursor) = (0, 0);
        let mut first = 0;
        while first < changes.len() {
            let mut last = first;
            while last + 1 < changes.len()
                && changes[last + 1].before.start <= changes[last].before.end + 6
            {
                last += 1;
            }
            let before_start = changes[first].before.start.saturating_sub(3);
            let after_start = changes[first].after.start.saturating_sub(3);
            let before_end = (changes[last].before.end + 3).min(before_count);
            let after_end = (changes[last].after.end + 3).min(after_count);
            writeln!(
                output,
                "@@ -{},{} +{},{} @@",
                before_start + usize::from(before_end != before_start),
                before_end - before_start,
                after_start + usize::from(after_end != after_start),
                after_end - after_start
            )
            .unwrap();
            skip_lines(&mut before_lines, before_start - before_cursor);
            skip_lines(&mut after_lines, after_start - after_cursor);
            before_cursor = before_start;
            after_cursor = after_start;
            for change in &changes[first..=last] {
                diff_lines(
                    &mut output,
                    ' ',
                    before_lines
                        .by_ref()
                        .take(change.before.start - before_cursor),
                );
                skip_lines(&mut after_lines, change.after.start - after_cursor);
                diff_lines(
                    &mut output,
                    '-',
                    before_lines.by_ref().take(change.before.len()),
                );
                diff_lines(
                    &mut output,
                    '+',
                    after_lines.by_ref().take(change.after.len()),
                );
                before_cursor = change.before.end;
                after_cursor = change.after.end;
            }
            diff_lines(
                &mut output,
                ' ',
                before_lines.by_ref().take(before_end - before_cursor),
            );
            skip_lines(&mut after_lines, after_end - after_cursor);
            before_cursor = before_end;
            after_cursor = after_end;
            first = last + 1;
        }
    }
    output
}

struct LineChange {
    before: std::ops::Range<usize>,
    after: std::ops::Range<usize>,
}

fn line_changes(
    before: &str,
    after: &str,
    before_count: usize,
    after_count: usize,
) -> Vec<LineChange> {
    let prefix = before
        .split_inclusive('\n')
        .zip(after.split_inclusive('\n'))
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = before
        .split_inclusive('\n')
        .rev()
        .take(before_count - prefix)
        .zip(after.split_inclusive('\n').rev().take(after_count - prefix))
        .take_while(|(left, right)| left == right)
        .count();
    let before_middle = prefix..before_count - suffix;
    let after_middle = prefix..after_count - suffix;
    // Bound Myers to 200k line references, edit distance 1024 (~4 MiB of trace),
    // and 64 MiB of line-comparison bytes. Beyond any ceiling, replace only the
    // remaining middle as one coarse hunk; common prefix/suffix stay context.
    // A linear-space differ is the upgrade if highly divergent files need finer review.
    if !before_middle.is_empty()
        && !after_middle.is_empty()
        && before_middle.len() + after_middle.len() <= 200_000
    {
        let left: Vec<_> = before
            .split_inclusive('\n')
            .skip(prefix)
            .take(before_middle.len())
            .collect();
        let right: Vec<_> = after
            .split_inclusive('\n')
            .skip(prefix)
            .take(after_middle.len())
            .collect();
        if let Some(mut changes) = myers_changes(&left, &right) {
            for change in &mut changes {
                change.before = change.before.start + prefix..change.before.end + prefix;
                change.after = change.after.start + prefix..change.after.end + prefix;
            }
            return changes;
        }
    }
    vec![LineChange {
        before: before_middle,
        after: after_middle,
    }]
}

fn myers_changes(before: &[&str], after: &[&str]) -> Option<Vec<LineChange>> {
    let mut trace: Vec<Vec<usize>> = Vec::new();
    let mut comparison_bytes = 64 * 1024 * 1024usize;
    for depth in 0..=1024.min(before.len() + after.len()) {
        // Entry i follows diagonal 2*i-depth; only reachable parity is stored.
        let mut frontier = Vec::with_capacity(depth + 1);
        for index in 0..=depth {
            let mut x = if depth == 0 {
                0
            } else {
                let previous = &trace[depth - 1];
                if index == 0 || (index < depth && previous[index - 1] < previous[index]) {
                    previous[index]
                } else {
                    previous[index - 1] + 1
                }
            };
            let mut y = x + depth - 2 * index;
            while x < before.len() && y < after.len() {
                let cost = if before[x].len() == after[y].len() {
                    before[x].len() + 1
                } else {
                    1
                };
                comparison_bytes = comparison_bytes.checked_sub(cost)?;
                if before[x] != after[y] {
                    break;
                }
                x += 1;
                y += 1;
            }
            frontier.push(x);
            if x >= before.len() && y >= after.len() {
                trace.push(frontier);
                return Some(backtrack_changes(&trace, before.len(), after.len()));
            }
        }
        trace.push(frontier);
    }
    None
}

fn backtrack_changes(trace: &[Vec<usize>], mut x: usize, mut y: usize) -> Vec<LineChange> {
    let mut reversed = Vec::new();
    for depth in (1..trace.len()).rev() {
        let index = (x + depth - y) / 2;
        let previous = &trace[depth - 1];
        let insert = index == 0 || (index < depth && previous[index - 1] < previous[index]);
        let previous_index = if insert { index } else { index - 1 };
        let before = previous[previous_index];
        let after = before + depth - 1 - 2 * previous_index;
        reversed.push(LineChange {
            before: before..before + usize::from(!insert),
            after: after..after + usize::from(insert),
        });
        x = before;
        y = after;
    }
    let mut changes: Vec<LineChange> = Vec::new();
    for change in reversed.into_iter().rev() {
        if let Some(previous) = changes.last_mut()
            && previous.before.end == change.before.start
            && previous.after.end == change.after.start
        {
            previous.before.end = change.before.end;
            previous.after.end = change.after.end;
        } else {
            changes.push(change);
        }
    }
    changes
}

fn skip_lines<'a>(lines: &mut impl Iterator<Item = &'a str>, count: usize) {
    if count != 0 {
        lines.nth(count - 1);
    }
}

fn diff_lines<'a>(output: &mut String, prefix: char, lines: impl Iterator<Item = &'a str>) {
    for line in lines {
        output.push(prefix);
        output.push_str(line);
        if !line.ends_with('\n') {
            output.push_str("\n\\ No newline at end of file\n");
        }
    }
}

fn commit_status(status: CommitStatus) -> &'static str {
    match status {
        CommitStatus::Committed => "committed",
        CommitStatus::NotCommitted => "not_committed",
        CommitStatus::Partial => "partial",
        CommitStatus::OutcomeUnknown => "outcome_unknown",
    }
}

fn file_status(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Committed => "committed",
        FileStatus::NotCommitted => "not_committed",
        FileStatus::OutcomeUnknown => "outcome_unknown",
    }
}

fn escaped(text: &str, limit: usize) -> String {
    let visible: String = text
        .chars()
        .flat_map(char::escape_debug)
        .take(limit.saturating_add(1))
        .collect();
    clipped(&visible, limit)
}

fn count(value: usize, noun: &str) -> String {
    format!("{value} {noun}{}", if value == 1 { "" } else { "s" })
}

fn display_path(path: &str, limit: usize) -> String {
    let (prefix, path) = match path.strip_prefix(r"\\?\") {
        Some(path)
            if path
                .as_bytes()
                .get(0..3)
                .is_some_and(|drive| drive[0].is_ascii_alphabetic() && drive[1..] == *b":\\") =>
        {
            ("", path)
        }
        Some(stripped) if stripped.starts_with("UNC\\") => {
            let unc = &stripped[4..];
            if unc.split_once('\\').is_some_and(|(server, rest)| {
                !server.is_empty() && !rest.split('\\').next().unwrap_or_default().is_empty()
            }) {
                (r"\\", unc)
            } else {
                ("", path)
            }
        }
        _ => ("", path),
    };
    let characters = prefix.chars().chain(path.chars());
    let quoted = characters
        .clone()
        .any(|character| character != '\\' && character.escape_debug().count() != 1);
    let visible: String = characters
        .flat_map(|character| {
            // Quote exceptional paths so an escaped newline cannot imitate a literal \n.
            character
                .escape_debug()
                .skip(usize::from(character == '\\' && !quoted))
        })
        .take(limit.saturating_add(1))
        .collect();
    if quoted {
        return clipped(
            &format!("\"{}\"", clipped(&visible, limit.saturating_sub(2))),
            limit,
        );
    }
    clipped(&visible, limit)
}

fn clipped(text: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    match text.char_indices().nth(limit) {
        Some((end, _)) => {
            let mut output = text[..end].to_owned();
            output.pop();
            output.push('…');
            output
        }
        None => text.to_owned(),
    }
}

struct BoundedReport {
    output: String,
    lines: usize,
    chars: usize,
    max_lines: usize,
    max_chars: usize,
}

impl BoundedReport {
    fn new(max_lines: usize, max_chars: usize) -> Self {
        Self {
            output: String::new(),
            lines: 0,
            chars: 0,
            max_lines,
            max_chars,
        }
    }

    fn remaining_lines(&self) -> usize {
        self.max_lines - self.lines
    }

    fn detail_width(&self, slots: usize) -> usize {
        if slots == 0 {
            return 0;
        }
        (self.max_chars - self.chars)
            .saturating_sub(slots)
            .checked_div(slots)
            .unwrap_or(0)
            .min(240)
    }

    fn push(&mut self, text: &str, width: usize) -> bool {
        if self.lines == self.max_lines {
            return false;
        }
        let separator = usize::from(self.lines != 0);
        let remaining = (self.max_chars - self.chars).saturating_sub(separator);
        let text = clipped(text, remaining.min(width));
        if text.is_empty() {
            return false;
        }
        if separator != 0 {
            self.output.push('\n');
        }
        self.chars += text.chars().count() + separator;
        self.lines += 1;
        self.output.push_str(&text);
        true
    }
}
