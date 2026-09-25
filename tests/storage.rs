use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};
use ultra_edit::compiler;
use ultra_edit::storage::Storage;
use ultra_edit::workspace::Evidence;
use ultra_edit::{
    Change, CommitStatus, EditRequest, FileRequest, FileStatus, Inspection, PreparedPlan, Snapshot,
    Target, Workspace, digest,
};

fn plan(storage: &Storage, paths: &[&str]) -> PreparedPlan {
    let mut snapshots = BTreeMap::new();
    let mut files = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        let snapshot = compiler::snapshot(
            storage
                .resolve(Path::new(path))
                .expect("resolve")
                .to_string_lossy()
                .into_owned(),
            storage.read(Path::new(path)).expect("read"),
        );
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
    compiler::compile(
        &EditRequest {
            request_id: "storage-test".into(),
            files,
        },
        &snapshots,
    )
    .expect("compile")
}

#[test]
fn commit_preserves_bytes_and_retry_returns_original_receipt_after_external_change() {
    let directory = tempfile::tempdir().expect("directory");
    let target = directory.path().join("file.txt");
    fs::write(&target, "\u{feff}before\r\n\tkeep  \nfinal").expect("fixture");
    let storage = Storage::open(directory.path()).expect("storage");
    let lock = storage.lock().expect("lock");
    let plan = plan(&storage, &["file.txt"]);
    let receipt = storage.commit(&plan).expect("commit");
    assert_eq!(receipt.commit, CommitStatus::Committed);
    assert_eq!(receipt.files[0].changes_applied, 1);
    assert_eq!(
        fs::read(&target).expect("read"),
        "\u{feff}after\r\n\tkeep  \nfinal".as_bytes()
    );
    fs::write(&target, "later work").expect("external write");
    drop(lock);
    let reopened = Storage::open(directory.path()).expect("reopen");
    let _lock = reopened.lock().expect("lock");
    assert_eq!(reopened.commit(&plan).expect("retry"), receipt);
    assert_eq!(fs::read_to_string(target).expect("read"), "later work");
}

#[test]
fn stale_later_file_rejects_the_entire_batch() {
    let directory = tempfile::tempdir().expect("directory");
    fs::write(directory.path().join("first.txt"), "before").expect("fixture");
    fs::write(directory.path().join("second.txt"), "before").expect("fixture");
    let storage = Storage::open(directory.path()).expect("storage");
    let _lock = storage.lock().expect("lock");
    let plan = plan(&storage, &["first.txt", "second.txt"]);
    fs::write(directory.path().join("second.txt"), "newer").expect("external write");
    let receipt = storage.commit(&plan).expect("receipt");
    assert_eq!(receipt.commit, CommitStatus::NotCommitted);
    assert!(
        receipt
            .files
            .iter()
            .all(|file| file.status == FileStatus::NotCommitted && file.changes_applied == 0)
    );
    assert!(
        receipt.files[1]
            .error
            .as_deref()
            .expect("stale error")
            .contains("STALE_SNAPSHOT")
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("first.txt")).expect("read"),
        "before"
    );
    assert_eq!(storage.commit(&plan).expect("retry"), receipt);
}

#[test]
fn hardlink_aliases_cannot_compete_in_a_batch() {
    let directory = tempfile::tempdir().expect("directory");
    let first = directory.path().join("first.txt");
    let second = directory.path().join("alias.txt");
    fs::write(&first, "before").expect("fixture");
    fs::hard_link(&first, &second).expect("hardlink");
    let storage = Storage::open(directory.path()).expect("storage");
    let _lock = storage.lock().expect("lock");
    let plan = plan(&storage, &["first.txt", "alias.txt"]);
    let receipt = storage.commit(&plan).expect("receipt");
    assert_eq!(receipt.commit, CommitStatus::NotCommitted);
    assert!(receipt.files.iter().all(|file| {
        file.error
            .as_deref()
            .expect("duplicate error")
            .contains("DUPLICATE_TARGET")
    }));
    assert_eq!(fs::read_to_string(first).expect("read"), "before");
    assert_eq!(fs::read_to_string(second).expect("read"), "before");
}

