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
        return Err(Error::new(
            "READ_TOO_LARGE",
            format!(
                "Full reads support at most {MAX_FULL_BYTES} source bytes and {MAX_FULL_LINES} lines by default; this file has {} bytes. Read a line range or search for an exact span, or deliberately request the complete response with expected_bytes={}",
                text.len(),
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
        return Err(Error::new(
            "READ_TOO_LARGE",
            "A focused read supports at most 6000 source characters; choose a smaller range, search for an exact span, or use a full read",
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
        spans: snapshot.spans.clone(),
    };
    Ok((snapshot, view))
}

pub(crate) fn search(
    path: String,
    text: String,
    query: &str,
    offset: usize,
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
    // omitted matches do not grant a whole-file or line reference.
    let snapshot = Snapshot {
        id: new_id("s"),
        path,
        digest: digest(text.as_bytes()),
        text,
        spans: matches.iter().map(|hit| hit.span.clone()).collect(),
    };
    let result = SearchResult {
        snapshot: snapshot.id.clone(),
        path: snapshot.path.clone(),
        digest: snapshot.digest.clone(),
        query: query.into(),
        offset,
        next_offset: (offset + matches.len() < total_matches).then_some(offset + matches.len()),
        total_matches,
        omitted_matches: total_matches - matches.len(),
        matches,
    };
    Ok((snapshot, result))
}
