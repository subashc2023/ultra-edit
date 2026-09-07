use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;
use ultra_edit::storage::Storage;
use ultra_edit::{PreparedPlan, digest};

#[cfg(windows)]
use ultra_edit::{Draft, Inspection, Snapshot};

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
        Self::connect_after_prefix(&mut command, false)
    }

    fn connect_after_prefix(command: &mut Command, malformed_prefix: bool) -> Self {
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
        if malformed_prefix {
            let input = client.input.as_mut().unwrap();
            input.write_all(b"{\"invalid\":\"\\ud800\"}\n").unwrap();
            input.flush().unwrap();
            let error = client.output.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(error["error"]["code"], -32700);
            assert!(error["id"].is_null());
        }
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
        "ultra_edit_retry",
        "ultra_edit_diff",
        "ultra_edit_inspect",
        "ultra_edit_reconcile",
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
fn malformed_frames_return_protocol_errors_and_continue_buffered_requests() {
    for (malformed, code, id) in [
        (r#"{"jsonrpc":"2.0","id":99,"method":"tools/call","params":{"name":"ultra_edit_prepare","arguments":{"text":"a\ud800b"}}}"#.to_owned(), -32700, json!(null)),
        ("{broken".to_owned(), -32700, json!(null)),
        ("[]".to_owned(), -32600, json!(null)),
        (json!({"method":"notifications/custom","params":{}}).to_string(), -32600, json!(null)),
        (json!({"jsonrpc":"2.0","id":"invalid-id","method":"notifications/custom","params":[]}).to_string(), -32600, json!("invalid-id")),
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
        let error = client.output.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(error["error"]["code"], code, "{error}");
        assert_eq!(error["id"], id, "{error}");
        let response = client.output.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(response["id"], 10000);
        assert_eq!(response["result"], json!({}));
        assert_eq!(client.rpc("ping", json!({}))["result"], json!({}));
        assert!(client.rpc("tools/list", json!({}))["result"]["tools"].is_array());
        client.close();
    }
}

#[test]
fn concurrent_replies_do_not_drop_or_interleave_parse_errors() {
    let root = TempDir::new().unwrap();
    let mut client = Client::start(root.path());
    let mut buffered = String::new();
    for id in 10000..10064 {
        buffered.push_str(&json!({"jsonrpc":"2.0","id":id,"method":"ping"}).to_string());
        buffered.push_str("\n{broken\n");
    }
    client
        .input
        .as_mut()
        .unwrap()
        .write_all(buffered.as_bytes())
        .unwrap();
    client.input.as_mut().unwrap().flush().unwrap();
    let mut ids = std::collections::BTreeSet::new();
    let mut errors = 0;
    for _ in 0..128 {
        let response = client.output.recv_timeout(Duration::from_secs(5)).unwrap();
        if response["id"].is_null() {
            assert_eq!(response["error"]["code"], -32700, "{response}");
            errors += 1;
        } else {
            assert_eq!(response["result"], json!({}), "{response}");
            assert!(
                ids.insert(response["id"].as_u64().unwrap()),
                "Duplicate response: {response}"
            );
        }
    }
    assert_eq!(errors, 64);
    assert_eq!(ids, (10000..10064).collect());
    client.close();
}

#[test]
fn transport_framing_failure_exits_unsuccessfully() {
    for payload in [vec![0xff, b'\n'], vec![b'x'; 16 * 1024 * 1024 + 1]] {
        let root = TempDir::new().unwrap();
        let mut client = Client::start(root.path());
        // Oversized input may be rejected before the writer has sent its tail.
        let _ = client.input.as_mut().unwrap().write_all(&payload);
        let _ = client.input.as_mut().unwrap().flush();
        assert!(matches!(
            client.output.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        drop(client.input.take());
        assert!(!client.child.wait().unwrap().success());
    }
}

#[test]
fn broken_stdout_exits_without_panic_while_stdin_stays_open() {
    for malformed in [false, true] {
        let root = TempDir::new().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_ultra-edit-mcp"))
            .arg("--root")
            .arg(root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            sender.send((line, reader)).unwrap();
        });
        writeln!(
            input,
            "{}",
            json!({
                "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                    "protocolVersion":"2025-11-25","capabilities":{},
                    "clientInfo":{"name":"broken-output-test","version":"1"}
                }
            })
        )
        .unwrap();
        input.flush().unwrap();
        let (line, reader) = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["id"], 1);
        // The reader is returned here, rather than left in a background thread,
        // so dropping it really closes the server's stdout pipe.
        drop(reader);
        let payload = if malformed {
            "{broken\n".into()
        } else {
            (2..22)
                .map(|id| format!("{}\n", json!({"jsonrpc":"2.0","id":id,"method":"ping"})))
                .collect::<String>()
        };
        let _ = input.write_all(payload.as_bytes());
        let _ = input.flush();
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Broken stdout did not terminate the server while stdin remained open");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        drop(input);
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert_eq!(status.code(), Some(2), "{stderr}");
        assert!(stderr.contains("MCP output error:"), "{stderr}");
        assert!(!stderr.contains("panicked"), "{stderr}");
    }
}

#[test]
fn maximum_sized_protocol_line_is_accepted_before_the_next_message() {
    let root = TempDir::new().unwrap();
    let mut client = Client::start(root.path());
    let ping = json!({"jsonrpc":"2.0","id":10000,"method":"ping"}).to_string();
    let mut line = " ".repeat(16 * 1024 * 1024 - ping.len());
    line.push_str(&ping);
    line.push('\n');
    let input = client.input.as_mut().unwrap();
    input.write_all(line.as_bytes()).unwrap();
    input.flush().unwrap();
    let response = client.output.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(response["id"], 10000);
    assert_eq!(response["result"], json!({}));
    assert_eq!(client.rpc("ping", json!({}))["result"], json!({}));
    client.close();
}

#[test]
fn malformed_message_before_initialization_does_not_prevent_connection() {
    let root = TempDir::new().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ultra-edit-mcp"));
    command.arg("--root").arg(root.path());
    let mut client = Client::connect_after_prefix(&mut command, true);
    assert_eq!(client.rpc("ping", json!({}))["result"], json!({}));
    client.close();
}

#[test]
fn invalid_request_ids_are_not_silently_treated_as_notifications() {
    let root = TempDir::new().unwrap();
    let mut client = Client::start(root.path());
    for id in [
        json!(null),
        json!(true),
        json!(1.5),
        json!({}),
        json!([]),
        json!(u64::MAX),
    ] {
        client.send(json!({"jsonrpc":"2.0","id":id,"method":"ping","params":{}}));
        let error = client.output.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(error["error"]["code"], -32600, "{error}");
        assert!(error["id"].is_null() || error["id"] == id);
        assert_eq!(client.rpc("ping", json!({}))["result"], json!({}));
    }
    client.close();
}

#[test]
fn full_read_guard_pagination_and_scope_guards_work_over_mcp() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("large.txt"), "x\n".repeat(1000)).unwrap();
    let mut client = Client::start(root.path());
    let rejected = client.call(
        "ultra_edit_snapshot",
        json!({
            "path":"large.txt","selection":{"kind":"full"}
        }),
        true,
    );
    assert_eq!(rejected["error"]["code"], "READ_TOO_LARGE");
    assert!(
        rejected["error"]["message"]
            .as_str()
            .unwrap()
            .contains("2000")
    );
    let page = client.call(
        "ultra_edit_snapshot",
        json!({
            "path":"large.txt","selection":{"kind":"search","query":"x"}
        }),
        false,
    );
    assert_eq!(page["next_offset"], 20);
    let later = client.call("ultra_edit_snapshot", json!({
        "path":"large.txt","selection":{"kind":"search","query":"x","offset":980,"snapshot":page["snapshot"]}
    }), false);
    assert_eq!(later["matches"][0]["span"]["line"], 981);
    assert!(later["next_offset"].is_null());
    let full = client.call(
        "ultra_edit_snapshot",
        json!({
            "path":"large.txt","selection":{"kind":"full","expected_bytes":2000}
        }),
        false,
    );
    assert!(full["snapshot"].is_string());
    assert!(full.get("id").is_none());
    let mut guarded = edit("wrong-span", &later["snapshot"], "m981", "replacement");
    guarded["files"][0]["changes"][0]["target"]["expect"] = json!("wrong original");
    assert_eq!(
        client.call("ultra_edit_prepare", guarded, true)["diagnostics"][0]["code"],
        "EXPECTED_TEXT_MISMATCH"
    );
    client.close();
}

