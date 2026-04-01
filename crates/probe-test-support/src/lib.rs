use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use assert_cmd::cargo::CommandCargoExt;
use serde_json::Value;
use tempfile::TempDir;

#[derive(Clone, Debug)]
pub struct FakeHttpResponse {
    status_code: u16,
    content_type: String,
    body: String,
}

impl FakeHttpResponse {
    pub fn json_ok(body: Value) -> Self {
        Self::json_status(200, body)
    }

    pub fn json_status(status_code: u16, body: Value) -> Self {
        Self {
            status_code,
            content_type: String::from("application/json"),
            body: body.to_string(),
        }
    }

    pub fn text_status(status_code: u16, body: impl Into<String>) -> Self {
        Self {
            status_code,
            content_type: String::from("text/plain; charset=utf-8"),
            body: body.into(),
        }
    }

    pub fn text_event_stream(status_code: u16, body: impl Into<String>) -> Self {
        Self {
            status_code,
            content_type: String::from("text/event-stream"),
            body: body.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FakeHttpRequest {
    pub raw: String,
    pub method: String,
    pub path: String,
    pub body: String,
}

pub struct FakeOpenAiServer {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

pub struct FakeAppleFmServer {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeOpenAiServer {
    pub fn from_json_responses(responses: Vec<Value>) -> Self {
        Self::from_responses(
            responses
                .into_iter()
                .map(FakeHttpResponse::json_ok)
                .collect::<Vec<_>>(),
        )
    }

    pub fn from_responses(responses: Vec<FakeHttpResponse>) -> Self {
        let responses = Arc::new(Mutex::new(responses.into_iter()));
        Self::from_handler(move |_request| {
            responses
                .lock()
                .expect("fake openai response lock")
                .next()
                .expect("fake openai response should be queued")
        })
    }

    pub fn from_handler(
        handler: impl Fn(FakeHttpRequest) -> FakeHttpResponse + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        listener
            .set_nonblocking(true)
            .expect("set fake server nonblocking");
        let address = listener.local_addr().expect("fake server address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_thread = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let handler = Arc::new(handler);
        let handler_thread = Arc::clone(&handler);

        let thread = thread::spawn(move || {
            while !stop_thread.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_request(&mut stream);
                        requests_thread
                            .lock()
                            .expect("fake server request lock")
                            .push(request.clone());

                        let response = handler_thread(parse_http_request(request));
                        let payload = format!(
                            "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                            response.status_code,
                            status_text(response.status_code),
                            response.content_type,
                            response.body.len(),
                            response.body
                        );
                        stream
                            .write_all(payload.as_bytes())
                            .expect("write fake server response");
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("fake server accept failed: {error}"),
                }
            }
        });

        Self {
            base_url: format!("http://{address}/v1"),
            requests,
            stop,
            thread: Some(thread),
        }
    }

    pub fn base_url(&self) -> &str {
        self.base_url.as_str()
    }

    pub fn recorded_requests(&self) -> Vec<String> {
        self.requests
            .lock()
            .expect("fake server request lock")
            .clone()
    }

    pub fn finish(mut self) -> Vec<String> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .expect("fake server thread should exit cleanly");
        }
        self.recorded_requests()
    }
}

impl FakeAppleFmServer {
    pub fn from_json_responses(responses: Vec<Value>) -> Self {
        Self::from_responses(
            responses
                .into_iter()
                .map(FakeHttpResponse::json_ok)
                .collect::<Vec<_>>(),
        )
    }

    pub fn from_responses(responses: Vec<FakeHttpResponse>) -> Self {
        let responses = Arc::new(Mutex::new(responses.into_iter()));
        Self::from_handler(move |_request| {
            responses
                .lock()
                .expect("fake apple fm response lock")
                .next()
                .expect("fake apple fm response should be queued")
        })
    }

    pub fn from_handler(
        handler: impl Fn(FakeHttpRequest) -> FakeHttpResponse + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        listener
            .set_nonblocking(true)
            .expect("set fake server nonblocking");
        let address = listener.local_addr().expect("fake server address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_thread = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let handler = Arc::new(handler);
        let handler_thread = Arc::clone(&handler);

        let thread = thread::spawn(move || {
            while !stop_thread.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_request(&mut stream);
                        requests_thread
                            .lock()
                            .expect("fake server request lock")
                            .push(request.clone());

                        let response = handler_thread(parse_http_request(request));
                        let payload = format!(
                            "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                            response.status_code,
                            status_text(response.status_code),
                            response.content_type,
                            response.body.len(),
                            response.body
                        );
                        stream
                            .write_all(payload.as_bytes())
                            .expect("write fake server response");
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("fake server accept failed: {error}"),
                }
            }
        });

        Self {
            base_url: format!("http://{address}"),
            requests,
            stop,
            thread: Some(thread),
        }
    }

    pub fn base_url(&self) -> &str {
        self.base_url.as_str()
    }

    pub fn recorded_requests(&self) -> Vec<String> {
        self.requests
            .lock()
            .expect("fake server request lock")
            .clone()
    }

    pub fn finish(mut self) -> Vec<String> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .expect("fake server thread should exit cleanly");
        }
        self.recorded_requests()
    }
}

