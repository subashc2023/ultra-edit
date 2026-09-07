use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use same_file::Handle;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tempfile::NamedTempFile;

use crate::compiler::MAX_TEXT_BYTES;
use crate::model::{
    CommitStatus, Error, FileOutcome, FileStatus, PreparedFile, PreparedPlan, Receipt, Snapshot,
    digest,
};

mod reconciliation;

pub struct Storage {
    root: PathBuf,
    state: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct SnapshotPruning {
    pub dry_run: bool,
    pub older_than_seconds: u64,
    pub eligible: Vec<SnapshotPruningCandidate>,
    pub eligible_bytes: u64,
    pub retained_snapshots: usize,
    pub removed_snapshots: usize,
}

#[derive(Debug, Serialize)]
pub struct SnapshotPruningCandidate {
    pub snapshot: String,
    pub bytes: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checked {
    checksum: String,
    payload: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum JournalEvent {
    Begin {
        plan_id: String,
        plan_digest: String,
        file_count: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preflight_failed: Option<bool>,
    },
    Intent {
        index: usize,
    },
    Outcome {
        index: usize,
        status: FileStatus,
        error: Option<String>,
    },
    Finished,
}

struct Journal {
    plan_id: String,
    plan_digest: String,
    file_count: usize,
    outcomes: Vec<(FileStatus, Option<String>)>,
    preflight_failed: Option<bool>,
    had_intent: bool,
    pending: bool,
    finished: bool,
}

struct TargetFile {
    path: PathBuf,
    identity: Handle,
}

impl Storage {
    pub fn open(root: &Path) -> Result<Self, Error> {
        let root = fs::canonicalize(root)?;
        if !root.is_dir() {
            return Err(Error::new(
                "INVALID_WORKSPACE",
                "Workspace must be a directory",
            ));
        }
        let state = root.join(".ultra-edit");
        ensure_directory(&state)?;
        Ok(Self { root, state })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn lock(&self) -> Result<File, Error> {
        ensure_directory(&self.state)?;
        let path = self.state.join("coordinator.lock");
        regular_or_missing(&path)?;
        let lock = file_options().create(true).truncate(false).open(path)?;
        // One workspace lock serializes cooperating processes. Use ordered target locks
        // if independent-file throughput becomes a measured bottleneck.
        lock.lock()?;
        Ok(lock)
    }

    pub fn resolve(&self, path: &Path) -> Result<PathBuf, Error> {
        let path = fs::canonicalize(self.root.join(path))?;
        if !path.starts_with(&self.root) || path.starts_with(&self.state) {
            return Err(Error::new(
                "PATH_OUTSIDE_WORKSPACE",
                "Targets must be inside the workspace and outside .ultra-edit",
            ));
        }
        if !fs::metadata(&path)?.is_file() {
            return Err(Error::new(
                "NOT_REGULAR_FILE",
                "Only regular files are supported",
            ));
        }
        Ok(path)
    }

    pub fn read(&self, path: &Path) -> Result<String, Error> {
        let bytes = read_target(&self.resolve(path)?)?;
        String::from_utf8(bytes).map_err(|_| {
            Error::new(
                "UNSUPPORTED_ENCODING",
                "Only valid UTF-8 files are supported",
            )
        })
    }

    pub fn exists(&self, kind: &str, id: &str) -> Result<bool, Error> {
        regular_or_missing(&self.object_path(kind, id)?)
    }

    pub fn put<T: Serialize>(&self, kind: &str, id: &str, value: &T) -> Result<(), Error> {
        let path = self.object_path(kind, id)?;
        let bytes = encode(value)?;
        if regular_or_missing(&path)? {
            if fs::read(path)? == bytes {
                return Ok(());
            }
            return Err(Error::new(
                "OBJECT_CONFLICT",
                "Immutable object already exists",
            ));
        }
        let parent = parent(&path)?;
        let mut staged = temporary(parent)?;
        staged.write_all(&bytes)?;
        staged.as_file().sync_all()?;
        staged
            .persist_noclobber(&path)
            .map_err(|error| Error::from(error.error))?;
        sync_directory(parent)?;
        Ok(())
    }

    pub fn get<T: DeserializeOwned>(&self, kind: &str, id: &str) -> Result<T, Error> {
        let path = self.object_path(kind, id)?;
        if !regular_or_missing(&path)? {
            return Err(Error::new(
                "REFERENCE_NOT_FOUND",
                format!("Unknown {kind} reference: {id}"),
            ));
        }
        decode(&fs::read(path)?)
    }

    pub(crate) fn object_ids(&self, kind: &str) -> Result<Vec<String>, Error> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(self.directory(kind)?)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                regular_or_missing(&path)?;
                let id = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .ok_or_else(|| Error::new("STORE_CORRUPT", "Invalid stored object filename"))?;
                safe_component(id)?;
                ids.push(id.to_owned());
            }
        }
        ids.sort_unstable();
        Ok(ids)
    }

