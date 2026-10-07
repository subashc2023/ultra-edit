//! Changes kept literal by a CR in `old` or a span's `expect`, CRLF adaptation, and
//! the refusal to repair an undo.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use ultra_edit::compiler::{compile, snapshot};
use ultra_edit::storage::Storage;
use ultra_edit::workspace::{EditResult, Evidence};
use ultra_edit::*;

fn exact(id: &str, old: &str, text: &str) -> Change {
    Change {
        id: id.into(),
        target: Target::Exact {
            old: old.into(),
            scope: None,
            lines: None,
        },
        text: text.into(),
    }
}

fn count(id: &str, old: &str, text: &str, expected: usize) -> Change {
    Change {
        id: id.into(),
        target: Target::All {
            old: old.into(),
            scope: None,
            expected,
            lines: None,
        },
        text: text.into(),
    }
}

fn span(id: &str, span: &str, expect: Option<&str>, text: &str) -> Change {
    Change {
        id: id.into(),
        target: Target::Span {
            span: span.into(),
            expect: expect.map(Into::into),
        },
        text: text.into(),
    }
}

fn request(base: &Snapshot, changes: Vec<Change>) -> EditRequest {
    EditRequest {
        request_id: "test".into(),
        files: vec![FileRequest {
            path: None,
            base: base.id.clone(),
            changes,
        }],
    }
}

fn bases(base: &Snapshot) -> BTreeMap<String, Snapshot> {
    BTreeMap::from([(base.id.clone(), base.clone())])
}

fn codes(warnings: &[Diagnostic]) -> Vec<&str> {
    warnings.iter().map(|w| w.code.as_str()).collect()
}

