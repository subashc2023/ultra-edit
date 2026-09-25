//! Near-miss candidates for a target that matched nowhere.
//!
//! Tiers run in order and the first with results supplies up to three regions:
//!
//! 1. `exact`: literal occurrences of an unmet span expectation elsewhere.
//! 2. `whitespace`: equal after CRLF becomes LF, spaces and tabs before a line
//!    ending are dropped, other runs of spaces and tabs become one space, and
//!    the needle's own leading and trailing spaces and tabs are trimmed.
//! 3. `similar`: close by edit distance; see `similar_regions`.
//!
//! Only failure diagnostics search. Each tier is linear in the searched text,
//! apart from a fixed number of alignments of bounded size.

use std::cmp::Reverse;
use std::ops::Range;

use crate::model::{Candidate, CandidateKind};

const MAX_CANDIDATES: usize = 3;
/// Longer regions omit their text rather than clip it, so a partial region is
/// never copied as a target.
const MAX_TEXT_CHARS: usize = 2_000;
const MAX_SIMILAR_LINES: usize = 200_000;
const MAX_SIMILAR_NEEDLE_CHARS: usize = 20_000;
/// Scope lines ranked against the needle, each placing one window.
const RANKED_LINES: usize = 8;
/// Alignment work per window, the cost of aligning two 2,000-character texts.
const MAX_ALIGNMENT_CELLS: usize = 4_000_000;
const MAX_ALIGNED_WINDOW_CHARS: usize = 20_000;
const MIN_SIMILARITY: usize = 70;
/// Message characters clients display before clipping.
const MESSAGE_CHARS: usize = 240;
/// Escaped characters of actual span text quoted by an expectation mismatch.
const QUOTED_CHARS: usize = 60;
const REPAIR: &str = "(ultra_edit_repair can replace just this change)";

type Found = Vec<(Range<usize>, Option<u8>)>;

/// Closest regions of `text[scope]` for a `needle` that matched nowhere, from
/// the first tier with results. Only an unmet expectation enables `exact`.
/// Each search spends its scope and needle bytes from the request's `budget`;
/// once that is exhausted, failures carry no candidates.
pub(crate) fn find(
    budget: &mut usize,
    text: &str,
    scope: Range<usize>,
    needle: &str,
    exact: bool,
) -> Vec<Candidate> {
    let cost = scope.len().saturating_add(needle.len());
    let Some(left) = budget.checked_sub(cost) else {
        return Vec::new();
    };
    *budget = left;
    if needle.is_empty() || scope.is_empty() {
        return Vec::new();
    }
    let mut kind = CandidateKind::Exact;
    let mut found = if exact {
        exact_regions(text, &scope, needle)
    } else {
        Vec::new()
    };
    if found.is_empty() {
        kind = CandidateKind::Whitespace;
        found = whitespace_regions(text, &scope, needle);
    }
    if found.is_empty() {
        kind = CandidateKind::Similar;
        found = similar_regions(text, &scope, needle);
    }
    located(text, kind, found)
}

/// Completes a message for a target with no occurrences and some candidates.
pub(crate) fn not_found(expected: usize, candidates: &[Candidate]) -> String {
    format!(
        "Expected {expected} occurrence(s), found 0; {}",
        advice(candidates)
    )
}

/// Describes an unmet span expectation, quoting the text the span selected.
pub(crate) fn mismatch(actual: &str, candidates: &[Candidate]) -> String {
    let rest = if candidates.is_empty() {
        "inspect the original snapshot and choose the intended span".to_owned()
    } else {
        advice(candidates)
    };
    // The quote yields room to the advice; the three extra characters are its
    // quotes and a clipping ellipsis.
    let fixed = "Span holds , not expect; ".len() + rest.chars().count() + 3;
    let room = MESSAGE_CHARS.saturating_sub(fixed);
    let actual = quoted(actual, room.min(QUOTED_CHARS));
    format!("Span holds {actual}, not expect; {rest}")
}

