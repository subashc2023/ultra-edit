use crate::compiler::{line_ranges, occurrences_page, snapshot};
use crate::model::{Error, RangeRead, SearchMatch, SearchResult, Snapshot, Span, digest, new_id};

const MAX_RANGE_LINES: usize = 200;
const MAX_RANGE_CHARS: usize = 6_000;
const MAX_FULL_BYTES: usize = 24_000;
const MAX_FULL_LINES: usize = 400;
const MAX_QUERY_CHARS: usize = 1_000;
const MAX_MATCHES: usize = 20;
const CONTEXT_CHARS: usize = 80;

pub(crate) fn read_full(
    path: String,
    text: String,
    expected_bytes: Option<usize>,
) -> Result<Snapshot, Error> {
    if let Some(expected_bytes) = expected_bytes {
        if text.len() != expected_bytes {
            return Err(Error::new(
                "READ_SIZE_CHANGED",
                format!(
                    "Expected {expected_bytes} source bytes, found {}; read a range or search before confirming the new byte count",
                    text.len()
                ),
            ));
        }
    } else if text.len() > MAX_FULL_BYTES
        || line_ranges(&text).take(MAX_FULL_LINES + 1).count() > MAX_FULL_LINES
    {
        // Both measurements are reported so the rejected limit is never guessed.
        return Err(Error::new(
            "READ_TOO_LARGE",
            format!(
                "Full reads support at most {MAX_FULL_BYTES} source bytes and {MAX_FULL_LINES} lines by default; this file has {} bytes and {} lines. Read a line range or search for an exact span, or deliberately request the complete response with expected_bytes={}",
                text.len(),
                line_ranges(&text).count(),
                text.len()
            ),
        ));
    }
    Ok(snapshot(path, text))
}

pub(crate) fn read_range(
    path: String,
    text: String,
    first: usize,
    last: usize,
) -> Result<(Snapshot, RangeRead), Error> {
    if first == 0 || first > last {
        return Err(Error::new(
            "INVALID_LINE_RANGE",
            "Line numbers must be positive and the first must not exceed the last",
        ));
    }
    let too_many_lines = last - first >= MAX_RANGE_LINES;
    let mut total_lines = 0;
    let mut spans = Vec::new();
    for (index, range) in line_ranges(&text).enumerate() {
        total_lines = index + 1;
        if !too_many_lines && (first..=last).contains(&total_lines) {
            spans.push(Span {
                id: format!("r{total_lines}"),
                start: range.start,
                end: range.end,
                line: total_lines,
            });
        }
    }
    if last > total_lines {
        return Err(Error::new(
            "INVALID_LINE_RANGE",
            format!("File has {total_lines} line(s); requested through line {last}"),
        ));
    }
    if too_many_lines {
        return Err(Error::new(
            "READ_TOO_LARGE",
            "A focused read supports at most 200 lines; choose a smaller range",
        ));
    }
    let start = spans[0].start;
    let end = spans[spans.len() - 1].end;
    let selected = &text[start..end];
    if selected.chars().take(MAX_RANGE_CHARS + 1).count() > MAX_RANGE_CHARS {
        // A plain full read may be over its own limits; the exact byte count is not.
        return Err(Error::new(
            "READ_TOO_LARGE",
            format!(
                "A focused read supports at most {MAX_RANGE_CHARS} source characters; choose a smaller range, search for an exact span, or request the complete file with expected_bytes={}",
                text.len()
            ),
        ));
    }
    spans.push(Span {
        id: "selection".into(),
        start,
        end,
        line: first,
    });
    let snapshot = Snapshot {
        id: new_id("s"),
        path,
        digest: digest(text.as_bytes()),
        text,
        spans,
    };
    let view = RangeRead {
        snapshot: snapshot.id.clone(),
        path: snapshot.path.clone(),
        digest: snapshot.digest.clone(),
        total_lines,
        total_bytes: snapshot.text.len(),
        start,
        end,
        text: snapshot.text[start..end].into(),
        spans: span_summary(&snapshot.spans),
        lines: line_listing(&snapshot.spans, &snapshot.text),
    };
    Ok((snapshot, view))
}

