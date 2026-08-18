//! Full prompt -> stream -> tool loop over real HTTP against a local fake
//! OpenAI-compatible server (no network). Round one returns a tool call;
//! the continuation returns text and stops. This is the #209 exit
//! criterion, fixture-style: the fake server is the fixture.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn sse_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

/// Serve chat-completions: first call streams a tool call, second streams
/// text. Returns the bound port.
fn spawn_fake_openai() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_thread = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let hit = hits_thread.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                    break;
                }
                let lowered = line.to_lowercase();
                if let Some(value) = lowered.strip_prefix("content-length:") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; content_length];
            let _ = reader.read_exact(&mut body);
            let request: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            let has_tool_results = request["messages"]
                .as_array()
                .map(|messages| messages.iter().any(|message| message["role"] == "tool"))
                .unwrap_or(false);
            let sse = if hit == 0 && !has_tool_results {
                concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"Checking.\"}}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"command\\\": \\\"echo hello\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
                    "data: [DONE]\n\n",
                )
            } else {
                concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"The command ran.\"},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4,\"total_tokens\":13}}\n\n",
                    "data: [DONE]\n\n",
                )
            };
            let _ = stream.write_all(sse_response(sse).as_bytes());
        }
    });
    (port, hits)
}

#[test]
fn prompt_stream_tool_loop_completes_over_http() {
    let (port, hits) = spawn_fake_openai();
    let mut child = Command::new(env!("CARGO_BIN_EXE_probe-bin"))
        .arg("acp")
        .env("PROBE_TRANSPORT", "openai")
        .env("PROBE_INFERENCE_URL", format!("http://127.0.0.1:{port}"))
        .env("PROBE_MODEL", "test-model")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let (sender, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in stdout.lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                return;
            }
        }
    });
    let next = |predicate: &dyn Fn(&serde_json::Value) -> bool| -> serde_json::Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .expect("timed out");
            let line = lines.recv_timeout(remaining).expect("line");
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            if predicate(&value) {
                return value;
            }
        }
    };

    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":1}}}}"#).unwrap();
    next(&|value| value["id"] == 1);
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"session/new","params":{{"cwd":"/tmp","mcpServers":[]}}}}"#).unwrap();
    let session = next(&|value| value["id"] == 2);
    let session_id = session["result"]["sessionId"].as_str().unwrap().to_string();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"sessionId":"{session_id}","prompt":[{{"type":"text","text":"run echo"}}]}}}}"#
    )
    .unwrap();

    // Round one: text chunk streams, then the shell tool escalates.
    let permission = next(&|value| value["method"] == "session/request_permission");
    assert_eq!(permission["params"]["toolCall"]["rawInput"]["command"], "echo hello");
    let request_id = permission["id"].clone();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":{request_id},"result":{{"outcome":{{"outcome":"selected","optionId":"allow_once"}}}}}}"#
    )
    .unwrap();

    // Tool executes (still the honest #210 stub), continuation runs against
    // the fake server, and the prompt completes.
    let response = next(&|value| value["id"] == 3);
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert!(hits.load(Ordering::SeqCst) >= 2, "expected an initial round and a continuation");
    let _ = child.kill();
    let _ = child.wait();
}

/// Live Gemini smoke, env-gated exactly like the archived suite: skips
/// without a credential, never runs in CI-less default loops.
#[test]
fn live_gemini_smoke_when_credentialed() {
    let key = std::env::var("GOOGLE_GENERATIVE_AI_API_KEY")
        .or_else(|_| std::env::var("GEMINI_API_KEY"))
        .ok();
    let Some(_key) = key else {
        eprintln!("live_gemini_smoke_when_credentialed: skipped (no API key in env)");
        return;
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_probe-bin"))
        .arg("acp")
        .env("PROBE_TRANSPORT", "gemini")
        .env("PROBE_MODEL", std::env::var("PROBE_MODEL").unwrap_or_else(|_| "gemini-2.0-flash".into()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":1}}}}"#).unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"session/new","params":{{"cwd":"/tmp","mcpServers":[]}}}}"#).unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let session: serde_json::Value = serde_json::from_str(&line).unwrap();
    let session_id = session["result"]["sessionId"].as_str().unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"sessionId":"{session_id}","prompt":[{{"type":"text","text":"Reply with the single word: ready"}}]}}}}"#
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        assert!(std::time::Instant::now() < deadline, "live smoke timed out");
        line.clear();
        if stdout.read_line(&mut line).unwrap() == 0 {
            panic!("stream closed before prompt completed");
        }
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        if value["id"] == 3 {
            assert_eq!(value["result"]["stopReason"], "end_turn");
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}