#[test]
fn span_expect_holding_cr_keeps_new_literal_and_span_without_expect_adapts() {
    let base = snapshot("crlf.txt".into(), "a\r\nb\r\nc\r\n".into());
    // Whole file, expect with CR: CRLF converts to LF.
    let plan = compile(
        &request(
            &base,
            vec![span("s", "r0", Some("a\r\nb\r\nc\r\n"), "a\nb\nc\n")],
        ),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "a\nb\nc\n");
    assert!(
        !codes(&plan.warnings).contains(&"EOL_ADAPTED"),
        "{:?}",
        plan.warnings
    );
    // Line spans with an expect holding CR.
    let plan = compile(
        &request(&base, vec![span("s", "r1..r2", Some("a\r\nb"), "x\ny")]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "x\ny\r\nc\r\n");
    // Without expect a span is adapted (stated rule).
    let plan = compile(
        &request(&base, vec![span("s", "r0", None, "a\nb\n")]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "a\r\nb\r\n");
    assert_eq!(codes(&plan.warnings), ["EOL_ADAPTED"]);
    // With an LF-only expect, adapted.
    let plan = compile(
        &request(&base, vec![span("s", "r1..r2", Some("a\nb"), "x\ny")]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "x\r\ny\r\nc\r\n");
}

#[test]
fn lines_and_after_are_never_literal_by_expect() {
    let base = snapshot(
        "crlf.txt".into(),
        "alpha one\r\nbeta two\r\ngamma\r\n".into(),
    );
    let lines = Change {
        id: "l".into(),
        target: Target::Lines {
            lines: [1, 2],
            expect: Some("alpha one\r\nbeta two\r\n".into()),
            expect_last: None,
        },
        text: "x\ny".into(),
    };
    let plan = compile(&request(&base, vec![lines]), &bases(&base)).unwrap();
    assert_eq!(plan.files[0].output, "x\r\ny\r\ngamma\r\n");
    let insert = Change {
        id: "i".into(),
        target: Target::Insert {
            after: 1,
            expect: Some("alpha one\r\n".into()),
        },
        text: "x\ny".into(),
    };
    let plan = compile(&request(&base, vec![insert]), &bases(&base)).unwrap();
    assert_eq!(
        plan.files[0].output,
        "alpha one\r\nx\r\ny\r\nbeta two\r\ngamma\r\n"
    );
}

#[test]
fn eol_adapted_lists_only_adapted_changes_and_count_in_scope_stays_literal() {
    let base = snapshot("crlf.txt".into(), "a\r\nb\r\nc\r\nd\r\n".into());
    let scoped = Change {
        id: "lit".into(),
        target: Target::All {
            old: "\r\n".into(),
            scope: None,
            expected: 2,
            lines: Some([1, 2]),
        },
        text: "\n".into(),
    };
    let plan = compile(
        &request(&base, vec![scoped, exact("ad", "c\nd", "C\nD")]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "a\nb\nC\r\nD\r\n");
    let warning = plan
        .warnings
        .iter()
        .find(|w| w.code == "EOL_ADAPTED")
        .expect("adapted");
    assert!(
        warning.message.starts_with("Change ad:"),
        "{}",
        warning.message
    );
    assert_eq!(warning.change_id.as_deref(), Some("ad"));
}

#[test]
fn whitespace_edge_judges_literal_text_as_written() {
    // old holds CR, so new is literal; the dropped trailing space joins text.
    let base = snapshot("crlf.txt".into(), "x = a \r\nb\r\n".into());
    let plan = compile(
        &request(&base, vec![exact("c", "a \r\nb", "a\nb")]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "x = a\nb\r\n");
}

/// A storage commit of a tampered plan whose literal-by-old replacement was
/// swapped for its CRLF-adapted form, which the compiler never derives.
#[test]
fn validate_plan_rejects_an_adapted_replacement_for_a_literal_change() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("file.txt");
    fs::write(&target, "a\r\nb\r\n").unwrap();
    let storage = Storage::open(directory.path()).unwrap();
    let _lock = storage.lock().unwrap();
    let base = snapshot(
        storage
            .resolve(Path::new("file.txt"))
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        storage.read(Path::new("file.txt")).unwrap(),
    );
    let plan = compile(
        &request(&base, vec![count("c", "\r\n", "\n", 2)]),
        &bases(&base),
    )
    .unwrap();
    assert_eq!(plan.files[0].output, "a\nb\n");
    // Arbitrary tampering is rejected.
    let mut other = plan.clone();
    for replacement in &mut other.files[0].replacements {
        replacement.text = "\n\n".into();
    }
    other.files[0].output = "a\n\nb\n\n".into();
    assert_eq!(storage.commit(&other).unwrap_err().code, "INVALID_PLAN");
    // The adapted form, the no-op the fix removed, must be rejected too.
    let mut adapted = plan.clone();
    for replacement in &mut adapted.files[0].replacements {
        replacement.text = "\r\n".into();
    }
    adapted.files[0].output = "a\r\nb\r\n".into();
    let outcome = storage.commit(&adapted);
    assert_eq!(
        outcome
            .as_ref()
            .map(|r| r.commit)
            .map_err(|e| e.code.clone()),
        Err("INVALID_PLAN".into()),
        "{outcome:?}"
    );
}

#[test]
fn validate_plan_accepts_compiled_literal_changes() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("file.txt");
    fs::write(&target, "a\r\nb\r\n").unwrap();
    let storage = Storage::open(directory.path()).unwrap();
    let _lock = storage.lock().unwrap();
    let base = snapshot(
        storage
            .resolve(Path::new("file.txt"))
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        storage.read(Path::new("file.txt")).unwrap(),
    );
    let plan = compile(
        &request(&base, vec![span("s", "r0", Some("a\r\nb\r\n"), "A\nB\n")]),
        &bases(&base),
    )
    .unwrap();
    let receipt = storage.commit(&plan).unwrap();
    assert_eq!(receipt.commit, CommitStatus::Committed);
    assert_eq!(fs::read(&target).unwrap(), b"A\nB\n");
}

// ---------- workspace ----------

fn setup(files: &[(&str, &str)]) -> (TempDir, Workspace) {
    let dir = TempDir::new().unwrap();
    for (name, text) in files {
        fs::write(dir.path().join(name), text).unwrap();
    }
    let workspace = Workspace::open(dir.path()).unwrap();
    (dir, workspace)
}

fn at(path: &str, changes: Vec<Change>) -> FileRequest {
    FileRequest {
        path: Some(path.into()),
        base: String::new(),
        changes,
    }
}

fn path_request(files: Vec<FileRequest>) -> EditRequest {
    EditRequest {
        request_id: String::new(),
        files,
    }
}

fn receipt(result: EditResult) -> (Receipt, bool) {
    match result {
        EditResult::Completed {
            receipt, replayed, ..
        } => (receipt, replayed),
        other => panic!("Expected a receipt, got {other:?}"),
    }
}

fn rejected(result: EditResult) -> Preparation {
    match result {
        EditResult::Rejected { preparation } => preparation,
        other => panic!("Expected a rejection, got {other:?}"),
    }
}

#[test]
fn a_crlf_to_lf_conversion_replays_and_undoes_to_the_original_bytes() {
    let (dir, workspace) = setup(&[("a.txt", "a\r\nb\r\n")]);
    let path = dir.path().join("a.txt");
    let make = || path_request(vec![at("a.txt", vec![count("", "\r\n", "\n", 2)])]);
    let (done, replayed) = receipt(workspace.edit(make()).unwrap());
    assert!(!replayed);
    assert!(done.warnings.iter().all(|w| w.code != "EOL_ADAPTED"));
    assert_eq!(fs::read(&path).unwrap(), b"a\nb\n");
    let (again, replayed) = receipt(workspace.edit(make()).unwrap());
    assert!(replayed);
    assert_eq!(again.plan_id, done.plan_id);
    assert_eq!(fs::read(&path).unwrap(), b"a\nb\n");
    let (undone, _) = receipt(workspace.undo(&done.plan_id, "").unwrap());
    assert_eq!(undone.commit, CommitStatus::Committed);
    assert_eq!(fs::read(&path).unwrap(), b"a\r\nb\r\n");
    // The same request again after undo replays; it does not apply twice.
    let (third, replayed) = receipt(workspace.edit(make()).unwrap());
    assert!(replayed);
    assert_eq!(third.plan_id, done.plan_id);
    assert_eq!(fs::read(&path).unwrap(), b"a\r\nb\r\n");
}

#[test]
fn a_normal_draft_still_repairs_and_an_undo_draft_is_refused_for_any_id() {
    let (dir, workspace) = setup(&[("a.txt", "x x\n")]);
    // A rejected edit with an explicit ID repairs as before.
    let mut req = path_request(vec![at("a.txt", vec![exact("c", "x", "y")])]);
    req.request_id = "edit-1".into();
    let draft = rejected(workspace.edit(req).unwrap());
    let fixed = workspace
        .repair(&draft.reference, "", vec![count("c", "x", "y", 2)])
        .unwrap();
    assert!(fixed.ready);
    workspace.commit(&fixed.reference).unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "y y\n"
    );
    // An undo with an explicit ID, refused, cannot be repaired under an explicit ID either.
    let plan = fixed.reference.clone();
    fs::write(dir.path().join("a.txt"), "z\n").unwrap();
    let undo = rejected(workspace.undo(&plan, "undo-1").unwrap());
    let error = workspace
        .repair(
            &undo.reference,
            "repair-1",
            vec![span("undo-1", "r0", None, "x x\n")],
        )
        .unwrap_err();
    assert_eq!(error.code, "INVALID_REFERENCE");
    assert!(error.message.chars().count() <= 240);
}

#[test]
fn after_a_refused_undo_repair_the_documented_recovery_works() {
    // docs/reference.md: "an undo's draft cannot be repaired (INVALID_REFERENCE);
    // undo the plan again instead." Undoing again the way the first undo was made
    // (request_id omitted, as ultra_edit_undo allows) replays the stale rejection,
    // so the reference must say a new request_id is needed, as the error does.
    let (dir, workspace) = setup(&[("a.txt", "x\n")]);
    let path = dir.path().join("a.txt");
    let (done, _) = receipt(
        workspace
            .edit(path_request(vec![at("a.txt", vec![exact("", "x", "y")])]))
            .unwrap(),
    );
    fs::write(&path, "other\n").unwrap();
    let draft = rejected(workspace.undo(&done.plan_id, "").unwrap());
    let error = workspace
        .repair(
            &draft.reference,
            "",
            vec![span("undo-1", "r0", None, "x\n")],
        )
        .unwrap_err();
    assert_eq!(error.code, "INVALID_REFERENCE");
    assert!(error.message.contains("new request_id"));
    fs::write(&path, "y\n").unwrap();
    let again = rejected(workspace.undo(&done.plan_id, "").unwrap());
    assert!(again.replayed);
    assert_eq!(fs::read_to_string(&path).unwrap(), "y\n");
    let reference = include_str!("../docs/reference.md");
    let at = reference
        .find("cannot be repaired (`INVALID_REFERENCE`)")
        .unwrap();
    let sentence = &reference[at..at + 200];
    assert!(
        sentence.contains("request_id"),
        "docs omit the new request_id: {sentence:?}"
    );
}

#[test]
fn an_undo_plan_without_receipt_is_still_not_repairable() {
    // Evidence kinds: a ready undo always commits in the same call, so its plan has
    // a receipt and REPAIR_CLOSED applies first.
    let (dir, workspace) = setup(&[("a.txt", "x\n")]);
    let (done, _) = receipt(
        workspace
            .edit(path_request(vec![at("a.txt", vec![exact("", "x", "y")])]))
            .unwrap(),
    );
    let (undone, _) = receipt(workspace.undo(&done.plan_id, "").unwrap());
    let error = workspace
        .repair(&undone.plan_id, "", vec![span("undo-1", "r0", None, "q\n")])
        .unwrap_err();
    assert_eq!(error.code, "REPAIR_CLOSED");
    let Evidence::Plan(_) = workspace.evidence(&undone.plan_id).unwrap() else {
        panic!("plan");
    };
    assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "x\n");
}

// ---------- MCP ----------

struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<Value>,
    next_id: u64,
}