/// Explains candidates briefly; the candidates themselves carry every line.
fn advice(candidates: &[Candidate]) -> String {
    let Some(first) = candidates.first() else {
        return String::new();
    };
    let (count, line) = (candidates.len(), first.line);
    let many = count > 1;
    let place = if line == first.end_line {
        format!("line {line}")
    } else {
        format!("lines {line}-{}", first.end_line)
    };
    let percent = first.similarity.unwrap_or_default();
    let finding = match (first.kind, many) {
        (CandidateKind::Exact, false) => format!("the expected text is at {place}"),
        (CandidateKind::Exact, true) => {
            format!("the expected text occurs {count} times, first at line {line}")
        }
        (CandidateKind::Whitespace, false) => {
            format!("a candidate at {place} differs only in whitespace")
        }
        (CandidateKind::Whitespace, true) => {
            format!("{count} candidates, first at line {line}, differ only in whitespace")
        }
        (CandidateKind::Similar, false) => format!("a {percent}% similar candidate is at {place}"),
        (CandidateKind::Similar, true) => {
            format!("{count} similar candidates, best {percent}% at line {line}")
        }
    };
    let complete = candidates.iter().all(|candidate| candidate.text.is_some());
    let action = match (first.kind, many, complete) {
        (_, false, false) => "Read it and copy its exact text into `old`",
        (_, true, false) => "Read one and copy its exact text into `old`",
        (CandidateKind::Similar, false, true) => "Verify it, then copy its exact text into `old`",
        (CandidateKind::Similar, true, true) => "Verify one, then copy its exact text into `old`",
        (_, false, true) => "Copy its exact text into `old`",
        (_, true, true) => "Copy the intended one's exact text into `old`",
    };
    format!("{finding}. {action} {REPAIR}.")
}

/// Source text on one line, escaped like a Rust string literal and clipped at
/// `limit` escaped characters; an ellipsis after the closing quote marks a clip.
fn quoted(text: &str, limit: usize) -> String {
    let mut output = String::from('"');
    let mut width = 0;
    for character in text.chars() {
        let escaped: String = if character == '\'' {
            character.into()
        } else {
            character.escape_debug().collect()
        };
        width += escaped.chars().count();
        if width > limit {
            output.push_str("\"…");
            return output;
        }
        output.push_str(&escaped);
    }
    output.push('"');
    output
}

/// Numbers each region's first and last line, counting line breaks once up to
/// the furthest region. An empty region ends on its first line.
fn located(text: &str, kind: CandidateKind, found: Found) -> Vec<Candidate> {
    let bytes = text.as_bytes();
    let mut boundaries: Vec<(usize, usize)> = found
        .iter()
        .enumerate()
        .flat_map(|(index, (region, _))| {
            let last = region.end.saturating_sub(1).max(region.start);
            [(region.start, 2 * index), (last, 2 * index + 1)]
        })
        .collect();
    boundaries.sort_unstable();
    let mut lines = vec![0; boundaries.len()];
    let (mut offset, mut line) = (0, 1);
    for (position, slot) in boundaries {
        line += bytes[offset..position]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count();
        offset = position;
        lines[slot] = line;
    }
    found
        .into_iter()
        .enumerate()
        .map(|(index, (region, similarity))| {
            let source = &text[region];
            Candidate {
                kind,
                line: lines[2 * index],
                end_line: lines[2 * index + 1],
                text: source
                    .chars()
                    .nth(MAX_TEXT_CHARS)
                    .is_none()
                    .then(|| source.to_owned()),
                similarity,
            }
        })
        .collect()
}

fn exact_regions(text: &str, scope: &Range<usize>, needle: &str) -> Found {
    text[scope.clone()]
        .match_indices(needle)
        .take(MAX_CANDIDATES)
        .map(|(offset, _)| {
            let start = scope.start + offset;
            (start..start + needle.len(), None)
        })
        .collect()
}

