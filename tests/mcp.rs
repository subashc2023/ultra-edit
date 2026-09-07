use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;
use ultra_edit::{PreparedPlan, digest};

struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<Value>,
    next_id: u64,
}

impl Client {
    fn start(root: &Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ultra-edit-mcp"));
        command.arg("--root").arg(root);
        Self::connect(command)
    }

    fn connect(mut command: Command) -> Self {
        let mut child = command
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
                let line = line.unwrap();
                let value = serde_json::from_str(&line)
                    .unwrap_or_else(|error| panic!("Non-protocol stdout: {error}: {line}"));
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
        let initialized = client.rpc(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "ultra-edit-test", "version": "1"},
            }),
        );
        assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
        assert!(initialized["result"]["capabilities"]["tools"].is_object());
        let instructions = initialized["result"]["instructions"].as_str().unwrap();
        assert!(instructions.starts_with("ALWAYS use these direct MCP tools"));
        assert!(
            instructions.len() <= 2_000,
            "Claude truncates server instructions"
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
            assert_eq!(response["jsonrpc"], "2.0");
            if response.get("id").is_some() {
                assert_eq!(response["id"], id, "{response}");
                return response;
            }
        }
    }

    fn call(&mut self, name: &str, arguments: Value, is_error: bool) -> Value {
        let response = self.rpc("tools/call", json!({"name":name,"arguments":arguments}));
        assert!(response.get("error").is_none(), "{response}");
        let result = &response["result"];
        assert_eq!(
            result["isError"].as_bool().unwrap_or(false),
            is_error,
            "{result}"
        );
        let content = result["structuredContent"].clone();
        assert!(content.is_object(), "{result}");
        let fallback: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(fallback, content);
        content
    }

    fn range(&mut self, path: &str, first: usize, last: usize) -> Value {
        self.call(
            "ultra_edit_snapshot",
            json!({"path":path,"selection":{"kind":"range","first":first,"last":last}}),
            false,
        )
    }

    fn full(&mut self, path: &str) -> Value {
        self.call(
            "ultra_edit_snapshot",
            json!({"path":path,"selection":{"kind":"full"}}),
            false,
        )
    }

    fn close(mut self) {
        drop(self.input.take());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Server did not exit after stdin EOF"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn edit(request_id: &str, base: &Value, span: &str, text: &str) -> Value {
    json!({"request_id":request_id,"files":[{"base":base,"changes":[{
        "id":"change","target":{"kind":"span","span":span},"text":text
    }]}]})
}

