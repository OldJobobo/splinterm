use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{
        Mutex,
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use splinterm_mcp::MAXIMUM_LINE_BYTES;

const SERVER: &str = env!("CARGO_BIN_EXE_splinterm-mcp");
const TIMEOUT: Duration = Duration::from_secs(5);

struct Harness {
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<String>,
    reader: Option<thread::JoinHandle<()>>,
    seen: Mutex<Vec<Value>>,
}

impl Harness {
    fn spawn() -> Self {
        let mut child = Command::new(SERVER)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, output) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                sender.send(line.unwrap()).unwrap_or(());
            }
        });
        Self {
            child,
            input: Some(input),
            output,
            reader: Some(reader),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn send(&mut self, value: &Value) {
        let input = self.input.as_mut().unwrap();
        serde_json::to_writer(&mut *input, value).unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
    }

    fn receive(&self) -> Value {
        let line = self.output.recv_timeout(TIMEOUT).unwrap();
        let value: Value = serde_json::from_str(&line).unwrap_or_else(|error| {
            panic!("stdout was not pure newline-delimited JSON: {error}: {line:?}")
        });
        self.seen.lock().unwrap().push(value.clone());
        value
    }

    fn receive_id(&self, expected: i64) -> Value {
        loop {
            let value = self.receive();
            if value.get("id").and_then(Value::as_i64) == Some(expected) {
                return value;
            }
        }
    }

    fn initialize(&mut self) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "splinterm-black-box-test", "version": "1"}
            }
        }));
        self.receive_id(1)
    }

    fn initialized(&mut self) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
    }

    fn close_input(&mut self) {
        drop(self.input.take());
    }

    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                self.reader
                    .take()
                    .expect("stdout reader was already joined")
                    .join()
                    .expect("stdout reader thread panicked");
                while let Ok(line) = self.output.recv() {
                    let value = serde_json::from_str::<Value>(&line).unwrap_or_else(|error| {
                        panic!("stdout was not pure newline-delimited JSON: {error}: {line:?}")
                    });
                    self.seen.get_mut().unwrap().push(value);
                }
                return status;
            }
            assert!(Instant::now() < deadline, "server did not shut down");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn seen(&self) -> Vec<Value> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "test call sites build owned JSON values"
)]
fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