/// Streams the normalized scope through a KMP matcher, keeping the original
/// start of each of the last `pattern.len()` normalized bytes, so memory stays
/// proportional to the needle.
fn whitespace_regions(text: &str, scope: &Range<usize>, needle: &str) -> Found {
    let mut pattern: Vec<u8> = Normalized::new(needle.as_bytes())
        .map(|(byte, _)| byte)
        .collect();
    // Runs are already single spaces, so trimming removes at most one per end.
    if pattern.last() == Some(&b' ') {
        pattern.pop();
    }
    if pattern.first() == Some(&b' ') {
        pattern.remove(0);
    }
    if pattern.is_empty() {
        return Vec::new();
    }
    let shape = Shape {
        newline: false,
        ..Shape::of(needle)
    };
    let prefix = prefix_table(&pattern);
    let mut starts = vec![0; pattern.len()];
    // Once a match completes the ring is full, so `slot` is its oldest entry.
    let mut slot = 0;
    let mut matched = 0;
    let mut found = Vec::new();
    let source = &text.as_bytes()[scope.clone()];
    for (byte, range) in Normalized::new(source) {
        starts[slot] = range.start;
        slot = if slot + 1 == pattern.len() {
            0
        } else {
            slot + 1
        };
        while matched > 0 && byte != pattern[matched] {
            matched = prefix[matched - 1];
        }
        if byte == pattern[matched] {
            matched += 1;
        }
        if matched == pattern.len() {
            let region = scope.start + starts[slot]..scope.start + range.end;
            found.push((shape.widen(text.as_bytes(), scope, region), None));
            if found.len() == MAX_CANDIDATES {
                break;
            }
            // Candidates never overlap.
            matched = 0;
        }
    }
    found
}

/// Whitespace-normalized bytes, each with the source byte range it stands for.
/// Only ASCII bytes change, so a normalized match of UTF-8 text starts and ends
/// on source character boundaries.
struct Normalized<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Normalized<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
}

impl Iterator for Normalized<'_> {
    type Item = (u8, Range<usize>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let start = self.position;
            let byte = *self.bytes.get(start)?;
            if byte == b' ' || byte == b'\t' {
                let mut end = start + 1;
                while end < self.bytes.len() && matches!(self.bytes[end], b' ' | b'\t') {
                    end += 1;
                }
                self.position = end;
                let rest = &self.bytes[end..];
                if rest.starts_with(b"\n") || rest.starts_with(b"\r\n") {
                    continue;
                }
                return Some((b' ', start..end));
            }
            if byte == b'\r' && self.bytes.get(start + 1) == Some(&b'\n') {
                self.position = start + 2;
                return Some((b'\n', start..start + 2));
            }
            self.position = start + 1;
            return Some((byte, start..start + 1));
        }
    }
}

