use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use ultra_edit::compiler;
use ultra_edit::storage::Storage;
use ultra_edit::{
    Change, CommitStatus, EditRequest, FileRequest, FileStatus, PreparedPlan, Target,
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