    pub(crate) fn prune_snapshots(
        &self,
        retained: &BTreeSet<String>,
        older_than: Duration,
        apply: bool,
    ) -> Result<SnapshotPruning, Error> {
        let cutoff = SystemTime::now().checked_sub(older_than).ok_or_else(|| {
            Error::new(
                "INVALID_RETENTION",
                "Retention age is outside the supported time range",
            )
        })?;
        self.assert_no_unknown()?;
        for entry in fs::read_dir(self.directory("journals")?)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
            {
                regular_or_missing(&path)?;
                let id = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .ok_or_else(|| Error::new("STORE_CORRUPT", "Invalid journal filename"))?;
                let plan: PreparedPlan = self.get("plans", id)?;
                if plan.id != id || self.journal(&plan)?.is_none() {
                    return Err(Error::new(
                        "STORE_CORRUPT",
                        "Journal does not match a retained plan",
                    ));
                }
            }
        }
        let mut result = SnapshotPruning {
            dry_run: !apply,
            older_than_seconds: older_than.as_secs(),
            eligible: Vec::new(),
            eligible_bytes: 0,
            retained_snapshots: 0,
            removed_snapshots: 0,
        };
        for id in self.object_ids("snapshots")? {
            let snapshot: Snapshot = self.get("snapshots", &id)?;
            if snapshot.id != id || snapshot.digest != digest(snapshot.text.as_bytes()) {
                return Err(Error::new(
                    "STORE_CORRUPT",
                    "Snapshot does not match its filename or content digest",
                ));
            }
            let metadata = fs::metadata(self.object_path("snapshots", &id)?)?;
            if retained.contains(&id) || metadata.modified()? > cutoff {
                result.retained_snapshots += 1;
            } else {
                result.eligible_bytes = result
                    .eligible_bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| {
                        Error::new(
                            "RESOURCE_LIMIT",
                            "Snapshot storage size exceeds the supported range",
                        )
                    })?;
                result.eligible.push(SnapshotPruningCandidate {
                    snapshot: id,
                    bytes: metadata.len(),
                });
            }
        }
        // Validate all retained history and candidates before unlinking any snapshot.
        // The coordinator lock excludes cooperating readers and request publication.
        if apply {
            let removal = (|| {
                for candidate in &result.eligible {
                    let path = self.object_path("snapshots", &candidate.snapshot)?;
                    regular_or_missing(&path)?;
                    fs::remove_file(path)?;
                    result.removed_snapshots += 1;
                }
                sync_directory(&self.directory("snapshots")?)?;
                Ok::<_, Error>(())
            })();
            if let Err(error) = removal {
                return Err(Error::new(
                    "PRUNE_INCOMPLETE",
                    format!(
                        "Removed {} snapshots before pruning failed: {error}; rerun a dry run to inspect remaining candidates",
                        result.removed_snapshots
                    ),
                ));
            }
        }
        Ok(result)
    }

    /// The caller must hold the coordinator lock throughout receipt lookup and commit.
    pub fn receipt(&self, plan: &PreparedPlan) -> Result<Option<Receipt>, Error> {
        let Some(journal) = self.journal(plan)? else {
            return Ok(None);
        };
        let mut outcomes = journal.outcomes;
        if journal.pending {
            outcomes.push((
                FileStatus::OutcomeUnknown,
                Some("INTERRUPTED: write intent exists without a durable outcome; reconciliation required".into()),
            ));
        }
        while outcomes.len() < plan.files.len() {
            outcomes.push((
                FileStatus::NotCommitted,
                Some("INTERRUPTED: target was not attempted".into()),
            ));
        }
        Ok(Some(make_receipt(plan, outcomes)))
    }

    /// Authorizes a new immutable attempt only when durable evidence excludes any target write.
    /// The caller holds the coordinator lock through creation of the new attempt.
    pub fn check_retry(&self, plan: &PreparedPlan) -> Result<(), Error> {
        validate_plan(plan)?;
        let eligible = self.journal(plan)?.is_some_and(|journal| {
            journal.finished
                && !journal.had_intent
                && journal.outcomes.iter().all(|(status, error)| {
                    *status == FileStatus::NotCommitted
                        && journal
                            .preflight_failed
                            .unwrap_or_else(|| error.as_deref().is_some_and(legacy_preflight_error))
                })
        });
        if !eligible {
            return Err(Error::new(
                "RETRY_CLOSED",
                "Retry requires a completed preflight failure with no write intent; inspect the receipt before creating a fresh edit",
            ));
        }
        self.assert_no_unknown()
    }

    fn journal(&self, plan: &PreparedPlan) -> Result<Option<Journal>, Error> {
        self.reconciliation(plan)?;
        let path = self.journal_path(&plan.id)?;
        if !regular_or_missing(&path)? {
            return Ok(None);
        }
        let journal = read_journal(&path)?;
        if journal.plan_id != plan.id
            || journal.plan_digest != digest(&serde_json::to_vec(plan)?)
            || journal.file_count != plan.files.len()
        {
            return Err(Error::new(
                "JOURNAL_CORRUPT",
                "Journal does not match its immutable plan",
            ));
        }
        Ok(Some(journal))
    }

    /// Complete preflight precedes mutation. File replacement is not a multi-file transaction,
    /// and checking bytes before rename does not exclude non-cooperating external writers.
    pub fn commit(&self, plan: &PreparedPlan) -> Result<Receipt, Error> {
        self.commit_with(plan, &mut Filesystem)
    }

    fn commit_with(
        &self,
        plan: &PreparedPlan,
        backend: &mut impl Persistence,
    ) -> Result<Receipt, Error> {
        if let Some(receipt) = self.receipt(plan)? {
            return Ok(receipt);
        }
        validate_plan(plan)?;
        self.assert_no_unknown()?;
        let (targets, failures) = self.preflight(plan);
        let preflight_failed = failures.iter().any(Option::is_some);
        let mut journal = file_options()
            .create_new(true)
            .open(self.journal_path(&plan.id)?)?;
        backend
            .record(
                &mut journal,
                &JournalEvent::Begin {
                    plan_id: plan.id.clone(),
                    plan_digest: digest(&serde_json::to_vec(plan)?),
                    file_count: plan.files.len(),
                    preflight_failed: Some(preflight_failed),
                },
            )
            .map_err(journal_error)?;
        sync_directory(&self.directory("journals")?).map_err(journal_error)?;
        let mut outcomes = Vec::new();
        let mut stopped = false;
        for (index, file) in plan.files.iter().enumerate() {
            let (status, error) = if preflight_failed {
                (FileStatus::NotCommitted, Some(failures[index].clone().unwrap_or_else(|| {
                    "BATCH_PREFLIGHT_FAILED: another target failed; no target writes attempted".into()
                })))
            } else if stopped {
                (
                    FileStatus::NotCommitted,
                    Some("NOT_ATTEMPTED: an earlier persistence operation failed".into()),
                )
            } else if let Some(target) = &targets[index] {
                match backend.stage(&target.path, file.output.as_bytes()) {
                    Err(error) => (
                        FileStatus::NotCommitted,
                        Some(format!("STAGING_FAILED: {error}")),
                    ),
                    Ok(staged) => match self.recheck(target, file) {
                        Err(error) => (FileStatus::NotCommitted, Some(error.to_string())),
                        Ok(()) => {
                            backend
                                .record(&mut journal, &JournalEvent::Intent { index })
                                .map_err(journal_error)?;
                            match backend.replace(staged.path(), &target.path) {
                                Ok(()) => (FileStatus::Committed, None),
                                Err(error) => (
                                    FileStatus::OutcomeUnknown,
                                    Some(format!(
                                        "REPLACEMENT_UNCERTAIN: {error}; reconciliation required"
                                    )),
                                ),
                            }
                        }
                    },
                }
            } else {
                return Err(Error::new(
                    "INVALID_PLAN",
                    "Preflight did not resolve a target",
                ));
            };
            stopped |= status != FileStatus::Committed;
            backend
                .record(
                    &mut journal,
                    &JournalEvent::Outcome {
                        index,
                        status,
                        error: error.clone(),
                    },
                )
                .map_err(journal_error)?;
            outcomes.push((status, error));
        }
        backend
            .record(&mut journal, &JournalEvent::Finished)
            .map_err(journal_error)?;
        Ok(make_receipt(plan, outcomes))
    }

    fn preflight(&self, plan: &PreparedPlan) -> (Vec<Option<TargetFile>>, Vec<Option<String>>) {
        let mut targets = Vec::new();
        let mut failures = Vec::new();
        for file in &plan.files {
            let checked = (|| {
                let path = self.resolve(Path::new(&file.base.path))?;
                let target = TargetFile {
                    identity: Handle::from_path(&path)?,
                    path,
                };
                self.recheck(&target, file)?;
                Ok::<_, Error>(target)
            })();
            match checked {
                Ok(target) => {
                    targets.push(Some(target));
                    failures.push(None);
                }
                Err(error) => {
                    targets.push(None);
                    failures.push(Some(error.to_string()));
                }
            }
        }
        let mut identities = HashMap::new();
        for (index, target) in targets.iter().enumerate() {
            if let Some(target) = target
                && let Some(previous) = identities.insert(&target.identity, index)
            {
                let error =
                    "DUPLICATE_TARGET: aliases or hardlinks refer to the same file".to_string();
                failures[index] = Some(error.clone());
                failures[previous] = Some(error);
            }
        }
        (targets, failures)
    }

    fn recheck(&self, target: &TargetFile, file: &PreparedFile) -> Result<(), Error> {
        let path = self.resolve(Path::new(&file.base.path))?;
        if path != Path::new(&file.base.path)
            || path != target.path
            || Handle::from_path(&path)? != target.identity
        {
            return Err(Error::new(
                "STALE_SNAPSHOT",
                "Target identity changed after preflight",
            ));
        }
        if read_target(&path)? != file.base.text.as_bytes() {
            return Err(Error::new(
                "STALE_SNAPSHOT",
                "Current bytes differ from the prepared base",
            ));
        }
        if fs::metadata(path)?.permissions().readonly() {
            return Err(Error::new(
                "READ_ONLY_TARGET",
                "Read-only files cannot be replaced",
            ));
        }
        Ok(())
    }

    fn assert_no_unknown(&self) -> Result<(), Error> {
        let reconciled = self.checked_reconciliations()?;
        for entry in fs::read_dir(self.directory("journals")?)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
            {
                regular_or_missing(&path)?;
                if path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|id| reconciled.contains(id))
                {
                    continue;
                }
                let journal = read_journal(&path)?;
                if journal.pending
                    || journal
                        .outcomes
                        .iter()
                        .any(|(status, _)| *status == FileStatus::OutcomeUnknown)
                {
                    return Err(Error::new(
                        "RECONCILIATION_REQUIRED",
                        format!(
                            "Plan {} has an uncertain outcome; new mutations are blocked",
                            journal.plan_id
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    fn directory(&self, kind: &str) -> Result<PathBuf, Error> {
        safe_component(kind)?;
        ensure_directory(&self.state)?;
        let directory = self.state.join(kind);
        ensure_directory(&directory)?;
        Ok(directory)
    }

    fn object_path(&self, kind: &str, id: &str) -> Result<PathBuf, Error> {
        safe_component(id)?;
        Ok(self.directory(kind)?.join(format!("{id}.json")))
    }

    fn journal_path(&self, id: &str) -> Result<PathBuf, Error> {
        safe_component(id)?;
        Ok(self.directory("journals")?.join(format!("{id}.jsonl")))
    }
}

trait Persistence {
    fn stage(&mut self, target: &Path, bytes: &[u8]) -> io::Result<NamedTempFile> {
        let parent = target
            .parent()
            .ok_or_else(|| io::Error::other("Target has no parent"))?;
        let mut staged = temporary(parent)?;
        staged.write_all(bytes)?;
        staged
            .as_file()
            .set_permissions(fs::metadata(target)?.permissions())?;
        staged.as_file().sync_all()?;
        Ok(staged)
    }

    fn replace(&mut self, staged: &Path, target: &Path) -> io::Result<()> {
        fs::rename(staged, target)?;
        sync_directory(
            target
                .parent()
                .ok_or_else(|| io::Error::other("Target has no parent"))?,
        )
    }

    fn record(&mut self, journal: &mut File, event: &JournalEvent) -> io::Result<()> {
        append_event(journal, event)
    }
}

struct Filesystem;
impl Persistence for Filesystem {}

fn temporary(directory: &Path) -> io::Result<NamedTempFile> {
    tempfile::Builder::new()
        .prefix(".ultra-edit-")
        .make_in(directory, |path| file_options().create_new(true).open(path))
}

fn file_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn append_event(journal: &mut File, event: &JournalEvent) -> io::Result<()> {
    let mut bytes = encode(event).map_err(io::Error::other)?;
    bytes.push(b'\n');
    journal.write_all(&bytes)?;
    journal.sync_all()
}

fn read_journal(path: &Path) -> Result<Journal, Error> {
    let bytes = fs::read(path)?;
    let mut journal: Option<Journal> = None;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.last() != Some(&b'\n') {
            if journal.as_ref().is_some_and(|log| log.finished) {
                return Err(Error::new(
                    "JOURNAL_CORRUPT",
                    "Unexpected bytes after finished journal",
                ));
            }
            break; // A torn trailing record provides no durable evidence.
        }
        let event: JournalEvent =
            decode(line).map_err(|error| Error::new("JOURNAL_CORRUPT", error.message))?;
        match (&mut journal, event) {
            (
                None,
                JournalEvent::Begin {
                    plan_id,
                    plan_digest,
                    file_count,
                    preflight_failed,
                },
            ) if file_count > 0 => {
                journal = Some(Journal {
                    plan_id,
                    plan_digest,
                    file_count,
                    outcomes: Vec::new(),
                    preflight_failed,
                    had_intent: false,
                    pending: false,
                    finished: false,
                });
            }
            (Some(log), JournalEvent::Intent { index })
                if !log.finished
                    && log.preflight_failed != Some(true)
                    && !log.pending
                    && index == log.outcomes.len()
                    && index < log.file_count
                    && log
                        .outcomes
                        .iter()
                        .all(|(status, _)| *status == FileStatus::Committed) =>
            {
                log.had_intent = true;
                log.pending = true;
            }
            (
                Some(log),
                JournalEvent::Outcome {
                    index,
                    status,
                    error,
                },
            ) if !log.finished
                && index == log.outcomes.len()
                && index < log.file_count
                && (log.pending || status == FileStatus::NotCommitted)
                && (status == FileStatus::Committed) == error.is_none() =>
            {
                log.outcomes.push((status, error));
                log.pending = false;
            }
            (Some(log), JournalEvent::Finished)
                if !log.finished && !log.pending && log.outcomes.len() == log.file_count =>
            {
                log.finished = true;
            }
            _ => {
                return Err(Error::new(
                    "JOURNAL_CORRUPT",
                    "Invalid journal event sequence",
                ));
            }
        }
    }
    journal.ok_or_else(|| {
        Error::new(
            "JOURNAL_CORRUPT",
            "Journal has no complete header; reconciliation required",
        )
    })
}

fn legacy_preflight_error(error: &str) -> bool {
    // Older journals cannot distinguish preflight from a pre-write recheck failure.
    // Both may emit these errors, but a complete journal without any Intent still
    // proves no target write, so that legacy ambiguity remains safe to retry.
    error.split_once(": ").is_some_and(|(code, _)| {
        matches!(
            code,
            "BATCH_PREFLIGHT_FAILED"
                | "IO_ERROR"
                | "PATH_OUTSIDE_WORKSPACE"
                | "NOT_REGULAR_FILE"
                | "STALE_SNAPSHOT"
                | "READ_ONLY_TARGET"
                | "DUPLICATE_TARGET"
                | "FILE_TOO_LARGE"
        )
    })
}

fn make_receipt(plan: &PreparedPlan, outcomes: Vec<(FileStatus, Option<String>)>) -> Receipt {
    let files: Vec<_> = plan
        .files
        .iter()
        .zip(outcomes)
        .map(|(file, (status, error))| FileOutcome {
            path: file.base.path.clone(),
            before: file.base.id.clone(),
            after_digest: digest(file.output.as_bytes()),
            status,
            changes_applied: if status == FileStatus::Committed {
                file.change_ids.len()
            } else {
                0
            },
            error,
        })
        .collect();
    let committed = files
        .iter()
        .filter(|file| file.status == FileStatus::Committed)
        .count();
    let commit = if files
        .iter()
        .any(|file| file.status == FileStatus::OutcomeUnknown)
    {
        CommitStatus::OutcomeUnknown
    } else if committed == files.len() {
        CommitStatus::Committed
    } else if committed == 0 {
        CommitStatus::NotCommitted
    } else {
        CommitStatus::Partial
    };
    Receipt {
        request_id: plan.request.request_id.clone(),
        plan_id: plan.id.clone(),
        commit,
        files,
        validation: "not_requested".into(),
        warnings: plan.warnings.clone(),
        undo: (committed > 0 && commit != CommitStatus::OutcomeUnknown).then(|| plan.id.clone()),
    }
}

fn validate_plan(plan: &PreparedPlan) -> Result<(), Error> {
    if plan.files.is_empty()
        || plan.files.len() != plan.request.files.len()
        || plan.request.request_id.trim().is_empty()
    {
        return Err(Error::new(
            "INVALID_PLAN",
            "Plan must match a nonempty request",
        ));
    }
    let mut change_ids = std::collections::HashSet::new();
    for (file, request) in plan.files.iter().zip(&plan.request.files) {
        if file.base.text.len() > MAX_TEXT_BYTES || file.output.len() > MAX_TEXT_BYTES {
            return Err(Error::new(
                "FILE_TOO_LARGE",
                "Prepared files must not exceed 16 MiB",
            ));
        }
        if file.base.id != request.base
            || request.changes.is_empty()
            || file.change_ids
                != request
                    .changes
                    .iter()
                    .map(|change| change.id.clone())
                    .collect::<Vec<_>>()
            || request
                .changes
                .iter()
                .any(|change| change.id.trim().is_empty() || !change_ids.insert(&change.id))
        {
            return Err(Error::new(
                "INVALID_PLAN",
                "Prepared changes do not match their request identities",
            ));
        }
        if file.base.digest != digest(file.base.text.as_bytes()) {
            return Err(Error::new(
                "INVALID_PLAN",
                "Snapshot digest does not match its bytes",
            ));
        }
        let mut end = 0;
        let mut output = String::new();
        let mut previous_start = None;
        let mut seen = std::collections::HashSet::new();
        for replacement in &file.replacements {
            if replacement.start < end
                || replacement.end < replacement.start
                || !file.base.text.is_char_boundary(replacement.start)
                || !file.base.text.is_char_boundary(replacement.end)
                || previous_start == Some(replacement.start)
                || (previous_start.is_some()
                    && replacement.start == end
                    && replacement.start == replacement.end)
            {
                return Err(Error::new(
                    "INVALID_PLAN",
                    "Replacement ranges are invalid or overlap",
                ));
            }
            if !request
                .changes
                .iter()
                .any(|change| change.id == replacement.change_id && change.text == replacement.text)
            {
                return Err(Error::new(
                    "INVALID_PLAN",
                    "Replacement does not match a requested change",
                ));
            }
            seen.insert(&replacement.change_id);
            output.push_str(&file.base.text[end..replacement.start]);
            output.push_str(&replacement.text);
            end = replacement.end;
            previous_start = Some(replacement.start);
        }
        output.push_str(&file.base.text[end..]);
        if output != file.output || seen.len() != file.change_ids.len() {
            return Err(Error::new(
                "INVALID_PLAN",
                "Prepared output differs from its replacements",
            ));
        }
    }
    Ok(())
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    let payload = serde_json::to_value(value)?;
    Ok(serde_json::to_vec(&Checked {
        checksum: digest(&serde_json::to_vec(&payload)?),
        payload,
    })?)
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    let checked: Checked = serde_json::from_slice(bytes)
        .map_err(|error| Error::new("STORE_CORRUPT", error.to_string()))?;
    if digest(&serde_json::to_vec(&checked.payload)?) != checked.checksum {
        return Err(Error::new(
            "STORE_CORRUPT",
            "Stored object checksum mismatch",
        ));
    }
    serde_json::from_value(checked.payload)
        .map_err(|error| Error::new("STORE_CORRUPT", error.to_string()))
}

fn safe_component(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(Error::new(
            "INVALID_REFERENCE",
            "Storage names must contain 1-128 ASCII letters, digits, underscores, or hyphens",
        ));
    }
    let upper = value.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && matches!(upper.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(Error::new(
            "INVALID_REFERENCE",
            "Reserved platform device names are not valid storage names",
        ));
    }
    Ok(())
}

fn ensure_directory(path: &Path) -> Result<(), Error> {
    match create_directory(path) {
        Ok(()) => sync_directory(parent(path)?)?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || fs::canonicalize(path)? != path {
        return Err(Error::new(
            "UNSAFE_STATE_PATH",
            "State directories must be real directories inside the workspace",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn create_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_directory(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

fn regular_or_missing(path: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(Error::new(
            "UNSAFE_STATE_PATH",
            "State files must be regular files, never symlinks",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn parent(path: &Path) -> Result<&Path, Error> {
    path.parent()
        .ok_or_else(|| Error::new("INVALID_PATH", "Path has no parent directory"))
}

fn read_target(path: &Path) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_TEXT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(Error::new(
            "FILE_TOO_LARGE",
            "Files larger than 16 MiB are not supported",
        ));
    }
    Ok(bytes)
}

fn journal_error(error: io::Error) -> Error {
    Error::new(
        "JOURNAL_UNCERTAIN",
        format!(
            "Could not durably record commit progress: {error}. Some targets may have committed; retrieve the receipt and do not replay the mutation"
        ),
    )
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> io::Result<()> {
    // std has no portable directory-flush API on Windows. File contents and journal
    // records are synced for process-crash recovery; power-loss durability is not promised.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler;
    use crate::model::{Change, EditRequest, FileRequest, Target};
    use std::collections::BTreeMap;

    fn fixture() -> (tempfile::TempDir, Storage, PreparedPlan) {
        let directory = tempfile::tempdir().expect("temporary workspace");
        let storage = Storage::open(directory.path()).expect("open workspace");
        let mut snapshots = BTreeMap::new();
        let mut files = Vec::new();
        for index in 0..3 {
            let path = directory.path().join(format!("file-{index}.txt"));
            fs::write(&path, "before").expect("write fixture");
            let canonical = storage.resolve(&path).expect("resolve");
            let snapshot =
                compiler::snapshot(canonical.to_string_lossy().into_owned(), "before".into());
            files.push(FileRequest {
                base: snapshot.id.clone(),
                changes: vec![Change {
                    id: format!("change-{index}"),
                    target: Target::Exact {
                        old: "before".into(),
                        scope: None,
                    },
                    text: "after".into(),
                }],
            });
            snapshots.insert(snapshot.id.clone(), snapshot);
        }
        let plan = compiler::compile(
            &EditRequest {
                request_id: "fault-test".into(),
                files,
            },
            &snapshots,
        )
        .expect("compile fixture");
        (directory, storage, plan)
    }

    struct FailSecondStage(usize);
    impl Persistence for FailSecondStage {
        fn stage(&mut self, target: &Path, bytes: &[u8]) -> io::Result<NamedTempFile> {
            self.0 += 1;
            if self.0 == 2 {
                return Err(io::Error::other("injected staging failure"));
            }
            Filesystem.stage(target, bytes)
        }
    }

    #[test]
    fn stage_failure_reports_partial_and_never_attempts_later_targets() {
        let (_directory, storage, plan) = fixture();
        let _lock = storage.lock().expect("lock");
        let mut backend = FailSecondStage(0);
        let receipt = storage.commit_with(&plan, &mut backend).expect("receipt");
        assert_eq!(receipt.commit, CommitStatus::Partial);
        assert_eq!(
            receipt
                .files
                .iter()
                .map(|file| file.changes_applied)
                .collect::<Vec<_>>(),
            [1, 0, 0]
        );
        assert_eq!(backend.0, 2);
        assert_eq!(
            fs::read_to_string(&plan.files[0].base.path).expect("read"),
            "after"
        );
        assert_eq!(
            fs::read_to_string(&plan.files[1].base.path).expect("read"),
            "before"
        );
        assert_eq!(
            fs::read_to_string(&plan.files[2].base.path).expect("read"),
            "before"
        );
        assert_eq!(storage.commit(&plan).expect("repeat"), receipt);
        assert_eq!(
            storage
                .check_retry(&plan)
                .expect_err("partial retry closed")
                .code,
            "RETRY_CLOSED"
        );
    }

    struct FailSecondReplacement(usize);
    impl Persistence for FailSecondReplacement {
        fn replace(&mut self, staged: &Path, target: &Path) -> io::Result<()> {
            self.0 += 1;
            Filesystem.replace(staged, target)?;
            if self.0 == 2 {
                return Err(io::Error::other("failure after replacement took effect"));
            }
            Ok(())
        }
    }

    #[test]
    fn replacement_error_is_unknown_even_when_new_bytes_are_visible() {
        let (_directory, storage, plan) = fixture();
        let _lock = storage.lock().expect("lock");
        let receipt = storage
            .commit_with(&plan, &mut FailSecondReplacement(0))
            .expect("receipt");
        assert_eq!(receipt.commit, CommitStatus::OutcomeUnknown);
        assert_eq!(receipt.files[0].status, FileStatus::Committed);
        assert_eq!(receipt.files[1].status, FileStatus::OutcomeUnknown);
        assert_eq!(receipt.files[2].status, FileStatus::NotCommitted);
        assert_eq!(receipt.files[1].changes_applied, 0);
        assert_eq!(receipt.undo, None);
        assert_eq!(
            fs::read_to_string(&plan.files[1].base.path).expect("read"),
            "after"
        );
        assert_eq!(
            fs::read_to_string(&plan.files[2].base.path).expect("read"),
            "before"
        );
        assert_eq!(storage.commit(&plan).expect("repeat"), receipt);
        assert_eq!(
            storage
                .check_retry(&plan)
                .expect_err("uncertain retry closed")
                .code,
            "RETRY_CLOSED"
        );
        let mut another = plan.clone();
        another.id = "another-plan".into();
        assert_eq!(
            storage.commit(&another).expect_err("block new writes").code,
            "RECONCILIATION_REQUIRED"
        );
    }

    struct FailOutcomeRecord;
    impl Persistence for FailOutcomeRecord {
        fn record(&mut self, journal: &mut File, event: &JournalEvent) -> io::Result<()> {
            if matches!(event, JournalEvent::Outcome { .. }) {
                journal.write_all(b"{\"checksum\":\"torn")?;
                journal.sync_all()?;
                return Err(io::Error::other("injected journal failure after mutation"));
            }
            append_event(journal, event)
        }
    }

    #[test]
    fn missing_durable_outcome_recovers_unknown_without_replay() {
        let (directory, storage, plan) = fixture();
        let lock = storage.lock().expect("lock");
        let error = storage
            .commit_with(&plan, &mut FailOutcomeRecord)
            .expect_err("journal failure");
        assert_eq!(error.code, "JOURNAL_UNCERTAIN");
        assert_eq!(
            fs::read_to_string(&plan.files[0].base.path).expect("read"),
            "after"
        );
        drop(lock);
        let reopened = Storage::open(directory.path()).expect("reopen");
        let _lock = reopened.lock().expect("lock");
        let receipt = reopened.receipt(&plan).expect("recover").expect("receipt");
        assert_eq!(receipt.commit, CommitStatus::OutcomeUnknown);
        assert_eq!(receipt.files[0].status, FileStatus::OutcomeUnknown);
        assert_eq!(receipt.files[1].status, FileStatus::NotCommitted);
        assert_eq!(reopened.commit(&plan).expect("repeat"), receipt);
        assert_eq!(
            reopened
                .check_retry(&plan)
                .expect_err("interrupted retry closed")
                .code,
            "RETRY_CLOSED"
        );
        assert_eq!(
            fs::read_to_string(&plan.files[1].base.path).expect("read"),
            "before"
        );
    }

    struct ChangeAfterStage;
    impl Persistence for ChangeAfterStage {
        fn stage(&mut self, target: &Path, bytes: &[u8]) -> io::Result<NamedTempFile> {
            let staged = Filesystem.stage(target, bytes)?;
            fs::write(target, "external writer")?;
            Ok(staged)
        }
    }

    #[test]
    fn base_is_rechecked_after_staging_and_before_write_intent() {
        let (_directory, storage, plan) = fixture();
        let _lock = storage.lock().expect("lock");
        let receipt = storage
            .commit_with(&plan, &mut ChangeAfterStage)
            .expect("receipt");
        assert_eq!(receipt.commit, CommitStatus::NotCommitted);
        assert!(
            receipt.files[0]
                .error
                .as_deref()
                .expect("stale error")
                .contains("STALE_SNAPSHOT")
        );
        assert_eq!(
            fs::read_to_string(&plan.files[0].base.path).expect("read"),
            "external writer"
        );
        assert_eq!(
            fs::read_to_string(&plan.files[1].base.path).expect("read"),
            "before"
        );
    }

    #[test]
    fn staging_failures_and_incomplete_preflight_journals_cannot_be_retried() {
        let (_directory, storage, plan) = fixture();
        let _lock = storage.lock().expect("lock");
        let receipt = storage
            .commit_with(&plan, &mut FailSecondStage(1))
            .expect("receipt");
        assert_eq!(receipt.commit, CommitStatus::NotCommitted);
        assert_eq!(
            storage.check_retry(&plan).expect_err("stage failure").code,
            "RETRY_CLOSED"
        );

        let mut incomplete = plan.clone();
        incomplete.id = "incomplete-preflight".into();
        let mut journal = file_options()
            .create_new(true)
            .open(storage.journal_path(&incomplete.id).unwrap())
            .unwrap();
        append_event(
            &mut journal,
            &JournalEvent::Begin {
                plan_id: incomplete.id.clone(),
                plan_digest: digest(&serde_json::to_vec(&incomplete).unwrap()),
                file_count: incomplete.files.len(),
                preflight_failed: Some(true),
            },
        )
        .unwrap();
        assert_eq!(
            storage
                .check_retry(&incomplete)
                .expect_err("incomplete")
                .code,
            "RETRY_CLOSED"
        );
        append_event(&mut journal, &JournalEvent::Intent { index: 0 }).unwrap();
        assert_eq!(
            storage
                .check_retry(&incomplete)
                .expect_err("invalid preflight intent")
                .code,
            "JOURNAL_CORRUPT"
        );
    }

    #[test]
    fn single_target_stale_failure_after_staging_is_not_classified_as_preflight() {
        let (_directory, storage, mut plan) = fixture();
        let _lock = storage.lock().expect("lock");
        plan.files.truncate(1);
        plan.request.files.truncate(1);
        let receipt = storage
            .commit_with(&plan, &mut ChangeAfterStage)
            .expect("receipt");
        assert_eq!(receipt.commit, CommitStatus::NotCommitted);
        assert!(
            receipt.files[0]
                .error
                .as_deref()
                .unwrap()
                .contains("STALE_SNAPSHOT")
        );
        assert_eq!(
            storage
                .check_retry(&plan)
                .expect_err("post-staging failure")
                .code,
            "RETRY_CLOSED"
        );
    }

    #[test]
    fn legacy_preflight_retry_requires_complete_no_intent_evidence() {
        for had_intent in [false, true] {
            let (_directory, storage, plan) = fixture();
            let _lock = storage.lock().expect("lock");
            let mut journal = file_options()
                .create_new(true)
                .open(storage.journal_path(&plan.id).unwrap())
                .unwrap();
            append_event(
                &mut journal,
                &JournalEvent::Begin {
                    plan_id: plan.id.clone(),
                    plan_digest: digest(&serde_json::to_vec(&plan).unwrap()),
                    file_count: plan.files.len(),
                    preflight_failed: None,
                },
            )
            .unwrap();
            if had_intent {
                append_event(&mut journal, &JournalEvent::Intent { index: 0 }).unwrap();
            }
            for index in 0..plan.files.len() {
                append_event(&mut journal, &JournalEvent::Outcome {
                    index,
                    status: FileStatus::NotCommitted,
                    error: Some(if index == 0 { "READ_ONLY_TARGET: Read-only files cannot be replaced" } else { "BATCH_PREFLIGHT_FAILED: another target failed; no target writes attempted" }.into()),
                }).unwrap();
            }
            assert_eq!(
                storage.check_retry(&plan).expect_err("unfinished").code,
                "RETRY_CLOSED"
            );
            append_event(&mut journal, &JournalEvent::Finished).unwrap();
            if had_intent {
                assert_eq!(
                    storage
                        .check_retry(&plan)
                        .expect_err("intent disqualifies retry")
                        .code,
                    "RETRY_CLOSED"
                );
            } else {
                storage
                    .check_retry(&plan)
                    .expect("legacy preflight is safe to retry");
            }
        }
    }

    #[test]
    fn legacy_single_target_stale_recheck_without_write_intent_remains_retryable() {
        let (_directory, storage, mut plan) = fixture();
        let _lock = storage.lock().expect("lock");
        plan.files.truncate(1);
        plan.request.files.truncate(1);
        let mut journal = file_options()
            .create_new(true)
            .open(storage.journal_path(&plan.id).unwrap())
            .unwrap();
        append_event(
            &mut journal,
            &JournalEvent::Begin {
                plan_id: plan.id.clone(),
                plan_digest: digest(&serde_json::to_vec(&plan).unwrap()),
                file_count: 1,
                preflight_failed: None,
            },
        )
        .unwrap();
        append_event(
            &mut journal,
            &JournalEvent::Outcome {
                index: 0,
                status: FileStatus::NotCommitted,
                error: Some("STALE_SNAPSHOT: Current bytes differ from the prepared base".into()),
            },
        )
        .unwrap();
        append_event(&mut journal, &JournalEvent::Finished).unwrap();
        storage
            .check_retry(&plan)
            .expect("legacy evidence proves no target write");
        assert_eq!(
            storage.receipt(&plan).unwrap().unwrap().commit,
            CommitStatus::NotCommitted
        );
        assert_eq!(
            fs::read_to_string(&plan.files[0].base.path).unwrap(),
            "before"
        );
    }

    #[test]
    fn journal_corruption_blocks_new_mutation() {
        let (_directory, storage, plan) = fixture();
        let _lock = storage.lock().expect("lock");
        fs::write(storage.journal_path("corrupt").expect("path"), b"broken\n")
            .expect("corrupt journal");
        assert_eq!(
            storage.commit(&plan).expect_err("fail closed").code,
            "JOURNAL_CORRUPT"
        );
        assert_eq!(
            fs::read_to_string(&plan.files[0].base.path).expect("read"),
            "before"
        );
    }
}