fn prefix_table(pattern: &[u8]) -> Vec<usize> {
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

/// Needle edges that normalization discards. A candidate region absorbs the
/// matching source spaces, tabs, or final line ending, so its text replaces
/// what the needle meant to replace.
#[derive(Clone, Copy)]
struct Shape {
    leading: bool,
    trailing: bool,
    newline: bool,
}

impl Shape {
    fn of(needle: &str) -> Self {
        let blank = |byte: Option<&u8>| matches!(byte, Some(b' ' | b'\t'));
        Self {
            leading: blank(needle.as_bytes().first()),
            trailing: blank(needle.as_bytes().last()),
            newline: needle.ends_with('\n'),
        }
    }

    fn widen(self, text: &[u8], scope: &Range<usize>, mut region: Range<usize>) -> Range<usize> {
        let blank = |byte: u8| byte == b' ' || byte == b'\t';
        if self.leading {
            while region.start > scope.start && blank(text[region.start - 1]) {
                region.start -= 1;
            }
        }
        if self.newline || self.trailing {
            let mut end = region.end;
            while end < scope.end && blank(text[end]) {
                end += 1;
            }
            if !self.newline {
                region.end = end;
            } else if text[end..scope.end].starts_with(b"\r\n") {
                region.end = end + 2;
            } else if text[end..scope.end].starts_with(b"\n") {
                region.end = end + 1;
            }
        }
        region
    }
}

/// Scores windows of the needle's line count on squeezed text: every line
/// without leading or trailing spaces and tabs, and with inner runs as one space.
///
/// The needle's longest squeezed line is its anchor. Scope lines are ranked by
/// how many of the anchor's byte bigrams they share (multiset intersection, both
/// sides padded with a line break), then by the Dice coefficient of those
/// bigrams, then by position. Each of the best `RANKED_LINES` lines places a
/// window where the anchor would fall, widened by a line on each side to absorb
/// an inserted or deleted line. A window is scored by fitting alignment, the
/// fewest character edits turning the needle into any substring of the window,
/// as `100 * (L - edits) / L` rounded down, where `L` is the longer of the needle
/// and that substring. When that alignment exceeds its work bound, the unwidened
/// window is scored whole by bigram Dice instead, as `200 * shared / (a + b)`
/// rounded down for bigram counts `a` and `b`. Scores of at least 70 are
/// reported best first, then by position, without overlapping regions.
fn similar_regions(text: &str, scope: &Range<usize>, needle: &str) -> Found {
    if needle.chars().nth(MAX_SIMILAR_NEEDLE_CHARS).is_some() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    for line in scope_lines(text, scope) {
        if lines.len() == MAX_SIMILAR_LINES {
            return Vec::new();
        }
        lines.push(line);
    }
    // A final line ending is restored by `Shape::widen`, not scored.
    let body = needle
        .strip_suffix('\n')
        .map_or(needle, |body| body.strip_suffix('\r').unwrap_or(body));
    let needle_lines: Vec<Vec<u8>> = body
        .split('\n')
        .map(|line| {
            let mut squeezed = Vec::new();
            squeeze_into(
                line.strip_suffix('\r').unwrap_or(line).as_bytes(),
                &mut squeezed,
            );
            squeezed
        })
        .collect();
    let Some((anchor, anchor_line)) = needle_lines
        .iter()
        .enumerate()
        .max_by_key(|(index, line)| (line.len(), Reverse(*index)))
    else {
        return Vec::new();
    };
    if anchor_line.is_empty() {
        return Vec::new();
    }
    let squeezed_needle = needle_lines.join(&b'\n');
    let pattern: Vec<char> = String::from_utf8_lossy(&squeezed_needle).chars().collect();

    let bytes = text.as_bytes();
    let mut anchor_bigrams = Bigrams::new(anchor_line);
    let mut ranked: Vec<((usize, usize), usize)> = Vec::with_capacity(RANKED_LINES + 1);
    // Squeezed widths and fingerprints size and place later windows without
    // another pass over the text.
    let mut widths = Vec::with_capacity(lines.len());
    let mut prints = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        let tally = anchor_bigrams.compare(bytes, std::slice::from_ref(line));
        widths.push(tally.width);
        prints.push(tally.fingerprint);
        let key = (tally.shared, tally.dice);
        if key.0 == 0 || (ranked.len() == RANKED_LINES && key <= ranked[RANKED_LINES - 1].0) {
            continue;
        }
        // Equal keys keep the earlier line first.
        let at = ranked.partition_point(|(ranked_key, _)| *ranked_key >= key);
        ranked.insert(at, (key, index));
        ranked.truncate(RANKED_LINES);
    }

    // Squeezing is idempotent, so needle lines fingerprint as source lines do.
    let needle_prints: Vec<u64> = needle_lines
        .iter()
        .map(|line| {
            let whole = 0..line.len();
            anchor_bigrams
                .compare(line, std::slice::from_ref(&whole))
                .fingerprint
        })
        .collect();
    let height = needle_lines.len();
    let latest = lines.len().saturating_sub(height);
    let window_limit = MAX_ALIGNED_WINDOW_CHARS.min(MAX_ALIGNMENT_CELLS / pattern.len());
    let shape = Shape::of(needle);
    let mut needle_bigrams = None;
    let mut scored = Vec::new();
    for (_, ranked_line) in ranked {
        let first = ranked_line.saturating_sub(anchor).min(latest);
        let last = (first + height).min(lines.len());
        let widened = &lines[first.saturating_sub(1)..(last + 1).min(lines.len())];
        let window = window_chars(text, widened, window_limit)
            .or_else(|| window_chars(text, &lines[first..last], window_limit));
        let (hit, placed) = match window {
            // Each ranked line proposes the closest region through itself, so
            // a better neighbor cannot hide it.
            Some(window) => (aligned(&pattern, &window, &lines[ranked_line], shape), 0),
            None => {
                // Whole-window Dice barely changes when a window shifts by a
                // line, so settle the window where the most lines equal the
                // needle's at the same offset, and let that count break ties.
                let placed = |first: usize| {
                    prints[first..(first + height).min(lines.len())]
                        .iter()
                        .zip(&needle_prints)
                        .filter(|(print, wanted)| print == wanted)
                        .count()
                };
                let (count, first) = [first.saturating_sub(1), (first + 1).min(latest)]
                    .into_iter()
                    .map(|shifted| (placed(shifted), shifted))
                    .fold((placed(first), first), |best, next| {
                        if next.0 > best.0 { next } else { best }
                    });
                let last = (first + height).min(lines.len());
                let bigrams = needle_bigrams.get_or_insert_with(|| Bigrams::new(&squeezed_needle));
                // Squeezed lines joined by line breaks.
                let width = widths[first..last].iter().sum::<usize>() + (last - first - 1);
                (
                    dice_window(bytes, &lines[first..last], width, bigrams),
                    count,
                )
            }
        };
        if let Some((score, region)) = hit {
            scored.push((score, placed, shape.widen(bytes, scope, region)));
        }
    }
    scored.sort_by_key(|(score, placed, region)| {
        (Reverse(*score), Reverse(*placed), region.start, region.end)
    });
    let mut found: Found = Vec::new();
    for (score, _, region) in scored {
        if found
            .iter()
            .any(|(kept, _)| kept.start < region.end && region.start < kept.end)
        {
            continue;
        }
        found.push((region, Some(score)));
        if found.len() == MAX_CANDIDATES {
            break;
        }
    }
    found
}

