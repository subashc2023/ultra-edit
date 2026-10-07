use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::{Range, RangeInclusive};

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
/// Visible characters a line target's `expect` needs before it guards lines the
/// base did not disclose: fewer, like a lone `}`, could match after a line shift.
pub const MIN_GUARD_CHARS: usize = 8;
/// Change IDs an `EOL_ADAPTED` warning names before counting the rest.
const ADAPTED_IDS: usize = 3;

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

/// A snapshot the engine takes of a `path` file, disclosing only `r0`: the caller
/// saw the file through another view, so its line numbers may be stale, and line
/// targets need `expect`.
pub fn file_snapshot(path: String, text: String) -> Snapshot {
    Snapshot {
        id: new_id("s"),
        path,
        digest: digest(text.as_bytes()),
        spans: vec![Span {
            id: "r0".into(),
            start: 0,
            end: text.len(),
            line: 0,
        }],
        text,
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

/// Whole lines of a text, numbered like [`line_ranges`]. Each line runs from its
/// body through its terminator, if any; only the last line can lack one.
pub(crate) struct LineIndex {
    bodies: Vec<Range<usize>>,
    len: usize,
}

impl LineIndex {
    pub(crate) fn new(text: &str) -> Self {
        Self {
            bodies: line_ranges(text).collect(),
            len: text.len(),
        }
    }

    /// Lines in the text; an empty text has one empty line.
    fn count(&self) -> usize {
        self.bodies.len()
    }

    fn body(&self, line: usize) -> Range<usize> {
        self.bodies[line - 1].clone()
    }

    /// Where the line's terminator ends: the next body's start, or the text's end.
    fn end(&self, line: usize) -> usize {
        self.bodies.get(line).map_or(self.len, |next| next.start)
    }

    fn terminator<'t>(&self, text: &'t str, line: usize) -> &'t str {
        &text[self.bodies[line - 1].end..self.end(line)]
    }

    /// Native Read shows an empty line after a final line ending; it holds no bytes,
    /// so naming it stands for the last line.
    fn clamp(&self, text: &str, line: usize) -> usize {
        let count = self.count();
        if line == count + 1 && !self.terminator(text, count).is_empty() {
            count
        } else {
            line
        }
    }

    /// One-based `[first, last]` with the phantom line clamped, when every line exists.
    fn bounds(&self, text: &str, first: usize, last: usize) -> Option<(usize, usize)> {
        let last = self.clamp(text, last);
        (1 <= first && first <= last && last <= self.count()).then_some((first, last))
    }
}

/// Whether every line ending in `text` is CRLF: at least one, no bare LF, no lone CR.
pub(crate) fn crlf_only(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut crlf = false;
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => crlf = true,
            b'\r' => return false,
            b'\n' if index == 0 || bytes[index - 1] != b'\r' => return false,
            _ => {}
        }
    }
    crlf
}

/// Text as written to a base whose line endings are all CRLF: a text holding LF but
/// no CR has each LF become CRLF, since views such as native Read hide the CR.
/// Any other text, or any text for another base, stays literal.
pub(crate) fn adapt_eol(crlf: bool, text: &str) -> Cow<'_, str> {
    if crlf && adapts(text) {
        Cow::Owned(text.replace('\n', "\r\n"))
    } else {
        Cow::Borrowed(text)
    }
}

fn adapts(text: &str) -> bool {
    text.contains('\n') && !text.contains('\r')
}