#[test]
fn diff_and_byte_warnings_are_available_before_and_after_commit() {
    let root = TempDir::new().unwrap();
    let before: String = (1..=44).map(|line| format!("line {line}\r\n")).collect();
    fs::write(root.path().join("file.txt"), &before).unwrap();
    let mut client = Client::start(root.path());
    let snapshot = client.full("file.txt");
    let request = json!({"request_id":"review","files":[{"base":snapshot["snapshot"],"changes":[
        {"id":"one","target":{"kind":"exact","old":"line 10\r\n"},"text":"changed 10\n"},
        {"id":"two","target":{"kind":"exact","old":"line 35"},"text":"changed\u{0000}35"}
    ]}]});
    let prepared = client.call("ultra_edit_prepare", request.clone(), false);
    assert_eq!(prepared["warning_count"], 2);
    let diff = client.call(
        "ultra_edit_diff",
        json!({"plan":prepared["reference"]}),
        false,
    );
    let text = diff["diff"].as_str().unwrap();
    assert!(text.contains("-line 10\r\n+changed 10\n"), "{text}");
    assert!(!text.contains("line 22"), "{text}");
    assert!(diff["next_offset"].is_null());
    let committed = client.call("ultra_edit", request, false);
    assert_eq!(committed["warning_count"], 2);
    assert_eq!(committed["warnings"], prepared["warnings"]);
    client.close();
    let mut client = Client::start(root.path());
    let status = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"receipt","request_id":"review"}}),
        false,
    );
    assert_eq!(status, committed);
    assert_eq!(
        fs::read_to_string(root.path().join("file.txt")).unwrap(),
        before
            .replace("line 10\r\n", "changed 10\n")
            .replace("line 35", "changed\u{0}35")
    );
    client.close();
}

