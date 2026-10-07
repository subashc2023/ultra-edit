use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;

use crate::candidates;
use crate::model::{
    Candidate, Change, Diagnostic, EditRequest, PreparedFile, PreparedPlan, Replacement, Snapshot,
    Span, Target, digest, new_id,
};
use crate::reading;

pub const MAX_CHANGES: usize = 1_000;
pub const MAX_REPLACEMENTS: usize = 10_000;
pub const MAX_OVERLAP_DIAGNOSTICS: usize = 128;
/// Maximum bytes in each source/output file and in all inserted text across a plan.
pub const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;
/// Source and needle bytes that a request's failed targets may search for
/// near-miss candidates; failures beyond it are reported without candidates.
pub const MAX_CANDIDATE_SEARCH_BYTES: usize = 2 * MAX_TEXT_BYTES;
/// Failed targets per request that search for candidates, as many as a compact
/// response shows. Each search aligns a bounded number of windows, so this caps
/// the time a failing request holds the workspace lock.
pub const MAX_CANDIDATE_SEARCHES: usize = 6;
/// Match starts an ambiguous exact target locates by line.
const AMBIGUOUS_LINES: usize = 5;

struct Budget {
    spans_left: usize,
    bytes_left: usize,
    search_bytes_left: usize,
    searches_left: usize,
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

    /// Runs one candidate search against the request's shared limits.
    fn candidates(&mut self, search: impl FnOnce(&mut usize) -> Vec<Candidate>) -> Vec<Candidate> {
        let Some(left) = self.searches_left.checked_sub(1) else {
            return Vec::new();
        };
        self.searches_left = left;
        search(&mut self.search_bytes_left)
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
        search_bytes_left: MAX_CANDIDATE_SEARCH_BYTES,
        searches_left: MAX_CANDIDATE_SEARCHES,
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
                "Only one file entry may target a path. Merge these changes into one entry with one base: continue a range or search with `snapshot` so that base discloses every span they use, or target text with an unscoped exact `old`.",
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
    let mut warnings = Vec::new();
    // Edge warnings follow every byte warning, so the few a compact response
    // shows never trade a NUL or mixed line ending for a whitespace lint.
    let mut edge_warnings = Vec::new();
    let files = resolved
        .into_iter()
        .map(|(base, replacements, changes)| {
            let targets: HashMap<&str, &Change> = changes
                .iter()
                .map(|change| (change.id.as_str(), change))
                .collect();
            let mut placed = Vec::new();
            let mut output = String::new();
            let mut cursor = 0;
            for replacement in &replacements {
                output.push_str(&base.text[cursor..replacement.start]);
                let start = output.len();
                output.push_str(&replacement.text);
                cursor = replacement.end;
                placed.push((replacement.change_id.as_str(), start..output.len()));
            }
            output.push_str(&base.text[cursor..]);
            warn_output(&base.path, &output, &mut warnings);
            // Neighbours are judged in the output, where an adjacent change's text
            // may stand beside this one instead of the original bytes.
            let mut edges = Vec::new();
            for (id, range) in placed {
                if let Some(change) = targets.get(id) {
                    for edge in whitespace_edges(&output, &range, change) {
                        let junction = match edge {
                            Edge::DropsTrailing | Edge::AddsTrailing => range.end,
                            Edge::DropsLeading | Edge::AddsLeading => range.start,
                        };
                        edges.push((*change, edge, junction));
                    }
                }
            }
            warn_edges(&base.path, &output, edges, &mut edge_warnings);
            PreparedFile {
                base: base.clone(),
                output,
                replacements,
                change_ids: changes.iter().map(|change| change.id.clone()).collect(),
            }
        })
        .collect();
    warnings.append(&mut edge_warnings);
    Ok(PreparedPlan {
        id: new_id("p"),
        request: request.clone(),
        files,
        warnings,
    })
}

/// Whitespace at one edge of an exact target that the replacement drops beside
/// other text, joining them, or adds beside more whitespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Edge {
    DropsTrailing,
    AddsTrailing,
    DropsLeading,
    AddsLeading,
}