#[test]
fn startup_requires_explicit_root_and_keeps_protocol_stdout_clean() {
    let root = TempDir::new().unwrap();
    for args in [
        vec![],
        vec!["--root"],
        vec!["--root", "missing-workspace"],
        vec!["--help", "extra"],
        vec!["--claude-context"],
        vec!["--claude-context", "Bash"],
        vec!["--claude-context", "SessionStart", "extra"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ultra-edit-mcp"))
            .current_dir(root.path())
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
        assert!(!root.path().join(".ultra-edit").exists());
    }
    for argument in ["--help", "--version"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ultra-edit-mcp"))
            .current_dir(root.path())
            .arg(argument)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(!output.stdout.is_empty());
        assert!(!root.path().join(".ultra-edit").exists());
    }
    let mut client = Client::start(root.path());
    let listed = client.rpc("tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().unwrap();
    for name in [
        "ultra_edit_snapshot",
        "ultra_edit",
        "ultra_edit_status",
        "ultra_edit_prepare",
        "ultra_edit_commit",
        "ultra_edit_repair",
        "ultra_edit_undo",
    ] {
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        for keyword in ["oneOf", "anyOf", "allOf"] {
            assert!(
                tool["inputSchema"].get(keyword).is_none(),
                "Claude requires object-root schemas: {tool}"
            );
        }
        assert!(!tool["description"].as_str().unwrap().is_empty());
    }
    assert_eq!(client.rpc("ping", json!({}))["result"], json!({}));
    assert!(client.rpc("unsupported/method", json!({}))["error"].is_object());
    client.close();
}

#[test]
fn buffered_custom_notifications_do_not_block_valid_requests() {
    let root = TempDir::new().unwrap();
    let mut client = Client::start(root.path());
    assert_eq!(client.rpc("ping", json!({}))["result"], json!({}));
    let buffered = format!(
        "\u{feff}{}\r\n{}\r\n",
        json!({"jsonrpc":"2.0","method":"notifications/custom","params":{}}),
        json!({"jsonrpc":"2.0","id":10000,"method":"ping"})
    );
    let input = client.input.as_mut().unwrap();
    input.write_all(buffered.as_bytes()).unwrap();
    input.flush().unwrap();
    let response = client.output.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(response["id"], 10000);
    assert_eq!(response["result"], json!({}));
    client.close();
}

#[test]
fn malformed_frames_close_without_stalling_buffered_requests() {
    for malformed in [
        json!({"method":"notifications/custom","params":{}}),
        json!({"jsonrpc":"2.0","method":"notifications/custom","params":[]}),
    ] {
        let root = TempDir::new().unwrap();
        let mut client = Client::start(root.path());
        assert_eq!(client.rpc("ping", json!({}))["result"], json!({}));
        let buffered = format!(
            "{malformed}\n{}\n",
            json!({"jsonrpc":"2.0","id":10000,"method":"ping"})
        );
        let input = client.input.as_mut().unwrap();
        input.write_all(buffered.as_bytes()).unwrap();
        input.flush().unwrap();
        assert!(matches!(
            client.output.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        client.close();
    }
}

#[test]
fn bundled_plugin_launches_hooks_and_mcp_without_path_lookup() {
    let root = TempDir::new().unwrap();
    let package = tempfile::Builder::new()
        .prefix("ultra edit's $ bundle ")
        .tempdir()
        .unwrap();
    fs::create_dir(package.path().join("runtime")).unwrap();
    fs::copy(
        env!("CARGO_BIN_EXE_ultra-edit-mcp"),
        package.path().join(format!(
            "runtime/ultra-edit-mcp{}",
            std::env::consts::EXE_SUFFIX
        )),
    )
    .unwrap();
    let executable = |configuration: &Value| {
        let path = configuration["command"].as_str().unwrap();
        assert!(path.starts_with("${CLAUDE_PLUGIN_ROOT}/runtime/"));
        let mut command =
            Command::new(path.replace("${CLAUDE_PLUGIN_ROOT}", package.path().to_str().unwrap()));
        for argument in configuration["args"].as_array().unwrap() {
            command.arg(
                argument
                    .as_str()
                    .unwrap()
                    .replace("${CLAUDE_PROJECT_DIR}", root.path().to_str().unwrap()),
            );
        }
        command.current_dir(root.path()).env("PATH", "");
        command
    };
    let configuration: Value =
        serde_json::from_str(include_str!("../plugin/claude-code/hooks/hooks.json")).unwrap();
    for event in ["SessionStart", "SubagentStart"] {
        let entry = &configuration["hooks"][event][0];
        // All session sources (including compaction/fork) and agent types need it.
        assert!(entry.get("matcher").is_none());
        let handler = &entry["hooks"][0];
        assert_eq!(handler["type"], "command");
        assert_ne!(handler["async"], true);
        let output = executable(handler).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["hookSpecificOutput"]["hookEventName"], event);
        let context = value["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert_eq!(
            context,
            include_str!("../plugin/claude-code/instructions.md")
        );
        assert!(
            context.len() < 10_000,
            "Claude clips oversized hook context"
        );
        assert!(fs::read_dir(root.path()).unwrap().next().is_none());
    }
    let configuration: Value =
        serde_json::from_str(include_str!("../plugin/claude-code/.mcp.json")).unwrap();
    let mut client = Client::connect(executable(&configuration["mcpServers"]["ultra-edit"]));
    assert_eq!(client.rpc("ping", json!({}))["result"], json!({}));
    let listed = client.rpc("tools/list", json!({}));
    assert!(
        listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "ultra_edit")
    );
    client.close();
}

#[test]
fn focused_batch_edit_preserves_bytes_and_retries_after_restart() {
    let root = TempDir::new().unwrap();
    let first = "\u{feff}private header\r\nlet value = 1;\nprivate footer";
    fs::write(root.path().join("one.txt"), first).unwrap();
    fs::write(root.path().join("two.txt"), "α\told  \r\n").unwrap();
    let mut client = Client::start(root.path());
    let one = client.range("one.txt", 2, 2);
    let two = client.range("two.txt", 1, 1);
    assert_eq!(one["text"], "let value = 1;");
    assert!(!one.to_string().contains("private"));
    let literal = "let value = '$1\\path\t😀';  ";
    let mut request = edit("batch", &one["snapshot"], "selection", literal);
    request["files"]
        .as_array_mut()
        .unwrap()
        .push(json!({"base":two["snapshot"],"changes":[{
            "id":"second","target":{"kind":"exact","old":"old","scope":"selection"},"text":"new"
        }]}));
    let committed = client.call("ultra_edit", request.clone(), false);
    assert_eq!(committed["commit"], "committed");
    assert_eq!(committed["request_id"], "batch");
    assert_eq!(
        fs::read_to_string(root.path().join("one.txt")).unwrap(),
        format!("\u{feff}private header\r\n{literal}\nprivate footer")
    );
    assert_eq!(
        fs::read_to_string(root.path().join("two.txt")).unwrap(),
        "α\tnew  \r\n"
    );
    assert!(committed.get("files").is_none());
    client.close();

    let mut client = Client::start(root.path());
    let status = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"receipt","request_id":"batch"}}),
        false,
    );
    assert_eq!(status, committed);
    assert_eq!(client.call("ultra_edit", request.clone(), false), committed);
    let receipt = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"receipt","request_id":"batch","full":true}}),
        false,
    );
    assert_eq!(receipt["receipt"]["files"].as_array().unwrap().len(), 2);
    assert_eq!(receipt["receipt"]["validation"], "not_requested");
    let evidence = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"evidence","reference":one["snapshot"]}}),
        false,
    );
    assert_eq!(evidence["value"]["text"], first);
    request["files"][0]["changes"][0]["text"] = json!("different");
    assert_eq!(
        client.call("ultra_edit", request, true)["error"]["code"],
        "REQUEST_ID_REUSED"
    );
    client.close();
}

