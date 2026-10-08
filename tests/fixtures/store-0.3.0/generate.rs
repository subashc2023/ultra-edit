//! Generates the Ultra Edit 0.3.0 store fixture checked in at
//! `tests/fixtures/store-0.3.0`. It was run once against the 0.3.0 engine (commit
//! 9adb7de), from `examples/store_fixture.rs`, as
//! `cargo run --example store_fixture -- tests/fixtures/store-0.3.0`. This copy documents
//! how the objects were made; it is not compiled, and later code must never regenerate
//! the fixture, whose purpose is to pin what 0.3.0 wrote.
//!
//! The store holds every persisted kind: request bindings for edit, repair, and undo;
//! plans with committed, not committed, and outcome-unknown journals; a rejected draft;
//! snapshots large enough to use blobs; an inspection, its reconciliation, and an
//! uncertain marker. `manifest.json` records the workspace root the objects name, the
//! arguments of each request, its receipt, and SHA-256 digests of each object's typed
//! `serde_json::to_vec` bytes, which later code must reproduce.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use ultra_edit::storage::Storage;
use ultra_edit::workspace::{EditResult, Evidence};
use ultra_edit::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: store_fixture <output directory>")?,
    );
    let scratch = tempfile::tempdir()?;
    let root = scratch.path().join("root");
    fs::create_dir(&root)?;
    fs::write(
        root.join("a.txt"),
        "\u{feff}alpha = 1\r\nbeta = 2\r\nalpha = 1\r\n",
    )?;
    let large: String = (1..=300)
        .map(|line| format!("line {line}: stored as a blob once it passes 4 KiB\n"))
        .collect();
    fs::write(root.join("b.txt"), &large)?;
    fs::write(root.join("c.txt"), "one\ntwo\nthree\n")?;
    fs::write(root.join("e.txt"), "before\n")?;
    fs::write(root.join("f.txt"), "before\n")?;
    let workspace = Workspace::open(&root)?;
    let mut requests = BTreeMap::new();

    // Committed edit with a derived request ID: an unscoped exact, a scoped replace-all,
    // and a guarded span from a focused read of a file stored as blobs.
    let a = workspace.read("a.txt")?;
    let b = workspace.read_range("b.txt", 100, 102)?;
    let edit = EditRequest {
        request_id: String::new(),
        files: vec![
            FileRequest {
                base: a.id.clone(),
                changes: vec![
                    change(
                        Target::Exact {
                            old: "beta = 2".into(),
                            scope: None,
                        },
                        "beta = 3",
                    ),
                    change(
                        Target::All {
                            old: "alpha".into(),
                            scope: "r0".into(),
                            expected: 2,
                        },
                        "gamma",
                    ),
                ],
            },
            FileRequest {
                base: b.snapshot.clone(),
                changes: vec![change(
                    Target::Span {
                        span: "r101".into(),
                        expect: Some("line 101: stored as a blob once it passes 4 KiB".into()),
                    },
                    "line one hundred one",
                )],
            },
        ],
    };
    let receipt = completed(workspace.edit(edit.clone())?);
    requests.insert(
        "edit",
        json!({"request_id": receipt.request_id, "arguments": edit, "receipt": receipt}),
    );

    // A rejected edit leaves a draft with candidates; its repair and the undo of that
    // repair bind derived IDs of their own.
    let c = workspace.read("c.txt")?;
    let rejected = EditRequest {
        request_id: String::new(),
        files: vec![FileRequest {
            base: c.id.clone(),
            changes: vec![change(
                Target::Exact {
                    old: "tow".into(),
                    scope: None,
                },
                "2",
            )],
        }],
    };
    let EditResult::Rejected { preparation } = workspace.edit(rejected.clone())? else {
        return Err("expected a rejected draft".into());
    };
    requests.insert(
        "rejected",
        json!({"request_id": preparation.request_id, "arguments": rejected, "draft": preparation.reference, "receipt": null}),
    );
    let corrections = vec![Change {
        id: "1.1".into(),
        target: Target::Exact {
            old: "two".into(),
            scope: None,
        },
        text: "2".into(),
    }];
    let repaired = workspace.repair(&preparation.reference, "", corrections.clone())?;
    let repair_receipt = workspace.commit(&repaired.reference)?;
    requests.insert(
        "repair",
        json!({"request_id": repaired.request_id, "reference": preparation.reference, "changes": corrections, "plan": repaired.reference, "receipt": repair_receipt}),
    );
    let undo_receipt = completed(workspace.undo(&repaired.reference, "")?);
    requests.insert(
        "undo",
        json!({"request_id": undo_receipt.request_id, "reference": repaired.reference, "receipt": undo_receipt}),
    );

    // A commit whose preflight found the file changed records `not_committed`.
    let f = workspace.read("f.txt")?;
    let stale = EditRequest {
        request_id: "stale-0.3.0".into(),
        files: vec![FileRequest {
            base: f.id.clone(),
            changes: vec![change(
                Target::Exact {
                    old: "before".into(),
                    scope: None,
                },
                "after",
            )],
        }],
    };
    let stale_plan = workspace.prepare(stale.clone())?;
    fs::write(root.join("f.txt"), "external\n")?;
    let stale_receipt = workspace.commit(&stale_plan.reference)?;
    requests.insert(
        "stale",
        json!({"request_id": "stale-0.3.0", "arguments": stale, "plan": stale_plan.reference, "receipt": stale_receipt}),
    );

    // An interrupted commit: a durable intent without an outcome, as a crash leaves it,
    // then inspected and accepted. Its uncertain marker stays until a later scan.
    let e = workspace.read("e.txt")?;
    let interrupted = EditRequest {
        request_id: "interrupted-0.3.0".into(),
        files: vec![FileRequest {
            base: e.id.clone(),
            changes: vec![change(
                Target::Exact {
                    old: "before".into(),
                    scope: None,
                },
                "after",
            )],
        }],
    };
    let prepared = workspace.prepare(interrupted.clone())?;
    let Evidence::Plan(plan) = workspace.evidence(&prepared.reference)? else {
        return Err("expected a plan".into());
    };
    let state = root.join(".ultra-edit");
    fs::create_dir_all(state.join("uncertain"))?;
    File::create_new(state.join("uncertain").join(&plan.id))?;
    let mut journal = record(json!({
        "event": "begin", "plan_id": plan.id,
        "plan_digest": digest(&serde_json::to_vec(&plan)?),
        "file_count": plan.files.len(),
    }));
    journal.extend(record(json!({"event": "intent", "index": 0})));
    let mut file = File::create_new(state.join("journals").join(format!("{}.jsonl", plan.id)))?;
    file.write_all(&journal)?;
    file.sync_all()?;
    let inspection = workspace.inspect(&plan.id)?;
    let reconciliation = workspace.reconcile(ReconciliationRequest {
        inspection: inspection.id.clone(),
        decision: ReconciliationDecision::AcceptCurrent,
        note: "Fixture: accept the current file after an interrupted commit.".into(),
    })?;
    requests.insert(
        "interrupted",
        json!({"request_id": "interrupted-0.3.0", "arguments": interrupted, "plan": plan.id, "inspection": inspection.id, "receipt": workspace.receipt("interrupted-0.3.0")?, "reconciliation": reconciliation}),
    );

    let storage = Storage::open(&root)?;
    let mut objects = BTreeMap::new();
    for kind in [
        "snapshots",
        "plans",
        "drafts",
        "inspections",
        "reconciliations",
    ] {
        for entry in fs::read_dir(state.join(kind))? {
            let id = entry?
                .path()
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or("object name")?
                .to_owned();
            let bytes = match kind {
                "snapshots" => serde_json::to_vec(&storage.get::<Snapshot>(kind, &id)?)?,
                "plans" => serde_json::to_vec(&storage.get::<PreparedPlan>(kind, &id)?)?,
                "drafts" => serde_json::to_vec(&storage.get::<Draft>(kind, &id)?)?,
                "inspections" => serde_json::to_vec(&storage.get::<Inspection>(kind, &id)?)?,
                _ => serde_json::to_vec(&storage.get::<Reconciliation>(kind, &id)?)?,
            };
            objects.insert(format!("{kind}/{id}"), digest(&bytes));
        }
    }
    let manifest = json!({
        "generator": "ultra-edit 0.3.0 (examples/store_fixture.rs)",
        "root": workspace.root().to_str().ok_or("root")?,
        "requests": requests,
        "objects": objects,
    });

    // Markers and the lock are recreated on open; `.gitignore` would hide the fixture.
    let target = output.join("state");
    fs::create_dir_all(&target)?;
    copy_state(&state, &target)?;
    let mut bytes = serde_json::to_vec_pretty(&manifest)?;
    bytes.push(b'\n');
    fs::write(output.join("manifest.json"), bytes)?;
    Ok(())
}

fn change(target: Target, text: &str) -> Change {
    Change {
        id: String::new(),
        target,
        text: text.into(),
    }
}

fn completed(result: EditResult) -> Receipt {
    match result {
        EditResult::Completed { receipt, .. } => receipt,
        EditResult::Rejected { preparation } => panic!("rejected: {preparation:?}"),
    }
}

fn record(payload: Value) -> Vec<u8> {
    let checksum = digest(&serde_json::to_vec(&payload).expect("payload"));
    let mut bytes =
        serde_json::to_vec(&json!({"checksum": checksum, "payload": payload})).expect("record");
    bytes.push(b'\n');
    bytes
}

fn copy_state(from: &Path, to: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(
            name.as_ref(),
            ".gitignore" | "CACHEDIR.TAG" | "coordinator.lock"
        ) || name.starts_with(".ultra-edit-")
        {
            continue;
        }
        if entry.file_type()?.is_dir() {
            fs::create_dir_all(to.join(&*name))?;
            copy_state(&entry.path(), &to.join(&*name))?;
        } else {
            fs::copy(entry.path(), to.join(&*name))?;
        }
    }
    Ok(())
}