#[test]
#[allow(clippy::too_many_lines, reason = "one ordered black-box stdio session")]
fn lifecycle_capabilities_tools_resources_updates_and_cancellation_are_exact() {
    let mut server = Harness::spawn();
    let initialization = server.initialize();
    assert_eq!(initialization["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(
        initialization["result"]["capabilities"],
        json!({"resources": {"subscribe": true}, "tools": {}})
    );

    server.send(&request(
        2,
        "tools/call",
        json!({
            "name": "splinterm.spike.echo",
            "arguments": {"message": "too early"}
        }),
    ));
    assert_eq!(server.receive_id(2)["error"]["code"], -32600);
    server.initialized();

    server.send(&request(3, "tools/list", json!({})));
    let tools = server.receive_id(3)["result"]["tools"]
        .as_array()
        .unwrap()
        .clone();
    let names = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "splinterm.spike.echo",
            "splinterm.spike.fail",
            "splinterm.spike.wait_for_cancel"
        ]
    );
    let annotations = json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false
    });
    assert!(tools.iter().all(|tool| {
        tool["annotations"] == annotations
            && tool["execution"] == json!({"taskSupport": "forbidden"})
    }));
    assert_eq!(
        tools[0]["inputSchema"],
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "additionalProperties": false,
            "properties": {"message": {"type": "string"}},
            "required": ["message"],
            "type": "object"
        })
    );
    let empty_schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "additionalProperties": false,
        "type": "object"
    });
    assert_eq!(tools[1]["inputSchema"], empty_schema);
    assert_eq!(tools[2]["inputSchema"], empty_schema);
    assert_eq!(
        tools[0]["outputSchema"],
        json!({
            "$defs": {
                "EchoData": {
                    "additionalProperties": false,
                    "properties": {
                        "message": {"type": "string"},
                        "revision": {"format": "uint64", "minimum": 0, "type": "integer"}
                    },
                    "required": ["message", "revision"],
                    "type": "object"
                }
            },
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "additionalProperties": false,
            "properties": {
                "data": {"$ref": "#/$defs/EchoData"},
                "ok": {"type": "boolean"},
                "schema": {"type": "string"},
                "tool": {"type": "string"}
            },
            "required": ["schema", "tool", "ok", "data"],
            "type": "object"
        })
    );
    assert!(tools[1].get("outputSchema").is_none());
    assert!(tools[2].get("outputSchema").is_none());

    server.send(&request(4, "resources/list", json!({})));
    let resources = server.receive_id(4);
    assert_eq!(
        resources["result"]["resources"],
        json!([{
            "uri": "splinterm://topology",
            "name": "splinterm topology spike",
            "description": "Deterministic SDK-spike state",
            "mimeType": "application/json"
        }])
    );

    server.send(&request(5, "resources/templates/list", json!({})));
    let templates = server.receive_id(5);
    assert_eq!(
        templates["result"]["resourceTemplates"],
        json!([
            {
                "uriTemplate": "splinterm://splints/{splint_id}/terminal",
                "name": "splinterm terminal spike",
                "mimeType": "application/json"
            },
            {
                "uriTemplate": "splinterm://splints/{splint_id}/control",
                "name": "splinterm control spike",
                "mimeType": "application/json"
            }
        ])
    );

    for (id, uri) in [
        (6, "splinterm://topology"),
        (7, "splinterm://splints/example/terminal"),
        (8, "splinterm://splints/example/control"),
    ] {
        server.send(&request(id, "resources/read", json!({"uri": uri})));
        let response = server.receive_id(id);
        assert_eq!(response["result"]["contents"][0]["uri"], uri);
        let text = response["result"]["contents"][0]["text"].as_str().unwrap();
        assert!(serde_json::from_str::<Value>(text).is_ok());
    }

    server.send(&request(
        9,
        "resources/subscribe",
        json!({"uri": "splinterm://topology"}),
    ));
    assert_eq!(server.receive_id(9)["result"], json!({}));

    server.send(&request(
        10,
        "tools/call",
        json!({
            "name": "splinterm.spike.echo",
            "arguments": {"message": "hello"}
        }),
    ));
    let mut echo = None;
    let mut update = false;
    while echo.is_none() || !update {
        let message = server.receive();
        if message.get("id").and_then(Value::as_i64) == Some(10) {
            echo = Some(message);
        } else if message["method"] == "notifications/resources/updated" {
            assert_eq!(message["params"]["uri"], "splinterm://topology");
            update = true;
        }
    }
    let result = &echo.unwrap()["result"];
    assert_eq!(result["isError"], false);
    let compact: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(compact, result["structuredContent"]);
    assert_eq!(compact["data"]["message"], "hello");

    server.send(&request(
        11,
        "tools/call",
        json!({
            "name": "splinterm.spike.echo",
            "arguments": {"message": "hello", "unexpected": true}
        }),
    ));
    let closed_rejection = server.receive_id(11);
    assert_eq!(closed_rejection["result"]["isError"], true);
    assert!(
        closed_rejection["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unknown field")
    );

    server.send(&request(
        12,
        "tools/call",
        json!({"name": "splinterm.spike.fail", "arguments": {}}),
    ));
    let failure = server.receive_id(12);
    assert_eq!(failure["result"]["isError"], true);
    let failure_text: Value =
        serde_json::from_str(failure["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(failure_text, failure["result"]["structuredContent"]);
    assert_eq!(failure_text["error"]["code"], "SPIKE_FAILURE");

    server.send(&request(
        13,
        "tools/call",
        json!({
            "name": "splinterm.spike.wait_for_cancel",
            "arguments": {}
        }),
    ));
    thread::sleep(Duration::from_millis(50));
    server.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": {"requestId": 13, "reason": "black-box proof"}
    }));
    server.send(&request(14, "ping", json!({})));
    let mut saw_ping = false;
    let mut saw_cancellation_update = false;
    while !saw_ping || !saw_cancellation_update {
        let message = server.receive();
        assert_ne!(message.get("id").and_then(Value::as_i64), Some(13));
        saw_ping |= message.get("id").and_then(Value::as_i64) == Some(14);
        saw_cancellation_update |= message["method"] == "notifications/resources/updated";
    }
    server.send(&request(
        15,
        "resources/read",
        json!({"uri": "splinterm://topology"}),
    ));
    let state = server.receive_id(15);
    let state: Value =
        serde_json::from_str(state["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(state["cancellationsObserved"], 1);

    server.send(&request(
        16,
        "resources/unsubscribe",
        json!({"uri": "splinterm://topology"}),
    ));
    assert_eq!(server.receive_id(16)["result"], json!({}));
    server.send(&request(
        17,
        "tools/call",
        json!({
            "name": "splinterm.spike.echo",
            "arguments": {"message": "after unsubscribe"}
        }),
    ));
    server.send(&request(18, "ping", json!({})));
    let mut ids = Vec::new();
    while !ids.contains(&17) || !ids.contains(&18) {
        let message = server.receive();
        assert_ne!(message["method"], "notifications/resources/updated");
        if let Some(id) = message.get("id").and_then(Value::as_i64) {
            ids.push(id);
        }
    }

    server.close_input();
    assert!(server.wait().success());
    let seen = server.seen();
    assert!(
        seen.iter()
            .all(|message| message.get("id").and_then(Value::as_i64) != Some(13)),
        "cancelled request emitted a late response"
    );
    let unsubscribe = seen
        .iter()
        .position(|message| message.get("id").and_then(Value::as_i64) == Some(16))
        .expect("unsubscribe response was retained");
    assert!(
        seen[unsubscribe + 1..]
            .iter()
            .all(|message| { message["method"] != "notifications/resources/updated" })
    );
}

#[test]
fn every_non_target_protocol_version_is_rejected() {
    for version in [
        "2024-11-05",
        "2025-03-26",
        "2025-06-18",
        "2026-07-28",
        "unknown-version",
    ] {
        let mut server = Harness::spawn();
        server.send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": version,
                "capabilities": {},
                "clientInfo": {"name": "version-test", "version": "1"}
            }
        }));
        let response = server.receive_id(1);
        assert_eq!(response["error"]["code"], -32600, "version {version}");
        assert!(!server.wait().success());
    }
}

