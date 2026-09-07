use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use crate::model::{
    Change, Diagnostic, EditRequest, PreparedFile, PreparedPlan, Replacement, Snapshot, Span,
    Target, digest, new_id,
};

pub const MAX_CHANGES: usize = 1_000;
pub const MAX_REPLACEMENTS: usize = 10_000;
pub const MAX_OVERLAP_DIAGNOSTICS: usize = 128;
/// Maximum bytes in each source/output file and in all inserted text across a plan.
pub const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;

struct Budget {
    spans_left: usize,
    bytes_left: usize,
}

impl Budget {
    fn reserve(&mut self, count: usize, text: &str) -> bool {
        let Some(spans_left) = self.spans_left.checked_sub(count) else {
            return false;
        };
        let Some(bytes_left) = text
            .len()
            .checked_mul(count)
            .and_then(|bytes| self.bytes_left.checked_sub(bytes))
        else {
            return false;
        };
        self.spans_left = spans_left;
        self.bytes_left = bytes_left;
        true
    }
}

pub fn snapshot(path: String, text: String) -> Snapshot {
    let mut spans = vec![Span {
        id: "r0".into(),
        start: 0,
        end: text.len(),
        line: 0,
    }];
    spans.extend(line_ranges(&text).enumerate().map(|(index, range)| Span {
        id: format!("r{}", index + 1),
        start: range.start,
        end: range.end,
        line: index + 1,
    }));
    Snapshot {
        id: new_id("s"),
        path,
        digest: digest(text.as_bytes()),
        text,
        spans,
    }
}

/// Line bodies exclude the leading BOM and LF/CRLF terminators. Yield only
/// offsets so focused reads allocate span IDs for disclosed lines alone.
pub(crate) fn line_ranges(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut start = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let content = &text[start..];
    let empty = content.is_empty().then_some(start..start);
    content
        .split_inclusive('\n')
        .map(move |line| {
            let body = line
                .strip_suffix('\n')
                .map_or(line, |body| body.strip_suffix('\r').unwrap_or(body));
            let range = start..start + body.len();
            start += line.len();
            range
        })
        .chain(empty)
}