/// The range a line target replaces and the text it writes there, derived from the
/// base bytes alone, or `None` when a line is out of range or the target is not a
/// line target. `new` is the change's text, already adapted to the base's line
/// endings. Stored plans are checked by recomputing this.
///
/// Replaced lines include their terminators. Text that does not end in a line feed
/// inherits the last line's terminator, so CRLF and a missing final newline persist.
/// Empty text deletes the lines; deleting through a last line that lacks a
/// terminator also takes the line ending before the range, so the file still lacks
/// one. An insertion after line n writes whole lines after its terminator; at an
/// end without a final newline it writes the file's first terminator, then `new`.
pub(crate) fn derived_replacement(
    text: &str,
    lines: &LineIndex,
    target: &Target,
    new: &str,
) -> Option<(Range<usize>, String)> {
    let with_terminator = |terminator: &str| {
        if new.ends_with('\n') {
            new.to_owned()
        } else {
            format!("{new}{terminator}")
        }
    };
    match target {
        Target::Lines {
            lines: [first, last],
            ..
        } => {
            let (first, last) = lines.bounds(text, *first, *last)?;
            let mut start = lines.body(first).start;
            let terminator = lines.terminator(text, last);
            let written = if !new.is_empty() {
                with_terminator(terminator)
            } else {
                if terminator.is_empty() && first > 1 {
                    start = lines.body(first - 1).end;
                }
                String::new()
            };
            Some((start..lines.end(last), written))
        }
        Target::Insert { after, .. } => {
            let after = lines.clamp(text, *after);
            if after > lines.count() {
                return None;
            }
            let top = lines.body(1).start;
            if top == text.len() {
                // Nothing to separate from: an empty file receives `new` as given.
                return Some((top..top, new.to_owned()));
            }
            let first = match lines.terminator(text, 1) {
                "" => "\n",
                terminator => terminator,
            };
            if after == 0 {
                return Some((top..top, with_terminator(first)));
            }
            let at = lines.end(after);
            Some(match lines.terminator(text, after) {
                "" => (at..at, format!("{first}{new}")),
                terminator => (at..at, with_terminator(terminator)),
            })
        }
        _ => None,
    }
}

/// Whether `replacement` is what the compiler derives for `change` from `text`,
/// with the change's text literal or adapted to an all-CRLF base. Line targets
/// are recomputed in full; other targets write their text unchanged.
pub(crate) fn derives(
    text: &str,
    lines: &OnceCell<LineIndex>,
    crlf: &OnceCell<bool>,
    change: &Change,
    replacement: &Replacement,
) -> bool {
    let crlf = *crlf.get_or_init(|| crlf_only(text));
    let adapted = adapt_eol(crlf, &change.text);
    match change.target {
        Target::Lines { .. } | Target::Insert { .. } => {
            let lines = lines.get_or_init(|| LineIndex::new(text));
            let found = Some((replacement.start..replacement.end, replacement.text.clone()));
            derived_replacement(text, lines, &change.target, &change.text) == found
                || derived_replacement(text, lines, &change.target, &adapted) == found
        }
        _ => replacement.text == change.text || replacement.text == adapted,
    }
}

/// How a compile treats LF-only text for an all-CRLF base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    /// Writes CRLF, and says so with an `EOL_ADAPTED` warning.
    Adapt,
    /// Writes every byte as given, as an undo restoring recorded bytes must.
    Literal,
}

/// Plans `request` against its base snapshots, adapting line endings ([`Eol::Adapt`]).
pub fn compile(
    request: &EditRequest,
    snapshots: &BTreeMap<String, Snapshot>,
) -> Result<PreparedPlan, Vec<Diagnostic>> {
    compile_with(request, snapshots, Eol::Adapt)
}

