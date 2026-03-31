use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

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
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        listener
            .set_nonblocking(true)
            .expect("set fake server nonblocking");
        let address = listener.local_addr().expect("fake server address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_thread = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);

        let thread = thread::spawn(move || {
            let mut response_index = 0_usize;
            while response_index < responses.len() && !stop_thread.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_request(&mut stream);
                        requests_thread
                            .lock()
                            .expect("fake server request lock")
                            .push(request);

                        let response = &responses[response_index];
                        response_index += 1;
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

#[cfg(test)]
mod tests {
    use super::{
        FakeAppleFmServer, FakeHttpResponse, FakeOpenAiServer, ProbeTestEnvironment,
        normalize_workspace_path,
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
}