impl Drop for FakeOpenAiServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for FakeAppleFmServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub struct ProbeTestEnvironment {
    temp_dir: TempDir,
    probe_home: PathBuf,
    workspace: PathBuf,
}

impl ProbeTestEnvironment {
    pub fn new() -> Self {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let probe_home = temp_dir.path().join(".probe");
        let workspace = temp_dir.path().join("workspace");
        fs::create_dir_all(&probe_home).expect("create probe home");
        fs::create_dir_all(&workspace).expect("create workspace");
        Self {
            temp_dir,
            probe_home,
            workspace,
        }
    }

    pub fn probe_home(&self) -> &Path {
        self.probe_home.as_path()
    }

    pub fn workspace(&self) -> &Path {
        self.workspace.as_path()
    }

    pub fn temp_root(&self) -> &Path {
        self.temp_dir.path()
    }

    pub fn write_workspace_file(&self, relative_path: impl AsRef<Path>, contents: &str) {
        let path = self.workspace.join(relative_path.as_ref());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture parent");
        }
        fs::write(path, contents).expect("write fixture file");
    }

    pub fn seed_coding_workspace(&self) {
        self.write_workspace_file("README.md", "# Probe acceptance fixture\n");
        self.write_workspace_file(
            "src/main.rs",
            "fn main() {\n    println!(\"PROBE_FIXTURE_MAIN\");\n}\n",
        );
        self.write_workspace_file(
            "src/lib.rs",
            "pub fn beta_function() -> &'static str {\n    \"probe\"\n}\n",
        );
        self.write_workspace_file("hello.txt", "hello world\n");
    }
}

static SNAPSHOT_ROOT_ONCE: Once = Once::new();

pub fn configure_snapshot_root() {
    SNAPSHOT_ROOT_ONCE.call_once(|| {
        let root = workspace_root();
        // SAFETY: tests call this once before snapshot assertions and do not
        // mutate the variable concurrently afterward.
        unsafe {
            std::env::set_var("INSTA_WORKSPACE_ROOT", root.as_os_str());
        }
    });
}

pub fn probe_cli_command() -> Command {
    configure_snapshot_root();
    Command::cargo_bin("probe-cli").expect("probe-cli binary should build for tests")
}

pub fn write_openai_attach_server_config(
    environment: &ProbeTestEnvironment,
    server: &FakeOpenAiServer,
    model_id: &str,
) -> PathBuf {
    let address = server
        .base_url()
        .strip_prefix("http://")
        .expect("base url should start with http://")
        .strip_suffix("/v1")
        .expect("base url should end with /v1");
    let (host, port) = address.rsplit_once(':').expect("host:port pair");
    let port = port.parse::<u16>().expect("port should parse");
    write_attach_server_config(
        environment.probe_home(),
        "psionic-local.json",
        serde_json::json!({
            "mode": "attach",
            "host": host,
            "port": port,
            "backend": "cpu",
            "binary_path": null,
            "model_path": null,
            "model_id": model_id,
            "reasoning_budget": null
        }),
    )
}

pub fn write_apple_fm_attach_server_config(
    environment: &ProbeTestEnvironment,
    server: &FakeAppleFmServer,
    model_id: &str,
) -> PathBuf {
    let address = server
        .base_url()
        .strip_prefix("http://")
        .expect("base url should start with http://");
    let (host, port) = address.rsplit_once(':').expect("host:port pair");
    let port = port.parse::<u16>().expect("port should parse");
    write_attach_server_config(
        environment.probe_home(),
        "psionic-apple-fm.json",
        serde_json::json!({
            "mode": "attach",
            "api_kind": "apple_foundation_models",
            "host": host,
            "port": port,
            "backend": "cpu",
            "binary_path": null,
            "model_path": null,
            "model_id": model_id,
            "reasoning_budget": null
        }),
    )
}

