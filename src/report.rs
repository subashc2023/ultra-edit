use crate::model::{CommitStatus, FileStatus, PreparedPlan, Receipt};
use std::fmt::Write;

/// Show literal replacement spans, with control characters and BOMs escaped.
/// Excerpts are bounded independently so a long line cannot consume later regions.
/// The plan reference retrieves the complete evidence, including elided source.
pub fn preview(plan: &PreparedPlan, max_lines: usize, max_chars: usize) -> String {
    let mut report = BoundedReport::new(max_lines, max_chars);
    let regions: usize = plan.files.iter().map(|file| file.replacements.len()).sum();
    report.push(
        &format!(
            "Prepared: {}, {}; plan: {}",
            count(plan.files.len(), "file"),
            count(regions, "region"),
            escaped(&plan.id, max_chars)
        ),
        max_chars,
    );
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
            count(changes, "confirmed change"),
            receipt.files.len(),
            escaped(&receipt.request_id, max_chars)
        ),
        max_chars,
    );
    let slots = (receipt.files.len().saturating_mul(2) + 3).min(report.remaining_lines());
    let width = report.detail_width(slots);
    if width < 8 {
        return report.output;
    }
    for (label, value) in [
        ("Plan", receipt.plan_id.as_str()),
        ("Undo", receipt.undo.as_deref().unwrap_or("none")),
        ("Validation", receipt.validation.as_str()),
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
            &format!("  {}; changes: {changes}{error}", file_status(file.status)),
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

/// Produce a complete deterministic unified diff using one full-file hunk.
/// Unchanged lines may therefore appear as deletions and additions. Source UTF-8,
/// BOMs, and CR bytes are retained literally, including CRLF line endings. Each
/// unterminated last line receives the standard no-final-newline marker. Paths
/// use Rust debug quoting; this report is evidence, not a patch-input protocol.
pub fn diff(plan: &PreparedPlan) -> String {
    let mut output = String::new();
    for file in &plan.files {
        if file.base.text == file.output {
            continue;
        }
        let before_lines = file.base.text.split_inclusive('\n').count();
        let after_lines = file.output.split_inclusive('\n').count();
        writeln!(output, "--- {:?}", format!("a/{}", file.base.path)).unwrap();
        writeln!(output, "+++ {:?}", format!("b/{}", file.base.path)).unwrap();
        writeln!(
            output,
            "@@ -{},{} +{},{} @@",
            usize::from(before_lines != 0),
            before_lines,
            usize::from(after_lines != 0),
            after_lines
        )
        .unwrap();
        diff_lines(&mut output, '-', &file.base.text);
        diff_lines(&mut output, '+', &file.output);
    }
    output
}

fn diff_lines(output: &mut String, prefix: char, text: &str) {
    for line in text.split_inclusive('\n') {
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