#[test]
fn maximum_line_is_accepted_and_oversized_line_shuts_down() {
    let mut accepted = Harness::spawn();
    let base = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let mut line = Vec::with_capacity(MAXIMUM_LINE_BYTES);
    line.extend_from_slice(base);
    line.resize(MAXIMUM_LINE_BYTES - 1, b' ');
    line.push(b'\n');
    accepted.input.as_mut().unwrap().write_all(&line).unwrap();
    accepted.input.as_mut().unwrap().flush().unwrap();
    assert_eq!(accepted.receive_id(1)["result"], json!({}));
    accepted.close_input();
    assert!(!accepted.wait().success());

    let mut oversized = Harness::spawn();
    let prefix = br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{"padding":""#;
    let suffix = b"\"}}\n";
    let padding = MAXIMUM_LINE_BYTES + 1 - prefix.len() - suffix.len();
    let mut line = Vec::with_capacity(MAXIMUM_LINE_BYTES + 1);
    line.extend_from_slice(prefix);
    line.resize(prefix.len() + padding, b'x');
    line.extend_from_slice(suffix);
    assert_eq!(line.len(), MAXIMUM_LINE_BYTES + 1);
    serde_json::from_slice::<Value>(&line[..line.len() - 1]).unwrap();
    let input = oversized.input.as_mut().unwrap();
    let _ = input.write_all(&line);
    let _ = input.flush();
    oversized.close_input();
    assert!(!oversized.wait().success());
    assert!(oversized.seen().is_empty());
}
