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
pub struct EditRequest {
    pub request_id: String,
    pub files: Vec<FileRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRequest {
    pub base: String,
    pub changes: Vec<Change>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub id: String,
    pub target: Target,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}{}", uuid::Uuid::new_v4().simple())
}

pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