/// Edge whitespace in this sense; other Unicode spaces are left to the author.
const EDGE_WHITESPACE: [char; 4] = [' ', '\t', '\r', '\n'];
/// Characters a `WHITESPACE_EDGE` warning quotes on each side of the junction,
/// short enough that the message fits the 240 characters clients display.
const EDGE_CONTEXT_CHARS: usize = 40;
/// `WHITESPACE_EDGE` warnings per file; each one scans its output line.
const MAX_EDGE_WARNINGS: usize = 8;

/// Finds the edges where `old` and `new` agree on their outermost visible character
/// but only one of them carries whitespace there, and the output beside the
/// replacement at `placed` makes the difference visible. Dropped whitespace joins the
/// neighbouring text, or joins two lines; added spaces or tabs land beside more
/// whitespace, or at the end of a line. Span changes have no `old` to compare and are
/// never checked.
fn whitespace_edges(output: &str, placed: &Range<usize>, change: &Change) -> Vec<Edge> {
    let old = match &change.target {
        Target::Exact { old, .. } | Target::All { old, .. } => old.as_str(),
        Target::Span { .. } => return Vec::new(),
    };
    let new = change.text.as_str();
    let (old_core, new_core) = (
        old.trim_matches(EDGE_WHITESPACE),
        new.trim_matches(EDGE_WHITESPACE),
    );
    if old_core.is_empty() || new_core.is_empty() {
        return Vec::new();
    }
    let mut edges = Vec::new();
    let after = output[placed.end..].chars().next();
    if old_core.chars().next_back() == new_core.chars().next_back() {
        let old_end = &old[old.trim_end_matches(EDGE_WHITESPACE).len()..];
        let new_end = &new[new.trim_end_matches(EDGE_WHITESPACE).len()..];
        if drops(old_end, new_end, after) {
            edges.push(Edge::DropsTrailing);
        } else if adds(old_end, new_end) && matches!(after, None | Some(' ' | '\t' | '\r' | '\n')) {
            edges.push(Edge::AddsTrailing);
        }
    }
    // A byte-order mark starts the file rather than text a target could join.
    let before = output[..placed.start]
        .chars()
        .next_back()
        .filter(|ch| *ch != '\u{feff}' || placed.start != 3);
    if old_core.chars().next() == new_core.chars().next() {
        let old_start = &old[..old.len() - old.trim_start_matches(EDGE_WHITESPACE).len()];
        let new_start = &new[..new.len() - new.trim_start_matches(EDGE_WHITESPACE).len()];
        if drops(old_start, new_start, before) {
            edges.push(Edge::DropsLeading);
        } else if adds(old_start, new_start) && matches!(before, Some(' ' | '\t')) {
            // Added indentation at the start of a line is usually deliberate.
            edges.push(Edge::AddsLeading);
        }
    }
    edges
}

/// Whether `new` drops all of `old`'s edge whitespace beside `neighbour`, so text
/// joins: any visible neighbour, or a space or tab once a line ending is gone.
fn drops(old: &str, new: &str, neighbour: Option<char>) -> bool {
    !old.is_empty()
        && new.is_empty()
        && neighbour.is_some_and(|ch| {
            !ch.is_whitespace() || (old.contains('\n') && matches!(ch, ' ' | '\t'))
        })
}

/// Whether `new` adds spaces or tabs, and nothing else, at an edge where `old` has none.
fn adds(old: &str, new: &str) -> bool {
    old.is_empty() && !new.is_empty() && new.chars().all(|ch| matches!(ch, ' ' | '\t'))
}