/// Line bodies within `scope`, bounded as `compiler::line_ranges` bounds them
/// for the whole text.
fn scope_lines<'a>(text: &'a str, scope: &Range<usize>) -> impl Iterator<Item = Range<usize>> + 'a {
    let mut start = scope.start;
    if start == 0 && text.starts_with('\u{feff}') {
        start = 3;
    }
    text[start..scope.end.max(start)]
        .split_inclusive('\n')
        .map(move |line| {
            let body = line
                .strip_suffix('\n')
                .map_or(line, |body| body.strip_suffix('\r').unwrap_or(body));
            let range = start..start + body.len();
            start += line.len();
            range
        })
}

/// Appends a line without leading or trailing spaces and tabs, and with each
/// inner run of them as one space.
fn squeeze_into(line: &[u8], out: &mut Vec<u8>) {
    let mut started = false;
    let mut space = false;
    for &byte in line {
        if byte == b' ' || byte == b'\t' {
            space = started;
        } else {
            if space {
                out.push(b' ');
                space = false;
            }
            out.push(byte);
            started = true;
        }
    }
}

/// Squeezed window text as characters with the source bytes each stands for; a
/// line break stands for the line ending it replaces. `None` above `limit`.
fn window_chars(
    text: &str,
    lines: &[Range<usize>],
    limit: usize,
) -> Option<Vec<(char, Range<usize>)>> {
    let mut chars = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            chars.push(('\n', lines[index - 1].end..line.start));
        }
        let mut started = false;
        let mut run = None;
        for (offset, character) in text[line.clone()].char_indices() {
            let at = line.start + offset;
            if character == ' ' || character == '\t' {
                if started && run.is_none() {
                    run = Some(at);
                }
                continue;
            }
            if let Some(run) = run.take() {
                chars.push((' ', run..at));
            }
            chars.push((character, at..at + character.len_utf8()));
            started = true;
            if chars.len() > limit {
                return None;
            }
        }
    }
    (chars.len() <= limit).then_some(chars)
}