#[test]
fn read_rejects_unsupported_encoding_and_non_workspace_targets() {
    let directory = tempfile::tempdir().expect("directory");
    let external = tempfile::NamedTempFile::new().expect("external fixture");
    fs::write(directory.path().join("utf16.txt"), [0xff, 0xfe, b'a', 0]).expect("fixture");
    let storage = Storage::open(directory.path()).expect("storage");
    let _lock = storage.lock().expect("lock");
    assert_eq!(
        storage
            .read(Path::new("utf16.txt"))
            .expect_err("encoding")
            .code,
        "UNSUPPORTED_ENCODING"
    );
    assert_eq!(
        storage.resolve(external.path()).expect_err("outside").code,
        "PATH_OUTSIDE_WORKSPACE"
    );
    assert_eq!(
        storage
            .resolve(Path::new(".ultra-edit/coordinator.lock"))
            .expect_err("internal")
            .code,
        "PATH_OUTSIDE_WORKSPACE"
    );
    assert_eq!(
        storage.resolve(Path::new(".")).expect_err("directory").code,
        "NOT_REGULAR_FILE"
    );
}

#[test]
fn missing_and_unrepresentable_targets_are_reported_with_their_path() {
    let directory = tempfile::tempdir().expect("directory");
    let storage = Storage::open(directory.path()).expect("storage");
    let _lock = storage.lock().expect("lock");
    let error = storage
        .resolve(Path::new("missing.txt"))
        .expect_err("missing target");
    assert_eq!(error.code, "TARGET_MISSING");
    assert!(error.message.contains("missing.txt"), "{}", error.message);
    assert!(!error.message.contains(r"\\?\"), "{}", error.message);
    assert_eq!(
        storage
            .resolve(Path::new("a\0.txt"))
            .expect_err("unrepresentable path")
            .code,
        "INVALID_PATH"
    );
}

#[test]
fn state_directory_failures_are_distinguished_from_target_failures() {
    let directory = tempfile::tempdir().expect("directory");
    fs::write(directory.path().join(".ultra-edit"), "not a directory").expect("state file");
    assert_eq!(
        Storage::open(directory.path())
            .map(drop)
            .expect_err("state path is a file")
            .code,
        "UNSAFE_STATE_PATH"
    );

    let workspace = tempfile::tempdir().expect("directory");
    let storage = Storage::open(workspace.path()).expect("storage");
    let _lock = storage.lock().expect("lock");
    storage.put("examples", "a1", &"value").expect("put");
    let error = {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // An exclusive share mode denies every other open of this stored object.
            let _exclusive = fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(workspace.path().join(".ultra-edit/examples/a1.json"))
                .expect("exclusive handle");
            storage
                .get::<String>("examples", "a1")
                .expect_err("state object unavailable")
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let state = workspace.path().join(".ultra-edit");
            let original = fs::metadata(&state).expect("metadata").permissions();
            fs::set_permissions(&state, fs::Permissions::from_mode(0o500))
                .expect("read-only state directory");
            // Privileged processes, such as root in most CI containers, bypass
            // directory permissions, so this mode cannot make the store unwritable.
            let probe = state.join("permission-probe");
            if fs::write(&probe, "").is_ok() {
                fs::remove_file(&probe).expect("remove probe");
                fs::set_permissions(&state, original).expect("restore permissions");
                eprintln!("skipped: directory permissions do not restrict this process");
                return;
            }
            let error = storage
                .put("others", "a1", &"value")
                .expect_err("state directory unwritable");
            fs::set_permissions(&state, original).expect("restore permissions");
            error
        }
    };
    assert_eq!(error.code, "STATE_DIR_UNAVAILABLE");
    assert!(error.message.contains(".ultra-edit"), "{}", error.message);
}

#[test]
fn a_drive_root_or_a_state_directory_cannot_be_a_workspace() {
    let directory = tempfile::tempdir().expect("directory");
    let state = directory.path().join(".ultra-edit");
    fs::create_dir(&state).expect("state directory");
    let nested = state.join("journals");
    fs::create_dir(&nested).expect("nested state directory");
    for root in [state.as_path(), nested.as_path()] {
        assert_eq!(
            Storage::open(root)
                .map(drop)
                .expect_err("state directory")
                .code,
            "INVALID_WORKSPACE"
        );
    }
    let filesystem_root = directory
        .path()
        .ancestors()
        .last()
        .expect("filesystem root");
    // The rejection must precede any state creation, whatever the root already holds.
    let root_state = filesystem_root.join(".ultra-edit");
    let existing_root_state = root_state.exists();
    assert_eq!(
        Storage::open(filesystem_root)
            .map(drop)
            .expect_err("filesystem root")
            .code,
        "INVALID_WORKSPACE"
    );
    assert_eq!(root_state.exists(), existing_root_state);
}

#[test]
fn a_new_workspace_ignores_its_own_state_directory_without_overwriting_changes() {
    let directory = tempfile::tempdir().expect("directory");
    let state = directory.path().join(".ultra-edit");
    Storage::open(directory.path()).expect("storage");
    assert_eq!(
        fs::read_to_string(state.join(".gitignore")).expect("gitignore"),
        "*\n"
    );
    assert_eq!(
        fs::read_to_string(state.join("CACHEDIR.TAG")).expect("cache tag"),
        "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by Ultra Edit.\n# For information about cache directory tags see https://bford.info/cachedir/\n"
    );
    fs::write(state.join(".gitignore"), "*\n!keep\n").expect("operator change");
    Storage::open(directory.path()).expect("reopen");
    assert_eq!(
        fs::read_to_string(state.join(".gitignore")).expect("gitignore"),
        "*\n!keep\n"
    );
}

#[test]
fn object_store_is_immutable_checksums_payloads_and_rejects_path_traversal() {
    let directory = tempfile::tempdir().expect("directory");
    let storage = Storage::open(directory.path()).expect("storage");
    let _lock = storage.lock().expect("lock");
    let value = vec!["first", "second"];
    storage.put("examples", "a1", &value).expect("put");
    storage
        .put("examples", "a1", &value)
        .expect("idempotent put");
    assert_eq!(
        storage.get::<Vec<String>>("examples", "a1").expect("get"),
        value
    );
    assert_eq!(
        storage
            .put("examples", "a1", &vec!["changed"])
            .expect_err("immutable")
            .code,
        "OBJECT_CONFLICT"
    );
    assert!(!storage.exists("examples", "missing").expect("exists"));
    assert_eq!(
        storage
            .get::<Vec<String>>("examples", "missing")
            .expect_err("unknown")
            .code,
        "REFERENCE_NOT_FOUND"
    );
    assert_eq!(
        storage
            .put("../escape", "id", &value)
            .expect_err("traversal")
            .code,
        "INVALID_REFERENCE"
    );
    let path = directory.path().join(".ultra-edit/examples/a1.json");
    let contents = fs::read_to_string(&path).expect("read object");
    fs::write(path, contents.replace("first", "wrong")).expect("corrupt object");
    assert_eq!(
        storage
            .get::<Vec<String>>("examples", "a1")
            .expect_err("checksum")
            .code,
        "STORE_CORRUPT"
    );
}

#[test]
fn invalid_prepared_output_cannot_be_committed() {
    let directory = tempfile::tempdir().expect("directory");
    fs::write(directory.path().join("file.txt"), "before").expect("fixture");
    let storage = Storage::open(directory.path()).expect("storage");
    let _lock = storage.lock().expect("lock");
    let mut plan = plan(&storage, &["file.txt"]);
    plan.files[0].output = "unrequested output".into();
    assert_eq!(
        storage.commit(&plan).expect_err("invalid plan").code,
        "INVALID_PLAN"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("file.txt")).expect("read"),
        "before"
    );
}