#[cfg(windows)]
#[test]
fn canonical_windows_paths_are_display_only_across_mcp_responses() {
    let root = TempDir::new().unwrap();
    let target = root.path().join("file.txt");
    fs::write(&target, "old\r\nkeep\r\n").unwrap();
    let canonical = fs::canonicalize(&target)
        .unwrap()
        .into_os_string()
        .into_string()
        .unwrap();
    let displayed = ultra_edit::report::path_for_display(&canonical).into_owned();

    let mut client = Client::start(root.path());
    let full = client.full("file.txt");
    assert_eq!(full["path"], displayed);
    let range = client.range("file.txt", 1, 1);
    assert_eq!(range["path"], displayed);
    let search = client.call(
        "ultra_edit_snapshot",
        json!({"path":"file.txt","selection":{"kind":"search","query":"old"}}),
        false,
    );
    assert_eq!(search["path"], displayed);

    let request = json!({
        "request_id": "windows-display",
        "files": [{
            "base": full["snapshot"],
            "changes": [{
                "id": "mixed-endings",
                "target": {"kind": "exact", "old": "old"},
                "text": "new\nextra",
            }],
        }],
    });
    let prepared = client.call("ultra_edit_prepare", request.clone(), false);
    assert_eq!(prepared["warnings"][0]["file"], displayed);
    let plan_id = prepared["reference"].as_str().unwrap();
    let diff = client.call("ultra_edit_diff", json!({"plan": plan_id}), false);
    assert!(
        diff["diff"]
            .as_str()
            .unwrap()
            .starts_with(&format!("--- {:?}\n", format!("a/{displayed}"))),
        "{diff}"
    );

    let snapshot_evidence = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"evidence","reference":full["snapshot"]}}),
        false,
    );
    assert_eq!(snapshot_evidence["value"]["path"], displayed);
    let plan_evidence = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"evidence","reference":plan_id}}),
        false,
    );
    assert_eq!(
        plan_evidence["value"]["files"][0]["base"]["path"],
        displayed
    );
    assert_eq!(plan_evidence["value"]["warnings"][0]["file"], displayed);

    let rejected = client.call(
        "ultra_edit_prepare",
        json!({
            "request_id": "windows-display-draft",
            "files": [{
                "base": full["snapshot"],
                "changes": [{
                    "id": "missing",
                    "target": {"kind": "exact", "old": "absent"},
                    "text": "unused",
                }],
            }],
        }),
        true,
    );
    let draft_evidence = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"evidence","reference":rejected["reference"]}}),
        false,
    );
    assert_eq!(draft_evidence["value"]["diagnostics"][0]["file"], displayed);

    let committed = client.call("ultra_edit", request, false);
    assert_eq!(committed["warnings"][0]["file"], displayed);
    let receipt = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"receipt","request_id":"windows-display","full":true}}),
        false,
    );
    assert_eq!(receipt["receipt"]["files"][0]["path"], displayed);
    assert_eq!(receipt["receipt"]["warnings"][0]["file"], displayed);

    let inspection = client.call("ultra_edit_inspect", json!({"plan":plan_id}), false);
    assert_eq!(inspection["files"][0]["path"], displayed);
    let inspection_evidence = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"evidence","reference":inspection["inspection"]}}),
        false,
    );
    let evidence = &inspection_evidence["value"];
    assert_eq!(evidence["plan"]["files"][0]["base"]["path"], displayed);
    assert_eq!(evidence["receipt"]["files"][0]["path"], displayed);
    assert_eq!(evidence["files"][0]["path"], displayed);
    client.close();

    let storage = Storage::open(root.path()).unwrap();
    let stored_snapshot: Snapshot = storage
        .get("snapshots", full["snapshot"].as_str().unwrap())
        .unwrap();
    assert_eq!(stored_snapshot.path, canonical);
    let stored_plan: PreparedPlan = storage.get("plans", plan_id).unwrap();
    assert_eq!(stored_plan.files[0].base.path, canonical);
    assert_eq!(
        stored_plan.warnings[0].file.as_deref(),
        Some(canonical.as_str())
    );
    let stored_draft: Draft = storage
        .get("drafts", rejected["reference"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        stored_draft.diagnostics[0].file.as_deref(),
        Some(canonical.as_str())
    );
    let stored_receipt = storage.receipt(&stored_plan).unwrap().unwrap();
    assert_eq!(stored_receipt.files[0].path, canonical);
    let stored_inspection: Inspection = storage
        .get("inspections", inspection["inspection"].as_str().unwrap())
        .unwrap();
    assert_eq!(stored_inspection.files[0].path, canonical);
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
    assert!(receipt["receipt"].get("validation").is_none());
    assert!(
        !committed["report"]
            .as_str()
            .unwrap()
            .contains("Validation:")
    );
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
    let request = json!({"request_id":"ambiguous","files":[{"base":snapshot["snapshot"],"changes":[{
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
fn retry_preflight_failure_keeps_original_receipt_and_candidate() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("file.txt");
    fs::write(&path, "before").unwrap();
    let mut client = Client::start(root.path());
    let base = client.full("file.txt");
    let plan = client.call(
        "ultra_edit_prepare",
        edit("preflight", &base["snapshot"], "r0", "after"),
        false,
    )["reference"]
        .clone();
    let original_permissions = fs::metadata(&path).unwrap().permissions();
    let mut read_only = original_permissions.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&path, read_only).unwrap();
    let failed = client.call("ultra_edit_commit", json!({"plan":plan}), true);
    fs::set_permissions(&path, original_permissions).unwrap();
    assert_eq!(failed["commit"], "not_committed");
    assert_eq!(fs::read_to_string(&path).unwrap(), "before");
    let arguments = json!({"plan":plan,"request_id":"preflight-retry"});
    let retried = client.call("ultra_edit_retry", arguments.clone(), false);
    assert_eq!(retried["commit"], "committed");
    assert_eq!(fs::read_to_string(&path).unwrap(), "after");
    assert_eq!(client.call("ultra_edit_retry", arguments, false), retried);
    assert_eq!(
        client.call("ultra_edit_commit", json!({"plan":plan}), true),
        failed
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
    let mut invalid_edit = edit("invalid", &snapshot["snapshot"], "r0", "bad");
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
fn path_shaped_snapshot_references_remain_non_path_data_over_mcp() {
    let root = TempDir::new().unwrap();
    let reference = r"\\?\C:\looks-like-a-path";
    let mut client = Client::start(root.path());
    let rejected = client.call(
        "ultra_edit_prepare",
        json!({
            "request_id": "invalid-reference",
            "files": [{
                "base": reference,
                "changes": [{
                    "id": "unused",
                    "target": {"kind": "exact", "old": "unused"},
                    "text": "unused",
                }],
            }],
        }),
        true,
    );
    let mentions = |diagnostics: &Value| {
        diagnostics
            .as_array()
            .unwrap()
            .iter()
            .map(|diagnostic| {
                diagnostic["message"]
                    .as_str()
                    .unwrap()
                    .matches(reference)
                    .count()
            })
            .sum::<usize>()
    };
    assert_eq!(mentions(&rejected["diagnostics"]), 1);
    let evidence = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"evidence","reference":rejected["reference"]}}),
        false,
    );
    assert_eq!(evidence["value"]["request"]["files"][0]["base"], reference);
    assert!(
        evidence["value"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .all(|diagnostic| diagnostic["file"].is_null())
    );
    assert_eq!(mentions(&evidence["value"]["diagnostics"]), 1);
    client.close();
}

#[test]
fn interrupted_commit_and_corrupt_journal_preserve_unknown_status_and_references() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("file.txt"), "x").unwrap();
    let mut client = Client::start(root.path());
    let snapshot = client.full("file.txt");
    let request = edit("interrupted", &snapshot["snapshot"], "r0", "xx");
    let preview = client.call("ultra_edit_prepare", request.clone(), false);
    let evidence = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"evidence","reference":preview["reference"]}}),
        false,
    );
    let storage = Storage::open(root.path()).unwrap();
    let plan: PreparedPlan = storage
        .get("plans", evidence["value"]["id"].as_str().unwrap())
        .unwrap();
    let mut displayed = serde_json::to_value(&plan).unwrap();
    ultra_edit::report::display_paths(&mut displayed);
    assert_eq!(evidence["value"], displayed);
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
    let inspection = client.call("ultra_edit_inspect", json!({"plan":plan.id}), false);
    assert_eq!(inspection["receipt_error"]["code"], "JOURNAL_CORRUPT");
    assert_eq!(inspection["files"][0]["state"]["digest"], digest(b"xx"));
    let evidence = client.call(
        "ultra_edit_status",
        json!({"query":{"kind":"evidence","reference":inspection["inspection"]}}),
        false,
    );
    assert_eq!(
        evidence["value"]["files"][0]["state"]["content"]["text"],
        "xx"
    );
    let accepted = json!({"inspection":inspection["inspection"],"decision":"accept_current","note":"Fixture operator reviewed journal and current bytes"});
    fs::write(root.path().join("file.txt"), "changed since inspection").unwrap();
    assert_eq!(
        client.call("ultra_edit_reconcile", accepted.clone(), true)["error"]["code"],
        "STALE_INSPECTION"
    );
    fs::write(root.path().join("file.txt"), "xx").unwrap();
    let resolved = client.call("ultra_edit_reconcile", accepted.clone(), false);
    assert_eq!(resolved["plan_id"], plan.id);
    assert_eq!(
        client.call("ultra_edit_reconcile", accepted, false),
        resolved
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
    let request = edit("cancelled", &snapshot["snapshot"], "r0", "xx");
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