/// Scores the window substring closest to `pattern` that overlaps the source
/// `line`, returning its source bytes.
fn aligned(
    pattern: &[char],
    window: &[(char, Range<usize>)],
    line: &Range<usize>,
    shape: Shape,
) -> Option<(u8, Range<usize>)> {
    let wanted = pattern.len();
    // A shorter window forces more deletions than any passing score allows.
    if 10 * window.len() < 7 * wanted {
        return None;
    }
    let inside =
        |(_, range): &(char, Range<usize>)| line.start <= range.start && range.end <= line.end;
    let through = window.iter().position(inside)?..window.iter().rposition(inside)? + 1;
    let chars: Vec<char> = window.iter().map(|(character, _)| *character).collect();
    // Passing needs 10 * edits <= 3 * L, and L <= needle + edits.
    let (edits, mut span) = align(pattern, &chars, through, 3 * wanted / 7, shape)?;
    let length = wanted.max(span.len());
    let score = 100 * length.saturating_sub(edits) / length;
    // A substituted edge space or line break is not part of the candidate.
    let gap = |character: &char| matches!(character, ' ' | '\n');
    while !span.is_empty() && gap(&chars[span.start]) && !pattern.first().is_some_and(gap) {
        span.start += 1;
    }
    while !span.is_empty() && gap(&chars[span.end - 1]) && !pattern.last().is_some_and(gap) {
        span.end -= 1;
    }
    if score < MIN_SIMILARITY || span.is_empty() {
        return None;
    }
    let region = window[span.start].1.start..window[span.end - 1].1.end;
    Some((u8::try_from(score).ok()?, region))
}

/// Semi-global edit distance: the fewest insertions, deletions, and
/// substitutions turning `pattern` into some substring of `window` that
/// overlaps `through`. Substring edges never split a word, and where the
/// pattern begins or ends with a word character the substring begins or ends a
/// word; a needle that began with indentation or ended with a line ending must
/// meet a line edge instead. Of the alignments within `limit` edits, the one
/// chosen has the most similar substring; ties prefer a length closer to the
/// pattern's, then the earliest end. Row minimums never decrease, so this stops
/// as soon as none can remain.
fn align(
    pattern: &[char],
    window: &[char],
    through: Range<usize>,
    limit: usize,
    shape: Shape,
) -> Option<(usize, Range<usize>)> {
    // Disallowed starts stay far above any real cost without overflowing.
    const DISALLOWED: usize = usize::MAX / 4;
    let word = |character: char| character.is_alphanumeric() || character == '_';
    let before = |column: usize| column > 0 && word(window[column - 1]);
    let after = |column: usize| column < window.len() && word(window[column]);
    let may_start = |column: usize| {
        column < through.end
            && if shape.leading {
                column == 0 || window[column - 1] == '\n'
            } else if pattern.first().is_some_and(|first| word(*first)) {
                after(column) && !before(column)
            } else {
                !(before(column) && after(column))
            }
    };
    let may_end = |column: usize| {
        column > through.start
            && if shape.newline {
                column == window.len() || window[column] == '\n'
            } else if pattern.last().is_some_and(|last| word(*last)) {
                before(column) && !after(column)
            } else {
                !(before(column) && after(column))
            }
    };
    let columns = window.len() + 1;
    let mut cost: Vec<usize> = (0..columns)
        .map(|column| if may_start(column) { 0 } else { DISALLOWED })
        .collect();
    let mut start: Vec<usize> = (0..columns).collect();
    let mut next_cost = vec![0; columns];
    let mut next_start = vec![0; columns];
    for wanted in pattern {
        next_cost[0] = cost[0] + 1;
        next_start[0] = 0;
        let mut lowest = next_cost[0];
        for column in 1..columns {
            let substitute = cost[column - 1] + usize::from(window[column - 1] != *wanted);
            let delete = cost[column] + 1;
            let insert = next_cost[column - 1] + 1;
            let (value, origin) = if substitute <= delete && substitute <= insert {
                (substitute, start[column - 1])
            } else if delete <= insert {
                (delete, start[column])
            } else {
                (insert, next_start[column - 1])
            };
            next_cost[column] = value;
            next_start[column] = origin;
            lowest = lowest.min(value);
        }
        if lowest > limit {
            return None;
        }
        std::mem::swap(&mut cost, &mut next_cost);
        std::mem::swap(&mut start, &mut next_start);
    }
    let mut best: Option<(usize, Range<usize>)> = None;
    for column in (0..columns).filter(|column| may_end(*column) && cost[*column] <= limit) {
        let (edits, span) = (cost[column], start[column]..column);
        let better = best.as_ref().is_none_or(|(best_edits, best_span)| {
            let length = pattern.len().max(span.len());
            let best_length = pattern.len().max(best_span.len());
            // Compares (L - edits) / L exactly.
            let kept = length.saturating_sub(edits) * best_length;
            let best_kept = best_length.saturating_sub(*best_edits) * length;
            kept > best_kept
                || (kept == best_kept
                    && span.len().abs_diff(pattern.len()) < best_span.len().abs_diff(pattern.len()))
        });
        if better {
            best = Some((edits, span));
        }
    }
    best
}

