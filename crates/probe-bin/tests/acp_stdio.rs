//! End-to-end: spawn the real binary, speak ACP v1 over its stdio exactly
//! as the sarah-computer-controller's SDK client does, and assert the
//! handshake, streaming updates, stop reasons, and cancellation. The old
//! Rust probe had a stdio_protocol suite; this is its zerobase descendant.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

struct AcpChild {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl AcpChild {
    fn spawn(transport: &str) -> AcpChild {
        let mut child = Command::new(env!("CARGO_BIN_EXE_probe-bin"))
            .arg("acp")
            .env("PROBE_TRANSPORT", transport)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn probe-bin");
        let stdin = child.stdin.take().unwrap();
        let stdout: ChildStdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        AcpChild { child, stdin, lines }
    }

    fn send(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn next_json(&self, timeout: Duration) -> serde_json::Value {
        let line = self.lines.recv_timeout(timeout).expect("line before timeout");
        serde_json::from_str(&line).expect("valid JSON line")
    }

    /// Read lines until one satisfies the predicate; panics on timeout.
    fn wait_for(&self, timeout: Duration, predicate: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .expect("timed out waiting for matching line");
            let value = self.next_json(remaining);
            if predicate(&value) {
                return value;
            }
        }
    }
}

impl Drop for AcpChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn initialize_and_open_session(child: &mut AcpChild) -> String {
    child.send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},"clientInfo":{"name":"sarah-computer-controller"}}}"#);
    let response = child.next_json(Duration::from_secs(5));
    assert_eq!(response["result"]["protocolVersion"], 1);
    assert_eq!(response["result"]["agentCapabilities"]["loadSession"], true);
    assert_eq!(response["result"]["authMethods"], serde_json::json!([]));

    child.send(r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#);
    let response = child.next_json(Duration::from_secs(5));
    response["result"]["sessionId"].as_str().unwrap().to_string()
}

#[test]
fn full_prompt_streams_updates_and_stops_end_turn() {
    let mut child = AcpChild::spawn("stub");
    let session_id = initialize_and_open_session(&mut child);
    child.send(&format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"sessionId":"{session_id}","prompt":[{{"type":"text","text":"say hello"}}]}}}}"#
    ));
    let update = child.wait_for(Duration::from_secs(5), |value| value["method"] == "session/update");
    assert_eq!(update["params"]["update"]["sessionUpdate"], "agent_message_chunk");
    let response = child.wait_for(Duration::from_secs(5), |value| value["id"] == 3);
    assert_eq!(response["result"]["stopReason"], "end_turn");
}

#[test]
fn cancel_interrupts_a_slow_stream_promptly() {
    let mut child = AcpChild::spawn("stub-slow");
    let session_id = initialize_and_open_session(&mut child);
    child.send(&format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"sessionId":"{session_id}","prompt":[{{"type":"text","text":"work"}}]}}}}"#
    ));
    // Wait for streaming to actually start, then cancel.
    child.wait_for(Duration::from_secs(5), |value| value["method"] == "session/update");
    child.send(&format!(
        r#"{{"jsonrpc":"2.0","method":"session/cancel","params":{{"sessionId":"{session_id}"}}}}"#
    ));
    let response = child.wait_for(Duration::from_secs(5), |value| value["id"] == 3);
    assert_eq!(response["result"]["stopReason"], "cancelled");
}

#[test]
fn tool_loop_escalates_permission_and_a_denial_still_completes() {
    let mut child = AcpChild::spawn("stub-tool");
    let session_id = initialize_and_open_session(&mut child);
    child.send(&format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"sessionId":"{session_id}","prompt":[{{"type":"text","text":"check the repo"}}]}}}}"#
    ));
    // The stub-tool transport asks for a shell command; with no tool catalog
    // yet the kind is "other", which still escalates under default policy.
    let permission = child.wait_for(Duration::from_secs(5), |value| value["method"] == "session/request_permission");
    let options = permission["params"]["options"].as_array().unwrap();
    assert!(options.iter().all(|option| {
        !option["optionId"].as_str().unwrap().to_lowercase().contains("bypass")
            && !option["name"].as_str().unwrap().to_lowercase().contains("bypass")
    }));
    // Deny it: probe must degrade honestly and still finish the prompt.
    let request_id = permission["id"].clone();
    child.send(&format!(
        r#"{{"jsonrpc":"2.0","id":{request_id},"result":{{"outcome":{{"outcome":"selected","optionId":"reject_once"}}}}}}"#
    ));
    let failed_update = child.wait_for(Duration::from_secs(5), |value| {
        value["method"] == "session/update"
            && value["params"]["update"]["sessionUpdate"] == "tool_call_update"
            && value["params"]["update"]["status"] == "failed"
    });
    assert_eq!(failed_update["params"]["update"]["toolCallId"], "tool_0");
    let response = child.wait_for(Duration::from_secs(5), |value| value["id"] == 3);
    assert_eq!(response["result"]["stopReason"], "end_turn");
}

#[test]
fn requests_before_initialize_are_refused() {
    let mut child = AcpChild::spawn("stub");
    child.send(r#"{"jsonrpc":"2.0","id":9,"method":"session/new","params":{"cwd":"/tmp"}}"#);
    let response = child.next_json(Duration::from_secs(5));
    assert!(response["error"]["message"].as_str().unwrap().contains("Not initialized"));
}