#[test]
fn reads_and_preflight_reject_files_over_the_limit() {
    let directory = tempfile::tempdir().expect("directory");
    let target = directory.path().join("file.txt");
    fs::write(&target, "before").expect("fixture");
    let storage = Storage::open(directory.path()).expect("storage");
    let _lock = storage.lock().expect("lock");
    let plan = plan(&storage, &["file.txt"]);
    fs::OpenOptions::new()
        .write(true)
        .open(&target)
        .expect("open")
        .set_len(16 * 1024 * 1024 + 1)
        .expect("large file");
    assert_eq!(
        storage.read(Path::new("file.txt")).expect_err("limit").code,
        "FILE_TOO_LARGE"
    );
    let receipt = storage.commit(&plan).expect("receipt");
    assert_eq!(receipt.commit, CommitStatus::NotCommitted);
    assert!(
        receipt.files[0]
            .error
            .as_deref()
            .expect("limit error")
            .contains("FILE_TOO_LARGE")
    );
    assert_eq!(
        fs::metadata(target).expect("metadata").len(),
        16 * 1024 * 1024 + 1
    );
}

#[test]
fn read_only_targets_fail_preflight_without_an_uncertain_receipt() {
    let directory = tempfile::tempdir().expect("directory");
    let target = directory.path().join("file.txt");
    fs::write(&target, "before").expect("fixture");
    let storage = Storage::open(directory.path()).expect("storage");
    let _lock = storage.lock().expect("lock");
    let plan = plan(&storage, &["file.txt"]);
    let original = fs::metadata(&target).expect("metadata").permissions();
    let mut readonly = original.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&target, readonly).expect("read-only fixture");
    let receipt = storage.commit(&plan).expect("receipt");
    fs::set_permissions(&target, original).expect("restore fixture permissions");
    assert_eq!(receipt.commit, CommitStatus::NotCommitted);
    assert!(
        receipt.files[0]
            .error
            .as_deref()
            .expect("read-only error")
            .contains("READ_ONLY_TARGET")
    );
    assert_eq!(fs::read_to_string(target).expect("read"), "before");
}