impl Client {
    fn start(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ultra-edit-mcp"))
            .arg("--root")
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (sender, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let value: Value = serde_json::from_str(&line.unwrap()).unwrap();
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            output,
            next_id: 0,
        };
        client.rpc(
            "initialize",
            json!({"protocolVersion":"2025-11-25","capabilities":{},
                "clientInfo":{"name":"t","version":"1"}}),
        );
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }

    fn send(&mut self, value: Value) {
        let input = self.input.as_mut().unwrap();
        serde_json::to_writer(&mut *input, &value).unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        loop {
            let response = self.output.recv_timeout(Duration::from_secs(15)).unwrap();
            if response.get("id").is_some() {
                return response;
            }
        }
    }

    fn call(&mut self, name: &str, arguments: Value) -> (bool, Value) {
        let response = self.rpc("tools/call", json!({"name":name,"arguments":arguments}));
        let result = &response["result"];
        (
            result["isError"].as_bool().unwrap_or(false),
            result["structuredContent"].clone(),
        )
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn mcp_repair_of_an_undo_draft_is_a_tool_error() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("a.txt");
    fs::write(&path, "a\r\nb\r\n").unwrap();
    let mut client = Client::start(root.path());
    let (error, done) = client.call(
        "ultra_edit",
        json!({"files":[{"path":path,"changes":[{"old":"\r\n","new":"\n","count":2}]}]}),
    );
    assert!(!error, "{done}");
    assert_eq!(fs::read(&path).unwrap(), b"a\nb\n");
    fs::write(&path, "other\n").unwrap();
    let (error, undo) = client.call("ultra_edit_undo", json!({"plan":done["plan_id"]}));
    assert!(error, "{undo}");
    let (error, repaired) = client.call(
        "ultra_edit_repair",
        json!({"reference":undo["reference"],"changes":[{"id":"undo-1","span":"r0","new":"a\r\nb\r\n"}]}),
    );
    assert!(error, "{repaired}");
    assert_eq!(repaired["error"]["code"], "INVALID_REFERENCE", "{repaired}");
    fs::write(&path, "a\nb\n").unwrap();
    let (error, undone) = client.call(
        "ultra_edit_undo",
        json!({"plan":done["plan_id"],"request_id":"undo-2"}),
    );
    assert!(!error, "{undone}");
    assert_eq!(fs::read(&path).unwrap(), b"a\r\nb\r\n");
}

#[test]
fn a_0_3_0_repair_of_an_undo_draft_cannot_be_repaired_into_a_no_op() {
    // A 0.3.0 store could hold a repair of an undo's draft (0.3.0 allowed it).
    // Repairing that repair now compiles with line-ending adaptation.
    let (dir, workspace) = setup(&[("a.txt", "a\nb\n")]);
    let path = dir.path().join("a.txt");
    let (done, _) = receipt(
        workspace
            .edit(path_request(vec![at(
                "a.txt",
                vec![count("", "\n", "\r\n", 2)],
            )]))
            .unwrap(),
    );
    assert_eq!(fs::read(&path).unwrap(), b"a\r\nb\r\n");
    fs::write(&path, "x\r\n").unwrap();
    let undo = rejected(workspace.undo(&done.plan_id, "").unwrap());
    // Simulate the 0.3.0-era repair: its draft and its binding.
    let restore = span("undo-1", "r0", None, "a\nb\n");
    let storage = Storage::open(dir.path()).unwrap();
    let repaired_id = "d0123456789abcdef0123456789abcdef";
    {
        let _lock = storage.lock().unwrap();
        let mut draft: Value = storage.get("drafts", &undo.reference).unwrap();
        draft["id"] = json!(repaired_id);
        draft["request"]["request_id"] = json!("old-repair");
        storage.put("drafts", repaired_id, &draft).unwrap();
        storage
            .put(
                "requests",
                &digest(b"old-repair"),
                &json!({
                    "input": {"kind":"repair","reference":undo.reference,
                        "request_id":"old-repair","changes":[serde_json::to_value(&restore).unwrap()]},
                    "reference": repaired_id,
                }),
            )
            .unwrap();
    }
    fs::write(&path, "a\r\nb\r\n").unwrap();
    let outcome = workspace.repair(repaired_id, "new-repair", vec![restore]);
    match outcome {
        Err(error) => assert_eq!(error.code, "INVALID_REFERENCE"),
        Ok(preparation) => {
            let Evidence::Plan(plan) = workspace.evidence(&preparation.reference).unwrap() else {
                panic!("draft: {preparation:?}");
            };
            assert_eq!(
                plan.files[0].output, "a\nb\n",
                "a repaired undo plans a no-op: {:?}",
                plan.warnings
            );
        }
    }
}
