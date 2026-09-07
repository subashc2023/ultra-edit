use super::*;
use crate::model::{
    Diagnostic, Inspection, ObservedState, Reconciliation, ReconciliationRequest,
    TargetObservation, new_id,
};
use std::collections::HashSet;

impl Storage {
    /// Captures evidence without modifying target files. The caller holds the workspace lock.
    /// Retain the plan in the plans object store before reconciling this inspection.
    pub fn inspect(&self, plan: &PreparedPlan) -> Result<Inspection, Error> {
        validate_plan(plan)?;
        let journal_path = self.journal_path(&plan.id)?;
        if !regular_or_missing(&journal_path)? {
            return Err(Error::new(
                "NO_COMMIT_ATTEMPT",
                "The plan has no journal to inspect",
            ));
        }
        let journal = read_target(&journal_path)?;
        let (receipt, receipt_error) = match self.receipt(plan) {
            Ok(receipt) => (receipt, None),
            Err(error) => (None, Some(Diagnostic::new(&error.code, error.message))),
        };
        let reconciliation = if self.exists("reconciliations", &plan.id)? {
            Some(self.get("reconciliations", &plan.id)?)
        } else {
            None
        };
        let inspection = Inspection {
            id: new_id("i"),
            plan: plan.clone(),
            journal_digest: digest(&journal),
            journal: journal.into(),
            receipt,
            receipt_error,
            files: self.observe_targets(plan),
            reconciliation,
        };
        self.put("inspections", &inspection.id, &inspection)?;
        Ok(inspection)
    }

    /// Records acceptance of observed current state, never a historical commit outcome.
    /// The caller must hold the workspace lock throughout inspection lookup and publication.
    pub fn reconcile(&self, request: ReconciliationRequest) -> Result<Reconciliation, Error> {
        validate_request(&request)?;
        let inspection: Inspection = self.get("inspections", &request.inspection)?;
        let plan: PreparedPlan = self.get("plans", &inspection.plan.id)?;
        if inspection.id != request.inspection || inspection.plan != plan {
            return Err(Error::new(
                "STORE_CORRUPT",
                "Inspection does not match its reference and immutable plan",
            ));
        }
        if let Some(recorded) = self.reconciliation(&plan)? {
            if recorded.request == request {
                return Ok(recorded);
            }
            return Err(Error::new(
                "RECONCILIATION_CONFLICT",
                "This plan already has a different resolution; inspect its recorded decision",
            ));
        }
        validate_plan(&plan)?;
        if inspection
            .files
            .iter()
            .any(|file| matches!(file.state, ObservedState::Unavailable { .. }))
        {
            return Err(Error::new(
                "INSPECTION_INCOMPLETE",
                "A target could not be inspected safely; address its diagnostic and inspect again",
            ));
        }
        let journal_path = self.journal_path(&plan.id)?;
        if !regular_or_missing(&journal_path)?
            || read_target(&journal_path)? != inspection.journal.as_bytes()
            || digest(inspection.journal.as_bytes()) != inspection.journal_digest
            || self.observe_targets(&plan) != inspection.files
        {
            return Err(Error::new(
                "STALE_INSPECTION",
                "Journal or current target state changed; inspect again before accepting it",
            ));
        }
        // A corrupt regular journal can be acknowledged, but its historical error stays intact.
        match self.receipt(&plan) {
            Ok(Some(receipt)) if receipt.commit == CommitStatus::OutcomeUnknown => {}
            Err(error) if error.code == "JOURNAL_CORRUPT" => {}
            Ok(_) => {
                return Err(Error::new(
                    "NOT_UNCERTAIN",
                    "This plan has no uncertain outcome requiring reconciliation",
                ));
            }
            Err(error) => return Err(error),
        }
        let resolution = Reconciliation {
            plan_id: plan.id.clone(),
            plan_digest: digest(&serde_json::to_vec(&plan)?),
            journal_digest: inspection.journal_digest,
            request,
        };
        self.put("reconciliations", &plan.id, &resolution)?;
        Ok(resolution)
    }

    pub(super) fn reconciliation(
        &self,
        plan: &PreparedPlan,
    ) -> Result<Option<Reconciliation>, Error> {
        if !self.exists("reconciliations", &plan.id)? {
            return Ok(None);
        }
        let resolution: Reconciliation = self.get("reconciliations", &plan.id)?;
        let inspection: Inspection = self.get("inspections", &resolution.request.inspection)?;
        validate_request(&resolution.request)?;
        if resolution.plan_id != plan.id
            || resolution.plan_digest != digest(&serde_json::to_vec(plan)?)
            || inspection.id != resolution.request.inspection
            || inspection.plan != *plan
            || inspection.journal_digest != resolution.journal_digest
            || digest(inspection.journal.as_bytes()) != resolution.journal_digest
        {
            return Err(Error::new(
                "STORE_CORRUPT",
                "Reconciliation does not match its immutable inspection and plan",
            ));
        }
        let journal = self.journal_path(&plan.id)?;
        if !regular_or_missing(&journal)?
            || digest(&read_target(&journal)?) != resolution.journal_digest
        {
            return Err(Error::new(
                "RECONCILIATION_EVIDENCE_CHANGED",
                "The reconciled journal changed or is missing; restore the recorded evidence before further mutation",
            ));
        }
        Ok(Some(resolution))
    }