#[test]
fn coordinator_lock_serializes_independent_storage_handles() {
    use std::sync::mpsc;
    use std::time::Duration;

    let directory = tempfile::tempdir().expect("directory");
    let storage = Storage::open(directory.path()).expect("storage");
    let first = storage.lock().expect("first lock");
    let second = Storage::open(directory.path()).expect("second storage");
    let (started_tx, started_rx) = mpsc::channel();
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).expect("ready signal");
        let _lock = second.lock().expect("second lock");
        acquired_tx.send(()).expect("acquired signal");
    });
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("worker started");
    assert_eq!(
        acquired_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    );
    drop(first);
    acquired_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("worker acquired lock after release");
    worker.join().expect("worker finished");
}

#[cfg(unix)]
#[test]
fn changing_a_snapshotted_path_to_a_symlink_cannot_redirect_the_commit() {
    let directory = tempfile::tempdir().expect("directory");
    let first = directory.path().join("first.txt");
    let second = directory.path().join("second.txt");
    fs::write(&first, "before").expect("fixture");
    fs::write(&second, "before").expect("fixture");
    let storage = Storage::open(directory.path()).expect("storage");
    let _lock = storage.lock().expect("lock");
    let plan = plan(&storage, &["first.txt"]);
    fs::remove_file(&first).expect("replace fixture");
    std::os::unix::fs::symlink(&second, &first).expect("redirect fixture");
    let receipt = storage.commit(&plan).expect("receipt");
    assert_eq!(receipt.commit, CommitStatus::NotCommitted);
    assert!(
        receipt.files[0]
            .error
            .as_deref()
            .expect("stale error")
            .contains("STALE_SNAPSHOT")
    );
    assert_eq!(fs::read_to_string(second).expect("read"), "before");
}

#[cfg(unix)]
#[test]
fn state_symlinks_and_external_target_symlinks_are_rejected() {
    let directory = tempfile::tempdir().expect("directory");
    let external = tempfile::tempdir().expect("external directory");
    fs::write(external.path().join("file.txt"), "before").expect("fixture");
    std::os::unix::fs::symlink(
        external.path().join("file.txt"),
        directory.path().join("external.txt"),
    )
    .expect("external symlink");
    let storage = Storage::open(directory.path()).expect("storage");
    let _lock = storage.lock().expect("lock");
    assert_eq!(
        storage
            .resolve(Path::new("external.txt"))
            .expect_err("outside")
            .code,
        "PATH_OUTSIDE_WORKSPACE"
    );
    std::os::unix::fs::symlink(
        external.path(),
        directory.path().join(".ultra-edit/objects"),
    )
    .expect("state symlink");
    assert_eq!(
        storage
            .put("objects", "a1", &"value")
            .expect_err("unsafe state")
            .code,
        "UNSAFE_STATE_PATH"
    );
}

/// Over the blob threshold, with a single `before` for the shared plan helper.
fn large(tag: &str) -> String {
    let mut text = String::from("before\n");
    for line in 0..300 {
        text.push_str(&format!("{tag} line {line:04} stays unchanged\n"));
    }
    text
}

/// Encodes an object as 0.2.0 stored it: checksummed, with every string inline.
fn encoded(value: &impl Serialize) -> Vec<u8> {
    let payload = serde_json::to_value(value).expect("payload");
    let checksum = digest(&serde_json::to_vec(&payload).expect("payload bytes"));
    serde_json::to_vec(&json!({"checksum": checksum, "payload": payload})).expect("object")
}