/// Scores a window too large to align by the bigram Dice coefficient of its
/// whole squeezed text; the region spans its first to last visible character.
fn dice_window(
    bytes: &[u8],
    lines: &[Range<usize>],
    width: usize,
    needle: &mut Bigrams,
) -> Option<(u8, Range<usize>)> {
    // Dice cannot pass when one side has far more bigrams than the other.
    let (own, other) = (needle.total, width + 1);
    if 200 * own.min(other) < MIN_SIMILARITY * (own + other) {
        return None;
    }
    let score = needle.compare(bytes, lines).dice / 10_000;
    if score < MIN_SIMILARITY {
        return None;
    }
    let visible = |byte: &u8| *byte != b' ' && *byte != b'\t';
    let start = lines.iter().find_map(|line| {
        let offset = bytes[line.clone()].iter().position(visible)?;
        Some(line.start + offset)
    })?;
    let end = lines.iter().rev().find_map(|line| {
        let offset = bytes[line.clone()].iter().rposition(visible)?;
        Some(line.start + offset + 1)
    })?;
    Some((u8::try_from(score).ok()?, start..end))
}

/// Byte-bigram multiset of squeezed text, padded with a line break at each end
/// so a one-byte line still has a bigram. Comparisons squeeze their text on the
/// fly and reuse fixed tables; a presence bitmap skips absent bigrams cheaply.
struct Bigrams {
    counts: Vec<u32>,
    present: Vec<u64>,
    total: usize,
    /// Per bigram, the comparison that last used it and how many it matched.
    used: Vec<(u32, u32)>,
    generation: u32,
}

/// One comparison: bigrams shared (multiset intersection), their Dice
/// coefficient in millionths, the compared text's squeezed width in bytes, and
/// an FNV-1a fingerprint of its squeezed bytes.
struct Tally {
    shared: usize,
    dice: usize,
    width: usize,
    fingerprint: u64,
}

impl Bigrams {
    fn new(squeezed: &[u8]) -> Self {
        let mut counts = vec![0; 1 << 16];
        let mut present = vec![0u64; 1 << 10];
        let mut previous = b'\n';
        for &byte in squeezed.iter().chain(b"\n") {
            let key = (usize::from(previous) << 8) | usize::from(byte);
            counts[key] += 1;
            present[key >> 6] |= 1 << (key & 63);
            previous = byte;
        }
        Self {
            counts,
            present,
            total: squeezed.len() + 1,
            used: vec![(0, 0); 1 << 16],
            generation: 0,
        }
    }