#[test]
fn stale_file_rejects_whole_batch_and_never_refreshes_its_base() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("one.txt"), "original\n").unwrap();
    fs::write(root.path().join("two.txt"), "target\nfooter").unwrap();
    let mut client = Client::start(root.path());
    let one = client.range("one.txt", 1, 1);
    let two = client.range("two.txt", 1, 1);
    let mut request = edit("stale", &one["snapshot"], "selection", "changed");
    request["files"]
        .as_array_mut()
        .unwrap()
        .push(json!({"base":two["snapshot"],"changes":[{
            "id":"second","target":{"kind":"span","span":"selection"},"text":"changed"
        }]}));
    fs::write(root.path().join("two.txt"), "target\nexternal change").unwrap();
    let result = client.call("ultra_edit", request, true);
    assert_eq!(result["kind"], "rejected");
    assert!(
        result["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "STALE_SNAPSHOT")
    );
    assert_eq!(
        fs::read_to_string(root.path().join("one.txt")).unwrap(),
        "original\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("two.txt")).unwrap(),
        "target\nexternal change"
    );
    let unavailable = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"receipt","request_id":"stale"}}),
        false,
    );
    assert_eq!(unavailable["kind"], "receipt_unavailable");
    assert!(unavailable["receipt"].is_null());
    let unknown = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"receipt","request_id":"never-seen"}}),
        true,
    );
    assert_eq!(unknown["error"]["code"], "UNKNOWN_REQUEST");
    client.close();
}