    pub(super) fn checked_reconciliations(&self) -> Result<HashSet<String>, Error> {
        let mut reconciled = HashSet::new();
        // Scan resolutions as well as journals: deleting a reconciled journal must not reopen a plan.
        for entry in fs::read_dir(self.directory("reconciliations")?)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                regular_or_missing(&path)?;
                let id = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .ok_or_else(|| {
                        Error::new("STORE_CORRUPT", "Invalid reconciliation filename")
                    })?;
                let plan: PreparedPlan = self.get("plans", id)?;
                if plan.id != id || self.reconciliation(&plan)?.is_none() {
                    return Err(Error::new(
                        "STORE_CORRUPT",
                        "Reconciliation filename does not match its plan",
                    ));
                }
                reconciled.insert(id.to_owned());
            }
        }
        Ok(reconciled)
    }

    fn observe_targets(&self, plan: &PreparedPlan) -> Vec<TargetObservation> {
        let mut remaining = 64 * 1024 * 1024;
        plan.files
            .iter()
            .map(|file| {
                let state = self
                    .observe_target(Path::new(&file.base.path), remaining)
                    .unwrap_or_else(|error| ObservedState::Unavailable {
                        code: error.code,
                        message: error.message,
                    });
                if let ObservedState::File { content, .. } = &state {
                    remaining -= content.as_bytes().len();
                }
                TargetObservation {
                    path: file.base.path.clone(),
                    state,
                }
            })
            .collect()
    }

    fn observe_target(&self, path: &Path, remaining: usize) -> Result<ObservedState, Error> {
        if !path.is_absolute() || !path.starts_with(&self.root) || path.starts_with(&self.state) {
            return Err(Error::new(
                "PATH_OUTSIDE_WORKSPACE",
                "Inspected targets must stay inside the workspace and outside .ultra-edit",
            ));
        }
        let mut existing = path;
        loop {
            match fs::symlink_metadata(existing) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if existing == self.root {
                        return Err(Error::new(
                            "INVALID_WORKSPACE",
                            "Workspace root disappeared",
                        ));
                    }
                    existing = parent(existing)?;
                }
                Err(error) => return Err(error.into()),
                Ok(metadata) => {
                    // A missing leaf can conceal a redirected parent. Check the nearest existing
                    // ancestor too; canonical equality also detects Windows junction redirection.
                    if metadata.file_type().is_symlink() || fs::canonicalize(existing)? != existing
                    {
                        return Err(Error::new(
                            "TARGET_IDENTITY_CHANGED",
                            "The target or an ancestor is redirected; restore its path before inspection",
                        ));
                    }
                    if existing != path {
                        if !metadata.is_dir() {
                            return Err(Error::new(
                                "NOT_DIRECTORY",
                                "A target ancestor is not a directory",
                            ));
                        }
                        return Ok(ObservedState::Missing);
                    }
                    if !metadata.is_file() {
                        return Err(Error::new(
                            "NOT_REGULAR_FILE",
                            "Only regular files or safely absent paths can be reconciled",
                        ));
                    }
                    if metadata.len() > remaining as u64 {
                        return Err(observation_limit());
                    }
                    let bytes = read_target(path)?;
                    if bytes.len() > remaining {
                        return Err(observation_limit());
                    }
                    return Ok(ObservedState::File {
                        digest: digest(&bytes),
                        content: bytes.into(),
                    });
                }
            }
        }
    }
}

fn observation_limit() -> Error {
    Error::new(
        "RESOURCE_LIMIT",
        "Current file bytes in an inspection exceed 64 MiB",
    )
}

fn validate_request(request: &ReconciliationRequest) -> Result<(), Error> {
    if !request.inspection.starts_with('i') {
        return Err(Error::new(
            "INVALID_REFERENCE",
            "Expected an inspection reference",
        ));
    }
    if request.note.trim().is_empty() || request.note.chars().count() > 1_000 {
        return Err(Error::new(
            "INVALID_RECONCILIATION",
            "An operator note of 1..1,000 Unicode characters is required",
        ));
    }
    Ok(())
}