    /// Compares the squeezed `lines` of `text`, joined by line breaks.
    fn compare(&mut self, text: &[u8], lines: &[Range<usize>]) -> Tally {
        let Self {
            counts,
            present,
            total: own,
            used,
            generation,
        } = self;
        *generation += 1;
        let generation = *generation;
        let (mut previous, mut shared, mut total) = (b'\n', 0, 0);
        let mut fingerprint = 0xcbf2_9ce4_8422_2325_u64;
        let mut visit = |byte: u8| {
            fingerprint = (fingerprint ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
            let key = (usize::from(previous) << 8) | usize::from(byte);
            previous = byte;
            total += 1;
            if present[key >> 6] & (1 << (key & 63)) != 0 {
                let entry = &mut used[key];
                if entry.0 != generation {
                    *entry = (generation, 0);
                }
                if entry.1 < counts[key] {
                    entry.1 += 1;
                    shared += 1;
                }
            }
        };
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                visit(b'\n');
            }
            // Squeezes exactly as `squeeze_into` does.
            let (mut started, mut space) = (false, false);
            for &byte in &text[line.clone()] {
                if byte == b' ' || byte == b'\t' {
                    space = started;
                    continue;
                }
                if space {
                    visit(b' ');
                    space = false;
                }
                visit(byte);
                started = true;
            }
        }
        visit(b'\n');
        Tally {
            shared,
            // At most two million, computed without overflowing a 32-bit usize.
            dice: (2_000_000 * shared as u64 / (*own + total) as u64) as usize,
            width: total - 1,
            fingerprint,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(kind: CandidateKind, line: usize, end_line: usize, text: bool) -> Candidate {
        Candidate {
            kind,
            line,
            end_line,
            text: text.then(String::new),
            similarity: (kind == CandidateKind::Similar).then_some(100),
        }
    }

    #[test]
    fn every_message_fits_what_clients_display() {
        let line = 16_777_216;
        let actual = "\u{0}".repeat(100);
        for kind in [
            CandidateKind::Exact,
            CandidateKind::Whitespace,
            CandidateKind::Similar,
        ] {
            for count in 1..=MAX_CANDIDATES {
                for text in [false, true] {
                    let candidates = vec![candidate(kind, line - 1, line, text); count];
                    for message in [
                        not_found(usize::MAX, &candidates),
                        mismatch(&actual, &candidates),
                    ] {
                        assert!(message.chars().count() <= MESSAGE_CHARS, "{message}");
                    }
                }
            }
        }
        assert!(mismatch(&actual, &[]).chars().count() <= MESSAGE_CHARS);
        // Ordinary advice leaves the quote its full width.
        let message = mismatch(
            &"x".repeat(100),
            &[candidate(CandidateKind::Exact, 3, 3, true)],
        );
        assert!(message.contains(&format!("\"{}\"…", "x".repeat(QUOTED_CHARS))));
    }

    #[test]
    fn searches_stop_once_the_request_budget_is_spent() {
        let (text, needle) = ("\tlet x = 1;\n", "    let x = 1;");
        let cost = text.len() + needle.len();
        let mut budget = 2 * cost - 1;
        assert_eq!(
            find(&mut budget, text, 0..text.len(), needle, false).len(),
            1
        );
        assert_eq!(budget, cost - 1);
        assert!(find(&mut budget, text, 0..text.len(), needle, false).is_empty());
        assert_eq!(budget, cost - 1);
    }

    #[test]
    fn quoted_text_escapes_onto_one_line_and_clips_whole_escapes() {
        assert_eq!(quoted("\tsay \"hi\"\r\n", 60), r#""\tsay \"hi\"\r\n""#);
        assert_eq!(quoted("it's", 60), "\"it's\"");
        assert_eq!(quoted("abc\u{0}", 4), "\"abc\"…");
        assert_eq!(quoted("…", 1), "\"…\"");
    }

    #[test]
    fn alignment_finds_the_closest_substring() {
        let chars = |text: &str| text.chars().collect::<Vec<_>>();
        let align = |pattern: &str, window: &[char], limit, shape| {
            align(&chars(pattern), window, 0..window.len(), limit, shape)
        };
        let free = Shape::of("retries = 2");
        let (edits, span) = align("retries = 2", &chars("let retries = 3;"), 9, free).unwrap();
        assert_eq!((edits, span), (1, 4..15));
        assert!(align("abcdefg", &chars("zzzzzzz"), 3, free).is_none());
        // A leading word is never dropped by starting just after another word,
        // and an indented needle starts at a line.
        let window = chars("fn a() {\none();\ninserted();\ntwo();");
        assert!(align("one();\ntwo();", &window, 5, free).is_none());
        let indented = Shape::of("    one();\n    two();");
        let (edits, span) = align("one();\ntwo();", &window, 9, indented).unwrap();
        let text: String = window[span].iter().collect();
        assert_eq!((edits, text.as_str()), (6, "inserted();\ntwo();"));
        let (_, span) = align("(x, y)", &chars("call(x, z);"), 1, free).unwrap();
        assert_eq!(span, 4..10);
        // The substring must pass through the given range.
        let window = chars("let a = 12;\nlet a = 2;");
        let (_, span) = super::align(&chars("let a = 1;"), &window, 12..22, 3, free).unwrap();
        assert_eq!(window[span].iter().collect::<String>(), "let a = 2;");
    }
}
