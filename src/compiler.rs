use std::borrow::Cow;
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::{Range, RangeInclusive};

use crate::candidates;
use crate::model::{
    Candidate, CandidateKind, Change, Diagnostic, EditRequest, PreparedFile, PreparedPlan,
    Replacement, Snapshot, Span, Target, digest, new_id,
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
/// Visible characters that make a line guard strong wherever else its text occurs:
/// repeated lines such as `port = 8080` are what line numbers tell apart.
pub const MIN_GUARD_CHARS: usize = 8;
/// How far a word is read on each side of a match boundary, to quote it and to
/// look for non-ASCII letters.
const WORD_CHARS: usize = 40;
/// Matches ambiguity advice examines for one alone in its lines, which bounds
/// the word checks a long run of matches costs.
const ADVISED_MATCHES: usize = 10_000;
/// Change IDs an `EOL_ADAPTED` or `SPACES_ADAPTED` warning names before counting
/// the rest.
const ADAPTED_IDS: usize = 3;
/// Unicode spaces an ASCII space in `old` may stand for: rendered text, such as a
/// task's prompt, shows them as ordinary spaces, so text typed from it has one.
const ADAPTED_SPACES: [char; 4] = ['\u{a0}', '\u{2007}', '\u{2009}', '\u{202f}'];

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

    /// The line holding byte `offset`; a leading BOM belongs to line 1.
    fn line_of(&self, offset: usize) -> usize {
        self.bodies
            .partition_point(|body| body.start <= offset)
            .max(1)
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

/// Whether a change's text stays literal for an all-CRLF base: its `old` or span
/// `expect` holds a CR, so its author sees the CRs, and LF in its text is meant,
/// as when converting CRLF to LF.
fn literal(change: &Change) -> bool {
    match &change.target {
        Target::Exact { old, .. } | Target::All { old, .. } | Target::Every { old, .. } => {
            old.contains('\r')
        }
        Target::Span { expect, .. } => expect
            .as_deref()
            .is_some_and(|expect| expect.contains('\r')),
        Target::Lines { .. } | Target::Insert { .. } => false,
    }
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
/// with the change's text literal or adapted to an all-CRLF base, and for an
/// `exact` target, with the base's spaces that its `old` matched. Line targets
/// are recomputed in full; other targets write their text unchanged.
pub(crate) fn derives(
    text: &str,
    lines: &OnceCell<LineIndex>,
    crlf: &OnceCell<bool>,
    change: &Change,
    replacement: &Replacement,
) -> bool {
    // A change kept literal by its `old` was never adapted.
    let crlf = *crlf.get_or_init(|| crlf_only(text)) && !literal(change);
    let adapted = adapt_eol(crlf, &change.text);
    match &change.target {
        Target::Lines { .. } | Target::Insert { .. } => {
            let lines = lines.get_or_init(|| LineIndex::new(text));
            [change.text.as_str(), &adapted].into_iter().any(|written| {
                derived_replacement(text, lines, &change.target, written).is_some_and(
                    |(range, derived)| {
                        range.start == replacement.start
                            && kept_tail(text, replacement, range.end, &derived)
                    },
                )
            })
        }
        target => {
            let fits = |written: &str| {
                let rest = written.len().saturating_sub(replacement.text.len());
                kept_tail(text, replacement, replacement.end + rest, written)
            };
            let spaced = || match target {
                Target::Exact { old, .. } if adapts_spaces(old) => {
                    let old = adapt_eol(crlf, old);
                    kept_spaces(&old, spaced_at(text, replacement.start, &old)?, &adapted)
                }
                _ => None,
            };
            fits(&change.text) || fits(&adapted) || spaced().is_some_and(|written| fits(&written))
        }
    }
}

/// Whether `replacement` writes `derived` over the base up to `end`, or stops
/// early where the next change starts: then `derived` ends with the base bytes
/// it stops before, which [`merge_kept_overlaps`] left in place.
fn kept_tail(text: &str, replacement: &Replacement, end: usize, derived: &str) -> bool {
    replacement.end <= end
        && derived.starts_with(replacement.text.as_str())
        && text.get(replacement.end..end) == derived.get(replacement.text.len()..)
}

/// How a compile treats text typed from a view that hides bytes: LF-only text for
/// an all-CRLF base, and an `old`'s ASCII spaces where the base has Unicode ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    /// Writes CRLF, and the base's spaces where an `old` matched them, saying so
    /// with `EOL_ADAPTED` and `SPACES_ADAPTED` warnings.
    Adapt,
    /// Writes every byte as given, as an undo restoring recorded bytes must.
    Literal,
}

/// Plans `request` against its base snapshots, adapting line endings and spaces
/// ([`Eol::Adapt`]).
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
        keep_moves_apart(&mut context, &mut replacements, &mut diagnostics);
        refuse_restatements(&context, &replacements, &mut diagnostics);
        replacements.sort_by(|left, right| {
            (left.start, left.end, &left.change_id).cmp(&(right.start, right.end, &right.change_id))
        });
        merge_kept_overlaps(&base.text, &mut replacements);
        'overlaps: for (index, left) in replacements.iter().enumerate() {
            if overlap_limit_reached {
                break;
            }
            for right in &replacements[index + 1..] {
                if right.start > left.end {
                    break;
                }
                if conflicts(left, right) {
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
    let mut moves = HashMap::new();
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
            warn_moved(&base.path, &context.moved, &mut warnings);
            warn_adapted(&base.path, &context.adapted, &mut warnings);
            warn_spaced(&base.path, &context.spaced, &mut warnings);
            // Neighbours are judged in the output, where an adjacent change's text
            // may stand beside this one instead of the original bytes.
            let mut edges = Vec::new();
            for (id, range) in &placed {
                if let Some(change) = targets.get(id) {
                    let old = match &change.target {
                        Target::Exact { old, .. }
                        | Target::All { old, .. }
                        | Target::Every { old, .. } => old,
                        _ => continue,
                    };
                    let crlf = context.crlf() && !literal(change);
                    let (old, new) = (adapt_eol(crlf, old), adapt_eol(crlf, &change.text));
                    for edge in whitespace_edges(&output, range, &old, &new) {
                        let junction = match edge {
                            Edge::DropsTrailing | Edge::AddsTrailing => range.end,
                            Edge::DropsLeading | Edge::AddsLeading => range.start,
                        };
                        edges.push((*change, edge, junction));
                    }
                }
            }
            warn_edges(&base.path, &output, edges, &mut edge_warnings);
            warn_blank_runs(&base.path, &base.text, &output, &placed, &mut edge_warnings);
            warn_dropped_blank_runs(
                &base.path,
                &base.text,
                &output,
                &replacements,
                &placed,
                &mut edge_warnings,
            );
            warn_final_line_breaks(&base.path, &output, &placed, &targets, &mut edge_warnings);
            warnings.extend(context.whole_words);
            moves.extend(
                context
                    .moved
                    .into_iter()
                    .map(|moved| (moved.change.id.as_str(), moved.target)),
            );
            PreparedFile {
                base: base.clone(),
                output,
                replacements,
                change_ids: changes.iter().map(|change| change.id.clone()).collect(),
            }
        })
        .collect();
    warnings.append(&mut edge_warnings);
    // The plan records the lines each moved target applies to, so a stored
    // plan's line targets still derive its replacements and its `in` lines hold
    // its matches.
    let mut request = request.clone();
    for change in request.files.iter_mut().flat_map(|file| &mut file.changes) {
        if let Some(target) = moves.remove(change.id.as_str()) {
            change.target = target;
        }
    }
    Ok(PreparedPlan {
        id: new_id("p"),
        request,
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
/// `WHITESPACE_EDGE` warnings per file, each scanning its output line, and
/// likewise `BLANK_LINES_JOINED`, `BLANK_LINES_DROPPED` and
/// `FINAL_LINE_BREAK_KEPT` warnings.
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

/// Warns where whitespace on both sides of a change's edge holds more blank lines
/// in a row than `base` had anywhere, naming them as output lines: an `old` that
/// takes the blank lines above a block but stops before its last line ending
/// leaves one too many. The start or end of the file stands in for whitespace
/// beside a deletion, which can blank the line it cuts there too. Each run is
/// warned once, for the first change meeting it, and the bytes are written as
/// given. An edge beside text costs a byte on each side; only the whitespace
/// around an edge is scanned, and the base once, when an edge joins whitespace.
fn warn_blank_runs(
    path: &str,
    base: &str,
    output: &str,
    placed: &[(&str, Range<usize>)],
    warnings: &mut Vec<Diagnostic>,
) {
    let top = if output.starts_with('\u{feff}') { 3 } else { 0 };
    let longest = OnceCell::new();
    let mut measured: Option<usize> = None;
    let (mut warned, mut line, mut cursor) = (0, 1, 0);
    for (id, range) in placed {
        let deletion = range.is_empty();
        for junction in [range.start, range.end] {
            let (before, after) = output.split_at(junction);
            // An edge in the whitespace already measured meets the same run.
            if measured.is_some_and(|end| junction <= end)
                || !(before.ends_with(EDGE_WHITESPACE) || (deletion && junction == top))
                || !(after.starts_with(EDGE_WHITESPACE) || (deletion && after.is_empty()))
            {
                continue;
            }
            let start = before.trim_end_matches(EDGE_WHITESPACE).len();
            let end = output.len() - after.trim_start_matches(EDGE_WHITESPACE).len();
            measured = Some(end);
            let (first, count) = blank_lines(output, start..end);
            let most = *longest.get_or_init(|| longest_blank_run(base));
            if count <= most {
                continue;
            }
            if warned == MAX_EDGE_WARNINGS {
                return;
            }
            warned += 1;
            line += output.as_bytes()[cursor..first]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count();
            cursor = first;
            let run = if count == 1 {
                format!("a blank line, line {line}")
            } else {
                format!(
                    "{count} blank lines in a row, lines {line}-{}",
                    line + count - 1
                )
            };
            let had = if most == 0 {
                "none".to_owned()
            } else {
                format!("at most {most}")
            };
            let shown: String = id.chars().take(24).collect();
            let mut warning = at(
                Some(path),
                None,
                "BLANK_LINES_JOINED",
                format!(
                    "Change {shown}: the whitespace around its edge now holds {run}; the file had {had}"
                ),
            );
            warning.change_id = Some((*id).to_owned());
            warnings.push(warning);
        }
    }
}

/// Warns where a change that removes text leaves fewer blank lines in a row
/// around an edge than the file had around both of its ends, naming them as
/// output lines: an `old` that takes the blank lines above a function and one
/// below it leaves one too few. Blank lines at the start or end of the file
/// separate nothing, so whitespace reaching either counts none, and a change
/// that removes only whitespace, as an insertion does, is not judged. Each run
/// is judged once, for the first change with blank lines around both ends that
/// meets it, and the bytes are written as given. `placed` holds each
/// replacement's output range, in order. A change beside text costs a byte on
/// each side of each end; only the output's whitespace around the edges of a
/// change judged is scanned, once per run.
fn warn_dropped_blank_runs(
    path: &str,
    base: &str,
    output: &str,
    replacements: &[Replacement],
    placed: &[(&str, Range<usize>)],
    warnings: &mut Vec<Diagnostic>,
) {
    let around = |at| match whitespace_around(base, at) {
        (stretch, true) => blank_lines(base, stretch).1,
        _ => 0,
    };
    let mut measured: Option<usize> = None;
    let (mut warned, mut line, mut cursor) = (0, 1, 0);
    for (replacement, (id, range)) in replacements.iter().zip(placed) {
        if base[replacement.start..replacement.end]
            .trim_matches(EDGE_WHITESPACE)
            .is_empty()
        {
            continue;
        }
        let (above, below) = (around(replacement.start), around(replacement.end));
        let fewest = above.min(below);
        if fewest == 0 {
            continue;
        }
        for junction in [range.start, range.end] {
            // An edge in the whitespace already measured meets the same run.
            if measured.is_some_and(|end| junction <= end) {
                continue;
            }
            let (stretch, between) = whitespace_around(output, junction);
            measured = Some(stretch.end);
            if !between {
                continue;
            }
            let (first, count) = blank_lines(output, stretch.clone());
            if count >= fewest {
                continue;
            }
            if warned == MAX_EDGE_WARNINGS {
                return;
            }
            warned += 1;
            line += output.as_bytes()[cursor..first]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count();
            cursor = first;
            let run = match count {
                // Without a blank line, `first` starts the next text's line.
                0 if output[stretch.clone()].contains('\n') => {
                    format!("no blank line, between lines {} and {line}", line - 1)
                }
                0 => format!("no line break, line {line}"),
                1 => format!("a blank line, line {line}"),
                _ => format!(
                    "{count} blank lines in a row, lines {line}-{}",
                    line + count - 1
                ),
            };
            let shown: String = id.chars().take(QUOTED_ID_CHARS).collect();
            let mut warning = at(
                Some(path),
                None,
                "BLANK_LINES_DROPPED",
                format!(
                    "Change {shown}: the whitespace around its edge now holds {run}; the file had {above} in a row around its start and {below} around its end"
                ),
            );
            warning.change_id = Some((*id).to_owned());
            warnings.push(warning);
        }
    }
}

/// Characters a `FINAL_LINE_BREAK_KEPT` warning quotes of each line it names,
/// short enough that the message fits the 240 characters clients display.
const ENDING_CONTEXT_CHARS: usize = 32;

/// Warns where a `lines` or `after` change's text ends in a line break. A line
/// target adds a line ending only to text that lacks one, so this text is written
/// as given, and text that lists its lines joined by line breaks, as shorthand
/// examples do, comes out one blank line short: `"x\n\n"` leaves one blank line
/// after `x`, not two. The message names the text's last line, the blank lines
/// after it, and the line after those, at most eight per file. Text of blank
/// lines alone names no line and is not flagged. The bytes are still written
/// exactly as given.
fn warn_final_line_breaks(
    path: &str,
    output: &str,
    placed: &[(&str, Range<usize>)],
    targets: &HashMap<&str, &Change>,
    warnings: &mut Vec<Diagnostic>,
) {
    let top = if output.starts_with('\u{feff}') { 3 } else { 0 };
    let (mut warned, mut line, mut cursor) = (0, 1, 0);
    for (id, range) in placed {
        let Some(change) = targets.get(id) else {
            continue;
        };
        let end = range.start
            + output[range.clone()]
                .trim_end_matches(EDGE_WHITESPACE)
                .len();
        if !matches!(change.target, Target::Lines { .. } | Target::Insert { .. })
            || !change.text.ends_with('\n')
            || end == range.start
        {
            continue;
        }
        if warned == MAX_EDGE_WARNINGS {
            return;
        }
        warned += 1;
        line += output.as_bytes()[cursor..end]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count();
        cursor = end;
        let rest = &output[end..];
        let next = end + rest.len() - rest.trim_start_matches(EDGE_WHITESPACE).len();
        let (_, blank) = blank_lines(output, end..next);
        let start = output[..end].rfind('\n').map_or(top, |index| index + 1);
        let last = candidates::quoted(&output[start..end], ENDING_CONTEXT_CHARS);
        let then = if next < output.len() {
            let start = output[..next].rfind('\n').map_or(top, |index| index + 1);
            let stop = output[next..]
                .find('\n')
                .map_or(output.len(), |index| next + index);
            let text = &output[start..stop];
            format!(
                "line {} {}",
                line + output[end..next].matches('\n').count(),
                candidates::quoted(
                    text.strip_suffix('\r').unwrap_or(text),
                    ENDING_CONTEXT_CHARS
                )
            )
        } else {
            "the end of the file".to_owned()
        };
        let follows = match blank {
            0 if next < output.len() => format!("is followed directly by {then}"),
            0 => "ends the file".to_owned(),
            1 => format!("is followed by a blank line, then {then}"),
            count => format!("is followed by {count} blank lines, then {then}"),
        };
        let shown: String = change.id.chars().take(QUOTED_ID_CHARS).collect();
        warnings.push(at(
            Some(path),
            Some(change),
            "FINAL_LINE_BREAK_KEPT",
            format!(
                "Change {shown}: `new` ends in a line break, so no line ending is added after it: line {line} {last} {follows}"
            ),
        ));
    }
}

/// The blank lines, numbered as Read numbers lines, inside the whitespace
/// `stretch` of `text`: the byte where the first starts, and how many there are.
/// Each line ending in the stretch starts a line, and so does a stretch that
/// starts the file; the last line started is blank only if no text follows it.
fn blank_lines(text: &str, stretch: Range<usize>) -> (usize, usize) {
    let whitespace = &text[stretch.clone()];
    // An empty file has no line to be blank.
    let top =
        !whitespace.is_empty() && stretch.start == if text.starts_with('\u{feff}') { 3 } else { 0 };
    let first = match whitespace.find('\n') {
        Some(index) if !top => stretch.start + index + 1,
        _ => stretch.start,
    };
    let starts = usize::from(top) + whitespace.matches('\n').count();
    // The last line started holds text, or is the empty line Read shows after a
    // final line ending, unless the stretch runs to the end of the file without one.
    let open = whitespace.ends_with('\n') || stretch.end < text.len();
    (first, starts.saturating_sub(usize::from(open)))
}

/// The most blank lines in a row in `text`, as Read numbers lines; an empty file,
/// as in [`blank_lines`], has none.
fn longest_blank_run(text: &str) -> usize {
    text.strip_prefix('\u{feff}')
        .unwrap_or(text)
        .split_inclusive('\n')
        .fold((0, 0), |(run, longest), line| {
            let run = if line.trim_start_matches(EDGE_WHITESPACE).is_empty() {
                run + 1
            } else {
                0
            };
            (run, longest.max(run))
        })
        .1
}

/// The whitespace around byte `at` of `text`, and whether text stands on both
/// sides of it, as it must for its blank lines to separate anything.
fn whitespace_around(text: &str, at: usize) -> (Range<usize>, bool) {
    let (before, after) = text.split_at(at);
    let start = before.trim_end_matches(EDGE_WHITESPACE).len();
    let end = text.len() - after.trim_start_matches(EDGE_WHITESPACE).len();
    let top = if text.starts_with('\u{feff}') { 3 } else { 0 };
    (start..end, start > top && end < text.len())
}

/// Names the lines each moved target gave and the lines it applied to. A stale
/// read or a miscount put them there, so other numbers from it may be off.
fn warn_moved(path: &str, moved: &[Moved], warnings: &mut Vec<Diagnostic>) {
    let place = |target: &Target| match target {
        Target::Lines {
            lines: [first, last],
            ..
        } => format!("lines:[{first},{last}]"),
        Target::Insert { after, .. } => format!("after:{after}"),
        Target::Exact {
            lines: Some([first, last]),
            ..
        } => format!("in:[{first},{last}]"),
        Target::Exact { .. } | Target::All { .. } | Target::Every { .. } | Target::Span { .. } => {
            String::new()
        }
    };
    for moved in moved {
        let id: String = moved.change.id.chars().take(QUOTED_ID_CHARS).collect();
        let given = place(&moved.change.target);
        let message = match moved.target {
            // An `old` names the lines its match spans.
            Target::Exact {
                lines: Some([first, last]),
                ..
            } => {
                let used = if first == last {
                    format!("line {first}")
                } else {
                    format!("lines {first}-{last}")
                };
                format!(
                    "Change {id}: {given} did not hold `old`, which occurs only at {used}, so the change applied there; reread the file before more line edits"
                )
            }
            _ => format!(
                "Change {id}: {given} did not hold expect, which holds only at {}, so the change applied there; reread the file before more line edits",
                place(&moved.target)
            ),
        };
        warnings.push(at(Some(path), Some(moved.change), "LINES_MOVED", message));
    }
}

/// Names the changes an adaptation warning is about: the first `ADAPTED_IDS`
/// IDs, clipped, then how many more.
fn adapted_changes(ids: &[&str]) -> String {
    let mut named: Vec<String> = ids
        .iter()
        .take(ADAPTED_IDS)
        .map(|id| id.chars().take(24).collect())
        .collect();
    if ids.len() > ADAPTED_IDS {
        named.push(format!("{} more", ids.len() - ADAPTED_IDS));
    }
    let noun = if ids.len() == 1 { "Change" } else { "Changes" };
    format!("{noun} {}", named.join(", "))
}

/// Says which changes had LF written as CRLF to match an all-CRLF file.
fn warn_adapted(path: &str, adapted: &[&str], warnings: &mut Vec<Diagnostic>) {
    let Some(first) = adapted.first() else {
        return;
    };
    let mut warning = at(
        Some(path),
        None,
        "EOL_ADAPTED",
        format!(
            "{}: this file ends every line with CRLF, so LF in {} text was matched and written as CRLF; text holding a \\r stays literal",
            adapted_changes(adapted),
            if adapted.len() == 1 { "its" } else { "their" },
        ),
    );
    warning.change_id = Some((*first).to_owned());
    warnings.push(warning);
}

/// Says which changes matched the file's Unicode spaces for ASCII spaces in
/// `old`, given with the text each matched, and which spaces those were.
fn warn_spaced(path: &str, spaced: &[(&str, &str)], warnings: &mut Vec<Diagnostic>) {
    let Some((first, _)) = spaced.first() else {
        return;
    };
    let ids: Vec<&str> = spaced.iter().map(|(id, _)| *id).collect();
    let spaces: Vec<String> = ADAPTED_SPACES
        .iter()
        .filter(|space| spaced.iter().any(|(_, found)| found.contains(**space)))
        .map(|space| format!("U+{:04X}", u32::from(*space)))
        .collect();
    let spaces = match spaces.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => spaces.concat(),
    };
    let mut warning = at(
        Some(path),
        None,
        "SPACES_ADAPTED",
        format!(
            "{}: a space in `old` matched this file's {spaces}, which `new` keeps where it repeats `old`",
            adapted_changes(&ids),
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
        Target::Every { old, scope, lines } => (Some(old), scope.as_ref(), lines.as_ref()),
        Target::Span { span, .. } => (None, Some(span), None),
        Target::Lines { lines, .. } => (None, None, Some(lines)),
        Target::Insert { .. } => (None, None, None),
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
    spaces: OnceCell<bool>,
    disclosed: OnceCell<HashSet<usize>>,
    /// Changes whose text was adapted to CRLF, in request order.
    adapted: Vec<&'a str>,
    /// Changes whose `old` matched Unicode spaces for ASCII ones, with the text
    /// each matched, in request order.
    spaced: Vec<(&'a str, &'a str)>,
    /// Warnings for targets that took only whole-word matches, in request order.
    whole_words: Vec<Diagnostic>,
    /// The lines each line target addresses, which overlap advice names.
    extents: HashMap<&'a str, Extent>,
    /// Insertions whose text starts by restating lines that they keep, refused
    /// once the file's other changes show those lines stay.
    restated: Vec<Restated<'a>>,
    /// Where each line compared for a restatement ends without trailing
    /// whitespace, and its visible characters, so the run search reads a line
    /// once however many insertions compare it.
    trimmed: RefCell<HashMap<usize, (usize, usize)>>,
    /// Targets applied where their guard or `old` holds instead of at their
    /// stated lines, in request order.
    moved: Vec<Moved<'a>>,
}

/// A line target whose stated lines do not hold its guard, or an `old` whose
/// `in` lines hold no match, applied at the one other place where it holds.
struct Moved<'a> {
    change: &'a Change,
    /// The target with the lines it applies to, which the plan records.
    target: Target,
    /// What the stated lines get, given instead when the moved target would
    /// conflict with another change.
    refusal: Diagnostic,
}

impl<'a> FileContext<'a> {
    fn new(base: &'a Snapshot, eol: Eol) -> Self {
        Self {
            base,
            eol,
            lines: OnceCell::new(),
            crlf: OnceCell::new(),
            spaces: OnceCell::new(),
            disclosed: OnceCell::new(),
            adapted: Vec::new(),
            spaced: Vec::new(),
            whole_words: Vec::new(),
            extents: HashMap::new(),
            restated: Vec::new(),
            trimmed: RefCell::new(HashMap::new()),
            moved: Vec::new(),
        }
    }

    /// The target a change applies: its own, or the one its guard moved it to.
    fn target(&self, change: &'a Change) -> &Target {
        self.moved
            .iter()
            .find(|moved| moved.change.id == change.id)
            .map_or(&change.target, |moved| &moved.target)
    }

    /// Line `number`'s body without trailing whitespace, and its visible
    /// characters.
    fn trimmed(&self, number: usize) -> (&'a str, usize) {
        let base: &'a Snapshot = self.base;
        let body = self.lines().body(number);
        let (end, shown) = *self.trimmed.borrow_mut().entry(number).or_insert_with(|| {
            let trimmed = base.text[body.clone()].trim_end();
            (body.start + trimmed.len(), visible(trimmed))
        });
        (&base.text[body.start..end], shown)
    }

    /// Whether LF-only text is adapted for this base.
    fn crlf(&self) -> bool {
        self.eol == Eol::Adapt && *self.crlf.get_or_init(|| crlf_only(&self.base.text))
    }

    /// Whether an ASCII space in `old` may match a Unicode space in this base.
    fn spaces(&self) -> bool {
        self.eol == Eol::Adapt
            && *self
                .spaces
                .get_or_init(|| self.base.text.contains(ADAPTED_SPACES))
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
        let advice = match change.target {
            Target::Exact { .. } | Target::All { .. } | Target::Every { .. } => {
                format!("end `in` by line {total} or drop it to search the whole file")
            }
            _ if lines.body(1).start == self.base.text.len() => {
                "the file is empty; insert with after:0".to_owned()
            }
            _ => format!("use after:{total} to append"),
        };
        let message = if lines.terminator(&self.base.text, total).is_empty() {
            format!(
                "Line {line} is past the end: the file has {total} lines and no final line ending; {advice}"
            )
        } else {
            format!(
                "Line {line} is past the end: the file has {total} lines; native Read also shows an empty line {} after the final line ending, which holds no text; {advice}",
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
/// Trims a partial overlap between sorted neighbouring replacements when both
/// keep the shared bytes: the left text ends with them and the right text
/// starts with them, as when two `old` anchors share a few bytes of context.
/// The shared bytes then stay once, between the two changes; any other
/// overlap is left for the conflict check.
fn merge_kept_overlaps(text: &str, replacements: &mut [Replacement]) {
    for index in 1..replacements.len() {
        let (before, after) = replacements.split_at_mut(index);
        let (left, right) = (&mut before[index - 1], &after[0]);
        if !(left.start < right.start && right.start < left.end && left.end < right.end) {
            continue;
        }
        let Some(shared) = text.get(right.start..left.end) else {
            continue;
        };
        if left.text.ends_with(shared) && right.text.starts_with(shared) {
            left.text.truncate(left.text.len() - shared.len());
            left.end = right.start;
        }
    }
}

/// Whether two replacements, `left` sorted first, conflict. Boundary insertions
/// also conflict, so there is one explicit ordering policy, except beside a
/// deletion, where either order gives the same text.
fn conflicts(left: &Replacement, right: &Replacement) -> bool {
    let insertion = |replacement: &Replacement| replacement.start == replacement.end;
    let deletion = |replacement: &Replacement| {
        replacement.start < replacement.end && replacement.text.is_empty()
    };
    if (insertion(left) && deletion(right)) || (deletion(left) && insertion(right)) {
        right.start < left.end && left.start < right.start
    } else if insertion(left) || insertion(right) {
        right.start <= left.end
    } else {
        right.start < left.end
    }
}

/// Refuses each moved target whose replacement conflicts with another
/// change's, as its stated lines are refused: the other changes were placed
/// without knowing of the move. Each moved target is compared with every
/// replacement, at most `MAX_CHANGES` times `MAX_REPLACEMENTS` comparisons.
fn keep_moves_apart(
    context: &mut FileContext,
    replacements: &mut Vec<Replacement>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let clashes = |id: &str| {
        replacements
            .iter()
            .filter(|own| own.change_id == id)
            .any(|own| {
                replacements.iter().any(|other| {
                    let (left, right) = if (own.start, own.end) <= (other.start, other.end) {
                        (own, other)
                    } else {
                        (other, own)
                    };
                    other.change_id != id && conflicts(left, right)
                })
            })
    };
    let (refused, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut context.moved)
        .into_iter()
        .partition(|moved| clashes(&moved.change.id));
    context.moved = kept;
    for moved in refused {
        replacements.retain(|replacement| replacement.change_id != moved.change.id);
        context
            .restated
            .retain(|restated| restated.change.id != moved.change.id);
        diagnostics.push(moved.refusal);
    }
}

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
    let crlf = context.crlf() && !literal(change);
    let text = adapt_eol(crlf, &change.text);
    let adapted = matches!(text, Cow::Owned(_))
        || match &change.target {
            Target::Exact { old, .. } | Target::All { old, .. } | Target::Every { old, .. } => {
                crlf && adapts(old)
            }
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
        Target::Lines {
            lines,
            expect,
            expect_last,
        } => match context.line_bounds(change, *lines, diagnostics) {
            Some((first, last)) => (Guard::Prefix { first, last }, (expect, expect_last)),
            None => return,
        },
        Target::Insert { after, expect } => {
            let lines = context.lines();
            let line = lines.clamp(&base.text, *after);
            if line > lines.count() {
                diagnostics.push(context.out_of_range(change, *after));
                return;
            }
            // Empty text is one empty line: the line ending it writes. An empty file
            // receives text as given, and a last line without an ending would only
            // gain one, so there the text must say what to write.
            if change.text.is_empty() {
                let reason = if lines.body(1).start == base.text.len() {
                    Some("The file is empty, so empty text would insert nothing; give the text to insert".to_owned())
                } else if line > 0 && lines.terminator(&base.text, line).is_empty() {
                    Some(format!(
                        "Line {line} ends the file without a line ending, so empty text after it would add only that ending; give the text to insert, or to add the ending, replace line {line} with its text and a line feed"
                    ))
                } else {
                    None
                };
                if let Some(reason) = reason {
                    diagnostics.push(at(
                        Some(&base.path),
                        Some(change),
                        "EMPTY_INSERTION",
                        reason,
                    ));
                    return;
                }
            }
            (Guard::Above { line }, (expect, &None))
        }
        Target::Exact { .. } | Target::All { .. } | Target::Every { .. } => {
            resolve_exact(context, change, &text, replacements, diagnostics, budget);
            return;
        }
    };
    let (expect, expect_last) = expect;
    let expect = Expect {
        head: expect.as_deref(),
        tail: expect_last.as_deref(),
    };
    let (guard, moved) = match line_guard(context, change, guard, expect, diagnostics, budget) {
        Placed::Stated => (guard, None),
        Placed::Refused => return,
        Placed::Moved { guard, refusal } => {
            let mut target = change.target.clone();
            match (&mut target, guard) {
                (Target::Lines { lines, .. }, Guard::Prefix { first, last }) => {
                    *lines = [first, last];
                }
                (Target::Insert { after, .. }, Guard::Above { line }) => *after = line,
                _ => {}
            }
            let moved = Moved {
                change,
                target,
                refusal,
            };
            (guard, Some(moved))
        }
    };
    if let Guard::Above { line } = guard
        && let Some(restated) = restated_lines(change, context, line, &text)
    {
        context.restated.push(restated);
    }
    let target = moved.as_ref().map_or(&change.target, |moved| &moved.target);
    let found = derived_replacement(&base.text, context.lines(), target, &text);
    let Some((range, written)) = found else {
        return;
    };
    context.moved.extend(moved);
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

/// An insertion after `line` whose text starts by restating lines
/// `first..=line`, which `after` keeps. Its next lines also equal lines
/// `line + 1..=last`, which may be copied context or new lines that look the
/// same, such as a sibling method's decorator; `bare` when the restated lines
/// are all of its text.
struct Restated<'a> {
    change: &'a Change,
    line: usize,
    first: usize,
    last: usize,
    bare: bool,
}

/// Lines above the anchor a restatement is compared over: a longer one is not
/// the slip this check catches, and the bound keeps the search small.
const RESTATED_LINES: usize = 200;

fn visible(text: &str) -> usize {
    text.chars().filter(|ch| !ch.is_whitespace()).count()
}

/// Finds an insertion whose leading lines restate the lines ending at `line`,
/// compared without trailing whitespace, as when `expect` or the lines to
/// replace were copied into the text. Restated lines with fewer than
/// `MIN_GUARD_CHARS` visible characters, such as a lone `}`, may repeat on
/// purpose. Equal lines above can make several runs match; the longest is
/// named, since dropping a shorter one leaves text that restates the anchor
/// again.
fn restated_lines<'a>(
    change: &'a Change,
    context: &FileContext,
    line: usize,
    text: &str,
) -> Option<Restated<'a>> {
    let lines = context.lines();
    if line == 0 || line > lines.count() {
        return None;
    }
    let body = |number: usize| context.trimmed(number).0;
    let text = text.strip_suffix('\n').unwrap_or(text);
    fn trimmed(piece: &str) -> &str {
        piece.strip_suffix('\r').unwrap_or(piece).trim_end()
    }
    let pieces: Vec<&str> = text.split('\n').take(RESTATED_LINES).map(trimmed).collect();
    let lowest = (line + 1).saturating_sub(pieces.len()).max(1);
    // The lines a run may take, and visible characters summed from the anchor
    // up, so a run too faint to count is passed over without comparing it.
    let window: Vec<(&str, usize)> = (lowest..=line)
        .map(|number| context.trimmed(number))
        .collect();
    let mut shown = vec![0; window.len() + 1];
    for (index, (_, count)) in window.iter().enumerate().rev() {
        shown[index] = shown[index + 1] + count;
    }
    let restates = |first: usize| {
        let from = first - lowest;
        shown[from] >= MIN_GUARD_CHARS
            && window[from..]
                .iter()
                .zip(&pieces)
                .all(|((line, _), piece)| line == piece)
    };
    let first = (lowest..=line).find(|&first| restates(first))?;
    let kept = line + 1 - first;
    let more = text
        .split('\n')
        .skip(kept)
        .zip(line + 1..=lines.count())
        .take_while(|(piece, number)| body(*number) == trimmed(piece))
        .count();
    Some(Restated {
        change,
        line,
        first,
        last: line + more,
        bare: kept + more == text.split('\n').count(),
    })
}

/// Refuses each restating insertion unless other changes in the file delete
/// every restated or copied line that has text, whole: changes apply to the
/// original file, so those lines then appear once. Other changes that edit
/// those lines in place, each line keeping text, apply to the original lines
/// that stay, so the restated copy is dropped from `new`. Any other touch
/// makes which lines end up where the caller's to say, so it is asked for one
/// `lines` change in place of all the changes on those lines.
fn refuse_restatements(
    context: &FileContext,
    replacements: &[Replacement],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let lines = context.lines();
    for restated in &context.restated {
        let Restated {
            change,
            line,
            first,
            last,
            bare,
        } = *restated;
        let (low, high) = (lines.body(first).start, lines.body(last).end);
        let mut touching: Vec<_> = replacements
            .iter()
            .filter(|other| {
                other.change_id != change.id
                    && ((other.start < high && other.end > low)
                        || (other.start == other.end && low < other.start && other.start < high))
            })
            .collect();
        touching.sort_by_key(|other| other.start);
        let advice = if touching.is_empty() {
            Advice::Drop
        } else if deleted_whole(context, &touching, first, last) {
            continue;
        } else if edited_in_place(context, &touching) {
            Advice::Edited
        } else {
            let (low, high, partial) =
                merged_lines(context, replacements, change, [first, last], &touching);
            Advice::Merge(low, high, partial)
        };
        let Target::Insert { after, .. } = *context.target(change) else {
            continue;
        };
        let mut diagnostic = at(
            Some(&context.base.path),
            Some(change),
            "INSERT_REPEATS_LINE",
            restatement_message(after, [first, line, last], bare, advice),
        );
        diagnostic.conflicts = touching
            .iter()
            .map(|other| other.change_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        diagnostics.push(diagnostic);
    }
}

/// Whether `touching`, sorted by start, deletes every line of `first..=last`
/// that has text, whole.
fn deleted_whole(
    context: &FileContext,
    touching: &[&Replacement],
    first: usize,
    last: usize,
) -> bool {
    let lines = context.lines();
    let mut deleted: Vec<Range<usize>> = Vec::new();
    for other in touching {
        if other.start == other.end || visible(&other.text) != 0 {
            continue;
        }
        match deleted.last_mut() {
            Some(range) if range.end >= other.start => range.end = range.end.max(other.end),
            _ => deleted.push(other.start..other.end),
        }
    }
    let mut ranges = deleted.iter().peekable();
    (first..=last).all(|number| {
        let body = lines.body(number);
        if context.trimmed(number).1 == 0 {
            return true;
        }
        while ranges.next_if(|range| range.end < body.end).is_some() {}
        ranges
            .peek()
            .is_some_and(|range| range.start <= body.start && range.end >= body.end)
    })
}

/// Whether every change in `touching`, sorted by start, edits within one
/// line's body and leaves that line with text, so the line stays where it is.
fn edited_in_place(context: &FileContext, touching: &[&Replacement]) -> bool {
    let lines = context.lines();
    let text = &context.base.text;
    let mut index = 0;
    while index < touching.len() {
        let number = lines.line_of(touching[index].start);
        let body = lines.body(number);
        let (mut written, mut cursor) = (String::new(), body.start);
        while let Some(other) = touching.get(index) {
            let inside = if other.start == other.end {
                body.start < other.start && other.start < body.end
            } else {
                body.start <= other.start && other.end <= body.end
            };
            if !inside {
                if lines.line_of(other.start) == number {
                    return false;
                }
                break;
            }
            // Overlapping changes are refused on their own.
            if other.start < cursor {
                return false;
            }
            written.push_str(&text[cursor..other.start]);
            written.push_str(&other.text);
            cursor = other.end;
            index += 1;
        }
        written.push_str(&text[cursor..body.end]);
        if written.contains('\n') || visible(&written) == 0 {
            return false;
        }
    }
    true
}

/// The lines one `lines` change must cover to stand in for the insertion, the
/// lines `first..=last` it restates, and every change touching or bordering
/// them, widened until none is left partly outside, and whether a change also
/// edits beyond them.
fn merged_lines(
    context: &FileContext,
    replacements: &[Replacement],
    change: &Change,
    [first, last]: [usize; 2],
    touching: &[&Replacement],
) -> (usize, usize, bool) {
    let lines = context.lines();
    let line_at = |other: &Replacement| {
        (
            lines.line_of(other.start),
            lines.line_of(other.end.saturating_sub(1).max(other.start)),
        )
    };
    let (mut low, mut high) = touching.iter().fold((first, last), |(low, high), other| {
        let (first, last) = line_at(other);
        (low.min(first), high.max(last))
    });
    let mut ids: BTreeSet<&str> = touching
        .iter()
        .map(|other| other.change_id.as_str())
        .collect();
    loop {
        let (start, end) = (lines.body(low).start, lines.end(high));
        let mut grew = false;
        for other in replacements {
            if other.change_id == change.id {
                continue;
            }
            let meets = (other.start < end && other.end > start)
                || (other.start == other.end && start <= other.start && other.start <= end);
            if !meets {
                continue;
            }
            ids.insert(&other.change_id);
            let (first, last) = line_at(other);
            if first < low || last > high {
                (low, high) = (low.min(first), high.max(last));
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    let (start, end) = (lines.body(low).start, lines.end(high));
    let partial = replacements.iter().any(|other| {
        ids.contains(other.change_id.as_str()) && !(start <= other.start && other.end <= end)
    });
    (low, high, partial)
}

/// What a refused restatement tells the caller to do.
#[derive(Clone, Copy)]
enum Advice {
    /// No other change touches the lines: drop them, or replace them.
    Drop,
    /// Other changes edit the lines in place, which stay.
    Edited,
    /// One `lines` change over these lines stands in for all of them; true when
    /// a change also edits elsewhere and must be narrowed.
    Merge(usize, usize, bool),
}

/// The refusal for an insertion `after` whose text restates lines
/// `first..=line`, then lines down to `last`; within `MESSAGE_CHARS` at
/// 8-digit line numbers.
fn restatement_message(
    after: usize,
    [first, line, last]: [usize; 3],
    bare: bool,
    advice: Advice,
) -> String {
    let named = |low: usize, high: usize| {
        if low == high {
            format!("line {low}")
        } else {
            format!("lines {low}-{high}")
        }
    };
    let kept = named(first, line);
    let all = named(first, last);
    let (they, them, their) = if first == line {
        ("it", "it", "its")
    } else {
        ("they", "them", "their")
    };
    match advice {
        Advice::Merge(low, high, partial) => {
            let narrow = if partial {
                "; narrow any change with matches elsewhere to leave them"
            } else {
                ""
            };
            format!(
                "after:{after} repeats {all}, which other changes also touch; send them as one change, lines:[{low},{high}], with those lines as they should end{narrow}"
            )
        }
        Advice::Edited if bare => format!(
            "after:{after} keeps {all}, which another change edits in place, and new only repeats the old text; drop this insertion"
        ),
        Advice::Edited if last > line => format!(
            "after:{after} keeps {all}, which other changes edit in place, and new repeats their old text first; drop {all} from new and insert after:{last}, with line {last} as expect"
        ),
        Advice::Edited => format!(
            "after:{after} keeps {kept}, which another change edits in place, and new repeats {their} old text, so both versions would appear; drop {them} from new"
        ),
        Advice::Drop if bare => format!(
            "after:{after} keeps {kept} and new only repeats {all}; to repeat text on purpose, send lines:[{first},{last}] with those lines given twice"
        ),
        Advice::Drop if last > line => format!(
            "after:{after} keeps {kept} and new repeats {them} first, then {} too; if copied, send this new as lines:[{first},{last}], and if new, drop {kept} from new",
            named(line + 1, last)
        ),
        Advice::Drop => format!(
            "after:{after} keeps {kept} and new repeats {them} first, so {they} would appear twice; drop {them} from new, or send this new as lines:[{first},{line}]"
        ),
    }
}

/// Resolves an `exact`, `all`, or `every` target by searching its scope for `old`,
/// adapted like `text` to the base's line endings, and for an `exact` target found
/// nowhere as written, to the base's Unicode spaces. An `exact` target whose `in`
/// lines hold no match applies where [`moved_old`] places it, if anywhere.
fn resolve_exact<'a>(
    context: &mut FileContext<'a>,
    change: &'a Change,
    text: &str,
    replacements: &mut Vec<Replacement>,
    diagnostics: &mut Vec<Diagnostic>,
    budget: &mut Budget,
) {
    let base = context.base;
    // How many matches the target needs; `every` takes however many cut no word.
    let (old, scope, lines, expected) = match &change.target {
        Target::Exact { old, scope, lines } => (old, scope.as_deref(), lines, Some(1)),
        Target::All {
            old,
            scope,
            expected,
            lines,
        } => (old, scope.as_deref(), lines, Some(*expected)),
        Target::Every { old, scope, lines } => (old, scope.as_deref(), lines, None),
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
    if old.is_empty() || expected == Some(0) || (scope.is_some() && lines.is_some()) {
        return;
    }
    let Some((start, end)) = range else {
        return;
    };
    let old = old.as_ref();
    let Some(expected) = expected else {
        let found = resolve_every(context, change, old, start..end, text, diagnostics, budget);
        replacements.extend(found);
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
    // Every match counts, but a caller renaming `MAX_RETRIES` means its own uses, not
    // those inside `MAX_RETRIES_PER_HOST`: an `old` with one match that cuts no word,
    // or a count that numbers just those, takes them, and a warning names what stayed.
    // Whole-word matches never outnumber all matches, so only fewer are checked.
    let whole = if expected < actual {
        whole_words(&base.text, start..end, old, !exact)
    } else {
        None
    };
    let (actual, positions, stayed) = match &whole {
        Some(whole) if whole.len() == expected => (
            expected,
            whole.iter().copied().take(retained).collect(),
            actual - expected,
        ),
        _ => (actual, positions, 0),
    };
    // Text typed from a rendered view has an ASCII space where the file may hold
    // a no-break or thin one: an `old` found nowhere as written takes its one
    // match with those spaces, when `new` repeats the text around each of them.
    let spaced = if exact && actual == 0 && adapts_spaces(old) && context.spaces() {
        respaced(&base.text, start..end, old, text)
    } else {
        None
    };
    let (actual, positions, len, text) = match &spaced {
        Some((position, found, written)) => (1, vec![*position], found.len(), written.as_str()),
        None => (actual, positions, old.len(), text),
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
            let advice = match advice(context.lines(), &base.text, start..end, old) {
                Ambiguity::Alone(first, last) => format!(
                    "add surrounding text to `old`, or restrict it with \"in\":[{first},{last}]"
                ),
                Ambiguity::Context => "add surrounding text to `old`".to_owned(),
                Ambiguity::InsideWords => {
                    "each match is inside a longer word; copy whole words from those lines into `old`"
                        .to_owned()
                }
            };
            let lines: Vec<_> = lines.iter().map(usize::to_string).collect();
            let more = if actual > starts.len() {
                " and later"
            } else {
                ""
            };
            format!(
                "Expected {expected} occurrence(s), found {actual} overlapping starts ({non_overlapping} non-overlapping) at {noun} {}{more}; {advice}",
                lines.join(", ")
            )
        } else if !found.is_empty() {
            candidates::not_found(expected, old, &found)
        } else if actual != 0 {
            // A count that is off says where the matches are, as ambiguity does.
            let starts: Vec<_> = base.text[start..end]
                .match_indices(old)
                .take(AMBIGUOUS_LINES)
                .map(|(at, _)| start + at)
                .collect();
            let mut lines = line_numbers(&base.text, &starts);
            lines.dedup();
            let noun = if lines.len() == 1 { "line" } else { "lines" };
            let lines: Vec<_> = lines.iter().map(usize::to_string).collect();
            let more = if actual > starts.len() {
                " and later"
            } else {
                ""
            };
            let words = whole.as_ref().map_or(0, Vec::len);
            let words = if 0 < words && words < actual {
                format!(" or to {words} for those not inside longer words")
            } else {
                String::new()
            };
            format!(
                "Expected {expected} occurrence(s), found {actual} at {noun} {}{more}; set count to {actual} to replace every one{words}, or restrict `old` with \"in\":[first,last]",
                lines.join(", ")
            )
        } else {
            format!(
                "Expected {expected} occurrence(s), found 0 and nothing similar; read the file again and copy `old` exactly from it"
            )
        };
        let mut diagnostic = at(Some(&base.path), Some(change), code, message);
        diagnostic.expected = Some(expected);
        diagnostic.actual = Some(actual);
        diagnostic.candidates = found;
        if exact
            && actual == 0
            && lines.is_some()
            && let Some((position, target)) = moved_old(context, change, old)
        {
            if !budget.reserve(1, text) {
                diagnostics.push(resource_limit(base, change));
                return;
            }
            replacements.push(Replacement {
                start: position,
                end: position + old.len(),
                text: text.to_owned(),
                change_id: change.id.clone(),
            });
            context.moved.push(Moved {
                change,
                target,
                refusal: diagnostic,
            });
            return;
        }
        diagnostics.push(diagnostic);
        return;
    }
    if exact
        && let Some(&position) = positions.first()
        && let Some(cut) = word_cut(&base.text, start + position, len, Edges::of(old))
    {
        diagnostics.push(at(
            Some(&base.path),
            Some(change),
            "OLD_INSIDE_WORD",
            inside_word(&base.text, cut),
        ));
        return;
    }
    if !budget.reserve(actual, text) {
        diagnostics.push(resource_limit(base, change));
        return;
    }
    if stayed > 0 {
        let warning = stayed_inside_words(base, change, start..end, old, stayed);
        context.whole_words.push(warning);
    }
    if let Some((_, found, _)) = &spaced {
        context.spaced.push((&change.id, found));
    }
    replacements.extend(positions.into_iter().map(|position| Replacement {
        start: start + position,
        end: start + position + len,
        text: text.to_owned(),
        change_id: change.id.clone(),
    }));
}

/// Resolves an `every` target: each non-overlapping match of `old` in `range`
/// that cuts no word, left to right, at least one. Matches inside longer names
/// stay, and a warning names the first of them. Returns no replacements when
/// it pushes a diagnostic.
fn resolve_every<'a>(
    context: &mut FileContext<'a>,
    change: &'a Change,
    old: &str,
    range: Range<usize>,
    text: &str,
    diagnostics: &mut Vec<Diagnostic>,
    budget: &mut Budget,
) -> Vec<Replacement> {
    let base = context.base;
    // Past `ADVISED_MATCHES` starts the plan could not hold the replacements either.
    let Some(whole) = whole_words(&base.text, range.clone(), old, true) else {
        diagnostics.push(resource_limit(base, change));
        return Vec::new();
    };
    let actual = base.text[range.clone()].match_indices(old).count();
    if whole.is_empty() {
        let found = if actual == 0 {
            budget.candidates(|search| {
                candidates::find_target(search, &base.text, range.clone(), old)
            })
        } else {
            Vec::new()
        };
        let message = match first_cut(&base.text, range.clone(), old) {
            Some((word, line)) => format!(
                "No whole-word match: all {actual} are inside longer words, the first in `{word}` at line {line}; set count to {actual} to replace inside words"
            ),
            None if !found.is_empty() => candidates::not_found("at least 1", old, &found),
            None => "Expected at least 1 occurrence, found 0 and nothing similar; read the file again and copy `old` exactly from it".to_owned(),
        };
        let mut diagnostic = at(Some(&base.path), Some(change), "TARGET_NOT_FOUND", message);
        diagnostic.actual = Some(0);
        diagnostic.candidates = found;
        diagnostics.push(diagnostic);
        return Vec::new();
    }
    if !budget.reserve(whole.len(), text) {
        diagnostics.push(resource_limit(base, change));
        return Vec::new();
    }
    let stayed = actual.saturating_sub(whole.len());
    if stayed > 0 {
        let warning = stayed_inside_words(base, change, range.clone(), old, stayed);
        context.whole_words.push(warning);
    }
    whole
        .into_iter()
        .map(|position| Replacement {
            start: range.start + position,
            end: range.start + position + old.len(),
            text: text.to_owned(),
            change_id: change.id.clone(),
        })
        .collect()
}

/// Where an `exact` `old` applies when its `in` lines hold no match, as lines
/// from a stale read or a miscount may not, with the target naming the lines it
/// applies to: the match an unscoped `old` would take, the only one in the file
/// or the only whole-word one, cutting no word. Like a moved line target's guard,
/// `old` needs `MIN_GUARD_CHARS` visible characters, so short text that occurs
/// once by chance is not taken for the text the lines meant.
fn moved_old(context: &FileContext, change: &Change, old: &str) -> Option<(usize, Target)> {
    let text = &context.base.text;
    if visible(old) < MIN_GUARD_CHARS {
        return None;
    }
    let scan = scan_occurrences(text, old, 0, 1);
    let position = match scan.overlapping {
        0 => return None,
        1 => scan.positions[0],
        _ => match whole_words(text, 0..text.len(), old, false)?[..] {
            [position] => position,
            _ => return None,
        },
    };
    if word_cut(text, position, old.len(), Edges::of(old)).is_some() {
        return None;
    }
    let lines = context.lines();
    let first = lines.line_of(position);
    // A match that starts in a leading BOM is in no line's scope.
    if position < lines.body(first).start {
        return None;
    }
    let last = lines.line_of(position + old.len() - 1);
    let mut target = change.target.clone();
    if let Target::Exact { lines, .. } = &mut target {
        *lines = Some([first, last]);
    }
    Some((position, target))
}

/// Whether an ASCII space in `old` may stand for one of `ADAPTED_SPACES`: it has
/// one, and holds none of those, which text copied from the file would.
fn adapts_spaces(old: &str) -> bool {
    old.contains(' ') && !old.contains(ADAPTED_SPACES)
}

/// The one place in `range` where `old` matches with each ASCII space standing
/// for one of `ADAPTED_SPACES`: its offset into the range, the text there, and
/// `new` written as [`kept_spaces`] gives it. `None` when no place or several
/// match, or a space to adapt lies in the text `new` changes.
fn respaced<'t>(
    text: &'t str,
    range: Range<usize>,
    old: &str,
    new: &str,
) -> Option<(usize, &'t str, String)> {
    let scope = &text[range.clone()];
    // One character for each of the scope's, so offsets map back by counting.
    let read: String = scope
        .chars()
        .map(|ch| {
            if ADAPTED_SPACES.contains(&ch) {
                ' '
            } else {
                ch
            }
        })
        .collect();
    let matches = scan_occurrences(&read, old, 0, 1);
    if matches.overlapping != 1 {
        return None;
    }
    let skipped = read[..matches.positions[0]].chars().count();
    let position = scope
        .char_indices()
        .nth(skipped)
        .map_or(scope.len(), |(at, _)| at);
    let found = spaced_at(text, range.start + position, old)?;
    Some((position, found, kept_spaces(old, found, new)?))
}

/// The text from `start` that `old` matches one character for one, each of its
/// ASCII spaces standing for itself or one of `ADAPTED_SPACES`.
fn spaced_at<'t>(text: &'t str, start: usize, old: &str) -> Option<&'t str> {
    let rest = text.get(start..)?;
    let mut end = 0;
    for wanted in old.chars() {
        let found = rest[end..].chars().next()?;
        if found != wanted && !(wanted == ' ' && ADAPTED_SPACES.contains(&found)) {
            return None;
        }
        end += found.len_utf8();
    }
    Some(&rest[..end])
}

/// `new` with the text it repeats from `old`, their common prefix and suffix,
/// spelled as `found`, the file's text that `old` matched; `None` when `found`
/// differs from `old` between them, where `new` changes the text and the
/// spaces meant there are unknown. Text that an insertion or deletion could sit
/// on either side of, as ` b` added to `a b` could follow either `a` or `b`,
/// counts as changed.
fn kept_spaces(old: &str, found: &str, new: &str) -> Option<String> {
    /// `text` cut after its first `head` characters and before its last `tail`.
    fn cut(text: &str, head: usize, tail: usize) -> [&str; 3] {
        let offset = |chars: usize| {
            text.char_indices()
                .nth(chars)
                .map_or(text.len(), |(at, _)| at)
        };
        let (start, end) = (offset(head), offset(text.chars().count() - tail));
        [&text[..start], &text[start..end], &text[end..]]
    }
    let shorter = old.chars().count().min(new.chars().count());
    let prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(was, now)| was == now)
        .count();
    let suffix = old
        .chars()
        .rev()
        .zip(new.chars().rev())
        .take_while(|(was, now)| was == now)
        .count();
    // Where the prefix and suffix overlap, only text kept at every split is kept.
    let (head, tail) = (prefix.min(shorter - suffix), suffix.min(shorter - prefix));
    let [_, changed, _] = cut(old, head, tail);
    let [before, matched, after] = cut(found, head, tail);
    let [_, written, _] = cut(new, head, tail);
    (matched == changed).then(|| [before, written, after].concat())
}

/// Whether `old` holds text on at most one line, so the end of its match is
/// judged as well as the start.
fn one_line(old: &str) -> bool {
    old.split('\n')
        .filter(|line| !line.trim().is_empty())
        .nth(1)
        .is_none()
}

/// How the ends of a match of `old` meet the words around it.
#[derive(Clone, Copy)]
struct Edges {
    /// Whether the end is judged as well as the start, for an `old` with text
    /// on one line ([`one_line`]).
    end: bool,
    /// Whether a hyphen continues the word at the start, and at the end, of a
    /// match: where `old` begins or ends with a name that joins words with
    /// one, as the CSS class `btn-primary` or the YAML key `max-retries` does.
    /// That name is one word, so neither `btn-primary-outline` nor
    /// `--btn-primary-bg` holds `btn-primary` as a whole word, while other
    /// words `old` begins or ends with, given as context, keep hyphens as
    /// separators.
    hyphens: [bool; 2],
}

impl Edges {
    fn of(old: &str) -> Self {
        let named = |name: &[u8]| {
            name.windows(3)
                .any(|bytes| bytes[1] == b'-' && ascii_word(bytes[0]) && ascii_word(bytes[2]))
        };
        let bytes = old.as_bytes();
        let name = |byte: &&u8| ascii_word(**byte) || **byte == b'-';
        let first = bytes.iter().take_while(name).count();
        let last = bytes.iter().rev().take_while(name).count();
        Self {
            end: one_line(old),
            hyphens: [named(&bytes[..first]), named(&bytes[bytes.len() - last..])],
        }
    }

    /// Whether `byte` is part of a word around the start or end of a match.
    fn word(self, byte: u8, at_end: bool) -> bool {
        ascii_word(byte) || (self.hyphens[usize::from(at_end)] && byte == b'-')
    }
}

fn ascii_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Where a match at `start` cuts an ASCII word, such as `retries = 2` inside
/// `max_retries = 20`: text written without reading the file can match a
/// longer name or number by accident. Only the start of an `old` with text on
/// several lines counts, since its last line may end on part of a name as
/// context; blank lines around one line of text do not make it span lines. A
/// word holding
/// cased non-ASCII letters within `WORD_CHARS` of the cut, such as
/// `Hauptstraße`, is prose that callers cut to avoid typing them, while uncased
/// scripts such as Han separate words; a cut between two digits is a number
/// cut whatever follows, as in `100µs`. A letter after a backslash, as in
/// `\nRestart`, is an escape, but `\_` or `\1` is not. Returns the cut and
/// whether a hyphen continues the word there.
fn word_cut(text: &str, start: usize, len: usize, edges: Edges) -> Option<(usize, bool)> {
    let bytes = text.as_bytes();
    let escape =
        |at: usize| at >= 2 && bytes[at - 2] == b'\\' && bytes[at - 1].is_ascii_alphabetic();
    let joined = |at: usize, at_end: bool| {
        let word = |at: usize| bytes.get(at).is_some_and(|byte| edges.word(*byte, at_end));
        at > 0 && word(at - 1) && word(at) && !escape(at)
    };
    let letter = |ch: &char| {
        *ch == '_' || ch.is_ascii_alphanumeric() || ch.is_lowercase() || ch.is_uppercase()
    };
    let ascii = |at: usize| {
        text[..at]
            .chars()
            .rev()
            .take_while(letter)
            .take(WORD_CHARS)
            .chain(text[at..].chars().take_while(letter).take(WORD_CHARS))
            .all(|ch| ch.is_ascii())
    };
    let digits = |at: usize| bytes[at - 1].is_ascii_digit() && bytes[at].is_ascii_digit();
    let end = edges.end.then_some((start + len, true));
    std::iter::once((start, false))
        .chain(end)
        .find(|&(at, at_end)| joined(at, at_end) && (digits(at) || ascii(at)))
        .map(|(at, at_end)| (at, edges.hyphens[usize::from(at_end)]))
}

/// The ASCII word that `boundary` cuts, clipped, and its line; a hyphen
/// continues it when `hyphens`.
fn cut_word(text: &str, (boundary, hyphens): (usize, bool)) -> (&str, usize) {
    let word = |at: usize| {
        text.as_bytes()
            .get(at)
            .is_some_and(|byte| ascii_word(*byte) || (hyphens && *byte == b'-'))
    };
    let (mut low, mut high) = (boundary, boundary);
    while low > 0 && word(low - 1) && boundary - low < WORD_CHARS {
        low -= 1;
    }
    while word(high) && high - boundary < WORD_CHARS {
        high += 1;
    }
    (&text[low..high], line_numbers(text, &[boundary])[0])
}

/// Names the ASCII word that a cut cuts and its line.
fn inside_word(text: &str, cut: (usize, bool)) -> String {
    let (word, line) = cut_word(text, cut);
    format!(
        "`old` matches inside the word `{word}` at line {line}; give whole words, or add \"count\":1 to replace part of a word"
    )
}

/// The left-to-right matches of `old` in `range` that cut no word, as offsets into
/// the range, skipping ones that overlap an earlier match when `separate`, as a
/// count replaces them; `None` past `ADVISED_MATCHES` starts, which bounds the scan
/// as it bounds ambiguity advice.
fn whole_words(text: &str, range: Range<usize>, old: &str, separate: bool) -> Option<Vec<usize>> {
    let edges = Edges::of(old);
    let (mut whole, mut free) = (Vec::new(), 0);
    for (examined, at) in overlapping_starts(&text[range.clone()], old).enumerate() {
        if examined == ADVISED_MATCHES {
            return None;
        }
        if (!separate || at >= free) && word_cut(text, range.start + at, old.len(), edges).is_none()
        {
            whole.push(at);
            free = at + old.len();
        }
    }
    Some(whole)
}

/// The word that the first match of `old` in `range` to cut one cuts, and its line.
fn first_cut<'t>(text: &'t str, range: Range<usize>, old: &str) -> Option<(&'t str, usize)> {
    let edges = Edges::of(old);
    text[range.clone()]
        .match_indices(old)
        .find_map(|(at, _)| word_cut(text, range.start + at, old.len(), edges))
        .map(|cut| cut_word(text, cut))
}

/// `WHOLE_WORDS_ONLY`: a target took only the whole-word matches of `old` in
/// `range`, so `stayed` matches inside longer words are left; names the first.
fn stayed_inside_words(
    base: &Snapshot,
    change: &Change,
    range: Range<usize>,
    old: &str,
    stayed: usize,
) -> Diagnostic {
    let first = first_cut(&base.text, range, old);
    let left = match first {
        Some((word, line)) if stayed == 1 => {
            format!("the one inside `{word}` at line {line} stayed")
        }
        Some((word, line)) => {
            format!("{stayed} inside longer words stayed, the first in `{word}` at line {line}")
        }
        None => format!("{stayed} others stayed"),
    };
    let taken = match change.target {
        Target::Exact { .. } => "`old` took its one whole-word match",
        Target::Every { .. } => "`all` took only the whole-word matches",
        _ => "count took only the whole-word matches",
    };
    let id: String = change.id.chars().take(QUOTED_ID_CHARS).collect();
    at(
        Some(&base.path),
        Some(change),
        "WHOLE_WORDS_ONLY",
        format!("Change {id}: {taken}, so {left}"),
    )
}

/// What ambiguity advice can offer for `old` in a scope.
enum Ambiguity {
    /// `in` lines where a whole-word match is the only match.
    Alone(usize, usize),
    /// Every match examined shares its lines or cuts a word.
    Context,
    /// Every match cuts a word.
    InsideWords,
}

/// Finds the first whole-word match of `old` in `range` that no other match
/// shares lines with, which `in` with those lines then finds alone.
fn advice(lines: &LineIndex, text: &str, range: Range<usize>, old: &str) -> Ambiguity {
    let mut starts = overlapping_starts(&text[range.clone()], old)
        .map(|at| range.start + at)
        .peekable();
    // A final line feed ends the match's last line, which `in` already includes.
    let spanned = old.strip_suffix('\n').unwrap_or(old).matches('\n').count();
    let (mut previous, mut whole, mut examined) = (None, false, 0);
    let edges = Edges::of(old);
    while examined < ADVISED_MATCHES
        && let Some(at) = starts.next()
    {
        examined += 1;
        if word_cut(text, at, old.len(), edges).is_none() {
            whole = true;
            let first = lines.line_of(at);
            let last = first + spanned;
            let (low, high) = (lines.body(first).start, lines.end(last));
            // A match before line 1's body, in a BOM, is in no line's scope;
            // lines reaching past a partial scope may hold other matches.
            if low <= at
                && previous.is_none_or(|before| before < low)
                && starts.peek().is_none_or(|&after| after + old.len() > high)
                && (range.start <= low && high <= range.end
                    || overlapping_starts(&text[low..high], old).nth(1).is_none())
            {
                return Ambiguity::Alone(first, last);
            }
        }
        previous = Some(at);
    }
    if whole || starts.peek().is_some() {
        Ambiguity::Context
    } else {
        Ambiguity::InsideWords
    }
}

/// Overlapping starts of `old` in `source`, found lazily by KMP so a long,
/// repetitive `old` stays linear.
fn overlapping_starts<'s>(source: &'s str, old: &'s str) -> impl Iterator<Item = usize> + 's {
    let needle = old.as_bytes();
    let prefix = prefix_table(needle);
    let mut bytes = source.bytes().enumerate();
    let mut matched = 0;
    std::iter::from_fn(move || {
        if needle.is_empty() {
            return None;
        }
        for (index, byte) in bytes.by_ref() {
            while matched > 0 && byte != needle[matched] {
                matched = prefix[matched - 1];
            }
            if byte == needle[matched] {
                matched += 1;
            }
            if matched == needle.len() {
                matched = prefix[matched - 1];
                return Some(index + 1 - needle.len());
            }
        }
        None
    })
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

/// A line target's guard: the text of its first lines and, for `lines`, of its
/// last lines, each compared line by line without line endings.
#[derive(Clone, Copy)]
struct Expect<'a> {
    head: Option<&'a str>,
    tail: Option<&'a str>,
}

/// Where a line target's guard lets it apply.
enum Placed {
    /// At the stated lines.
    Stated,
    /// Nowhere; diagnostics say why.
    Refused,
    /// At the one other place where the guard holds; `refusal` is what the
    /// stated lines get.
    Moved { guard: Guard, refusal: Diagnostic },
}

/// Checks a line target's `expect`. Unless the base disclosed every addressed line,
/// line numbers from elsewhere may be stale, so one is required: for `lines`, it
/// must reach the range's last line, either by giving every line or by giving the
/// last lines too, and it must have `MIN_GUARD_CHARS` visible characters or match
/// this place alone, so a shifted number cannot find the same short text elsewhere.
/// When the stated lines do not hold it, a guard that reaches the target's end
/// with `MIN_GUARD_CHARS` visible characters places the target where it holds,
/// if that is exactly one place: a stale or miscounted number then applies only
/// to text the guard matches. Comparing line by line, ignoring line endings, lets
/// text copied from a view that hides `\r` guard CRLF lines.
fn line_guard(
    context: &FileContext,
    change: &Change,
    guard: Guard,
    expect: Expect,
    diagnostics: &mut Vec<Diagnostic>,
    budget: &mut Budget,
) -> Placed {
    let base = context.base;
    let lines = context.lines();
    let reject = |diagnostics: &mut Vec<Diagnostic>, code: &str, message: String| {
        diagnostics.push(at(Some(&base.path), Some(change), code, message));
        Placed::Refused
    };
    let head = expect.head.map(expected_lines).unwrap_or_default();
    let tail = expect.tail.map(expected_lines).unwrap_or_default();
    let (addressed, extent) = match guard {
        Guard::Above { line: 0 } => {
            if head.iter().all(|piece| piece.is_empty()) {
                return Placed::Stated;
            }
            return reject(
                diagnostics,
                "EXPECTED_TEXT_MISMATCH",
                "after:0 inserts at the top of the file, so no line precedes it for expect to guard; omit expect".into(),
            );
        }
        Guard::Prefix { first, last } => (first..=last, last - first + 1),
        Guard::Above { line } => (line..=line, line),
    };
    let required = !context.disclosed(addressed.clone());
    // An empty file has no line to guard; inserting at the top needs no expect.
    let empty = lines.body(1).start == base.text.len();
    let reaches_end = match guard {
        Guard::Prefix { .. } => head.len() >= extent || (!head.is_empty() && !tail.is_empty()),
        Guard::Above { .. } => !head.is_empty(),
    };
    if required && !reaches_end {
        let (first, last) = (*addressed.start(), *addressed.end());
        let lines_named = if first == last {
            format!("Line {first} was")
        } else {
            format!("Lines {first}-{last} were")
        };
        let advice = if empty {
            "the file is empty, so insert with after:0, which needs no expect".to_owned()
        } else {
            match guard {
                Guard::Prefix { .. } if first == last => {
                    "add expect with the line's current text".to_owned()
                }
                Guard::Prefix { .. } if head.is_empty() => {
                    "give expect as [first line, last line] with their current text, or every line"
                        .to_owned()
                }
                Guard::Prefix { .. } => format!(
                    "expect checks only the first {}, missing a shift inside the range: give it as [first line, last line], or every line",
                    head.len()
                ),
                Guard::Above { line } => format!("add expect with line {line}'s current text"),
            }
        };
        return reject(
            diagnostics,
            "LINE_GUARD_REQUIRED",
            format!(
                "{lines_named} not disclosed by this base, so the numbers may be stale; {advice}"
            ),
        );
    }
    let fits = match guard {
        Guard::Prefix { .. } => head.len() <= extent && tail.len() <= extent,
        Guard::Above { .. } => head.len() <= extent,
    };
    if !fits {
        let message = match guard {
            Guard::Prefix { first, last } if first == last => format!(
                "expect has {} lines but lines [{first},{last}] have 1",
                head.len().max(tail.len())
            ),
            Guard::Prefix { first, last } => format!(
                "expect has {} lines but lines [{first},{last}] have {extent}; give the range's first and last lines as [first line, last line]",
                head.len().max(tail.len())
            ),
            Guard::Above { line } => format!(
                "expect has {} lines but only {line} precede the insertion; it gives the lines ending at after",
                head.len()
            ),
        };
        return reject(diagnostics, "EXPECTED_TEXT_MISMATCH", message);
    }
    let matches_at = |start: usize, pieces: &[&str]| {
        pieces
            .iter()
            .enumerate()
            .all(|(offset, piece)| base.text[lines.body(start + offset)] == **piece)
    };
    // Where the head is, and where the range's last lines are.
    let head_start = match guard {
        Guard::Prefix { first, .. } => first,
        Guard::Above { line } => line + 1 - head.len(),
    };
    if !head.is_empty() && !matches_at(head_start, &head) {
        let found = find_lines(&base.text, lines, &head, 1);
        // Only a place where the whole range fits can be offered as its new place.
        let usable: Vec<usize> = found
            .iter()
            .copied()
            .filter(|&start| match guard {
                Guard::Prefix { first, last } => start + (last - first) <= lines.count(),
                Guard::Above { .. } => true,
            })
            .collect();
        let rest = relocated(
            context,
            &found,
            &usable,
            head.len(),
            budget,
            expect.head,
            |start| match guard {
                Guard::Prefix { first, last } => {
                    format!(
                        "use lines [{start},{}] if the whole range moved",
                        last - first + start
                    )
                }
                Guard::Above { .. } => format!("use after:{}", start + head.len() - 1),
            },
        );
        let refusal = mismatch_at(context, change, head_start, head.len(), "expect", rest);
        if reaches_end && let Some(guard) = moved(context, guard, &head, &tail) {
            return Placed::Moved { guard, refusal };
        }
        diagnostics.push(refusal);
        return Placed::Refused;
    }
    // Where the tail guard ends: the range's last line or, when the range ends in
    // blank lines and the tail names the text before them, that text's line.
    let mut tail_end = match guard {
        Guard::Prefix { last, .. } => last,
        Guard::Above { line } => line,
    };
    if let Guard::Prefix { first, last } = guard
        && !tail.is_empty()
        && !matches_at(last + 1 - tail.len(), &tail)
        && let Some(end) = before_blank_end(context, first, last, &head, &tail)
        && matches_at(end + 1 - tail.len(), &tail)
    {
        tail_end = end;
    }
    if let Guard::Prefix { first, last } = guard
        && !tail.is_empty()
        && tail_end == last
        && !matches_at(last + 1 - tail.len(), &tail)
    {
        // The range keeps its start; its end is where the last lines are now,
        // which must leave room for the head.
        let mut found = find_lines(&base.text, lines, &tail, first);
        let usable: Vec<usize> = found
            .iter()
            .copied()
            .filter(|&start| start + tail.len() >= first + head.len())
            .collect();
        if found.is_empty() {
            // Whole lines above the range are still whole lines, not part of one.
            found = find_lines(&base.text, lines, &tail, 1);
        }
        let rest = relocated(
            context,
            &found,
            &usable,
            tail.len(),
            budget,
            expect.tail,
            |start| format!("the range is [{first},{}]", start + tail.len() - 1),
        );
        let tail_start = last + 1 - tail.len();
        diagnostics.push(mismatch_at(
            context,
            change,
            tail_start,
            tail.len(),
            "the expected last lines",
            rest,
        ));
        return Placed::Refused;
    }
    // A line that both the head and the tail check is evidence once.
    let overlap = match guard {
        Guard::Prefix { first, .. } => {
            (first + head.len()).saturating_sub(tail_end + 1 - tail.len())
        }
        Guard::Above { .. } => 0,
    };
    let visible: usize = head
        .iter()
        .chain(&tail[overlap.min(tail.len())..])
        .map(|piece| piece.chars().filter(|ch| !ch.is_whitespace()).count())
        .sum();
    if !required || visible >= MIN_GUARD_CHARS {
        return Placed::Stated;
    }
    // A short guard, such as a blank line or `}`, must match this place alone: one
    // that matches elsewhere too could still match after the file shifted.
    let places: Vec<usize> = match guard {
        Guard::Prefix { first, last } => {
            let count = lines.count();
            let mut places: Vec<usize> = (1..=(count + 1).saturating_sub(extent))
                .filter(|&start| {
                    matches_at(start, &head) && matches_at(start + extent - tail.len(), &tail)
                })
                .take(AMBIGUOUS_LINES + 1)
                .collect();
            // Read's empty line after a final newline clamps to the last line, so a
            // range ending there keeps its end through a one-line shift while its
            // length changes: try the ranges one line longer and shorter too.
            if last == count {
                for start in [first.wrapping_sub(1), first + 1] {
                    let length = (count + 1).wrapping_sub(start);
                    if (1..=count).contains(&start)
                        && length >= head.len().max(tail.len())
                        && matches_at(start, &head)
                        && matches_at(count + 1 - tail.len(), &tail)
                    {
                        places.push(start);
                    }
                }
            }
            places
        }
        Guard::Above { .. } => (1..=(lines.count() + 1).saturating_sub(head.len()))
            .filter(|&start| matches_at(start, &head))
            .map(|start| start + head.len() - 1)
            .take(AMBIGUOUS_LINES + 1)
            .collect(),
    };
    if places.len() > 1 {
        let more = stronger_guard(context, guard, head.len(), tail.len(), extent);
        let more = match guard {
            _ if !more.is_empty() => more,
            Guard::Prefix { .. } if head.len() + tail.len() < extent => {
                "add lines to it (each part of [first, last] may hold several)".to_owned()
            }
            Guard::Above { line } if head.len() < line => {
                "extend it with the lines above".to_owned()
            }
            // Every line above is given; only the line below can single it out.
            Guard::Above { line } if line < lines.count() => format!(
                "replace lines [{line},{}] instead, repeating both in `new` and `expect`",
                line + 1
            ),
            Guard::Above { .. } => String::new(),
            Guard::Prefix { .. } => {
                "widen the range by a neighbouring line, repeating it in `new` and `expect`"
                    .to_owned()
            }
        };
        let advice = if more.is_empty() {
            "target the text with `old`".to_owned()
        } else {
            format!("{more}, or use `old`")
        };
        let count = if places.len() > AMBIGUOUS_LINES {
            format!("more than {AMBIGUOUS_LINES}")
        } else {
            places.len().to_string()
        };
        let noun = match guard {
            Guard::Prefix { .. } => "ranges",
            Guard::Above { .. } => "places",
        };
        let given: Vec<&str> = [expect.head, expect.tail].into_iter().flatten().collect();
        let quoted = candidates::quoted(&given.join(" "), 28);
        return reject(
            diagnostics,
            "LINE_GUARD_WEAK",
            format!(
                "expect {quoted} is short and matches {count} {noun} in this file, from line {}, so a stale number could pick another; {advice}",
                places[0]
            ),
        );
    }
    Placed::Stated
}

/// The one place other than the stated lines where a guard that reaches the
/// target's end, with `MIN_GUARD_CHARS` visible characters, holds as it would at
/// the stated lines, and the whole target fits: for `lines`, its first and last
/// lines there together, shifted alike.
fn moved(context: &FileContext, guard: Guard, head: &[&str], tail: &[&str]) -> Option<Guard> {
    let (base, lines) = (context.base, context.lines());
    let count = lines.count();
    let matches_at = |start: usize, pieces: &[&str]| {
        pieces
            .iter()
            .enumerate()
            .all(|(offset, piece)| base.text[lines.body(start + offset)] == **piece)
    };
    let shown = |pieces: &[&str]| pieces.iter().map(|piece| visible(piece)).sum::<usize>();
    let places: Vec<Guard> = match guard {
        Guard::Prefix { first, last } => {
            let extent = last - first + 1;
            // A line that both the head and the tail check is evidence once.
            let overlap = (head.len() + tail.len()).saturating_sub(extent);
            if shown(head) + shown(&tail[overlap.min(tail.len())..]) < MIN_GUARD_CHARS {
                return None;
            }
            (1..=(count + 1).saturating_sub(extent))
                .filter(|&start| {
                    let end = start + extent - 1;
                    matches_at(start, head)
                        && (tail.is_empty()
                            || matches_at(end + 1 - tail.len(), tail)
                            || before_blank_end(context, start, end, head, tail)
                                .is_some_and(|line| matches_at(line + 1 - tail.len(), tail)))
                })
                .take(2)
                .map(|start| Guard::Prefix {
                    first: start,
                    last: start + extent - 1,
                })
                .collect()
        }
        Guard::Above { .. } => {
            if shown(head) < MIN_GUARD_CHARS {
                return None;
            }
            (1..=(count + 1).saturating_sub(head.len()))
                .filter(|&start| matches_at(start, head))
                .take(2)
                .map(|start| Guard::Above {
                    line: start + head.len() - 1,
                })
                .collect()
        }
    };
    match places[..] {
        [place] => Some(place),
        _ => None,
    }
}

/// Names the fewest lines a weak guard could add to reach `MIN_GUARD_CHARS`, or
/// empty when none do. Only line numbers are named, never their text: the guard
/// exists because the number may be stale, so the text must come from the
/// caller's own view of the file, not from the place the number points to now.
fn stronger_guard(
    context: &FileContext,
    guard: Guard,
    head: usize,
    tail: usize,
    extent: usize,
) -> String {
    let (base, lines) = (context.base, context.lines());
    let visible = |range: std::ops::RangeInclusive<usize>| -> usize {
        range
            .map(|line| {
                base.text[lines.body(line)]
                    .chars()
                    .filter(|ch| !ch.is_whitespace())
                    .count()
            })
            .sum()
    };
    let span = |first: usize, last: usize| {
        if first == last {
            format!("line {first}")
        } else {
            format!("lines {first}-{last}")
        }
    };
    match guard {
        Guard::Prefix { first, last } if head + tail < extent => {
            let tail_start = last + 1 - tail;
            let tail_seen = visible(tail_start..=last);
            let head_seen = visible(first..=first + head - 1);
            // Grow one part toward the other, whichever needs fewer lines.
            let grown_head = (head + 1..=extent - tail)
                .find(|&k| visible(first..=first + k - 1) + tail_seen >= MIN_GUARD_CHARS);
            let grown_tail = (tail + 1..=extent - head)
                .find(|&k| head_seen + visible(last + 1 - k..=last) >= MIN_GUARD_CHARS);
            match (grown_head, grown_tail) {
                (Some(k), Some(j)) if j - tail < k - head => format!(
                    "make its last part {} (a part may hold several lines)",
                    span(last + 1 - j, last)
                ),
                (Some(k), _) => format!(
                    "make its first part {} (a part may hold several lines)",
                    span(first, first + k - 1)
                ),
                (None, Some(j)) => format!(
                    "make its last part {} (a part may hold several lines)",
                    span(last + 1 - j, last)
                ),
                (None, None) => String::new(),
            }
        }
        Guard::Prefix { first, last } => {
            let seen = visible(first..=last);
            if first > 1 && seen + visible(first - 1..=first - 1) >= MIN_GUARD_CHARS {
                format!(
                    "widen the range to lines [{},{last}], repeating line {} in `new` and `expect`",
                    first - 1,
                    first - 1
                )
            } else if last < lines.count() && seen + visible(last + 1..=last + 1) >= MIN_GUARD_CHARS
            {
                format!(
                    "widen the range to lines [{first},{}], repeating line {} in `new` and `expect`",
                    last + 1,
                    last + 1
                )
            } else {
                String::new()
            }
        }
        Guard::Above { line } => (head + 1..=line.min(head + 8))
            .find(|&k| visible(line + 1 - k..=line) >= MIN_GUARD_CHARS)
            .map(|k| format!("give {} as expect", span(line + 1 - k, line)))
            .unwrap_or_default(),
    }
}

/// Advice and candidates for guarded lines that are not where the target says:
/// a new place where the target fits, whole lines elsewhere, or near misses.
fn relocated(
    context: &FileContext,
    found: &[usize],
    usable: &[usize],
    count: usize,
    budget: &mut Budget,
    text: Option<&str>,
    retarget: impl Fn(usize) -> String,
) -> (String, Vec<Candidate>) {
    if !usable.is_empty() {
        (
            candidates::line_places(usable, AMBIGUOUS_LINES, retarget),
            line_candidates(context, usable, count),
        )
    } else if !found.is_empty() {
        // Whole lines, but the target cannot have moved there whole.
        (
            candidates::line_advice(&[]),
            line_candidates(context, found, count),
        )
    } else {
        fuzzy_advice(context, text.unwrap_or_default(), budget)
    }
}

/// The last line before the blank lines that end `first..=last`, when a tail
/// that names text, together with the head, guards strongly enough to stand
/// there: a model often guards a range that ends in a blank line with the text
/// line before it. The range itself is unchanged.
fn before_blank_end(
    context: &FileContext,
    first: usize,
    last: usize,
    head: &[&str],
    tail: &[&str],
) -> Option<usize> {
    let (base, lines) = (context.base, context.lines());
    let blank = |line: usize| base.text[lines.body(line)].trim().is_empty();
    if !blank(last) || tail.last().is_none_or(|piece| piece.trim().is_empty()) {
        return None;
    }
    let mut end = last;
    while end > first && blank(end) {
        end -= 1;
    }
    if end + 1 < first + head.len().max(tail.len()) {
        return None;
    }
    // A line that both the head and the tail check is evidence once.
    let overlap = (first + head.len()).saturating_sub(end + 1 - tail.len());
    let visible: usize = head
        .iter()
        .chain(&tail[overlap.min(tail.len())..])
        .map(|piece| piece.chars().filter(|ch| !ch.is_whitespace()).count())
        .sum();
    (visible >= MIN_GUARD_CHARS).then_some(end)
}

/// `EXPECTED_TEXT_MISMATCH` quoting the `count` lines from `start`.
fn mismatch_at(
    context: &FileContext,
    change: &Change,
    start: usize,
    count: usize,
    wanted: &str,
    (rest, found): (String, Vec<Candidate>),
) -> Diagnostic {
    let (base, lines) = (context.base, context.lines());
    let end = start + count - 1;
    let actual = &base.text[lines.body(start).start..lines.body(end).end];
    let subject = if start == end {
        format!("Line {start}")
    } else {
        format!("Lines {start}-{end}")
    };
    let mut diagnostic = at(
        Some(&base.path),
        Some(change),
        "EXPECTED_TEXT_MISMATCH",
        candidates::line_mismatch(&subject, actual, wanted, &rest),
    );
    diagnostic.candidates = found;
    diagnostic
}

/// Advice and candidates for line-wise text found nowhere as whole lines.
fn fuzzy_advice(
    context: &FileContext,
    text: &str,
    budget: &mut Budget,
) -> (String, Vec<Candidate>) {
    let base = context.base;
    let needle = adapt_eol(context.crlf(), text.strip_suffix('\n').unwrap_or(text));
    let whole = 0..base.text.len();
    let found =
        budget.candidates(|search| candidates::find(search, &base.text, whole, &needle, true));
    (candidates::line_advice(&found), found)
}

/// `exact` candidates for runs of `count` whole lines found at `starts`.
fn line_candidates(context: &FileContext, starts: &[usize], count: usize) -> Vec<Candidate> {
    let (text, lines) = (&context.base.text, context.lines());
    starts
        .iter()
        .take(candidates::MAX_CANDIDATES)
        .map(|&line| {
            let end_line = line + count - 1;
            let found = &text[lines.body(line).start..lines.body(end_line).end];
            Candidate {
                kind: CandidateKind::Exact,
                line,
                end_line,
                text: (found.chars().count() <= candidates::MAX_TEXT_CHARS).then(|| found.into()),
                similarity: None,
            }
        })
        .collect()
}

/// Lines from `from` on where `pieces` occur as consecutive whole line bodies, at
/// most one more than [`AMBIGUOUS_LINES`], so a caller can tell the cap was reached.
fn find_lines(text: &str, lines: &LineIndex, pieces: &[&str], from: usize) -> Vec<usize> {
    let count = lines.count();
    if pieces.is_empty() || pieces.len() > count {
        return Vec::new();
    }
    (from.max(1)..=count + 1 - pieces.len())
        .filter(|&start| {
            pieces
                .iter()
                .enumerate()
                .all(|(offset, piece)| text[lines.body(start + offset)] == **piece)
        })
        .take(AMBIGUOUS_LINES + 1)
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Responses clip messages at 240 characters, which would cut the advice.
    #[test]
    fn restatement_messages_fit_at_eight_digit_lines() {
        let (first, line) = (99_999_990, 99_999_996);
        for advice in [
            Advice::Drop,
            Advice::Edited,
            Advice::Merge(99_999_980, 99_999_999, false),
            Advice::Merge(99_999_980, 99_999_999, true),
        ] {
            for bare in [false, true] {
                for [first, last] in [[first, line + 2], [line, line + 1], [first, line]] {
                    let message = restatement_message(line + 1, [first, line, last], bare, advice);
                    assert!(message.chars().count() <= 240, "{message}");
                }
            }
        }
    }
}
