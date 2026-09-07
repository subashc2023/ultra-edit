use crate::compiler::{line_ranges, occurrences};
use crate::model::{Error, RangeRead, SearchMatch, SearchResult, Snapshot, Span, digest, new_id};

const MAX_RANGE_LINES: usize = 200;
const MAX_RANGE_CHARS: usize = 6_000;
const MAX_QUERY_CHARS: usize = 1_000;
const MAX_MATCHES: usize = 20;
const CONTEXT_CHARS: usize = 80;

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
    if last - first >= MAX_RANGE_LINES {
        return Err(Error::new(
            "READ_TOO_LARGE",
            "A focused read supports at most 200 lines; choose a smaller range",
        ));
    }
    let mut total_lines = 0;
    let mut spans = Vec::new();
    for (index, range) in line_ranges(&text).enumerate() {
        total_lines = index + 1;
        if (first..=last).contains(&total_lines) {
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
) -> Result<(Snapshot, SearchResult), Error> {
    if query.is_empty() || query.chars().take(MAX_QUERY_CHARS + 1).count() > MAX_QUERY_CHARS {
        return Err(Error::new(
            "INVALID_QUERY",
            "Literal search requires 1..1000 Unicode characters",
        ));
    }
    let (total_matches, positions) = occurrences(&text, query, MAX_MATCHES);
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
                    id: format!("m{}", index + 1),
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
        total_matches,
        omitted_matches: total_matches - matches.len(),
        matches,
    };
    Ok((snapshot, result))
}