/// Warns once per change and edge, in output order, quoting the resulting line
/// around the first junction. The bytes are written as given; the warning only
/// asks for a look, since native editors write the same text silently.
fn warn_edges(
    path: &str,
    output: &str,
    mut edges: Vec<(&Change, Edge, usize)>,
    warnings: &mut Vec<Diagnostic>,
) {
    edges.sort_by_key(|(_, edge, junction)| (*junction, *edge));
    let mut warned = BTreeSet::new();
    let (mut line, mut cursor) = (1, 0);
    for (change, edge, junction) in edges {
        if warned.len() == MAX_EDGE_WARNINGS {
            break;
        }
        if !warned.insert((change.id.as_str(), edge)) {
            continue;
        }
        line += output.as_bytes()[cursor..junction]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count();
        cursor = junction;
        let line_start = output[..junction].rfind('\n').map_or(0, |index| index + 1);
        let line_end = output[junction..]
            .find('\n')
            .map_or(output.len(), |index| junction + index);
        let before = &output[line_start..junction];
        let after = output[junction..line_end]
            .strip_suffix('\r')
            .unwrap_or(&output[junction..line_end]);
        let skipped = before.chars().count().saturating_sub(EDGE_CONTEXT_CHARS);
        let mut excerpt = String::new();
        if skipped > 0 {
            excerpt.push('…');
        }
        excerpt.extend(before.chars().skip(skipped));
        excerpt.extend(after.chars().take(EDGE_CONTEXT_CHARS));
        if after.chars().nth(EDGE_CONTEXT_CHARS).is_some() {
            excerpt.push('…');
        }
        let finding = match edge {
            Edge::DropsTrailing => "`old` ends in whitespace `new` drops, joining what follows",
            Edge::AddsTrailing => {
                "`new` ends in spaces or tabs `old` lacks, before whitespace or a line end"
            }
            Edge::DropsLeading => "`old` starts with whitespace `new` drops, joining what precedes",
            Edge::AddsLeading => {
                "`new` starts with spaces or tabs `old` lacks, after more whitespace"
            }
        };
        let id: String = change.id.chars().take(24).collect();
        warnings.push(at(
            Some(path),
            Some(change),
            "WHITESPACE_EDGE",
            format!(
                "Change {id}: {finding}; line {line} now reads {}",
                candidates::quoted(&excerpt, 2 * EDGE_CONTEXT_CHARS + 10)
            ),
        ));
    }
}