pub fn normalize_test_paths(value: &str, environment: &ProbeTestEnvironment) -> String {
    let temp_root = environment.temp_root().display().to_string();
    let replaced = normalize_workspace_path(value.replace(temp_root.as_str(), "$TEST_ROOT"));
    normalize_loopback_ports(normalize_session_path_segments(replaced.as_str()).as_str())
}

pub fn normalize_exec_stderr_for_snapshot(raw: &str, environment: &ProbeTestEnvironment) -> String {
    raw.lines()
        .map(|line| {
            if line.starts_with("backend_target ") {
                normalize_backend_target_line(line)
            } else if line.starts_with("session=") {
                normalize_session_line(line, environment)
            } else if line.starts_with("observability ") {
                normalize_observability_line(line)
            } else {
                normalize_test_paths(line, environment)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn normalize_chat_stderr_for_snapshot(raw: &str, environment: &ProbeTestEnvironment) -> String {
    raw.lines()
        .map(|line| {
            if line.starts_with("backend_target ") {
                normalize_backend_target_line(line)
            } else if line.starts_with("session=") {
                normalize_session_line(line, environment)
            } else if line.starts_with("resumed session=") {
                normalize_session_words(normalize_test_paths(line, environment).as_str())
            } else if line.starts_with("observability ") {
                normalize_observability_line(line)
            } else {
                normalize_session_words(normalize_test_paths(line, environment).as_str())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn selected_transcript_event_snapshot(probe_home: &Path) -> Value {
    let sessions_dir = probe_home.join("sessions");
    let session_dir = fs::read_dir(&sessions_dir)
        .expect("read sessions directory")
        .map(|entry| entry.expect("session entry").path())
        .next()
        .expect("session should exist");
    let transcript_path = session_dir.join("transcript.jsonl");
    let first_line = fs::read_to_string(transcript_path)
        .expect("read transcript")
        .lines()
        .next()
        .expect("first transcript line")
        .to_string();
    let event: Value = serde_json::from_str(&first_line).expect("decode transcript event");
    serde_json::json!({
        "turn_index": event["turn"]["index"],
        "observability": {
            "cache_signal": event["turn"]["observability"]["cache_signal"],
            "prompt_tokens": event["turn"]["observability"]["prompt_tokens"],
            "prompt_tokens_detail": event["turn"]["observability"]["prompt_tokens_detail"],
            "completion_tokens": event["turn"]["observability"]["completion_tokens"],
            "completion_tokens_detail": event["turn"]["observability"]["completion_tokens_detail"],
            "total_tokens": event["turn"]["observability"]["total_tokens"],
            "total_tokens_detail": event["turn"]["observability"]["total_tokens_detail"]
        },
        "items": event["turn"]["items"]
            .as_array()
            .expect("items array")
            .iter()
            .map(|item| {
                serde_json::json!({
                    "kind": item["kind"],
                    "name": item["name"],
                    "text": item["text"],
                    "tool_execution": item["tool_execution"]
                })
            })
            .collect::<Vec<_>>()
    })
}

pub fn normalized_acceptance_report_snapshot(
    report_path: &Path,
    environment: &ProbeTestEnvironment,
) -> Value {
    let mut value: Value =
        serde_json::from_str(&fs::read_to_string(report_path).expect("read acceptance report"))
            .expect("decode acceptance report");
    normalize_acceptance_value(&mut value, environment);
    value
}

pub fn normalized_tui_smoke_report_snapshot(
    report_path: &Path,
    environment: &ProbeTestEnvironment,
) -> Value {
    let mut value: Value =
        serde_json::from_str(&fs::read_to_string(report_path).expect("read tui smoke report"))
            .expect("decode tui smoke report");
    normalize_tui_smoke_value(&mut value, environment);
    value
}

pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir parent")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

pub fn normalize_workspace_path(value: impl AsRef<str>) -> String {
    let root = workspace_root();
    let root_str = root.to_string_lossy();
    value
        .as_ref()
        .replace(root_str.as_ref(), "$PROBE_WORKSPACE_ROOT")
}

fn read_request(stream: &mut std::net::TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("set fake server read timeout");
    let mut request = String::new();
    let mut buffer = [0_u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes) => {
                request.push_str(&String::from_utf8_lossy(&buffer[..bytes]));
                if bytes < buffer.len() {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => panic!("failed to read fake server request: {error}"),
        }
    }
    request
}

fn parse_http_request(raw: String) -> FakeHttpRequest {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .map_or((raw.as_str(), String::new()), |(head, body)| {
            (head, body.to_string())
        });
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    FakeHttpRequest {
        raw,
        method,
        path,
        body,
    }
}

fn status_text(status_code: u16) -> &'static str {
    match status_code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Response",
    }
}

fn write_attach_server_config(probe_home: &Path, file_name: &str, value: Value) -> PathBuf {
    let config_path = probe_home.join("server").join(file_name);
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).expect("create server config directory");
    }
    fs::write(
        &config_path,
        serde_json::to_string_pretty(&value).expect("encode server config"),
    )
    .expect("write server config");
    config_path
}

fn normalize_backend_target_line(line: &str) -> String {
    line.split_whitespace()
        .map(|field| {
            if field.starts_with("target=") {
                String::from("target=<target>")
            } else if field.starts_with("base_url=") {
                String::from("base_url=<base-url>")
            } else {
                field.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_session_line(line: &str, environment: &ProbeTestEnvironment) -> String {
    line.split_whitespace()
        .map(|field| {
            if field.starts_with("session=") {
                String::from("session=<session-id>")
            } else if let Some(value) = field.strip_prefix("transcript=") {
                format!("transcript={}", normalize_test_paths(value, environment))
            } else {
                field.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_observability_line(line: &str) -> String {
    let mut normalized = vec![String::from("observability")];
    for field in line.trim_start_matches("observability ").split_whitespace() {
        if field.starts_with("wallclock_ms=") {
            normalized.push(String::from("wallclock_ms=<dynamic>"));
        } else if field.starts_with("model_output_ms=") {
            normalized.push(String::from("model_output_ms=<dynamic>"));
        } else if field.starts_with("completion_tps=") {
            normalized.push(String::from("completion_tps=<dynamic>"));
        } else {
            normalized.push(field.to_string());
        }
    }
    normalized.join(" ")
}

fn normalize_acceptance_value(value: &mut Value, environment: &ProbeTestEnvironment) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                match key.as_str() {
                    "run_id" => *child = serde_json::json!("<run-id>"),
                    "git_commit_sha" => {
                        if child.is_string() {
                            *child = serde_json::json!("<git-sha>");
                        }
                    }
                    "git_dirty" => {
                        if child.is_boolean() {
                            *child = serde_json::json!("<git-dirty>");
                        }
                    }
                    "base_url" => *child = serde_json::json!("<base-url>"),
                    "session_id" | "latest_session_id" => {
                        if child.is_string() {
                            *child = serde_json::json!("<session-id>");
                        }
                    }
                    "transcript_path" | "latest_transcript_path" => {
                        if let Some(path) = child.as_str() {
                            *child = serde_json::json!(normalize_test_paths(path, environment));
                        }
                    }
                    "error" => {
                        if let Some(error) = child.as_str() {
                            let normalized = normalize_session_words(
                                normalize_test_paths(error, environment).as_str(),
                            );
                            *child = serde_json::json!(normalized);
                        }
                    }
                    "started_at_ms" | "finished_at_ms" | "duration_ms" | "median_elapsed_ms"
                    | "wallclock_ms" | "model_output_ms" => {
                        if child.is_number() {
                            *child = serde_json::json!("<ms>");
                        }
                    }
                    "completion_tokens_per_second_x1000" => {
                        if child.is_number() {
                            *child = serde_json::json!("<tps>");
                        }
                    }
                    _ => normalize_acceptance_value(child, environment),
                }
            }
        }
        Value::Array(values) => {
            for child in values {
                normalize_acceptance_value(child, environment);
            }
        }
        _ => {}
    }
}

fn normalize_tui_smoke_value(value: &mut Value, environment: &ProbeTestEnvironment) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                match key.as_str() {
                    "runtime_session_id" => {
                        if child.is_string() {
                            *child = serde_json::json!("<session-id>");
                        }
                    }
                    "final_render" | "last_status" => {
                        if let Some(text) = child.as_str() {
                            *child = serde_json::json!(normalize_session_words(
                                normalize_test_paths(text, environment).as_str(),
                            ));
                        }
                    }
                    _ => normalize_tui_smoke_value(child, environment),
                }
            }
        }
        Value::Array(values) => {
            for child in values {
                normalize_tui_smoke_value(child, environment);
            }
        }
        Value::String(text) => {
            *text = normalize_session_words(normalize_test_paths(text, environment).as_str());
        }
        _ => {}
    }
}

fn normalize_session_path_segments(value: &str) -> String {
    value
        .split('/')
        .map(|segment| {
            if segment.starts_with("sess_") {
                String::from("<session-id>")
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_session_words(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            if let Some((prefix, suffix)) = word.split_once('=')
                && suffix.starts_with("sess_")
            {
                format!("{prefix}=<session-id>")
            } else if word.starts_with("sess_") {
                String::from("<session-id>")
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_loopback_ports(value: &str) -> String {
    let mut normalized = String::new();
    let mut remaining = value;
    while let Some(index) = remaining.find("127.0.0.1:") {
        normalized.push_str(&remaining[..index]);
        normalized.push_str("127.0.0.1:<port>");
        let digits = &remaining[index + "127.0.0.1:".len()..];
        let consumed = digits
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .count();
        remaining = &digits[consumed..];
    }
    normalized.push_str(remaining);
    normalized
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        FakeAppleFmServer, FakeHttpResponse, FakeOpenAiServer, ProbeTestEnvironment,
        configure_snapshot_root, normalize_exec_stderr_for_snapshot, normalize_workspace_path,
        probe_cli_command, write_apple_fm_attach_server_config, write_openai_attach_server_config,
    };

    #[test]
    fn fake_server_captures_requests_and_replies() {
        let server =
            FakeOpenAiServer::from_responses(vec![FakeHttpResponse::text_status(200, "ok")]);
        let response = reqwest::blocking::get(server.base_url()).expect("issue test request");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("GET /v1 HTTP/1.1"));
    }

    #[test]
    fn fake_apple_fm_server_uses_root_base_url() {
        let server =
            FakeAppleFmServer::from_responses(vec![FakeHttpResponse::text_status(200, "ok")]);
        let response = reqwest::blocking::get(server.base_url()).expect("issue test request");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("GET / HTTP/1.1"));
    }

    #[test]
    fn probe_test_environment_seeds_coding_fixture() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        assert!(environment.workspace().join("README.md").exists());
        assert!(environment.workspace().join("src/main.rs").exists());
        assert!(environment.workspace().join("src/lib.rs").exists());
        assert!(environment.workspace().join("hello.txt").exists());
    }

    #[test]
    fn workspace_path_normalization_replaces_workspace_root() {
        let normalized = normalize_workspace_path(format!(
            "{}/crates/probe-core/src/runtime.rs",
            super::workspace_root().display()
        ));
        assert!(normalized.starts_with("$PROBE_WORKSPACE_ROOT"));
    }

    #[test]
    fn snapshot_root_configuration_sets_insta_workspace_root() {
        configure_snapshot_root();
        assert_eq!(
            std::env::var("INSTA_WORKSPACE_ROOT").ok(),
            Some(super::workspace_root().display().to_string())
        );
    }

    #[test]
    fn probe_cli_command_uses_current_binary_name() {
        let command = probe_cli_command();
        let program = command.get_program().to_string_lossy();
        assert!(program.contains("probe-cli"));
    }

    #[test]
    fn attach_config_helpers_write_expected_files() {
        let environment = ProbeTestEnvironment::new();
        let openai_server =
            FakeOpenAiServer::from_responses(vec![FakeHttpResponse::text_status(200, "ok")]);
        let apple_server =
            FakeAppleFmServer::from_responses(vec![FakeHttpResponse::text_status(200, "ok")]);

        let openai_path = write_openai_attach_server_config(&environment, &openai_server, "tiny");
        let apple_path =
            write_apple_fm_attach_server_config(&environment, &apple_server, "apple-model");

        assert!(openai_path.ends_with("psionic-local.json"));
        assert!(apple_path.ends_with("psionic-apple-fm.json"));
        assert!(
            fs::read_to_string(openai_path)
                .expect("read openai config")
                .contains("tiny")
        );
        assert!(
            fs::read_to_string(apple_path)
                .expect("read apple fm config")
                .contains("apple_foundation_models")
        );
    }

    #[test]
    fn stderr_normalization_stabilizes_dynamic_fields() {
        let environment = ProbeTestEnvironment::new();
        let raw = format!(
            "backend_target profile=tailnet target=100.0.0.1:8080 base_url=http://100.0.0.1:8080/v1\nsession=sess_123 transcript={}/sessions/sess_123/transcript.jsonl\nobservability wallclock_ms=12 model_output_ms=34 completion_tps=56",
            environment.temp_root().display()
        );
        let normalized = normalize_exec_stderr_for_snapshot(raw.as_str(), &environment);
        assert!(normalized.contains("target=<target>"));
        assert!(normalized.contains("base_url=<base-url>"));
        assert!(normalized.contains("session=<session-id>"));
        assert!(normalized.contains("wallclock_ms=<dynamic>"));
    }
}
