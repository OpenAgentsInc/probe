#![cfg(unix)]

use std::fs;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

use probe_client::{ProbeClient, ProbeClientConfig, ProbeClientTransportConfig};
use probe_core::runtime::PlainTextExecRequest;
use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
use probe_protocol::default_local_daemon_socket_path;
use probe_test_support::{FakeHttpResponse, FakeOpenAiServer, ProbeTestEnvironment};

const TEST_MODEL: &str = "tiny-qwen35";

#[test]
fn daemon_accepts_multiple_sequential_clients_and_preserves_sessions() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let fake_backend = FakeOpenAiServer::from_responses(vec![FakeHttpResponse::text_event_stream(
        200,
        concat!(
            "data: {\"id\":\"chatcmpl_daemon_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hello\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_daemon_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" from daemon\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_daemon_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":3,\"total_tokens\":6}}\n\n"
        ),
    )]);
    let mut daemon = DaemonProcess::start(environment.probe_home());
    let mut first_client = daemon_client(environment.probe_home());

    let outcome = first_client
        .exec_plain_text(PlainTextExecRequest {
            profile: test_profile(fake_backend.base_url()),
            prompt: String::from("hello"),
            title: Some(String::from("daemon session")),
            cwd: environment.workspace().to_path_buf(),
            system_prompt: None,
            harness_profile: None,
            tool_loop: None,
        })
        .expect("daemon turn should succeed");
    assert_eq!(outcome.assistant_text, "hello from daemon");

    drop(first_client);

    let mut second_client = daemon_client(environment.probe_home());
    let sessions = second_client
        .list_sessions()
        .expect("second daemon client should list sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "daemon session");
    let snapshot = second_client
        .resume_session(&sessions[0].id)
        .expect("second daemon client should resume session");
    assert_eq!(snapshot.session.title, "daemon session");
    assert!(!snapshot.transcript.is_empty());

    drop(second_client);
    daemon.stop();
}

#[test]
fn daemon_startup_reaps_stale_socket_before_accepting_clients() {
    let environment = ProbeTestEnvironment::new();
    let socket_path = default_local_daemon_socket_path(environment.probe_home());
    fs::create_dir_all(
        socket_path
            .parent()
            .expect("daemon socket path should have a parent directory"),
    )
    .expect("daemon socket parent should be creatable");
    let listener = UnixListener::bind(&socket_path).expect("stale socket should bind");
    drop(listener);
    assert!(socket_path.exists(), "stale socket should remain on disk");

    let mut daemon = DaemonProcess::start(environment.probe_home());
    let mut client = daemon_client(environment.probe_home());
    let sessions = client
        .list_sessions()
        .expect("daemon should accept client connections after stale cleanup");
    assert!(sessions.is_empty());

    drop(client);
    daemon.stop();
}

struct DaemonProcess {
    child: Child,
    probe_home: PathBuf,
}

impl DaemonProcess {
    fn start(probe_home: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_probe-daemon"))
            .arg("run")
            .arg("--probe-home")
            .arg(probe_home)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("probe-daemon should spawn");
        wait_for_daemon_socket(probe_home, &mut child);
        Self {
            child,
            probe_home: probe_home.to_path_buf(),
        }
    }

    fn stop(&mut self) {
        if self
            .child
            .try_wait()
            .expect("daemon child should support try_wait")
            .is_some()
        {
            return;
        }
        if let Ok(mut client) = try_daemon_client(self.probe_home.as_path()) {
            let _ = client.shutdown();
        } else {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        self.stop();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn daemon_client(probe_home: &Path) -> ProbeClient {
    try_daemon_client(probe_home).expect("daemon client should connect")
}

fn try_daemon_client(probe_home: &Path) -> Result<ProbeClient, probe_client::ProbeClientError> {
    let mut config = ProbeClientConfig::new(probe_home.to_path_buf(), "probe-daemon-test");
    config.transport = ProbeClientTransportConfig::LocalDaemon { socket_path: None };
    ProbeClient::connect(config)
}

fn wait_for_daemon_socket(probe_home: &Path, child: &mut Child) {
    let socket_path = default_local_daemon_socket_path(probe_home);
    for _ in 0..100 {
        if socket_path.exists() && UnixStream::connect(&socket_path).is_ok() {
            return;
        }
        if let Some(status) = child
            .try_wait()
            .expect("daemon child should support try_wait")
        {
            panic!("probe-daemon exited before exposing socket: {status}");
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!(
        "probe-daemon did not expose socket at {} in time",
        socket_path.display()
    );
}

fn test_profile(base_url: &str) -> BackendProfile {
    BackendProfile {
        name: String::from("daemon-test"),
        kind: BackendKind::OpenAiChatCompletions,
        base_url: String::from(base_url),
        model: String::from(TEST_MODEL),
        reasoning_level: None,
        api_key_env: String::from("PROBE_OPENAI_API_KEY"),
        timeout_secs: 30,
        attach_mode: ServerAttachMode::AttachToExisting,
        prefix_cache_mode: PrefixCacheMode::BackendDefault,
    }
}
