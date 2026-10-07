//! Request parsing errors, `expect` forms, derived-ID stability, and the MCP surface.

use serde_json::{Value, json};
use ultra_edit::mcp::RepairRequest;
use ultra_edit::workspace::resolve_ids;
use ultra_edit::*;

fn edit_error(value: Value) -> String {
    serde_json::from_value::<EditRequest>(value)
        .unwrap_err()
        .to_string()
}

fn repair_error(value: Value) -> String {
    serde_json::from_value::<RepairRequest>(value)
        .err()
        .unwrap()
        .to_string()
}

fn derived(value: Value) -> String {
    resolve_ids(serde_json::from_value::<EditRequest>(value).unwrap())
        .unwrap()
        .request_id
}

// Some of a change's own messages begin with "change"; its position still takes a
// colon, not the comma that joins a file's position to a change's.
#[test]
fn a_change_error_that_starts_with_change_keeps_the_colon_after_its_position() {
    let cases = [
        (
            json!({"old":"a"}),
            "change needs `new` (the replacement text; \"\" deletes)",
        ),
        (
            json!({"new":"b"}),
            "change needs one of old, span, lines, or after (or a verbose `target`)",
        ),
        (
            json!({"old":"a","target":{"kind":"exact","old":"a","scope":null},"text":"b"}),
            "change mixes verbose `target` with shorthand fields (old/span/lines/after/in/count/expect); use one form",
        ),
    ];
    for (change, message) in cases {
        let error = edit_error(json!({"files":[
            {"path":"a","changes":[{"old":"x","new":"y"}]},
            {"path":"b","changes":[{"old":"x","new":"y"}, change.clone()]}
        ]}));
        assert_eq!(error, format!("file 2, change 2: {message}"));
        let error = repair_error(
            json!({"reference":"plan","changes":[{"id":"1.1","old":"x","new":"y"}, change]}),
        );
        assert_eq!(error, format!("change 2: {message}"));
    }
}

#[test]
fn errors_are_numbered_from_one_and_nested_under_their_file() {
    assert_eq!(
        edit_error(
            json!({"files":[{"path":"a","changes":[]},{"path":"b","changes":[{"bogus":1}]}]})
        ),
        "file 2, change 1: unknown field `bogus`, expected one of `id`, `old`, `new`, `text`, `count`, `in`, `span`, `lines`, `after`, `expect`, `target`"
    );
    assert_eq!(
        edit_error(json!({"files":[{"path":"a","changes":null}]})),
        "file 1: invalid type: null, expected an array of changes"
    );
    assert_eq!(
        edit_error(json!({"files":null})),
        "invalid type: null, expected an array of files"
    );
    assert_eq!(
        edit_error(json!({"files":[{"path":"a"}]})),
        "file 1: missing field `changes`"
    );
    assert_eq!(
        edit_error(
            json!({"files":[{"path":"a","changes":[{"target":{"kind":"lines","lines":[1,2],"bogus":1},"text":""}]}]})
        ),
        "file 1, change 1: invalid `target`: unknown field `bogus`, expected one of `lines`, `expect`, `expect_last`"
    );
    // A string source keeps one position suffix, at the end.
    let error = serde_json::from_str::<EditRequest>(
        r#"{"files":[{"path":"a","changes":[{"target":{"kind":"exact","old":"a","old":"b"},"text":""}]}]}"#,
    )
    .unwrap_err()
    .to_string();
    assert_eq!(
        error,
        "file 1, change 1: invalid `target`: duplicate field `old` at line 1 column 79"
    );
    assert_eq!(
        repair_error(
            json!({"reference":"p","changes":[{"id":"1.1","lines":[1,1],"expect":7,"new":""}]})
        ),
        "change 1: `expect` takes the current text as a string, or for lines [first line, last line]"
    );
}

#[test]
fn expect_forms_parse_by_target_kind() {
    let parse = |change: Value| {
        serde_json::from_value::<Change>(change)
            .map(|c| c.target)
            .map_err(|e| e.to_string())
    };
    let form = "`expect` takes the current text as a string, or for lines [first line, last line]";
    for bad in [
        json!({"lines":[1,2],"expect":[["a"],"b"],"new":""}),
        json!({"lines":[1,2],"expect":["a",null],"new":""}),
        json!({"lines":[1,2],"expect":[],"new":""}),
        json!({"lines":[1,2],"expect":5,"new":""}),
        json!({"lines":[1,2],"expect":{"a":1},"new":""}),
        json!({"after":3,"expect":["a"],"new":"x"}),
        json!({"span":"r1","expect":["a","b"],"new":"x"}),
    ] {
        assert_eq!(parse(bad.clone()).unwrap_err(), form, "{bad}");
    }
    assert_eq!(
        parse(json!({"lines":[1,3],"expect":["a","b","c"],"new":""})).unwrap(),
        Target::Lines {
            lines: [1, 3],
            expect: Some("a\nb\nc".into()),
            expect_last: None
        }
    );
    // null reads as absent.
    assert_eq!(
        parse(json!({"lines":[1,2],"expect":null,"new":""})).unwrap(),
        Target::Lines {
            lines: [1, 2],
            expect: None,
            expect_last: None
        }
    );
    assert!(parse(json!({"lines":[1,2],"expect":"a","expect_last":"b","new":""})).is_err());
    assert!(
        parse(json!({"target":{"kind":"insert","after":1,"expect_last":"b"},"text":""})).is_err()
    );
}