pub fn compile_with(
    request: &EditRequest,
    snapshots: &BTreeMap<String, Snapshot>,
    eol: Eol,
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
        // A path file that could not be read keeps an empty base and already has its diagnostic.
        if file.base.trim().is_empty() && file.path.is_none() {
            diagnostics.push(at(
                path,
                None,
                "EMPTY_SNAPSHOT_ID",
                "Each file needs a path or a snapshot base",
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
        let mut context = FileContext::new(base, eol);
        let mut replacements = Vec::new();
        for change in &file.changes {
            resolve_change(
                &mut context,
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
                            "Changes {} ({}..{}) and {} ({}..{}) overlap; {}",
                            left.change_id,
                            left.start,
                            left.end,
                            right.change_id,
                            right.start,
                            right.end,
                            combine_advice(
                                context.extents.get(left.change_id.as_str()),
                                context.extents.get(right.change_id.as_str()),
                            )
                        ),
                    );
                    diagnostic.change_id = Some(left.change_id.clone());
                    diagnostic.conflicts.push(right.change_id.clone());
                    diagnostics.push(diagnostic);
                }
            }
        }
        if !duplicate_path {
            resolved.push((context, replacements, &file.changes));
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    for (context, replacements, _) in &resolved {
        let base = context.base;
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
        .map(|(context, replacements, changes)| {
            let base = context.base;
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
            warn_adapted(&base.path, &context.adapted, &mut warnings);
            // Neighbours are judged in the output, where an adjacent change's text
            // may stand beside this one instead of the original bytes.
            let crlf = context.crlf();
            let mut edges = Vec::new();
            for (id, range) in placed {
                if let Some(change) = targets.get(id) {
                    let old = match &change.target {
                        Target::Exact { old, .. } | Target::All { old, .. } => old,
                        _ => continue,
                    };
                    let (old, new) = (adapt_eol(crlf, old), adapt_eol(crlf, &change.text));
                    for edge in whitespace_edges(&output, &range, &old, &new) {
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

/// Finds the edges where an exact target's `old` and `new` agree on their outermost
/// visible character but only one of them carries whitespace there, and the output
/// beside the replacement at `placed` makes the difference visible. Dropped
/// whitespace joins the neighbouring text, or joins two lines; added spaces or tabs
/// land beside more whitespace, or at the end of a line. Span and line targets have
/// no `old` to compare and are never checked.
fn whitespace_edges(output: &str, placed: &Range<usize>, old: &str, new: &str) -> Vec<Edge> {
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

/// Says which changes had LF written as CRLF to match an all-CRLF file.
fn warn_adapted(path: &str, adapted: &[&str], warnings: &mut Vec<Diagnostic>) {
    let Some(first) = adapted.first() else {
        return;
    };
    let mut ids: Vec<String> = adapted
        .iter()
        .take(ADAPTED_IDS)
        .map(|id| id.chars().take(24).collect())
        .collect();
    if adapted.len() > ADAPTED_IDS {
        ids.push(format!("{} more", adapted.len() - ADAPTED_IDS));
    }
    let mut warning = at(
        Some(path),
        None,
        "EOL_ADAPTED",
        format!(
            "{} {}: this file ends every line with CRLF, so LF in {} text was matched and written as CRLF; text holding a \\r stays literal",
            if adapted.len() == 1 {
                "Change"
            } else {
                "Changes"
            },
            ids.join(", "),
            if adapted.len() == 1 { "its" } else { "their" },
        ),
    );
    warning.change_id = Some((*first).to_owned());
    warnings.push(warning);
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
    let (old, scope, lines) = match &change.target {
        Target::Exact { old, scope, lines } => (Some(old), scope.as_ref(), lines.as_ref()),
        Target::All {
            old,
            scope,
            expected,
            lines,
        } => {
            if *expected == 0 {
                diagnostics.push(at(
                    path,
                    Some(change),
                    "INVALID_EXPECTED_COUNT",
                    "Replace-all requires a positive expected count",
                ));
            }
            (Some(old), scope.as_ref(), lines.as_ref())
        }
        Target::Span { span, .. } => (None, Some(span), None),
        Target::Lines { lines, .. } => (None, None, Some(lines)),
        Target::Insert { .. } => {
            if change.text.is_empty() {
                diagnostics.push(at(
                    path,
                    Some(change),
                    "EMPTY_INSERTION",
                    "Inserted text must not be empty; to delete lines, use lines with new \"\"",
                ));
            }
            (None, None, None)
        }
    };
    if old.is_some_and(String::is_empty) {
        diagnostics.push(at(
            path,
            Some(change),
            "EMPTY_TARGET",
            "Exact search text must not be empty; to insert, use {\"after\":n,\"new\":...}, or replace adjacent text with itself plus the insertion",
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
    if old.is_some() && scope.is_some() && lines.is_some() {
        diagnostics.push(at(
            path,
            Some(change),
            "CONFLICTING_SCOPE",
            "Restrict `old` to a span ID or to lines, not both",
        ));
    }
    if let Some([first, last]) = lines
        && !(1 <= *first && first <= last)
    {
        diagnostics.push(at(
            path,
            Some(change),
            "INVALID_LINE_RANGE",
            format!("Lines [{first},{last}] must satisfy 1 <= first <= last"),
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
                    "{shown:?} is not a span ID; use r146, r146..r150, selection, m1, or \"lines\":[146,150]. This base discloses {disclosed}"
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
                "Span {shown} needs every line disclosed; this base discloses {disclosed}. Continue this snapshot to read the rest, or use \"lines\" with expect"
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

/// One file's base and what resolving its changes computes at most once.
struct FileContext<'a> {
    base: &'a Snapshot,
    eol: Eol,
    lines: OnceCell<LineIndex>,
    crlf: OnceCell<bool>,
    disclosed: OnceCell<HashSet<usize>>,
    /// Changes whose text was adapted to CRLF, in request order.
    adapted: Vec<&'a str>,
    /// The lines each line target addresses, which overlap advice names.
    extents: HashMap<&'a str, Extent>,
}

impl<'a> FileContext<'a> {
    fn new(base: &'a Snapshot, eol: Eol) -> Self {
        Self {
            base,
            eol,
            lines: OnceCell::new(),
            crlf: OnceCell::new(),
            disclosed: OnceCell::new(),
            adapted: Vec::new(),
            extents: HashMap::new(),
        }
    }

    /// Whether LF-only text is adapted for this base.
    fn crlf(&self) -> bool {
        self.eol == Eol::Adapt && *self.crlf.get_or_init(|| crlf_only(&self.base.text))
    }

    fn lines(&self) -> &LineIndex {
        self.lines.get_or_init(|| LineIndex::new(&self.base.text))
    }

    /// Whether the base disclosed every line in `lines` as a line span.
    fn disclosed(&self, lines: RangeInclusive<usize>) -> bool {
        let disclosed = self.disclosed.get_or_init(|| {
            self.base
                .spans
                .iter()
                .filter_map(|span| reading::line_id(&span.id))
                .collect()
        });
        // A range longer than the disclosed lines cannot be covered; this also bounds
        // the membership check by the snapshot's size.
        lines.end() - lines.start() < disclosed.len()
            && lines.into_iter().all(|line| disclosed.contains(&line))
    }

    /// `[first, last]` with the phantom line clamped, or `LINE_OUT_OF_RANGE`. A
    /// malformed pair was already reported by `validate_target`.
    fn line_bounds(
        &self,
        change: &Change,
        [first, last]: [usize; 2],
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<(usize, usize)> {
        if !(1 <= first && first <= last) {
            return None;
        }
        let lines = self.lines();
        let bounds = lines.bounds(&self.base.text, first, last);
        if bounds.is_none() {
            diagnostics.push(self.out_of_range(change, last));
        }
        bounds
    }

    fn out_of_range(&self, change: &Change, line: usize) -> Diagnostic {
        let lines = self.lines();
        let total = lines.count();
        let message = if lines.terminator(&self.base.text, total).is_empty() {
            format!(
                "Line {line} is past the end: the file has {total} lines and no final line ending; use after:{total} to append"
            )
        } else {
            format!(
                "Line {line} is past the end: the file has {total} lines; native Read also shows an empty line {} after the final line ending, which holds no text; use after:{total} to append",
                total + 1
            )
        };
        let mut diagnostic = at(
            Some(&self.base.path),
            Some(change),
            "LINE_OUT_OF_RANGE",
            message,
        );
        diagnostic.expected = Some(total);
        diagnostic.actual = Some(line);
        diagnostic
    }
}

/// The lines a line target addresses, for advice when it overlaps another change.
#[derive(Clone, Copy)]
enum Extent {
    Lines {
        first: usize,
        last: usize,
        /// The deletion also takes the line ending before `first`.
        takes_previous: bool,
    },
    After(usize),
}

/// How to merge two overlapping changes; line targets name the merged change.
fn combine_advice(left: Option<&Extent>, right: Option<&Extent>) -> String {
    let takes = |extent: Option<&Extent>| {
        matches!(
            extent,
            Some(Extent::Lines {
                takes_previous: true,
                ..
            })
        )
    };
    let note = if takes(left) || takes(right) {
        " (deleting a last line that has no line ending also takes the one before it)"
    } else {
        ""
    };
    match (left, right) {
        (
            Some(Extent::Lines { first, last, .. }),
            Some(Extent::Lines {
                first: other_first,
                last: other_last,
                ..
            }),
        ) => format!(
            "combine them into one lines change [{},{}]{note}",
            first.min(other_first),
            last.max(other_last)
        ),
        (Some(Extent::Lines { first, last, .. }), Some(Extent::After(_)))
        | (Some(Extent::After(_)), Some(Extent::Lines { first, last, .. })) => format!(
            "combine them into one lines change [{first},{last}] whose new text includes the insertion{note}"
        ),
        (Some(Extent::After(line)), Some(Extent::After(_))) => {
            format!("combine them into one after:{line} change")
        }
        _ => "combine them into one replacement".into(),
    }
}

/// Resolves one change against its file's base into replacements, or diagnostics
/// that explain why it cannot be planned.
fn resolve_change<'a>(
    context: &mut FileContext<'a>,
    change: &'a Change,
    replacements: &mut Vec<Replacement>,
    diagnostics: &mut Vec<Diagnostic>,
    budget: &mut Budget,
) {
    let base = context.base;
    let crlf = context.crlf();
    let text = adapt_eol(crlf, &change.text);
    let adapted = matches!(text, Cow::Owned(_))
        || match &change.target {
            Target::Exact { old, .. } | Target::All { old, .. } => crlf && adapts(old),
            Target::Span { expect, .. } => crlf && expect.as_deref().is_some_and(adapts),
            Target::Lines { .. } | Target::Insert { .. } => false,
        };
    if adapted {
        context.adapted.push(&change.id);
    }
    let (guard, expect) = match &change.target {
        Target::Span { span, expect } => {
            if let Some((start, end)) = scope_range(base, change, Some(span), diagnostics) {
                let actual = &base.text[start..end];
                let expect = expect.as_deref().map(|expect| adapt_eol(crlf, expect));
                if let Some(expect) = expect.filter(|text| *text != actual) {
                    // The span may be the wrong one, so search the whole snapshot.
                    let whole = 0..base.text.len();
                    let found = budget.candidates(|search| {
                        candidates::find(search, &base.text, whole, &expect, true)
                    });
                    let mut diagnostic = at(
                        Some(&base.path),
                        Some(change),
                        "EXPECTED_TEXT_MISMATCH",
                        candidates::mismatch("Span", actual, &found),
                    );
                    diagnostic.candidates = found;
                    diagnostics.push(diagnostic);
                    return;
                }
                push_replacement(
                    base,
                    change,
                    start..end,
                    &text,
                    replacements,
                    diagnostics,
                    budget,
                );
            }
            return;
        }
        Target::Lines { lines, expect } => match context.line_bounds(change, *lines, diagnostics) {
            Some((first, last)) => (Guard::Prefix { first, last }, expect),
            None => return,
        },
        Target::Insert { after, expect } => {
            let lines = context.lines();
            let line = lines.clamp(&base.text, *after);
            if line > lines.count() {
                diagnostics.push(context.out_of_range(change, *after));
                return;
            }
            // An empty insertion was reported by `validate_target`.
            if change.text.is_empty() {
                return;
            }
            (Guard::Above { line }, expect)
        }
        Target::Exact { .. } | Target::All { .. } => {
            resolve_exact(context, change, &text, replacements, diagnostics, budget);
            return;
        }
    };
    if !line_guard(
        context,
        change,
        guard,
        expect.as_deref(),
        diagnostics,
        budget,
    ) {
        return;
    }
    let found = derived_replacement(&base.text, context.lines(), &change.target, &text);
    let Some((range, written)) = found else {
        return;
    };
    let extent = match guard {
        Guard::Prefix { first, last } => Extent::Lines {
            first,
            last,
            takes_previous: range.start < context.lines().body(first).start,
        },
        Guard::Above { line } => Extent::After(line),
    };
    context.extents.insert(&change.id, extent);
    push_replacement(
        base,
        change,
        range,
        &written,
        replacements,
        diagnostics,
        budget,
    );
}

/// Resolves an `exact` or `all` target by searching its scope for `old`, adapted
/// like `text` to the base's line endings.
fn resolve_exact(
    context: &FileContext,
    change: &Change,
    text: &str,
    replacements: &mut Vec<Replacement>,
    diagnostics: &mut Vec<Diagnostic>,
    budget: &mut Budget,
) {
    let base = context.base;
    let (old, scope, lines, expected) = match &change.target {
        Target::Exact { old, scope, lines } => (old, scope.as_deref(), lines, 1),
        Target::All {
            old,
            scope,
            expected,
            lines,
        } => (old, scope.as_deref(), lines, *expected),
        Target::Span { .. } | Target::Lines { .. } | Target::Insert { .. } => return,
    };
    let old = adapt_eol(context.crlf(), old);
    let range = match lines {
        Some(lines) => context
            .line_bounds(change, *lines, diagnostics)
            .map(|(first, last)| {
                let lines = context.lines();
                (lines.body(first).start, lines.end(last))
            }),
        None => scope_range(base, change, scope, diagnostics),
    };
    if old.is_empty() || expected == 0 || (scope.is_some() && lines.is_some()) {
        return;
    }
    let Some((start, end)) = range else {
        return;
    };
    let old = old.as_ref();
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
            // The first match's whole lines: a line scope that holds it alone.
            let first = lines[0];
            let last = first + old.matches('\n').count();
            let lines: Vec<_> = lines.iter().map(usize::to_string).collect();
            let more = if actual > starts.len() {
                " and later"
            } else {
                ""
            };
            format!(
                "Expected {expected} occurrence(s), found {actual} overlapping starts ({non_overlapping} non-overlapping) at {noun} {}{more}; add surrounding text to `old`, or restrict it with \"in\":[{first},{last}]",
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
    if !budget.reserve(actual, text) {
        diagnostics.push(resource_limit(base, change));
        return;
    }
    replacements.extend(positions.into_iter().map(|position| Replacement {
        start: start + position,
        end: start + position + old.len(),
        text: text.to_owned(),
        change_id: change.id.clone(),
    }));
}

fn push_replacement(
    base: &Snapshot,
    change: &Change,
    range: Range<usize>,
    text: &str,
    replacements: &mut Vec<Replacement>,
    diagnostics: &mut Vec<Diagnostic>,
    budget: &mut Budget,
) {
    if !budget.reserve(1, text) {
        diagnostics.push(resource_limit(base, change));
        return;
    }
    replacements.push(Replacement {
        start: range.start,
        end: range.end,
        text: text.to_owned(),
        change_id: change.id.clone(),
    });
}

/// The lines a line target's `expect` is compared with.
#[derive(Clone, Copy)]
enum Guard {
    /// The first lines of `first..=last`, which the target replaces.
    Prefix { first: usize, last: usize },
    /// The lines ending at `line`, after which the target inserts.
    Above { line: usize },
}

/// Checks a line target's `expect`, and requires one, at least `MIN_GUARD_CHARS`
/// visible characters long, unless the base disclosed every addressed line: line
/// numbers from elsewhere may be stale. `expect` is compared line by line, ignoring
/// line endings, so text copied from a view that hides `\r` still guards CRLF lines.
fn line_guard(
    context: &FileContext,
    change: &Change,
    guard: Guard,
    expect: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
    budget: &mut Budget,
) -> bool {
    let base = context.base;
    let addressed = match guard {
        Guard::Prefix { first, last } => Some(first..=last),
        Guard::Above { line: 0 } => None,
        Guard::Above { line } => Some(line..=line),
    };
    let required = addressed.is_some_and(|lines| !context.disclosed(lines));
    let Some(expect) = expect else {
        if required {
            let finding = match guard {
                Guard::Prefix { first, last } if first != last => format!(
                    "Lines {first}-{last} were not disclosed by this base, so their numbers may be stale; add expect with the first line you expect there"
                ),
                Guard::Prefix { first: line, .. } | Guard::Above { line } => format!(
                    "Line {line} was not disclosed by this base, so its number may be stale; add expect with the line you expect there"
                ),
            };
            diagnostics.push(at(
                Some(&base.path),
                Some(change),
                "LINE_GUARD_REQUIRED",
                format!("{finding}, at least {MIN_GUARD_CHARS} visible characters"),
            ));
        }
        return !required;
    };
    let pieces = expected_lines(expect);
    let lines = context.lines();
    let count = pieces.len();
    let checked = match guard {
        Guard::Prefix { first, last } => {
            (count <= last - first + 1).then(|| first..=first + count - 1)
        }
        Guard::Above { line } => (count <= line).then(|| line + 1 - count..=line),
    };
    let Some(checked) = checked else {
        let finding = match guard {
            Guard::Prefix { first, last } => format!(
                "expect has {count} lines but lines [{first},{last}] have {}; it gives the first lines of the range",
                last - first + 1
            ),
            Guard::Above { line } => format!(
                "expect has {count} lines but only {line} precede the insertion; it gives the lines ending at after"
            ),
        };
        diagnostics.push(at(
            Some(&base.path),
            Some(change),
            "EXPECTED_TEXT_MISMATCH",
            finding,
        ));
        return false;
    };
    let matched = checked
        .clone()
        .zip(&pieces)
        .all(|(line, piece)| base.text[lines.body(line)] == **piece);
    if !matched {
        let actual = &base.text[lines.body(*checked.start()).start..lines.body(*checked.end()).end];
        // The lines may have moved, so search the whole snapshot for where they are.
        let needle = adapt_eol(context.crlf(), expect.strip_suffix('\n').unwrap_or(expect));
        let whole = 0..base.text.len();
        let found =
            budget.candidates(|search| candidates::find(search, &base.text, whole, &needle, true));
        let subject = if checked.start() == checked.end() {
            format!("Line {}", checked.start())
        } else {
            format!("Lines {}-{}", checked.start(), checked.end())
        };
        let mut diagnostic = at(
            Some(&base.path),
            Some(change),
            "EXPECTED_TEXT_MISMATCH",
            candidates::line_mismatch(&subject, actual, &found),
        );
        diagnostic.candidates = found;
        diagnostics.push(diagnostic);
        return false;
    }
    let visible: usize = pieces
        .iter()
        .map(|piece| piece.chars().filter(|ch| !ch.is_whitespace()).count())
        .sum();
    if required && visible < MIN_GUARD_CHARS {
        let extend = match guard {
            Guard::Prefix { .. } => "the following lines of the range",
            Guard::Above { .. } => "the lines above",
        };
        diagnostics.push(at(
            Some(&base.path),
            Some(change),
            "LINE_GUARD_WEAK",
            format!(
                "expect {} has under {MIN_GUARD_CHARS} visible characters, so it could still match after a line shift; extend it with {extend}, or target the text with `old`",
                candidates::quoted(expect, 40)
            ),
        ));
        return false;
    }
    true
}

/// A line-wise `expect` as line bodies: split on LF, each without one trailing CR,
/// and without the empty piece a final line feed leaves.
fn expected_lines(expect: &str) -> Vec<&str> {
    let mut pieces: Vec<&str> = expect
        .split('\n')
        .map(|piece| piece.strip_suffix('\r').unwrap_or(piece))
        .collect();
    if pieces.len() > 1 && pieces.last() == Some(&"") {
        pieces.pop();
    }
    pieces
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