fn warn_output(path: &str, output: &str, warnings: &mut Vec<Diagnostic>) {
    if output.contains('\0') {
        warnings.push(at(
            Some(path),
            None,
            "NUL_BYTE",
            "Candidate output contains a NUL byte; literal bytes are preserved",
        ));
    }
    let mut crlf = false;
    let mut bare_lf = false;
    for (index, byte) in output.bytes().enumerate() {
        if byte == b'\n' {
            if index > 0 && output.as_bytes()[index - 1] == b'\r' {
                crlf = true;
            } else {
                bare_lf = true;
            }
            if crlf && bare_lf {
                warnings.push(at(
                    Some(path),
                    None,
                    "MIXED_LINE_ENDINGS",
                    "Candidate output contains both CRLF and bare LF line endings; literal bytes are preserved",
                ));
                break;
            }
        }
    }
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
        Target::Span { span, .. } => (None, Some(span)),
    };
    if old.is_some_and(String::is_empty) {
        diagnostics.push(at(
            path,
            Some(change),
            "EMPTY_TARGET",
            "Exact search text must not be empty; for insertion, replace adjacent text with itself plus the insertion, or use a returned zero-width span",
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
    match span_range(base, id) {
        Ok(range) => Some(range),
        Err(message) => {
            diagnostics.push(at(Some(&base.path), Some(change), "UNKNOWN_SPAN", message));
            None
        }
    }
}

/// Characters of a span ID quoted back in a diagnostic.
const QUOTED_ID_CHARS: usize = 24;

/// Resolves a span ID the base disclosed, or a line range `rA..rB` whose every line
/// it disclosed, to byte offsets. A range runs from the start of line A's body to
/// the end of line B's, like `selection`, so B's terminator is kept. Otherwise the
/// error teaches what this base discloses.
fn span_range(base: &Snapshot, id: &str) -> Result<(usize, usize), String> {
    if let Some(span) = base.spans.iter().find(|span| span.id == id) {
        return Ok((span.start, span.end));
    }
    let shown: String = if id.chars().count() > QUOTED_ID_CHARS {
        id.chars().take(QUOTED_ID_CHARS).chain(['…']).collect()
    } else {
        id.into()
    };
    let disclosed = reading::disclosed_lines(&base.spans);
    let Some((first, last)) = line_range(id) else {
        let known_shape = id == "selection"
            || ["r", "m"]
                .iter()
                .any(|prefix| id.strip_prefix(prefix).is_some_and(is_number));
        return Err(
            if known_shape || !id.bytes().any(|byte| byte.is_ascii_digit()) {
                format!("Span {shown} is not disclosed by this base, which discloses {disclosed}")
            } else {
                format!(
                    "{shown:?} is not a span ID; spans look like r146, r146..r150, selection, or m1. This base discloses {disclosed}"
                )
            },
        );
    };
    if first > last {
        return Err(format!(
            "Span {shown} is reversed; write r{last}..r{first}. This base discloses {disclosed}"
        ));
    }
    let lines: HashMap<usize, &Span> = base
        .spans
        .iter()
        .filter_map(|span| Some((reading::line_id(&span.id)?, span)))
        .collect();
    // A range longer than the disclosed lines cannot be covered; this also bounds
    // the membership check by the snapshot's size.
    let covered =
        last - first < lines.len() && (first..=last).all(|line| lines.contains_key(&line));
    match (lines.get(&first), lines.get(&last)) {
        (Some(start), Some(end)) if covered && start.start <= end.end => Ok((start.start, end.end)),
        // Genuine line spans ascend; a store edited by hand may not.
        (Some(_), Some(_)) if covered => Err(format!(
            "Span {shown} spans lines whose recorded offsets are out of order; read the file again"
        )),
        // No further read discloses a line the file lacks; counting the lines
        // costs one scan, paid only on this failure.
        _ => match line_ranges(&base.text).count() {
            total if last > total => Err(format!(
                "Span {shown} runs past the file's last line, {total}; this base discloses {disclosed}"
            )),
            _ => Err(format!(
                "Span {shown} needs every line disclosed; this base discloses {disclosed}. Read the rest by continuing this snapshot"
            )),
        },
    }
}

/// Parses a line range `rA..rB` of one-based line numbers, in either order.
fn line_range(id: &str) -> Option<(usize, usize)> {
    let (first, last) = id.strip_prefix('r')?.split_once("..r")?;
    let number = |digits: &str| {
        (is_number(digits) && !digits.starts_with('0'))
            .then(|| digits.parse().ok())
            .flatten()
    };
    Some((number(first)?, number(last)?))
}

fn is_number(digits: &str) -> bool {
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
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
        Target::Span { span, expect } => {
            if let Some((start, end)) = scope_range(base, change, Some(span), diagnostics) {
                let actual = &base.text[start..end];
                if let Some(expect) = expect.as_ref().filter(|text| *text != actual) {
                    // The span may be the wrong one, so search the whole snapshot.
                    let whole = 0..base.text.len();
                    let found = budget.candidates(|search| {
                        candidates::find(search, &base.text, whole, expect, true)
                    });
                    let mut diagnostic = at(
                        Some(&base.path),
                        Some(change),
                        "EXPECTED_TEXT_MISMATCH",
                        candidates::mismatch(actual, &found),
                    );
                    diagnostic.candidates = found;
                    diagnostics.push(diagnostic);
                    return;
                }
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
    let retained = expected.min(budget.spans_left);
    let exact = matches!(change.target, Target::Exact { .. });
    let (actual, non_overlapping, positions) = if matches!(change.target, Target::All { .. }) {
        let mut matches = base.text[start..end].match_indices(old);
        let positions: Vec<_> = matches
            .by_ref()
            .take(retained)
            .map(|(index, _)| index)
            .collect();
        let actual = positions.len() + matches.count();
        (actual, actual, positions)
    } else {
        let matches = scan_occurrences(&base.text[start..end], old, 0, retained);
        (
            matches.overlapping,
            matches.non_overlapping,
            matches.positions,
        )
    };
    if actual != expected {
        let code = if actual == 0 {
            "TARGET_NOT_FOUND"
        } else if exact {
            "TARGET_AMBIGUOUS"
        } else {
            "EXPECTED_COUNT_MISMATCH"
        };
        // Counts explain a target that matched somewhere; only a miss searches.
        let found = if actual == 0 {
            budget.candidates(|search| candidates::find_target(search, &base.text, start..end, old))
        } else {
            Vec::new()
        };
        let message = if exact && actual != 0 {
            // Locations make the repair one step; only a failure pays for the rescan.
            let starts = scan_occurrences(&base.text[start..end], old, 0, AMBIGUOUS_LINES);
            let starts: Vec<_> = starts.positions.iter().map(|at| start + at).collect();
            let mut lines = line_numbers(&base.text, &starts);
            lines.dedup();
            let noun = if lines.len() == 1 { "line" } else { "lines" };
            let lines: Vec<_> = lines.iter().map(usize::to_string).collect();
            let more = if actual > starts.len() {
                " and later"
            } else {
                ""
            };
            format!(
                "Expected {expected} occurrence(s), found {actual} overlapping starts ({non_overlapping} non-overlapping) at {noun} {}{more}; add surrounding text to `old`, or scope it to a disclosed span holding one",
                lines.join(", ")
            )
        } else if !found.is_empty() {
            candidates::not_found(expected, old, &found)
        } else {
            format!(
                "Expected {expected} occurrence(s), found {actual}; inspect the snapshot and choose an explicit span or narrower scope"
            )
        };
        let mut diagnostic = at(Some(&base.path), Some(change), code, message);
        diagnostic.expected = Some(expected);
        diagnostic.actual = Some(actual);
        diagnostic.candidates = found;
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

/// One-based line numbers of ascending byte `offsets`, counting line feeds once.
fn line_numbers(text: &str, offsets: &[usize]) -> Vec<usize> {
    let (mut line, mut cursor) = (1, 0);
    offsets
        .iter()
        .map(|&offset| {
            line += text.as_bytes()[cursor..offset]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count();
            cursor = offset;
            line
        })
        .collect()
}

struct OccurrenceScan {
    overlapping: usize,
    non_overlapping: usize,
    positions: Vec<usize>,
}

pub(crate) fn occurrences_page(
    source: &str,
    old: &str,
    offset: usize,
    retained: usize,
) -> (usize, Vec<usize>) {
    let matches = scan_occurrences(source, old, offset, retained);
    (matches.overlapping, matches.positions)
}

fn scan_occurrences(source: &str, old: &str, offset: usize, retained: usize) -> OccurrenceScan {
    if old.len() > source.len() {
        return OccurrenceScan {
            overlapping: 0,
            non_overlapping: 0,
            positions: Vec::new(),
        };
    }
    let needle = old.as_bytes();
    // KMP keeps overlapping counts linear even for a long, highly repetitive needle.
    let prefix = prefix_table(needle);
    let mut overlapping = 0;
    let mut non_overlapping = 0;
    let mut next_non_overlapping_start = 0;
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
            let start = index + 1 - needle.len();
            overlapping += 1;
            if start >= next_non_overlapping_start {
                non_overlapping += 1;
                next_non_overlapping_start = index + 1;
            }
            if overlapping > offset && positions.len() < retained {
                positions.push(start);
            }
            matched = prefix[matched - 1];
        }
    }
    OccurrenceScan {
        overlapping,
        non_overlapping,
        positions,
    }
}

/// The KMP failure function: for each prefix of `pattern`, the length of its
/// longest proper prefix that is also a suffix.
pub(crate) fn prefix_table(pattern: &[u8]) -> Vec<usize> {
    let mut prefix = vec![0; pattern.len()];
    for index in 1..pattern.len() {
        let mut matched = prefix[index - 1];
        while matched > 0 && pattern[index] != pattern[matched] {
            matched = prefix[matched - 1];
        }
        if pattern[index] == pattern[matched] {
            matched += 1;
        }
        prefix[index] = matched;
    }
    prefix
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