#[test]
fn spellings_of_one_lines_guard_derive_one_request_id() {
    let file = |change: Value| json!({"files":[{"path":"a.rs","changes":[change]}]});
    assert_eq!(
        derived(file(
            json!({"lines":[3,7],"expect":["fn a() {","}"],"new":""})
        )),
        derived(file(
            json!({"target":{"kind":"lines","lines":[3,7],"expect":"fn a() {","expect_last":"}"},"text":""})
        ))
    );
    assert_eq!(
        derived(file(
            json!({"lines":"3-7","expect":["fn a() {","}"],"text":""})
        )),
        derived(file(
            json!({"lines":[3,7],"expect":["fn a() {","}"],"new":""})
        ))
    );
    assert_eq!(
        derived(file(json!({"lines":[4,4],"expect":"let x = 1;","new":""}))),
        derived(file(
            json!({"lines":[4,4],"expect":["let x = 1;"],"new":""})
        ))
    );
    assert_eq!(
        derived(file(json!({"lines":[4,6],"expect":"a\nb\nc","new":""}))),
        derived(file(json!({"lines":[4,6],"expect":["a","b","c"],"new":""})))
    );
    // A 0.3.0 verbose span change keeps its serialized form, so its derived ID.
    let request: EditRequest = serde_json::from_value(file(
        json!({"id":"c","target":{"kind":"span","span":"r1"},"text":"x"}),
    ))
    .unwrap();
    assert_eq!(
        serde_json::to_value(&request.files).unwrap(),
        json!([{"path":"a.rs","changes":[{"id":"c","target":{"kind":"span","span":"r1"},"text":"x"}]}])
    );
}

mod client {
    use std::io::{BufRead, BufReader, Write};
    use std::path::Path;
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    use serde_json::{Value, json};

    pub struct Client {
        child: Child,
        input: Option<ChildStdin>,
        output: Receiver<Value>,
        next_id: u64,
        pub instructions: String,
    }

    impl Client {
        pub fn start(root: &Path) -> Self {
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
                    let value = serde_json::from_str(&line.unwrap()).unwrap();
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
                instructions: String::new(),
            };
            let initialized = client.rpc(
                "initialize",
                json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"1"}}),
            );
            client.instructions = initialized["result"]["instructions"]
                .as_str()
                .unwrap()
                .to_owned();
            client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
            client
        }

        fn send(&mut self, value: Value) {
            let input = self.input.as_mut().unwrap();
            serde_json::to_writer(&mut *input, &value).unwrap();
            input.write_all(b"\n").unwrap();
            input.flush().unwrap();
        }

        pub fn rpc(&mut self, method: &str, params: Value) -> Value {
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
    }

    impl Drop for Client {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[test]
fn the_mcp_surface_advertises_the_shorthand() {
    let root = tempfile::TempDir::new().unwrap();
    let mut client = client::Client::start(root.path());
    assert!(
        client
            .instructions
            .contains("`lines` and `after` take Read's line numbers and need `expect`")
    );
    let listed = client.rpc("tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().unwrap();
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap()
            .clone()
    };
    let edit = tool("ultra_edit");
    assert_eq!(edit["_meta"]["anthropic/alwaysLoad"], true);
    assert!(
        edit["description"]
            .as_str()
            .unwrap()
            .contains("[first line, last line]")
    );
    let change = &edit["inputSchema"]["$defs"]["Change"];
    for hidden in ["target", "text"] {
        assert!(change["properties"].get(hidden).is_none(), "{change}");
    }
    assert_eq!(change["properties"]["expect"]["anyOf"][0]["type"], "string");
    assert_eq!(change["properties"]["expect"]["anyOf"][1]["minItems"], 2);
    assert!(change["properties"]["in"]["anyOf"].is_array());
    let repair = tool("ultra_edit_repair");
    assert!(repair["_meta"]["anthropic/alwaysLoad"].is_null());
    assert!(
        repair["inputSchema"]["$defs"]["Change"]["properties"]
            .get("id")
            .is_some()
    );
    let response = client.rpc(
        "tools/call",
        json!({"name":"ultra_edit_repair","arguments":{"reference":"x","changes":[{"id":"1.1","old":"one","new":"1"},{"lines":[1,1],"expect":7,"new":""}]}}),
    );
    assert_eq!(
        response["result"]["content"][0]["text"],
        "failed to deserialize parameters: change 2: `expect` takes the current text as a string, or for lines [first line, last line]"
    );
}

// The same, as an MCP client sees it.
#[test]
fn an_mcp_parse_error_for_a_change_reads_file_n_change_m_colon() {
    let root = tempfile::TempDir::new().unwrap();
    let path = root.path().join("f.txt");
    std::fs::write(&path, "one\ntwo\n").unwrap();
    let mut client = client::Client::start(root.path());
    let response = client.rpc(
        "tools/call",
        json!({"name":"ultra_edit","arguments":{"files":[{"path":path,"changes":[{"old":"one","new":"1"},{"old":"two"}]}]}}),
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n");
    assert_eq!(
        response["result"]["content"][0]["text"],
        "failed to deserialize parameters: file 1, change 2: change needs `new` (the replacement text; \"\" deletes)"
    );
}
