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
#[derive(Serialize)]
pub struct FullRead {
    pub snapshot: String,
    pub path: String,
    pub digest: String,
    pub text: String,
    pub spans: Vec<Span>,
}

impl From<Snapshot> for FullRead {
    fn from(snapshot: Snapshot) -> Self {
        Self {
            snapshot: snapshot.id,
            path: snapshot.path,
            digest: snapshot.digest,
            text: snapshot.text,
            spans: snapshot.spans,
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
    pub total_lines: usize,
    pub total_bytes: usize,
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub spans: Vec<Span>,
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
    pub next_offset: Option<usize>,
    pub total_matches: usize,
    pub omitted_matches: usize,
    pub matches: Vec<SearchMatch>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditRequest {
    pub request_id: String,
    pub files: Vec<FileRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileRequest {
    pub base: String,
    pub changes: Vec<Change>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub id: String,
    pub target: Target,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Exact {
        /// Literal original text; exactly one occurrence is required (including overlaps).
        old: String,
        /// A disclosed span ID, not source text; omit to search the entire stored file.
        scope: Option<String>,
    },
    All {
        old: String,
        /// A disclosed span ID such as selection, r0, or a returned match ID; never source text.
        scope: String,
        /// Required number of non-overlapping occurrences, counted left to right.
        expected: usize,
    },
    Span {
        /// A span ID disclosed by this exact base snapshot; never infer it from another read.
        span: String,
        /// Optional literal guard: selected original bytes must equal this text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
    },
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
        }
    }
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
    pub reference: String,
    pub ready: bool,
    pub diagnostics: Vec<Diagnostic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Diagnostic>,
    pub report: String,
    pub receipt: Option<Receipt>,
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