pub fn compile(
    request: &EditRequest,
    snapshots: &BTreeMap<String, Snapshot>,
) -> Result<PreparedPlan, Vec<Diagnostic>> {
    let change_count = request.files.iter().try_fold(0usize, |count, file| {
        count
            .checked_add(file.changes.len())
            .filter(|count| *count <= MAX_CHANGES)
    });
    if request.files.len() > MAX_CHANGES || change_count.is_none() {
        return Err(vec![Diagnostic::new(
            "RESOURCE_LIMIT",
            format!(
                "A request supports at most {MAX_CHANGES} files and {MAX_CHANGES} total changes"
            ),
        )]);
    }
    let mut diagnostics = Vec::new();
    if request.request_id.trim().is_empty() {
        diagnostics.push(Diagnostic::new(
            "EMPTY_REQUEST_ID",
            "Request ID must not be empty",
        ));
    }
    if request.files.is_empty() {
        diagnostics.push(Diagnostic::new(
            "EMPTY_FILES",
            "At least one file is required",
        ));
    }
    let mut change_ids = BTreeSet::new();
    for file in &request.files {
        let path = snapshots.get(&file.base).map(|base| base.path.as_str());
        if file.base.trim().is_empty() {
            diagnostics.push(at(
                path,
                None,
                "EMPTY_SNAPSHOT_ID",
                "Snapshot ID must not be empty",
            ));
        }
        if file.changes.is_empty() {
            diagnostics.push(at(
                path,
                None,
                "EMPTY_CHANGES",
                "At least one change is required per file",
            ));
        }
        for change in &file.changes {
            if change.id.trim().is_empty() {
                diagnostics.push(at(
                    path,
                    Some(change),
                    "EMPTY_CHANGE_ID",
                    "Change ID must not be empty",
                ));
            } else if !change_ids.insert(&change.id) {
                diagnostics.push(at(
                    path,
                    Some(change),
                    "DUPLICATE_CHANGE_ID",
                    "Change IDs must be unique across the request",
                ));
            }
            validate_target(change, path, &mut diagnostics);
        }
    }

    let mut target_paths = BTreeSet::new();
    let mut resolved = Vec::new();
    let mut budget = Budget {
        spans_left: MAX_REPLACEMENTS,
        bytes_left: MAX_TEXT_BYTES,
    };
    let mut overlap_count = 0;
    let mut overlap_limit_reached = false;
    for file in &request.files {
        let Some(base) = snapshots.get(&file.base) else {
            if !file.base.trim().is_empty() {
                diagnostics.push(Diagnostic::new(
                    "UNKNOWN_SNAPSHOT",
                    format!("Snapshot {} is unavailable; read the file again", file.base),
                ));
            }
            continue;
        };
        let duplicate_path = !target_paths.insert(&base.path);
        if duplicate_path {
            diagnostics.push(at(
                Some(&base.path),
                None,
                "DUPLICATE_TARGET_PATH",
                "Combine changes to the same target into one file entry",
            ));
        }
        if !validate_snapshot(base, &file.base, &mut diagnostics) {
            continue;
        }
        let mut replacements = Vec::new();
        for change in &file.changes {
            resolve_change(
                base,
                change,
                &mut replacements,
                &mut diagnostics,
                &mut budget,
            );
        }
        replacements.sort_by(|left, right| {
            (left.start, left.end, &left.change_id).cmp(&(right.start, right.end, &right.change_id))
        });
        'overlaps: for (index, left) in replacements.iter().enumerate() {
            if overlap_limit_reached {
                break;
            }
            for right in &replacements[index + 1..] {
                if right.start > left.end {
                    break;
                }
                // Boundary insertions also conflict, so there is one explicit ordering policy.
                let conflict = if left.start == left.end || right.start == right.end {
                    right.start <= left.end
                } else {
                    right.start < left.end
                };
                if conflict {
                    if overlap_count == MAX_OVERLAP_DIAGNOSTICS {
                        diagnostics.push(at(Some(&base.path), None, "RESOURCE_LIMIT", format!("More than {MAX_OVERLAP_DIAGNOSTICS} overlap diagnostics; remaining conflict discovery stopped")));
                        overlap_limit_reached = true;
                        break 'overlaps;
                    }
                    overlap_count += 1;
                    let mut diagnostic = at(
                        Some(&base.path),
                        None,
                        "OVERLAPPING_CHANGES",
                        format!(
                            "Changes {} ({}..{}) and {} ({}..{}) overlap; combine them into one replacement",
                            left.change_id,
                            left.start,
                            left.end,
                            right.change_id,
                            right.start,
                            right.end
                        ),
                    );
                    diagnostic.change_id = Some(left.change_id.clone());
                    diagnostic.conflicts.push(right.change_id.clone());
                    diagnostics.push(diagnostic);
                }
            }
        }
        if !duplicate_path {
            resolved.push((base, replacements, &file.changes));
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    for (base, replacements, _) in &resolved {
        let output_size = replacements
            .iter()
            .try_fold(base.text.len(), |size, replacement| {
                size.checked_sub(replacement.end - replacement.start)?
                    .checked_add(replacement.text.len())
            });
        if output_size.is_none_or(|size| size > MAX_TEXT_BYTES) {
            diagnostics.push(at(
                Some(&base.path),
                None,
                "RESOURCE_LIMIT",
                format!("Candidate output exceeds {MAX_TEXT_BYTES} bytes"),
            ));
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let files = resolved
        .into_iter()
        .map(|(base, replacements, changes)| {
            let mut output = String::new();
            let mut cursor = 0;
            for replacement in &replacements {
                output.push_str(&base.text[cursor..replacement.start]);
                output.push_str(&replacement.text);
                cursor = replacement.end;
            }
            output.push_str(&base.text[cursor..]);
            PreparedFile {
                base: base.clone(),
                output,
                replacements,
                change_ids: changes.iter().map(|change| change.id.clone()).collect(),
            }
        })
        .collect();
    Ok(PreparedPlan {
        id: new_id("p"),
        request: request.clone(),
        files,
    })
}

fn at(
    path: Option<&str>,
    change: Option<&Change>,
    code: &str,
    message: impl Into<String>,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::new(code, message);
    diagnostic.file = path.map(str::to_owned);
    diagnostic.change_id = change.map(|change| change.id.clone());
    diagnostic
}

fn validate_target(change: &Change, path: Option<&str>, diagnostics: &mut Vec<Diagnostic>) {
    let (old, scope) = match &change.target {
        Target::Exact { old, scope } => (Some(old), scope.as_ref()),
        Target::All {
            old,
            scope,
            expected,
        } => {
            if *expected == 0 {
                diagnostics.push(at(
                    path,
                    Some(change),
                    "INVALID_EXPECTED_COUNT",
                    "Replace-all requires a positive expected count",
                ));
            }
            (Some(old), Some(scope))
        }
        Target::Span { span } => (None, Some(span)),
    };
    if old.is_some_and(String::is_empty) {
        diagnostics.push(at(
            path,
            Some(change),
            "EMPTY_TARGET",
            "Exact search text must not be empty; use an empty span for insertion",
        ));
    }
    if scope.is_some_and(|id| id.trim().is_empty()) {
        diagnostics.push(at(
            path,
            Some(change),
            "EMPTY_SPAN_ID",
            "Span ID must not be empty",
        ));
    }
}

fn validate_snapshot(base: &Snapshot, id: &str, diagnostics: &mut Vec<Diagnostic>) -> bool {
    if base.text.len() > MAX_TEXT_BYTES {
        diagnostics.push(at(
            Some(&base.path),
            None,
            "RESOURCE_LIMIT",
            format!("Snapshot exceeds {MAX_TEXT_BYTES} bytes"),
        ));
        return false;
    }
    let before = diagnostics.len();
    if base.id != id || base.id.trim().is_empty() || base.path.trim().is_empty() {
        diagnostics.push(at(
            Some(&base.path),
            None,
            "INVALID_SNAPSHOT",
            "Snapshot identity or target path is invalid",
        ));
    }
    if digest(base.text.as_bytes()) != base.digest {
        diagnostics.push(at(
            Some(&base.path),
            None,
            "SNAPSHOT_DIGEST_MISMATCH",
            "Snapshot text does not match its recorded digest",
        ));
    }
    let mut span_ids = BTreeSet::new();
    for span in &base.spans {
        if span.id.trim().is_empty() || !span_ids.insert(&span.id) {
            diagnostics.push(at(
                Some(&base.path),
                None,
                "INVALID_SPAN_ID",
                format!("Snapshot span ID {:?} is empty or duplicated", span.id),
            ));
        }
        if span.start > span.end
            || !base.text.is_char_boundary(span.start)
            || !base.text.is_char_boundary(span.end)
        {
            diagnostics.push(at(
                Some(&base.path),
                None,
                "INVALID_SPAN",
                format!(
                    "Span {} is outside the snapshot or does not lie on UTF-8 boundaries",
                    span.id
                ),
            ));
        }
    }
    diagnostics.len() == before
}

fn scope_range(
    base: &Snapshot,
    change: &Change,
    id: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(usize, usize)> {
    let Some(id) = id else {
        return Some((0, base.text.len()));
    };
    if id.trim().is_empty() {
        return None;
    }
    match base.spans.iter().find(|span| span.id == id) {
        Some(span) => Some((span.start, span.end)),
        None => {
            diagnostics.push(at(
                Some(&base.path),
                Some(change),
                "UNKNOWN_SPAN",
                format!("Span {id} is unavailable in snapshot {}", base.id),
            ));
            None
        }
    }
}

fn resolve_change(
    base: &Snapshot,
    change: &Change,
    replacements: &mut Vec<Replacement>,
    diagnostics: &mut Vec<Diagnostic>,
    budget: &mut Budget,
) {
    let (old, scope, expected) = match &change.target {
        Target::Exact { old, scope } => (old.as_str(), scope.as_deref(), 1),
        Target::All {
            old,
            scope,
            expected,
        } => (old.as_str(), Some(scope.as_str()), *expected),
        Target::Span { span } => {
            if let Some((start, end)) = scope_range(base, change, Some(span), diagnostics) {
                if !budget.reserve(1, &change.text) {
                    diagnostics.push(resource_limit(base, change));
                    return;
                }
                replacements.push(Replacement {
                    start,
                    end,
                    text: change.text.clone(),
                    change_id: change.id.clone(),
                });
            }
            return;
        }
    };
    let range = scope_range(base, change, scope, diagnostics);
    if old.is_empty() || expected == 0 {
        return;
    }
    let Some((start, end)) = range else {
        return;
    };
    let (actual, positions) =
        occurrences(&base.text[start..end], old, expected.min(budget.spans_left));
    if actual != expected {
        let code = if actual == 0 {
            "TARGET_NOT_FOUND"
        } else if matches!(change.target, Target::Exact { .. }) {
            "TARGET_AMBIGUOUS"
        } else {
            "EXPECTED_COUNT_MISMATCH"
        };
        let mut diagnostic = at(
            Some(&base.path),
            Some(change),
            code,
            format!(
                "Expected {expected} occurrence(s), found {actual}; inspect the snapshot and choose an explicit span or narrower scope"
            ),
        );
        diagnostic.expected = Some(expected);
        diagnostic.actual = Some(actual);
        diagnostics.push(diagnostic);
        return;
    }
    if !budget.reserve(actual, &change.text) {
        diagnostics.push(resource_limit(base, change));
        return;
    }
    replacements.extend(positions.into_iter().map(|position| Replacement {
        start: start + position,
        end: start + position + old.len(),
        text: change.text.clone(),
        change_id: change.id.clone(),
    }));
}

pub(crate) fn occurrences(source: &str, old: &str, retained: usize) -> (usize, Vec<usize>) {
    if old.len() > source.len() {
        return (0, Vec::new());
    }
    let needle = old.as_bytes();
    // KMP keeps overlapping counts linear even for a long, highly repetitive needle.
    let mut prefix = vec![0; needle.len()];
    for index in 1..needle.len() {
        let mut matched = prefix[index - 1];
        while matched > 0 && needle[index] != needle[matched] {
            matched = prefix[matched - 1];
        }
        if needle[index] == needle[matched] {
            matched += 1;
        }
        prefix[index] = matched;
    }
    let mut count = 0;
    let mut positions = Vec::new();
    let mut matched = 0;
    for (index, byte) in source.bytes().enumerate() {
        while matched > 0 && byte != needle[matched] {
            matched = prefix[matched - 1];
        }
        if byte == needle[matched] {
            matched += 1;
        }
        if matched == needle.len() {
            count += 1;
            if positions.len() < retained {
                positions.push(index + 1 - needle.len());
            }
            matched = prefix[matched - 1];
        }
    }
    (count, positions)
}

fn resource_limit(base: &Snapshot, change: &Change) -> Diagnostic {
    at(
        Some(&base.path),
        Some(change),
        "RESOURCE_LIMIT",
        format!(
            "A plan supports at most {MAX_REPLACEMENTS} total replacements and {MAX_TEXT_BYTES} total inserted bytes"
        ),
    )
}