#[test]
fn preview_repair_commit_and_conditional_undo_use_retained_plans() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("file.txt"), "x x\r\n").unwrap();
    let mut client = Client::start(root.path());
    let snapshot = client.full("file.txt");
    let request = json!({"request_id":"ambiguous","files":[{"base":snapshot["id"],"changes":[{
        "id":"change","target":{"kind":"exact","old":"x"},"text":"y"
    }]}]});
    let rejected = client.call("ultra_edit_prepare", request, true);
    assert_eq!(rejected["kind"], "rejected");
    let repaired = client.call(
        "ultra_edit_repair",
        json!({"reference":rejected["reference"],"request_id":"fixed","changes":[{
            "id":"change","target":{"kind":"all","old":"x","scope":"r0","expected":2},"text":"y"
        }]}),
        false,
    );
    assert_eq!(repaired["kind"], "ready");
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "x x\r\n"
    );
    let plan = repaired["reference"].clone();
    let committed = client.call("ultra_edit_commit", json!({"plan":plan}), false);
    assert_eq!(committed["commit"], "committed");
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "y y\r\n"
    );
    client.close();
    let mut client = Client::start(root.path());
    let undone = client.call(
        "ultra_edit_undo",
        json!({"plan":plan,"request_id":"undo"}),
        false,
    );
    assert_eq!(undone["commit"], "committed");
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "x x\r\n"
    );
    assert_eq!(
        client.call(
            "ultra_edit_undo",
            json!({"plan":plan,"request_id":"undo"}),
            false
        ),
        undone
    );
    client.close();
}

#[test]
fn search_discloses_only_editable_matches_and_empty_files_allow_insertion() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("file.txt"), "header\nα α\nfooter").unwrap();
    fs::write(root.path().join("empty.txt"), "").unwrap();
    let mut client = Client::start(root.path());
    let found = client.call(
        "ultra_edit_snapshot",
        json!({"path":"file.txt","selection":{"kind":"search","query":"α"}}),
        false,
    );
    assert_eq!(found["total_matches"], 2);
    assert!(!found.to_string().contains("footer"));
    let bad = client.call(
        "ultra_edit",
        edit("undisclosed", &found["snapshot"], "r0", "bad"),
        true,
    );
    assert_eq!(bad["kind"], "rejected");
    client.call(
        "ultra_edit",
        edit(
            "match",
            &found["snapshot"],
            found["matches"][1]["span"]["id"].as_str().unwrap(),
            "β",
        ),
        false,
    );
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "header\nα β\nfooter"
    );
    let empty = client.range("empty.txt", 1, 1);
    client.call(
        "ultra_edit",
        edit("insert", &empty["snapshot"], "selection", "new\r\n"),
        false,
    );
    assert_eq!(fs::read(root.path().join("empty.txt")).unwrap(), b"new\r\n");
    client.close();
}

#[test]
fn malformed_arguments_and_outside_paths_fail_without_writes() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("workspace");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file.txt"), "x").unwrap();
    fs::write(parent.path().join("outside.txt"), "outside").unwrap();
    let mut client = Client::start(&root);
    let snapshot = client.full("file.txt");
    let mut invalid_edit = edit("invalid", &snapshot["id"], "r0", "bad");
    invalid_edit["force"] = json!(true);
    for (name, arguments) in [
        ("ultra_edit", invalid_edit),
        (
            "ultra_edit",
            json!({"request_id":"bad","files":"wrong type"}),
        ),
        (
            "ultra_edit_snapshot",
            json!({"path":"file.txt","root":parent.path(),"selection":{"kind":"full"}}),
        ),
        (
            "ultra_edit_snapshot",
            json!({"path":"file.txt","selection":{"kind":"range","first":-1,"last":1}}),
        ),
        (
            "ultra_edit_snapshot",
            json!({"path":"file.txt","selection":{"kind":"search","query":"x","first":1}}),
        ),
        ("not_a_tool", json!({})),
    ] {
        let response = client.rpc("tools/call", json!({"name":name,"arguments":arguments}));
        assert!(
            response["error"].is_object() || response["result"]["isError"] == true,
            "{response}"
        );
        assert_eq!(fs::read_to_string(root.join("file.txt")).unwrap(), "x");
    }
    for path in ["../outside.txt", ".ultra-edit/coordinator.lock"] {
        let rejected = client.call(
            "ultra_edit_snapshot",
            json!({"path":path,"selection":{"kind":"full"}}),
            true,
        );
        assert_eq!(rejected["error"]["code"], "PATH_OUTSIDE_WORKSPACE");
    }
    assert_eq!(
        fs::read_to_string(parent.path().join("outside.txt")).unwrap(),
        "outside"
    );
    client.close();
}

