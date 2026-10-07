use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub id: String,
    pub path: String,
    pub digest: String,
    pub text: String,
    pub spans: Vec<Span>,
}

/// Public read response; stored snapshots retain `id` for journal compatibility.
/// Disclosed references are summarized and listed by line; byte offsets remain
/// internal and are retrieved only as full snapshot evidence.
#[derive(Serialize)]
pub struct FullRead {
    pub snapshot: String,
    pub path: String,
    pub digest: String,
    pub text: String,
    pub spans: Vec<String>,
    pub lines: Vec<String>,
}

impl From<Snapshot> for FullRead {
    fn from(snapshot: Snapshot) -> Self {
        Self {
            spans: crate::reading::span_summary(&snapshot.spans),
            lines: crate::reading::line_listing(&snapshot.spans, &snapshot.text),
            snapshot: snapshot.id,
            path: snapshot.path,
            digest: snapshot.digest,
            text: snapshot.text,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub id: String,
    pub start: usize,
    pub end: usize,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangeRead {
    pub snapshot: String,
    pub path: String,
    pub digest: String,
    /// True when the continued snapshot no longer matches the current file, so
    /// an edit against it is rejected with `STALE_SNAPSHOT`.
    pub stale: bool,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub start: usize,
    pub end: usize,
    pub text: String,
    /// Every reference ID the snapshot discloses, including those a continued
    /// snapshot retained, with consecutive line IDs collapsed to ranges.
    pub spans: Vec<String>,
    /// Each line of this range as `"{id} | {body}"`.
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchMatch {
    pub span: Span,
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchResult {
    pub snapshot: String,
    pub path: String,
    pub digest: String,
    pub query: String,
    pub offset: usize,
    /// True when the continued snapshot no longer matches the current file, so
    /// an edit against it is rejected with `STALE_SNAPSHOT`.
    pub stale: bool,
    pub next_offset: Option<usize>,
    pub total_matches: usize,
    pub omitted_matches: usize,
    pub matches: Vec<SearchMatch>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditRequest {
    /// Omit to derive it from the files.
    #[serde(default)]
    pub request_id: String,
    pub files: Vec<FileRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileRequest {
    /// A file inside the workspace, edited like native Edit: the server reads its current
    /// bytes under the workspace lock as the base. Give `path` or `base`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A snapshot ID from ultra_edit_snapshot, needed for span targets. Give `path` or `base`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base: String,
    pub changes: Vec<Change>,
}

/// One change in its canonical form, which is how it is stored, serialized, and
/// hashed into a derived request ID. It is read from [`ChangeInput`], so callers may
/// write the shorthand (`old`/`new`, `span`, `lines`, `after`) or this verbose form,
/// and both spellings of one change derive the same ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ChangeInput")]
pub struct Change {
    /// Unique per request; omit for its 1-based "{file}.{change}" position, e.g. "1.2".
    pub id: String,
    pub target: Target,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Exact {
        /// Literal original text; exactly one occurrence is required (including overlaps).
        old: String,
        /// A disclosed span ID, not source text; omit to search the entire stored file.
        scope: Option<String>,
        /// Whole lines `[first, last]` to search instead of a span scope.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<[usize; 2]>,
    },
    All {
        old: String,
        /// A disclosed span ID such as selection, r0, or a returned match ID; never
        /// source text. Omitted, the count guards a search of the whole stored file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        /// Required number of non-overlapping occurrences, counted left to right.
        expected: usize,
        /// Whole lines `[first, last]` to search instead of a span scope.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<[usize; 2]>,
    },
    Span {
        /// A span ID disclosed by this exact base snapshot; never infer it from another read.
        span: String,
        /// Optional literal guard: selected original bytes must equal this text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
    },
    /// Whole lines `[first, last]`, terminators included. Text lacking a final line
    /// feed inherits the last line's terminator; empty text deletes the lines.
    Lines {
        lines: [usize; 2],
        /// The first lines of the range, compared line by line without line endings.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
    },
    /// Whole lines inserted after line `after`; 0 inserts before line 1.
    Insert {
        after: usize,
        /// The lines ending at `after`, compared line by line without line endings.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
    },
}

/// A change as callers write it. Exactly one of `old`, `span`, `lines`, or `after`
/// picks the target, or a verbose `target` gives it in canonical form; `text` is an
/// alias of `new`. Fields that are not plain strings are read as JSON values so a
/// mistake gets a message that teaches the accepted form.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeInput {
    #[serde(default)]
    id: String,
    old: Option<String>,
    new: Option<String>,
    text: Option<String>,
    count: Option<serde_json::Value>,
    #[serde(rename = "in")]
    within: Option<serde_json::Value>,
    span: Option<String>,
    lines: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
    expect: Option<String>,
    target: Option<serde_json::Value>,
}

impl TryFrom<ChangeInput> for Change {
    type Error = String;

    fn try_from(input: ChangeInput) -> Result<Self, String> {
        let text = match (input.new, input.text) {
            (Some(_), Some(_)) => return Err("give `new` or `text`, not both".into()),
            (Some(text), None) | (None, Some(text)) => Some(text),
            (None, None) => None,
        };
        let selectors: Vec<&str> = [
            ("old", input.old.is_some()),
            ("span", input.span.is_some()),
            ("lines", input.lines.is_some()),
            ("after", input.after.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, given)| given.then_some(name))
        .collect();
        if let Some(target) = input.target {
            let shorthand = !selectors.is_empty()
                || input.within.is_some()
                || input.count.is_some()
                || input.expect.is_some();
            if shorthand {
                return Err("change mixes verbose `target` with shorthand fields (old/span/lines/after/in/count/expect); use one form".into());
            }
            let target = serde_json::from_value(target)
                .map_err(|error| format!("invalid `target`: {error}"))?;
            let text = text.ok_or("verbose change needs `text` (the replacement; \"\" deletes)")?;
            return Ok(Self {
                id: input.id,
                target,
                text,
            });
        }
        let text = text.ok_or("change needs `new` (the replacement text; \"\" deletes)")?;
        match selectors.as_slice() {
            [_] => {}
            [] => {
                return Err(
                    "change needs one of old, span, lines, or after (or a verbose `target`)".into(),
                );
            }
            [first, second, ..] => {
                return Err(format!(
                    "give exactly one of old, span, lines, or after (found {first} and {second})"
                ));
            }
        }
        if input.old.is_none() {
            if input.count.is_some() {
                return Err("`count` is a positive occurrence count and needs `old`".into());
            }
            if input.within.is_some() {
                return Err(
                    "`in` restricts `old`; it does not apply to span, lines, or after".into(),
                );
            }
        } else if input.expect.is_some() {
            return Err("`expect` guards span, lines, or after; with `old`, the old text is already the guard".into());
        }
        let target = if let Some(old) = input.old {
            let (scope, lines) = match input.within {
                Some(within) => scope_input(within)?,
                None => (None, None),
            };
            match input.count {
                Some(count) => Target::All {
                    old,
                    scope,
                    expected: count_input(&count)?,
                    lines,
                },
                None => Target::Exact { old, scope, lines },
            }
        } else if let Some(span) = input.span {
            Target::Span {
                span,
                expect: input.expect,
            }
        } else if let Some(lines) = input.lines {
            Target::Lines {
                lines: lines_input(&lines)?,
                expect: input.expect,
            }
        } else {
            let after = input.after.as_ref().and_then(serde_json::Value::as_u64);
            Target::Insert {
                after: after
                    .and_then(|after| usize::try_from(after).ok())
                    .ok_or("after takes a line number such as 12; 0 inserts before line 1")?,
                expect: input.expect,
            }
        };
        Ok(Self {
            id: input.id,
            target,
            text,
        })
    }
}

const LINES_FORM: &str = "lines must be [first,last] with 1 <= first <= last, e.g. [146,150]";

/// Reads `lines` given as `[a, b]`, `a`, `"a"`, or `"a-b"`; strings are canonicalized
/// to the pair, so every spelling derives the same request ID.
fn lines_input(value: &serde_json::Value) -> Result<[usize; 2], String> {
    if let Some(text) = value.as_str()
        && text.trim_start().starts_with(['r', 'R'])
    {
        return Err(
            "lines takes numbers like [146,150]; span IDs such as r146..r150 go in `span`".into(),
        );
    }
    line_pair(value).ok_or_else(|| LINES_FORM.into())
}

fn line_pair(value: &serde_json::Value) -> Option<[usize; 2]> {
    let number = |value: &serde_json::Value| usize::try_from(value.as_u64()?).ok();
    let [first, last] = match value {
        serde_json::Value::Number(_) => [number(value)?; 2],
        serde_json::Value::Array(pair) => match pair.as_slice() {
            [first, last] => [number(first)?, number(last)?],
            _ => return None,
        },
        serde_json::Value::String(text) => {
            let parse = |digits: &str| {
                let digits = digits.trim();
                (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
                    .then(|| digits.parse().ok())
                    .flatten()
            };
            match text.split_once('-') {
                Some((first, last)) => [parse(first)?, parse(last)?],
                None => [parse(text)?; 2],
            }
        }
        _ => return None,
    };
    (1 <= first && first <= last).then_some([first, last])
}

/// Reads `in`: a span ID string, or lines in any form `lines` accepts.
fn scope_input(value: serde_json::Value) -> Result<(Option<String>, Option<[usize; 2]>), String> {
    let numeric = match &value {
        serde_json::Value::String(text) => text.trim().starts_with(|ch: char| ch.is_ascii_digit()),
        serde_json::Value::Number(_) | serde_json::Value::Array(_) => true,
        _ => false,
    };
    match value {
        serde_json::Value::String(span) if !numeric => Ok((Some(span), None)),
        _ if numeric => Ok((
            None,
            Some(line_pair(&value).ok_or(format!("`in` {LINES_FORM}"))?),
        )),
        _ => Err("`in` takes a span ID such as \"r12\" or lines like [72,87]".into()),
    }
}

fn count_input(value: &serde_json::Value) -> Result<usize, String> {
    match value.as_u64().map(usize::try_from) {
        Some(Ok(0)) => Err("`count` must be at least 1: how many times `old` occurs".into()),
        Some(Ok(count)) => Ok(count),
        _ => Err("`count` takes a whole number: how many times `old` occurs".into()),
    }
}

/// Describes the accepted change forms. The verbose `target` stays opaque, so the
/// schema a client pays for on every call lists only the shorthand.
impl schemars::JsonSchema for Change {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Change".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let line = serde_json::json!({"type": "integer", "minimum": 1});
        schemars::json_schema!({
            "type": "object",
            "description": "One of: {old,new} (add count:N to replace N occurrences, in to restrict the search); {span,new}; {lines:[a,b],new} replacing whole lines (\"\" deletes); {after:n,new} inserting lines. Text is literal.",
            "properties": {
                "id": {"type": "string", "description": "Unique per request; omit for its 1-based \"{file}.{change}\" position, e.g. \"1.2\"."},
                "old": {"type": "string", "description": "Literal original text; exactly one occurrence unless count is given."},
                "new": {"type": "string", "description": "Replacement text, written literally."},
                "count": {"type": "integer", "minimum": 1, "description": "Replace all occurrences of old, which must number exactly this many."},
                "in": {
                    "description": "Restricts old to a disclosed span ID or whole lines [first,last].",
                    "anyOf": [{"type": "string"}, {"type": "array", "items": line, "minItems": 2, "maxItems": 2}]
                },
                "span": {"type": "string", "description": "A span ID disclosed by this base, e.g. r12 or r12..r18."},
                "lines": {"type": "array", "items": line, "minItems": 2, "maxItems": 2, "description": "Whole lines [first,last] to replace, terminators included."},
                "after": {"type": "integer", "minimum": 0, "description": "Insert whole lines after this line; 0 inserts at the top."},
                "expect": {"type": "string", "description": "Guard for span (exact bytes) or lines/after (the first lines of the range, or those ending at after)."},
                "text": {"type": "string", "description": "Alias of new."},
                "target": {"type": "object", "description": "Verbose canonical target; see the reference."}
            }
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    /// Canonical filesystem path when the diagnostic identifies a file.
    pub file: Option<String>,
    pub change_id: Option<String>,
    pub code: String,
    pub message: String,
    pub expected: Option<usize>,
    pub actual: Option<usize>,
    pub conflicts: Vec<String>,
    /// Closest current regions for a target that matched nowhere, at most three.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<Candidate>,
}

impl Diagnostic {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            file: None,
            change_id: None,
            code: code.into(),
            message: message.into(),
            expected: None,
            actual: None,
            conflicts: Vec::new(),
            candidates: Vec::new(),
        }
    }
}

/// A current region that nearly matches a failed target. Lines are one-based
/// and inclusive, numbered like focused reads; byte offsets stay internal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub kind: CandidateKind,
    pub line: usize,
    pub end_line: usize,
    /// Exact current bytes; absent above 2,000 characters rather than clipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Percentage score, present only for `similar` candidates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub similarity: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    /// The unmet expectation occurs literally elsewhere.
    Exact,
    /// Equal after line-ending and space/tab normalization.
    Whitespace,
    /// Close by edit distance on whitespace-squeezed text.
    Similar,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replacement {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub change_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedFile {
    pub base: Snapshot,
    pub output: String,
    pub replacements: Vec<Replacement>,
    pub change_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedPlan {
    pub id: String,
    pub request: EditRequest,
    pub files: Vec<PreparedFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub id: String,
    pub request: EditRequest,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preparation {
    /// The bound request ID, including one derived from an omitted ID.
    pub request_id: String,
    pub reference: String,
    pub ready: bool,
    pub diagnostics: Vec<Diagnostic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Diagnostic>,
    pub report: String,
    pub receipt: Option<Receipt>,
    /// The request was already bound and this call attempted nothing new.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replayed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitStatus {
    Committed,
    NotCommitted,
    Partial,
    OutcomeUnknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Committed,
    NotCommitted,
    OutcomeUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileOutcome {
    pub path: String,
    pub before: String,
    #[serde(alias = "after_digest")]
    pub intended_digest: String,
    /// Snapshot of the bytes actually written, without disclosed spans. Committed files only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    pub status: FileStatus,
    pub changes_applied: usize,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub request_id: String,
    pub plan_id: String,
    pub commit: CommitStatus,
    pub files: Vec<FileOutcome>,
    pub validation: String,
    pub undo: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "encoding", rename_all = "snake_case", deny_unknown_fields)]
pub enum ByteContent {
    Utf8 { text: String },
    Binary { bytes: Vec<u8> },
}

impl ByteContent {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Utf8 { text } => text.as_bytes(),
            Self::Binary { bytes } => bytes,
        }
    }
}

impl From<Vec<u8>> for ByteContent {
    fn from(bytes: Vec<u8>) -> Self {
        match String::from_utf8(bytes) {
            Ok(text) => Self::Utf8 { text },
            Err(error) => Self::Binary {
                bytes: error.into_bytes(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservedState {
    File {
        digest: String,
        content: ByteContent,
    },
    Missing,
    Unavailable {
        code: String,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetObservation {
    pub path: String,
    pub state: ObservedState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inspection {
    pub id: String,
    pub plan: PreparedPlan,
    pub journal: ByteContent,
    pub journal_digest: String,
    pub receipt: Option<Receipt>,
    pub receipt_error: Option<Diagnostic>,
    pub files: Vec<TargetObservation>,
    pub reconciliation: Option<Reconciliation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationDecision {
    AcceptCurrent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationRequest {
    pub inspection: String,
    pub decision: ReconciliationDecision,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reconciliation {
    pub plan_id: String,
    pub plan_digest: String,
    pub journal_digest: String,
    pub request: ReconciliationRequest,
}

#[derive(Debug)]
pub struct Error {
    pub code: String,
    pub message: String,
    pub request_id: Option<String>,
    pub plan_id: Option<String>,
    pub commit: Option<CommitStatus>,
}

impl Error {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            request_id: None,
            plan_id: None,
            commit: None,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new("IO_ERROR", error.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::new("INVALID_JSON", error.to_string())
    }
}

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}{}", uuid::Uuid::new_v4().simple())
}

pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn parse(value: Value) -> Result<Change, String> {
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    fn change(target: Target, text: &str) -> Change {
        Change {
            id: String::new(),
            target,
            text: text.into(),
        }
    }

    #[test]
    fn each_shorthand_maps_to_its_canonical_change_and_serialization() {
        let exact = |scope: Option<&str>, lines| Target::Exact {
            old: "a".into(),
            scope: scope.map(str::to_owned),
            lines,
        };
        for (input, target, canonical) in [
            (
                json!({"old":"a","new":"b"}),
                exact(None, None),
                json!({"kind":"exact","old":"a","scope":null}),
            ),
            (
                json!({"old":"a","new":"b","in":"r5"}),
                exact(Some("r5"), None),
                json!({"kind":"exact","old":"a","scope":"r5"}),
            ),
            (
                json!({"old":"a","new":"b","in":[72,87]}),
                exact(None, Some([72, 87])),
                json!({"kind":"exact","old":"a","scope":null,"lines":[72,87]}),
            ),
            (
                json!({"old":"a","new":"b","count":3}),
                Target::All {
                    old: "a".into(),
                    scope: None,
                    expected: 3,
                    lines: None,
                },
                json!({"kind":"all","old":"a","expected":3}),
            ),
            (
                json!({"old":"a","new":"b","count":2,"in":"selection"}),
                Target::All {
                    old: "a".into(),
                    scope: Some("selection".into()),
                    expected: 2,
                    lines: None,
                },
                json!({"kind":"all","old":"a","scope":"selection","expected":2}),
            ),
            (
                json!({"span":"r2..r4","new":"b","expect":"x"}),
                Target::Span {
                    span: "r2..r4".into(),
                    expect: Some("x".into()),
                },
                json!({"kind":"span","span":"r2..r4","expect":"x"}),
            ),
            (
                json!({"lines":[146,150],"new":"b","expect":"x"}),
                Target::Lines {
                    lines: [146, 150],
                    expect: Some("x".into()),
                },
                json!({"kind":"lines","lines":[146,150],"expect":"x"}),
            ),
            (
                json!({"after":0,"text":"b"}),
                Target::Insert {
                    after: 0,
                    expect: None,
                },
                json!({"kind":"insert","after":0}),
            ),
        ] {
            let parsed = parse(input.clone()).unwrap();
            assert_eq!(parsed, change(target, "b"), "{input}");
            assert_eq!(
                serde_json::to_value(&parsed).unwrap(),
                json!({"id":"","target":canonical,"text":"b"}),
                "{input}"
            );
            // The canonical form reads back as itself.
            let verbose = serde_json::to_value(&parsed).unwrap();
            assert_eq!(parse(verbose).unwrap(), parsed);
        }
    }

    #[test]
    fn every_spelling_of_lines_is_canonicalized() {
        for lines in [
            json!([7, 7]),
            json!(7),
            json!("7"),
            json!("7-7"),
            json!(" 7 - 7 "),
        ] {
            let parsed = parse(json!({"lines":lines,"new":""})).unwrap();
            assert_eq!(
                parsed.target,
                Target::Lines {
                    lines: [7, 7],
                    expect: None
                },
                "{lines}"
            );
        }
        let parsed = parse(json!({"old":"a","new":"b","in":"12-14"})).unwrap();
        assert_eq!(
            parsed.target,
            Target::Exact {
                old: "a".into(),
                scope: None,
                lines: Some([12, 14])
            }
        );
    }

    #[test]
    fn malformed_changes_are_rejected_with_the_accepted_form() {
        for (input, message) in [
            (
                json!({"target":{"kind":"exact","old":"a"},"old":"a","text":"b"}),
                "change mixes verbose `target` with shorthand fields (old/span/lines/after/in/count/expect); use one form",
            ),
            (
                json!({"target":{"kind":"exact","old":"a"}}),
                "verbose change needs `text` (the replacement; \"\" deletes)",
            ),
            (
                json!({"old":"a"}),
                "change needs `new` (the replacement text; \"\" deletes)",
            ),
            (
                json!({"old":"a","new":"b","text":"c"}),
                "give `new` or `text`, not both",
            ),
            (
                json!({"new":"b"}),
                "change needs one of old, span, lines, or after (or a verbose `target`)",
            ),
            (
                json!({"old":"a","lines":[1,2],"new":"b"}),
                "give exactly one of old, span, lines, or after (found old and lines)",
            ),
            (
                json!({"span":"r1","count":2,"new":"b"}),
                "`count` is a positive occurrence count and needs `old`",
            ),
            (
                json!({"old":"a","count":0,"new":"b"}),
                "`count` must be at least 1: how many times `old` occurs",
            ),
            (
                json!({"old":"a","count":"all","new":"b"}),
                "`count` takes a whole number: how many times `old` occurs",
            ),
            (
                json!({"lines":[1,2],"in":"r1","new":"b"}),
                "`in` restricts `old`; it does not apply to span, lines, or after",
            ),
            (
                json!({"old":"a","expect":"a","new":"b"}),
                "`expect` guards span, lines, or after; with `old`, the old text is already the guard",
            ),
            (
                json!({"lines":[5,3],"new":"b"}),
                "lines must be [first,last] with 1 <= first <= last, e.g. [146,150]",
            ),
            (
                json!({"lines":0,"new":"b"}),
                "lines must be [first,last] with 1 <= first <= last, e.g. [146,150]",
            ),
            (
                json!({"lines":[1,2,3],"new":"b"}),
                "lines must be [first,last] with 1 <= first <= last, e.g. [146,150]",
            ),
            (
                json!({"lines":"r146..r150","new":"b"}),
                "lines takes numbers like [146,150]; span IDs such as r146..r150 go in `span`",
            ),
            (
                json!({"old":"a","in":[0,4],"new":"b"}),
                "`in` lines must be [first,last] with 1 <= first <= last, e.g. [146,150]",
            ),
            (
                json!({"old":"a","in":true,"new":"b"}),
                "`in` takes a span ID such as \"r12\" or lines like [72,87]",
            ),
            (
                json!({"after":-1,"new":"b"}),
                "after takes a line number such as 12; 0 inserts before line 1",
            ),
            (
                json!({"after":"12","new":"b"}),
                "after takes a line number such as 12; 0 inserts before line 1",
            ),
        ] {
            assert_eq!(parse(input.clone()).unwrap_err(), message, "{input}");
        }
        let target = parse(json!({"target":{"kind":"lines","lines":"3"},"text":"b"})).unwrap_err();
        assert!(target.starts_with("invalid `target`: "), "{target}");
        // Unknown keys list every accepted field.
        let unknown = parse(json!({"old":"a","new":"b","replace_all":true})).unwrap_err();
        assert!(
            unknown.starts_with("unknown field `replace_all`, expected one of"),
            "{unknown}"
        );
        for field in [
            "id", "old", "new", "text", "count", "in", "span", "lines", "after", "expect", "target",
        ] {
            assert!(unknown.contains(&format!("`{field}`")), "{unknown}");
        }
    }

    #[test]
    fn the_schema_lists_every_shorthand_field_and_requires_none() {
        let schema = serde_json::to_value(schemars::schema_for!(Change)).unwrap();
        let properties = schema["properties"].as_object().unwrap();
        let mut fields: Vec<_> = properties.keys().map(String::as_str).collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            [
                "after", "count", "expect", "id", "in", "lines", "new", "old", "span", "target",
                "text"
            ]
        );
        assert!(schema.get("required").is_none());
        assert_eq!(schema["properties"]["count"]["minimum"], 1);
    }
}
