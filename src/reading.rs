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

/// `retained` carries the references a continued snapshot already disclosed, so
/// one request can address distant ranges of the same immutable source.
pub(crate) fn read_range(
    path: String,
    text: String,
    first: usize,
    last: usize,
    mut retained: Vec<Span>,
) -> Result<(Snapshot, RangeRead), Error> {
    if first == 0 || first > last {
        return Err(Error::new(
            "INVALID_LINE_RANGE",
            "Line numbers must be positive and the first must not exceed the last",
        ));
    }
    let too_many_lines = last - first >= MAX_RANGE_LINES;
    let mut total_lines = 0;
    let mut disclosed = Vec::new();
    for (index, range) in line_ranges(&text).enumerate() {
        total_lines = index + 1;
        if !too_many_lines && (first..=last).contains(&total_lines) {
            disclosed.push(Span {
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
    let start = disclosed[0].start;
    let end = disclosed[disclosed.len() - 1].end;
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
    let lines = line_listing(&disclosed, &text);
    // `selection` always names the most recent range; a retained one would be
    // ambiguous. The line references it covered remain.
    retained.retain(|span| span.id != "selection");
    let mut spans = disclose(retained, disclosed);
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
        stale: false,
        total_lines,
        total_bytes: snapshot.text.len(),
        start,
        end,
        text: snapshot.text[start..end].into(),
        spans: span_summary(&snapshot.spans),
        lines,
    };
    Ok((snapshot, view))
}

/// Appends newly disclosed spans to the references a continued snapshot retains.
/// Re-reading a range or page must not duplicate a reference it already has.
fn disclose(mut spans: Vec<Span>, disclosed: impl IntoIterator<Item = Span>) -> Vec<Span> {
    for span in disclosed {
        if !spans
            .iter()
            .any(|known| (&known.id, known.start, known.end) == (&span.id, span.start, span.end))
        {
            spans.push(span);
        }
    }
    spans
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

/// Characters of a disclosure list in a diagnostic; longer lists end in `…`.
const MAX_DISCLOSED_CHARS: usize = 100;

/// Describes what a base discloses in words a diagnostic can teach with, such as
/// `lines 146-150, 1875-1879; selection = lines 1875-1879; m1-m3`. Unlike
/// [`span_summary`], line runs read as line numbers rather than as span IDs, so the
/// list is not mistaken for an ID. Bounded to about 100 characters, so a
/// diagnostic quoting it still fits the 240 characters clients display.
pub(crate) fn disclosed_lines(spans: &[Span]) -> String {
    let mut lines: Vec<usize> = spans.iter().filter_map(|span| line_id(&span.id)).collect();
    lines.sort_unstable();
    let mut matches: Vec<usize> = spans.iter().filter_map(|span| match_id(&span.id)).collect();
    matches.sort_unstable();
    let mut groups = Vec::new();
    if spans.iter().any(|span| span.id == "r0") {
        groups.push("r0 (whole file)".to_owned());
    }
    if !lines.is_empty() {
        let noun = if lines.len() == 1 { "line" } else { "lines" };
        groups.push(format!("{noun} {}", runs(&lines, "")));
    }
    if let Some(selection) = spans.iter().find(|span| span.id == "selection") {
        // A range read discloses each line it selects, so the last line span inside
        // the selection ends it.
        let last = spans
            .iter()
            .filter(|span| span.start >= selection.start && span.end <= selection.end)
            .filter_map(|span| line_id(&span.id))
            .max()
            .unwrap_or(selection.line);
        groups.push(if last > selection.line {
            format!("selection = lines {}-{last}", selection.line)
        } else {
            format!("selection = line {}", selection.line)
        });
    }
    if !matches.is_empty() {
        groups.push(runs(&matches, "m"));
    }
    groups.extend(
        spans
            .iter()
            .filter(|span| {
                !matches!(span.id.as_str(), "r0" | "selection")
                    && line_id(&span.id).is_none()
                    && match_id(&span.id).is_none()
            })
            .map(|span| span.id.clone()),
    );
    if groups.is_empty() {
        return "no spans".into();
    }
    let list = groups.join("; ");
    if list.chars().count() <= MAX_DISCLOSED_CHARS {
        return list;
    }
    // Clip at a separator so no partial number is shown.
    let clipped: String = list.chars().take(MAX_DISCLOSED_CHARS).collect();
    let cut = clipped.rfind([',', ';']).unwrap_or(clipped.len());
    format!("{}, …", &clipped[..cut])
}

/// Collapses sorted numbers into runs such as `1-3, 7`, each number with `prefix`.
fn runs(numbers: &[usize], prefix: &str) -> String {
    let mut parts = Vec::new();
    let mut index = 0;
    while index < numbers.len() {
        let first = numbers[index];
        let mut last = first;
        while index + 1 < numbers.len() && numbers[index + 1] <= last + 1 {
            index += 1;
            last = numbers[index];
        }
        parts.push(if first == last {
            format!("{prefix}{first}")
        } else {
            format!("{prefix}{first}-{prefix}{last}")
        });
        index += 1;
    }
    parts.join(", ")
}

/// Search-match IDs are `m` and a one-based match ordinal.
fn match_id(id: &str) -> Option<usize> {
    id.strip_prefix('m')
        .filter(|ordinal| ordinal.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|ordinal| ordinal.parse().ok())
        .filter(|ordinal| *ordinal > 0)
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
pub(crate) fn line_id(id: &str) -> Option<usize> {
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
    // omitted matches do not grant a whole-file or line reference.
    let spans = disclose(retained, matches.iter().map(|hit| hit.span.clone()));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn span(id: &str, start: usize, end: usize) -> Span {
        Span {
            id: id.into(),
            start,
            end,
            line: line_id(id).unwrap_or(1),
        }
    }

    #[test]
    fn disclosure_lists_read_as_line_numbers_and_never_as_span_ranges() {
        let full = snapshot("full.txt".into(), "a\nb\nc\n".into());
        assert_eq!(disclosed_lines(&full.spans), "r0 (whole file); lines 1-3");
        let (one, _) = read_range("f.txt".into(), "a\nb\nc\n".into(), 2, 2, Vec::new()).unwrap();
        assert_eq!(disclosed_lines(&one.spans), "line 2; selection = line 2");
        let text = "a\nb\nc\nd\ne\nf\n";
        let (first, _) = read_range("f.txt".into(), text.into(), 1, 2, Vec::new()).unwrap();
        let (continued, _) = read_range("f.txt".into(), text.into(), 4, 6, first.spans).unwrap();
        assert_eq!(
            disclosed_lines(&continued.spans),
            "lines 1-2, 4-6; selection = lines 4-6"
        );
        let (searched, _) = search(
            "f.txt".into(),
            text.into(),
            "\n",
            0,
            continued.spans.clone(),
        )
        .unwrap();
        assert_eq!(
            disclosed_lines(&searched.spans),
            "lines 1-2, 4-6; selection = lines 4-6; m1-m6"
        );
        assert_eq!(
            disclosed_lines(&[span("m2", 0, 1), span("m4", 2, 3), span("odd", 0, 0)]),
            "m2, m4; odd"
        );
        assert_eq!(disclosed_lines(&[]), "no spans");
        assert!(!disclosed_lines(&continued.spans).contains(".."));
    }

    #[test]
    fn long_disclosure_lists_clip_at_a_separator() {
        let spans: Vec<_> = (1..=200)
            .map(|line| span(&format!("r{}", line * 2), 0, 0))
            .collect();
        let list = disclosed_lines(&spans);
        assert!(list.starts_with("lines 2, 4, 6,"), "{list}");
        assert!(list.ends_with(", …"), "{list}");
        assert!(list.chars().count() <= MAX_DISCLOSED_CHARS + 3, "{list}");
        // No number is cut: every entry before the ellipsis is a whole even number.
        let numbers = list.trim_start_matches("lines ").trim_end_matches(", …");
        for number in numbers.split(", ") {
            assert_eq!(number.parse::<usize>().unwrap() % 2, 0, "{list}");
        }
    }
}