#[test]
fn interrupted_commit_and_corrupt_journal_preserve_unknown_status_and_references() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("file.txt"), "x").unwrap();
    let mut client = Client::start(root.path());
    let snapshot = client.full("file.txt");
    let request = edit("interrupted", &snapshot["id"], "r0", "xx");
    let preview = client.call("ultra_edit_prepare", request.clone(), false);
    let evidence = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"evidence","reference":preview["reference"]}}),
        false,
    );
    let plan: PreparedPlan = serde_json::from_value(evidence["value"].clone()).unwrap();
    client.close();
    // Same durable crash fixture as CLI recovery: intent persisted, outcome absent.
    let mut journal = Vec::new();
    for payload in [
        json!({"event":"begin","plan_id":plan.id,"plan_digest":digest(&serde_json::to_vec(&plan).unwrap()),"file_count":1}),
        json!({"event":"intent","index":0}),
    ] {
        let checksum = digest(&serde_json::to_vec(&payload).unwrap());
        journal
            .extend(serde_json::to_vec(&json!({"checksum":checksum,"payload":payload})).unwrap());
        journal.push(b'\n');
    }
    let journal_path = root
        .path()
        .join(".ultra-edit/journals")
        .join(format!("{}.jsonl", plan.id));
    fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
    fs::write(&journal_path, &journal).unwrap();
    fs::write(root.path().join("file.txt"), "xx").unwrap();
    let mut client = Client::start(root.path());
    let result = client.call("ultra_edit", request.clone(), true);
    assert_eq!(result["commit"], "outcome_unknown");
    assert_eq!(result["plan_id"], plan.id);
    assert_eq!(result["request_id"], "interrupted");
    let receipt = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"receipt","request_id":"interrupted","full":true}}),
        false,
    );
    assert_eq!(receipt["receipt"]["files"][0]["status"], "outcome_unknown");
    assert_eq!(receipt["receipt"]["files"][0]["changes_applied"], 0);
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "xx"
    );
    assert_eq!(fs::read(&journal_path).unwrap(), journal);
    client.close();
    journal.extend_from_slice(b"corrupt\n");
    fs::write(&journal_path, &journal).unwrap();
    let mut client = Client::start(root.path());
    let error = client.call("ultra_edit", request, true);
    assert_eq!(error["error"]["code"], "JOURNAL_CORRUPT");
    assert_eq!(error["commit"], "outcome_unknown");
    assert_eq!(error["request_id"], "interrupted");
    assert_eq!(error["plan_id"], plan.id);
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "xx"
    );
    assert_eq!(fs::read(&journal_path).unwrap(), journal);
    client.close();
}

#[test]
fn cancelled_call_can_be_recovered_without_reapplying_the_edit() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("file.txt"), "x").unwrap();
    let mut client = Client::start(root.path());
    let snapshot = client.full("file.txt");
    let request = edit("cancelled", &snapshot["id"], "r0", "xx");
    let storage = ultra_edit::storage::Storage::open(root.path()).unwrap();
    let lock = storage.lock().unwrap();
    client.send(
        json!({"jsonrpc":"2.0","id":10000,"method":"tools/call","params":{
            "name":"ultra_edit","arguments":request
        }}),
    );
    // A pending filesystem operation must not prevent protocol traffic or make
    // cancellation a promise of rollback. Recovery uses the durable request ID.
    assert_eq!(client.rpc("ping", json!({}))["result"], json!({}));
    client.send(
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{
            "requestId":10000,"reason":"Client stopped waiting"
        }}),
    );
    assert_eq!(client.rpc("ping", json!({}))["result"], json!({}));
    drop(lock);
    let recovered = client.call("ultra_edit", request.clone(), false);
    let status = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"receipt","request_id":"cancelled"}}),
        false,
    );
    assert_eq!(status["commit"], "committed");
    assert_eq!(recovered, status);
    assert_eq!(client.call("ultra_edit", request, false), status);
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "xx"
    );
    client.close();
}