fn object_path(root: &Path, kind: &str, id: &str) -> PathBuf {
    root.join(".ultra-edit")
        .join(kind)
        .join(format!("{id}.json"))
}

fn blob_names(root: &Path) -> BTreeSet<String> {
    match fs::read_dir(root.join(".ultra-edit/blobs")) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeSet::new(),
        entries => entries
            .expect("blobs")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("name")
            })
            .collect(),
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[test]
fn large_strings_round_trip_through_shared_blobs_while_journals_stay_inline() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    fs::write(root.join("first.txt"), large("alpha")).expect("fixture");
    fs::write(root.join("second.txt"), large("beta")).expect("fixture");
    let storage = Storage::open(root).expect("storage");
    let lock = storage.lock().expect("lock");
    let plan = plan(&storage, &["first.txt", "second.txt"]);
    for file in &plan.files {
        storage
            .put("snapshots", &file.base.id, &file.base)
            .expect("put snapshot");
        assert_eq!(
            storage
                .get::<Snapshot>("snapshots", &file.base.id)
                .expect("get snapshot"),
            file.base
        );
    }
    storage.put("plans", &plan.id, &plan).expect("put plan");
    storage
        .put("plans", &plan.id, &plan)
        .expect("an identical value is idempotent");
    let stored: PreparedPlan = storage.get("plans", &plan.id).expect("get plan");
    assert_eq!(stored, plan);
    // Journals digest the plan's serialization, which must not depend on how it was stored.
    assert_eq!(
        serde_json::to_vec(&stored).expect("stored"),
        serde_json::to_vec(&plan).expect("plan")
    );
    let expected: BTreeSet<_> = plan
        .files
        .iter()
        .flat_map(|file| {
            [
                digest(file.base.text.as_bytes()),
                digest(file.output.as_bytes()),
            ]
        })
        .collect();
    assert_eq!(expected.len(), 4);
    assert_eq!(blob_names(root), expected);
    for file in &plan.files {
        let blob = root
            .join(".ultra-edit/blobs")
            .join(digest(file.base.text.as_bytes()));
        assert_eq!(fs::read(blob).expect("blob"), file.base.text.as_bytes());
    }

    let receipt = storage.commit(&stored).expect("commit");
    assert_eq!(receipt.commit, CommitStatus::Committed);
    let inspection = storage.inspect(&stored).expect("inspect");
    assert_eq!(
        storage
            .get::<Inspection>("inspections", &inspection.id)
            .expect("get inspection"),
        inspection
    );
    // Committed-output snapshots and inspected current bytes reuse the output blobs.
    assert_eq!(blob_names(root), expected);
    let mut objects = vec![
        object_path(root, "plans", &plan.id),
        object_path(root, "inspections", &inspection.id),
    ];
    for file in &receipt.files {
        objects.push(object_path(
            root,
            "snapshots",
            file.after.as_deref().expect("after snapshot"),
        ));
    }
    for path in objects {
        let raw = fs::read(&path).expect("object");
        assert!(contains(&raw, "\"$blob\""), "{}", path.display());
        assert!(!contains(&raw, "line 0100"), "{}", path.display());
    }

    let journal = fs::read(
        root.join(".ultra-edit/journals")
            .join(format!("{}.jsonl", plan.id)),
    )
    .expect("journal");
    assert!(!contains(&journal, "$blob"));
    let begin: Value = serde_json::from_slice(
        journal
            .split(|byte| *byte == b'\n')
            .next()
            .expect("begin record"),
    )
    .expect("begin");
    assert_eq!(
        begin["payload"]["plan_digest"],
        digest(&serde_json::to_vec(&plan).expect("plan"))
    );
    drop(lock);
    let reopened = Storage::open(root).expect("reopen");
    let _lock = reopened.lock().expect("lock");
    let recovered: PreparedPlan = reopened.get("plans", &plan.id).expect("get plan");
    assert_eq!(
        reopened.receipt(&recovered).expect("receipt"),
        Some(receipt.clone())
    );
    for (file, outcome) in plan.files.iter().zip(&receipt.files) {
        let after: Snapshot = reopened
            .get("snapshots", outcome.after.as_deref().expect("after"))
            .expect("after snapshot");
        assert_eq!(after.text, file.output);
    }
}

