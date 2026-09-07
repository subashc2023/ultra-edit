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
        old: String,
        scope: Option<String>,
    },
    All {
        old: String,
        scope: String,
        expected: usize,
    },
    Span {
        span: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
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
    pub after_digest: String,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationDecision {
    AcceptCurrent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
