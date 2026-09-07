use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::compiler;
use crate::model::*;
use crate::reading;
use crate::report;
use crate::storage::Storage;

const REPORT_LINES: usize = 60;
const REPORT_CHARS: usize = 6_000;

/// Filesystem host. Each operation coordinates with other hosts using the same workspace root.
pub struct Workspace {
    storage: Storage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Input {
    Edit {
        request: EditRequest,
    },
    Repair {
        reference: String,
        request_id: String,
        changes: Vec<Change>,
    },
    Undo {
        reference: String,
        request_id: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    input: Input,
    reference: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum EditResult {
    Rejected { preparation: Preparation },
    Completed { receipt: Receipt, report: String },
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Evidence {
    Snapshot(Snapshot),
    Plan(PreparedPlan),
    Draft(Draft),
}

impl Workspace {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self {
            storage: Storage::open(root.as_ref())?,
        })
    }

    pub fn root(&self) -> &Path {
        self.storage.root()
    }

    /// Returns complete, unnormalized UTF-8 text; every returned span has been disclosed.
    pub fn read(&self, path: impl AsRef<Path>) -> Result<Snapshot, Error> {
        let _lock = self.storage.lock()?;
        let (path, text) = self.read_source(path.as_ref())?;
        let snapshot = compiler::snapshot(path, text);
        self.storage.put("snapshots", &snapshot.id, &snapshot)?;
        Ok(snapshot)
    }

    /// Discloses an inclusive line range and issues only its line-body and
    /// `selection` references. All original bytes are retained for stale checks.
    pub fn read_range(
        &self,
        path: impl AsRef<Path>,
        first: usize,
        last: usize,
    ) -> Result<RangeRead, Error> {
        let _lock = self.storage.lock()?;
        let (path, text) = self.read_source(path.as_ref())?;
        let (snapshot, view) = reading::read_range(path, text, first, last)?;
        self.storage.put("snapshots", &snapshot.id, &snapshot)?;
        Ok(view)
    }

    /// Counts overlapping literal matches and discloses up to 20 exact editable
    /// spans with bounded context. This reads a fresh immutable base each call.
    pub fn search(&self, path: impl AsRef<Path>, query: &str) -> Result<SearchResult, Error> {
        let _lock = self.storage.lock()?;
        let (path, text) = self.read_source(path.as_ref())?;
        let (snapshot, result) = reading::search(path, text, query)?;
        self.storage.put("snapshots", &snapshot.id, &snapshot)?;
        Ok(result)
    }

    fn read_source(&self, path: &Path) -> Result<(String, String), Error> {
        let path = self.storage.resolve(path)?;
        let text = self.storage.read(&path)?;
        let path = path
            .into_os_string()
            .into_string()
            .map_err(|_| Error::new("UNSUPPORTED_PATH", "Path is not Unicode"))?;
        Ok((path, text))
    }

    pub fn prepare(&self, request: EditRequest) -> Result<Preparation, Error> {
        let _lock = self.storage.lock()?;
        let input = Input::Edit {
            request: request.clone(),
        };
        if let Some(reference) = self.bound(&request.request_id, &input)? {
            return self.preparation(&reference);
        }
        self.prepare_new(input, request)
    }

    pub fn edit(&self, request: EditRequest) -> Result<EditResult, Error> {
        let _lock = self.storage.lock()?;
        let input = Input::Edit {
            request: request.clone(),
        };
        let preparation = match self.bound(&request.request_id, &input)? {
            Some(reference) => self.preparation(&reference)?,
            None => self.prepare_new(input, request)?,
        };
        self.finish(preparation)
    }

    pub fn commit(&self, reference: &str) -> Result<Receipt, Error> {
        let _lock = self.storage.lock()?;
        let plan = self.plan(reference)?;
        self.commit_plan(&plan)
    }

    /// Corrections replace changes in their original positions and retain the original snapshots.
    pub fn repair(
        &self,
        reference: &str,
        request_id: &str,
        changes: Vec<Change>,
    ) -> Result<Preparation, Error> {
        let _lock = self.storage.lock()?;
        let input = Input::Repair {
            reference: reference.into(),
            request_id: request_id.into(),
            changes: changes.clone(),
        };
        if let Some(reference) = self.bound(request_id, &input)? {
            return self.preparation(&reference);
        }
        let mut request = match self.evidence_unlocked(reference)? {
            Evidence::Plan(plan) => {
                if self.recorded_receipt(&plan)?.is_some() {
                    return Err(Error::new(
                        "REPAIR_CLOSED",
                        "A commit was attempted; inspect its receipt and read fresh snapshots",
                    ));
                }
                plan.request
            }
            Evidence::Draft(draft) => draft.request,
            Evidence::Snapshot(_) => {
                return Err(Error::new(
                    "INVALID_REFERENCE",
                    "Repair requires a draft or plan",
                ));
            }
        };
        if request_id == request.request_id {
            return Err(Error::new(
                "REQUEST_ID_REUSED",
                "Repair requires a new request ID",
            ));
        }
        if changes.is_empty() {
            return Err(Error::new(
                "INVALID_REPAIR",
                "At least one correction is required",
            ));
        }
        let mut corrections = BTreeMap::new();
        for change in changes {
            let id = change.id.clone();
            if corrections.insert(id, change).is_some() {
                return Err(Error::new(
                    "INVALID_REPAIR",
                    "Correction IDs must be unique",
                ));
            }
        }
        let mut original_ids = BTreeSet::new();
        for file in &request.files {
            for change in &file.changes {
                if !original_ids.insert(change.id.clone()) {
                    return Err(Error::new(
                        "INVALID_REPAIR",
                        "Original change IDs are not unique; submit a fresh request",
                    ));
                }
            }
        }
        for file in &mut request.files {
            for change in &mut file.changes {
                if let Some(correction) = corrections.remove(&change.id) {
                    *change = correction;
                }
            }
        }
        if !corrections.is_empty() {
            return Err(Error::new(
                "UNKNOWN_CHANGE",
                format!(
                    "No original change has ID(s): {}",
                    corrections.keys().cloned().collect::<Vec<_>>().join(", ")
                ),
            ));
        }
        request.request_id = request_id.into();
        self.prepare_new(input, request)
    }

    pub fn receipt(&self, request_id: &str) -> Result<Option<Receipt>, Error> {
        let _lock = self.storage.lock()?;
        validate_request_id(request_id)?;
        let key = digest(request_id.as_bytes());
        if !self.storage.exists("requests", &key)? {
            return Err(Error::new(
                "UNKNOWN_REQUEST",
                "No request with that ID is recorded in this workspace",
            ));
        }
        let binding: Binding = self.storage.get("requests", &key)?;
        match self.evidence_unlocked(&binding.reference)? {
            Evidence::Plan(plan) => self.recorded_receipt(&plan),
            Evidence::Draft(_) => Ok(None),
            Evidence::Snapshot(_) => Err(Error::new(
                "CORRUPT_STATE",
                "Request binding points to a snapshot",
            )),
        }
    }

    /// Restores confirmed writes only, conditional on their recorded after-bytes.
    pub fn undo(&self, reference: &str, request_id: &str) -> Result<EditResult, Error> {
        let _lock = self.storage.lock()?;
        let input = Input::Undo {
            reference: reference.into(),
            request_id: request_id.into(),
        };
        if let Some(reference) = self.bound(request_id, &input)? {
            return self.finish(self.preparation(&reference)?);
        }
        let plan = self.plan(reference)?;
        let receipt = self
            .recorded_receipt(&plan)?
            .ok_or_else(|| Error::new("NOT_COMMITTED", "The plan has no commit receipt"))?;
        if receipt.commit == CommitStatus::OutcomeUnknown {
            return Err(Error::new(
                "OUTCOME_UNKNOWN",
                "Reconcile the uncertain commit before undo or further mutation",
            ));
        }
        let mut files = Vec::new();
        for file in &plan.files {
            if !receipt.files.iter().any(|outcome| {
                outcome.path == file.base.path && outcome.status == FileStatus::Committed
            }) {
                continue;
            }
            let snapshot = compiler::snapshot(file.base.path.clone(), file.output.clone());
            self.storage.put("snapshots", &snapshot.id, &snapshot)?;
            files.push(FileRequest {
                base: snapshot.id,
                changes: vec![Change {
                    id: format!("undo-{}", files.len() + 1),
                    target: Target::Span { span: "r0".into() },
                    text: file.base.text.clone(),
                }],
            });
        }
        if files.is_empty() {
            return Err(Error::new(
                "NOT_COMMITTED",
                "There are no confirmed writes to undo",
            ));
        }
        let request = EditRequest {
            request_id: request_id.into(),
            files,
        };
        self.finish(self.prepare_new(input, request)?)
    }

    pub fn evidence(&self, reference: &str) -> Result<Evidence, Error> {
        let _lock = self.storage.lock()?;
        self.evidence_unlocked(reference)
    }

    pub fn diff(&self, reference: &str) -> Result<String, Error> {
        let _lock = self.storage.lock()?;
        Ok(report::diff(&self.plan(reference)?))
    }

    fn finish(&self, preparation: Preparation) -> Result<EditResult, Error> {
        if !preparation.ready {
            return Ok(EditResult::Rejected { preparation });
        }
        let receipt = self.commit_plan(&self.plan(&preparation.reference)?)?;
        let report = report::receipt(&receipt, REPORT_LINES, REPORT_CHARS);
        Ok(EditResult::Completed { receipt, report })
    }

    fn bound(&self, request_id: &str, input: &Input) -> Result<Option<String>, Error> {
        validate_request_id(request_id)?;
        let key = digest(request_id.as_bytes());
        if !self.storage.exists("requests", &key)? {
            return Ok(None);
        }
        let binding: Binding = self.storage.get("requests", &key)?;
        if binding.input != *input {
            return Err(Error::new(
                "REQUEST_ID_REUSED",
                "This request ID is already bound to different arguments",
            ));
        }
        Ok(Some(binding.reference))
    }

    fn prepare_new(&self, input: Input, request: EditRequest) -> Result<Preparation, Error> {
        validate_request_id(&request.request_id)?;
        if request.files.len() > 64 {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "A request may address at most 64 files",
            ));
        }
        let mut snapshots = BTreeMap::new();
        let mut diagnostics = Vec::new();
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut total_bytes = 0usize;
        for file in &request.files {
            if snapshots.contains_key(&file.base) {
                continue;
            }
            let snapshot: Snapshot = match self.storage.get("snapshots", &file.base) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    let mut diagnostic = Diagnostic::new("INVALID_SNAPSHOT", error.to_string());
                    diagnostic.file = Some(file.base.clone());
                    diagnostics.push(diagnostic);
                    continue;
                }
            };
            total_bytes = total_bytes.saturating_add(snapshot.text.len());
            if total_bytes > 64 * 1024 * 1024 {
                return Err(Error::new(
                    "RESOURCE_LIMIT",
                    "Snapshot bytes in a batch exceed 64 MiB",
                ));
            }
            let check = self.storage.resolve(Path::new(&snapshot.path)).and_then(|path| {
                if path != Path::new(&snapshot.path) {
                    return Err(Error::new("TARGET_IDENTITY_CHANGED", "The snapshot path no longer resolves to its original target"));
                }
                for previous in &paths {
                    if same_file::is_same_file(previous, &path)? {
                        return Err(Error::new("TARGET_ALIAS", "Multiple snapshots address the same target; combine their changes into one file entry"));
                    }
                }
                paths.push(path.clone());
                if self.storage.read(&path)? != snapshot.text {
                    return Err(Error::new("STALE_SNAPSHOT", "The file changed; read a fresh snapshot and submit a new request"));
                }
                Ok(())
            });
            if let Err(error) = check {
                let mut diagnostic = Diagnostic::new(&error.code, error.message);
                diagnostic.file = Some(snapshot.path.clone());
                diagnostics.push(diagnostic);
            }
            snapshots.insert(file.base.clone(), snapshot);
        }
        let compiled = compiler::compile(&request, &snapshots);
        let plan = match compiled {
            Ok(plan) => Some(plan),
            Err(errors) => {
                diagnostics.extend(errors);
                None
            }
        };
        let reference = if diagnostics.is_empty() {
            let plan = plan.ok_or_else(|| {
                Error::new(
                    "INTERNAL_ERROR",
                    "Compilation returned no plan and no diagnostics",
                )
            })?;
            self.storage.put("plans", &plan.id, &plan)?;
            plan.id
        } else {
            let draft = Draft {
                id: new_id("d"),
                request: request.clone(),
                diagnostics,
            };
            self.storage.put("drafts", &draft.id, &draft)?;
            draft.id
        };
        self.storage.put(
            "requests",
            &digest(request.request_id.as_bytes()),
            &Binding {
                input,
                reference: reference.clone(),
            },
        )?;
        self.preparation(&reference)
    }

    fn plan(&self, reference: &str) -> Result<PreparedPlan, Error> {
        if !reference.starts_with('p') {
            return Err(Error::new(
                "INVALID_REFERENCE",
                "Expected a prepared plan reference",
            ));
        }
        self.storage.get("plans", reference)
    }

    fn commit_plan(&self, plan: &PreparedPlan) -> Result<Receipt, Error> {
        if let Some(receipt) = self.recorded_receipt(plan)? {
            return Ok(receipt);
        }
        self.storage.commit(plan).map_err(|mut error| {
            error.request_id = Some(plan.request.request_id.clone());
            error.plan_id = Some(plan.id.clone());
            error.commit = Some(
                if matches!(error.code.as_str(), "JOURNAL_UNCERTAIN" | "JOURNAL_CORRUPT") {
                    CommitStatus::OutcomeUnknown
                } else {
                    CommitStatus::NotCommitted
                },
            );
            error
        })
    }

    fn recorded_receipt(&self, plan: &PreparedPlan) -> Result<Option<Receipt>, Error> {
        self.storage.receipt(plan).map_err(|mut error| {
            error.request_id = Some(plan.request.request_id.clone());
            error.plan_id = Some(plan.id.clone());
            error.commit = Some(CommitStatus::OutcomeUnknown);
            error
        })
    }

    fn evidence_unlocked(&self, reference: &str) -> Result<Evidence, Error> {
        match reference.as_bytes().first() {
            Some(b's') => self
                .storage
                .get("snapshots", reference)
                .map(Evidence::Snapshot),
            Some(b'p') => self.storage.get("plans", reference).map(Evidence::Plan),
            Some(b'd') => self.storage.get("drafts", reference).map(Evidence::Draft),
            _ => Err(Error::new(
                "INVALID_REFERENCE",
                "Expected a snapshot, plan, or draft reference",
            )),
        }
    }

    fn preparation(&self, reference: &str) -> Result<Preparation, Error> {
        match self.evidence_unlocked(reference)? {
            Evidence::Plan(plan) => {
                let receipt = self.recorded_receipt(&plan)?;
                let report = match &receipt {
                    Some(receipt) => report::receipt(receipt, REPORT_LINES, REPORT_CHARS),
                    None => report::preview(&plan, REPORT_LINES, REPORT_CHARS),
                };
                Ok(Preparation {
                    reference: plan.id.clone(),
                    ready: true,
                    diagnostics: vec![],
                    report,
                    receipt,
                })
            }
            Evidence::Draft(draft) => Ok(Preparation {
                reference: draft.id.clone(),
                ready: false,
                report: format!(
                    "Rejected: {} diagnostic(s); no target files changed. Inspect draft {}.",
                    draft.diagnostics.len(),
                    draft.id
                ),
                diagnostics: draft.diagnostics,
                receipt: None,
            }),
            Evidence::Snapshot(_) => Err(Error::new(
                "INVALID_REFERENCE",
                "Expected a plan or draft reference",
            )),
        }
    }
}

fn validate_request_id(request_id: &str) -> Result<(), Error> {
    if request_id.is_empty() || request_id.len() > 256 || request_id.chars().any(char::is_control) {
        return Err(Error::new(
            "INVALID_REQUEST_ID",
            "Request ID must be 1..256 UTF-8 bytes without control characters",
        ));
    }
    Ok(())
}