#[test]
fn objects_written_inline_by_0_2_0_still_load_and_accept_identical_puts() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    fs::write(root.join("file.txt"), large("legacy")).expect("fixture");
    let storage = Storage::open(root).expect("storage");
    let _lock = storage.lock().expect("lock");
    let plan = plan(&storage, &["file.txt"]);
    let snapshot = plan.files[0].base.clone();
    let legacy = [
        ("plans", plan.id.clone(), encoded(&plan)),
        ("snapshots", snapshot.id.clone(), encoded(&snapshot)),
    ];
    for (kind, id, bytes) in &legacy {
        fs::create_dir_all(root.join(".ultra-edit").join(kind)).expect("kind directory");
        fs::write(object_path(root, kind, id), bytes).expect("legacy object");
    }
    assert_eq!(
        storage
            .get::<Snapshot>("snapshots", &snapshot.id)
            .expect("legacy snapshot"),
        snapshot
    );
    let loaded: PreparedPlan = storage.get("plans", &plan.id).expect("legacy plan");
    assert_eq!(loaded, plan);
    assert_eq!(
        digest(&serde_json::to_vec(&loaded).expect("loaded")),
        digest(&serde_json::to_vec(&plan).expect("plan"))
    );
    storage
        .put("plans", &plan.id, &plan)
        .expect("the same value as the legacy object");
    storage
        .put("snapshots", &snapshot.id, &snapshot)
        .expect("the same value as the legacy object");
    let mut changed = snapshot.clone();
    changed.text.push_str("changed\n");
    assert_eq!(
        storage
            .put("snapshots", &snapshot.id, &changed)
            .expect_err("different value")
            .code,
        "OBJECT_CONFLICT"
    );
    for (kind, id, bytes) in &legacy {
        assert_eq!(
            &fs::read(object_path(root, kind, id)).expect("object"),
            bytes
        );
    }
    assert!(blob_names(root).is_empty());
    let receipt = storage.commit(&loaded).expect("commit");
    assert_eq!(receipt.commit, CommitStatus::Committed);
    assert_eq!(storage.receipt(&plan).expect("receipt"), Some(receipt));
}

#[test]
fn damaged_or_missing_blobs_are_store_corruption() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    fs::write(root.join("file.txt"), large("gamma")).expect("fixture");
    let storage = Storage::open(root).expect("storage");
    let _lock = storage.lock().expect("lock");
    let snapshot = plan(&storage, &["file.txt"]).files[0].base.clone();
    storage
        .put("snapshots", &snapshot.id, &snapshot)
        .expect("put");
    let name = digest(snapshot.text.as_bytes());
    let blob = root.join(".ultra-edit/blobs").join(&name);
    let original = fs::read(&blob).expect("blob");
    let get_error = || {
        storage
            .get::<Snapshot>("snapshots", &snapshot.id)
            .expect_err("damaged blob")
    };

    let mut damaged = original.clone();
    damaged[0] ^= 1;
    fs::write(&blob, &damaged).expect("damage blob");
    let error = get_error();
    assert_eq!(error.code, "STORE_CORRUPT");
    assert!(error.message.contains("digest"), "{}", error.message);

    // A later write of the same content must not adopt a truncated blob.
    fs::write(&blob, &original[..10]).expect("truncate blob");
    let mut again = snapshot.clone();
    again.id = "sagain".into();
    assert_eq!(
        storage
            .put("snapshots", &again.id, &again)
            .expect_err("length mismatch")
            .code,
        "STORE_CORRUPT"
    );
    assert!(!storage.exists("snapshots", &again.id).expect("exists"));

    fs::remove_file(&blob).expect("remove blob");
    let error = get_error();
    assert_eq!(error.code, "STORE_CORRUPT");
    assert!(error.message.contains(&name), "{}", error.message);
    assert!(error.message.contains("missing"), "{}", error.message);

    // Restoring the content repairs every object that references it.
    fs::write(&blob, &original).expect("restore blob");
    assert_eq!(
        storage
            .get::<Snapshot>("snapshots", &snapshot.id)
            .expect("restored"),
        snapshot
    );

    let invalid = [0xff, 0xfe, 0xfd];
    let invalid_name = digest(&invalid);
    fs::write(root.join(".ultra-edit/blobs").join(&invalid_name), invalid).expect("blob");
    fs::write(
        object_path(root, "snapshots", "sinvalid"),
        encoded(&json!({
            "id": "sinvalid",
            "path": snapshot.path,
            "digest": invalid_name,
            "text": {"$blob": invalid_name},
            "spans": [],
        })),
    )
    .expect("object");
    let error = storage
        .get::<Snapshot>("snapshots", "sinvalid")
        .expect_err("invalid UTF-8");
    assert_eq!(error.code, "STORE_CORRUPT");
    assert!(error.message.contains("UTF-8"), "{}", error.message);

    fs::create_dir_all(root.join(".ultra-edit/examples")).expect("kind directory");
    for reference in [json!("short"), json!(7), json!(invalid_name.to_uppercase())] {
        fs::write(
            object_path(root, "examples", "malformed"),
            encoded(&json!({"text": {"$blob": reference}})),
        )
        .expect("object");
        assert_eq!(
            storage
                .get::<Value>("examples", "malformed")
                .expect_err("malformed reference")
                .code,
            "STORE_CORRUPT"
        );
    }
    assert_eq!(
        storage
            .put("examples", "reserved", &json!({"text": {"$blob": name}}))
            .expect_err("reserved form")
            .code,
        "INVALID_OBJECT"
    );
}