/// Summarizes disclosed span IDs, collapsing consecutive line IDs into
/// `r12..r18`. Byte offsets stay server-side; full evidence still carries them.
pub(crate) fn span_summary(spans: &[Span]) -> Vec<String> {
    let mut summary = Vec::new();
    let mut run: Option<(usize, usize)> = None;
    for span in spans {
        match (line_id(&span.id), run) {
            (Some(line), Some((first, last))) if line == last + 1 => run = Some((first, line)),
            (Some(line), _) => {
                push_run(run, &mut summary);
                run = Some((line, line));
            }
            (None, _) => {
                push_run(run, &mut summary);
                run = None;
                summary.push(span.id.clone());
            }
        }
    }
    push_run(run, &mut summary);
    summary
}

/// Lists every disclosed line body as `"{id} | {body}"` so a line can be chosen
/// without counting newlines in the selected text. Whole-file `r0` is not a line.
pub(crate) fn line_listing(spans: &[Span], text: &str) -> Vec<String> {
    spans
        .iter()
        .filter(|span| line_id(&span.id).is_some())
        .map(|span| format!("{} | {}", span.id, &text[span.start..span.end]))
        .collect()
}

/// Line-body IDs are `r` and a one-based line number. `r0` covers the whole
/// file, so it never joins a line range or the listing.
fn line_id(id: &str) -> Option<usize> {
    id.strip_prefix('r')
        .and_then(|line| line.parse().ok())
        .filter(|line| *line > 0)
}

fn push_run(run: Option<(usize, usize)>, summary: &mut Vec<String>) {
    if let Some((first, last)) = run {
        summary.push(if first == last {
            format!("r{first}")
        } else {
            format!("r{first}..r{last}")
        });
    }
}

/// `retained` carries the references a continued snapshot already disclosed, so
/// one request can address every match paged through the same immutable source.
pub(crate) fn search(
    path: String,
    text: String,
    query: &str,
    offset: usize,
    retained: Vec<Span>,
) -> Result<(Snapshot, SearchResult), Error> {
    if query.is_empty() || query.chars().take(MAX_QUERY_CHARS + 1).count() > MAX_QUERY_CHARS {
        return Err(Error::new(
            "INVALID_QUERY",
            "Literal search requires 1..1000 Unicode characters",
        ));
    }
    let (total_matches, positions) = occurrences_page(&text, query, offset, MAX_MATCHES);
    if offset > total_matches {
        return Err(Error::new(
            "INVALID_SEARCH_OFFSET",
            format!(
                "Search found {total_matches} match(es); offset {offset} exceeds the match count. Start at offset 0 or use next_offset from a previous page"
            ),
        ));
    }
    let mut line = 1;
    let mut cursor = 0;
    let matches: Vec<_> = positions
        .into_iter()
        .enumerate()
        .map(|(index, start)| {
            line += text[cursor..start]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count();
            cursor = start;
            let end = start + query.len();
            let before_start = text[..start]
                .char_indices()
                .rev()
                .take_while(|(_, ch)| !matches!(ch, '\r' | '\n'))
                .take(CONTEXT_CHARS)
                .last()
                .map_or(start, |(offset, _)| offset);
            let after: String = text[end..]
                .chars()
                .take_while(|ch| !matches!(ch, '\r' | '\n'))
                .take(CONTEXT_CHARS)
                .collect();
            SearchMatch {
                span: Span {
                    id: format!("m{}", offset + index + 1),
                    start,
                    end,
                    line,
                },
                before: text[before_start..start].into(),
                after,
            }
        })
        .collect();
    // Only complete disclosed targets receive references. Context fragments and
    // omitted matches do not grant a whole-file or line reference. Re-requesting
    // a page must not duplicate a reference the retained snapshot already has.
    let mut spans = retained;
    for hit in &matches {
        if !spans.iter().any(|span| {
            (&span.id, span.start, span.end) == (&hit.span.id, hit.span.start, hit.span.end)
        }) {
            spans.push(hit.span.clone());
        }
    }
    let snapshot = Snapshot {
        id: new_id("s"),
        path,
        digest: digest(text.as_bytes()),
        text,
        spans,
    };
    let result = SearchResult {
        snapshot: snapshot.id.clone(),
        path: snapshot.path.clone(),
        digest: snapshot.digest.clone(),
        query: query.into(),
        offset,
        stale: false,
        next_offset: (offset + matches.len() < total_matches).then_some(offset + matches.len()),
        total_matches,
        omitted_matches: total_matches - matches.len(),
        matches,
    };
    Ok((snapshot, result))
}