#[test]
fn only_strings_at_the_threshold_become_blobs_and_keys_stay_inline() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let storage = Storage::open(root).expect("storage");
    let _lock = storage.lock().expect("lock");
    let small = json!({"short": "a".repeat(4095), "values": ["b", 1, null]});
    storage.put("examples", "small", &small).expect("put");
    // Objects without large strings keep the 0.2.0 encoding byte for byte.
    assert_eq!(
        fs::read(object_path(root, "examples", "small")).expect("object"),
        encoded(&small)
    );
    assert!(blob_names(root).is_empty());

    let key = "k".repeat(5000);
    let mut value = json!({
        "short": "a".repeat(4095),
        "long": "b".repeat(4096),
        "list": ["c".repeat(5000), "c".repeat(5000), {"nested": "d".repeat(4097)}],
    });
    value[&key] = json!("inline value");
    storage.put("examples", "large", &value).expect("put");
    assert_eq!(
        storage.get::<Value>("examples", "large").expect("get"),
        value
    );
    storage
        .put("examples", "large", &value)
        .expect("idempotent put");
    let raw = fs::read(object_path(root, "examples", "large")).expect("object");
    assert!(contains(&raw, &"a".repeat(4095)));
    assert!(contains(&raw, &key));
    assert!(!contains(&raw, &"b".repeat(4096)));
    assert_eq!(
        blob_names(root),
        BTreeSet::from([
            digest("b".repeat(4096).as_bytes()),
            digest("c".repeat(5000).as_bytes()),
            digest("d".repeat(4097).as_bytes()),
        ])
    );
}

#[test]
fn rereading_an_unchanged_file_stores_only_a_small_snapshot() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let text: String = (1..=3000)
        .map(|line| format!("const value_{line:05} = {line};\n"))
        .collect();
    fs::write(root.join("big.js"), &text).expect("fixture");
    let workspace = Workspace::open(root).expect("workspace");
    let first = workspace.read_range("big.js", 10, 15).expect("first read");
    let second = workspace
        .read_range("big.js", 2000, 2005)
        .expect("second read");
    assert_ne!(first.snapshot, second.snapshot);
    assert_eq!(blob_names(root), BTreeSet::from([digest(text.as_bytes())]));
    for snapshot in [&first.snapshot, &second.snapshot] {
        let bytes = fs::metadata(object_path(root, "snapshots", snapshot))
            .expect("snapshot object")
            .len();
        assert!(bytes < 2048, "{bytes}");
        let Evidence::Snapshot(stored) = workspace.evidence(snapshot).expect("evidence") else {
            panic!("Expected snapshot evidence")
        };
        assert_eq!(stored.text, text);
    }
    let changed = text.replacen("= 1;", "= 2;", 1);
    fs::write(root.join("big.js"), &changed).expect("external edit");
    workspace.read_range("big.js", 1, 1).expect("fresh read");
    assert_eq!(
        blob_names(root),
        BTreeSet::from([digest(text.as_bytes()), digest(changed.as_bytes())])
    );
}
