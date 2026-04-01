#![cfg(unix)]

use std::fs;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

use probe_client::{ProbeClient, ProbeClientConfig, ProbeClientTransportConfig};
use probe_core::runtime::{PlainTextExecRequest, PlainTextResumeRequest};
use probe_core::tools::{ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction, ToolLoopConfig};
use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
use probe_protocol::default_local_daemon_socket_path;
use probe_protocol::runtime::{
    DetachedSessionRecoveryState, DetachedSessionStatus, StartSessionRequest,
};
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
    let detached_sessions = second_client
        .list_detached_sessions()
        .expect("daemon should list detached sessions");
    assert_eq!(detached_sessions.len(), 1);
    assert_eq!(detached_sessions[0].session_id, sessions[0].id);
    assert_eq!(
        detached_sessions[0].status,
        DetachedSessionStatus::Completed
    );

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

#[test]
fn detached_session_registry_tracks_background_work_after_client_disconnect() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let fake_backend = delayed_completion_backend(Duration::from_millis(200), "hello from queue");
    let profile = test_profile(fake_backend.base_url());
    let mut daemon = DaemonProcess::start(environment.probe_home());
    let mut client = daemon_client(environment.probe_home());
    let session = client
        .start_session(StartSessionRequest {
            title: Some(String::from("detached queue")),
            cwd: environment.workspace().to_path_buf(),
            profile: profile.clone(),
            system_prompt: None,
            harness_profile: None,
        })
        .expect("daemon should start session");
    let session_id = session.session.id.clone();
    let detached_before = client
        .list_detached_sessions()
        .expect("daemon should list detached sessions");
    assert_eq!(detached_before.len(), 1);
    assert_eq!(detached_before[0].status, DetachedSessionStatus::Idle);

    let queued = client
        .queue_plain_text_session_turn(PlainTextResumeRequest {
            session_id: session_id.clone(),
            profile: profile.clone(),
            prompt: String::from("say hello from the detached queue"),
            tool_loop: None,
        })
        .expect("queue turn should be accepted");
    assert_eq!(
        queued.turn.status,
        probe_protocol::runtime::QueuedTurnStatus::Running
    );

    drop(client);

    let summary = wait_for_detached_status(
        environment.probe_home(),
        &session_id,
        DetachedSessionStatus::Completed,
    );
    assert_eq!(summary.queued_turn_count, 0);
    assert_eq!(summary.pending_approval_count, 0);
    let inspected = daemon_client(environment.probe_home())
        .inspect_detached_session(&session_id)
        .expect("detached session should be inspectable");
    assert_eq!(inspected.summary.status, DetachedSessionStatus::Completed);
    assert!(
        inspected
            .turn_control
            .recent_turns
            .iter()
            .any(|turn| turn.turn_id == queued.turn.turn_id)
    );

    daemon.stop();
}

#[test]
fn daemon_restart_keeps_approval_paused_sessions_resumable() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let fake_backend = approval_pause_backend(Duration::from_millis(50));
    let profile = test_profile(fake_backend.base_url());
    let mut daemon = DaemonProcess::start(environment.probe_home());
    let mut client = daemon_client(environment.probe_home());
    let session = client
        .start_session(StartSessionRequest {
            title: Some(String::from("approval pause")),
            cwd: environment.workspace().to_path_buf(),
            profile: profile.clone(),
            system_prompt: None,
            harness_profile: None,
        })
        .expect("daemon should start session");
    let session_id = session.session.id.clone();

    let error = client
        .continue_plain_text_session(PlainTextResumeRequest {
            session_id: session_id.clone(),
            profile: profile.clone(),
            prompt: String::from("patch hello.txt"),
            tool_loop: Some(approval_pause_tool_loop()),
        })
        .expect_err("approval pause should surface through probe-client");
    assert!(matches!(
        error,
        probe_client::ProbeClientError::ToolApprovalPending { .. }
    ));

    daemon.kill_ungraceful();
    let mut restarted = DaemonProcess::start(environment.probe_home());
    let mut reattached = daemon_client(environment.probe_home());
    let inspected = reattached
        .inspect_detached_session(&session_id)
        .expect("approval-paused detached session should remain inspectable");
    assert_eq!(
        inspected.summary.status,
        DetachedSessionStatus::ApprovalPaused
    );
    assert_eq!(
        inspected.summary.recovery_state,
        DetachedSessionRecoveryState::ApprovalPausedResumable
    );
    assert_eq!(inspected.summary.pending_approval_count, 1);
    assert!(inspected.turn_control.active_turn.is_some());
    assert!(
        inspected
            .session
            .pending_approvals
            .iter()
            .any(|approval| approval.tool_call_id == "call_patch_1")
    );

    drop(reattached);
    restarted.kill_ungraceful();
}

#[test]
fn daemon_restart_marks_running_turns_as_failed_when_the_process_dies() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let fake_backend =
        delayed_completion_backend(Duration::from_millis(500), "this should never complete");
    let profile = test_profile(fake_backend.base_url());
    let mut daemon = DaemonProcess::start(environment.probe_home());
    let mut client = daemon_client(environment.probe_home());
    let session = client
        .start_session(StartSessionRequest {
            title: Some(String::from("restart failure")),
            cwd: environment.workspace().to_path_buf(),
            profile: profile.clone(),
            system_prompt: None,
            harness_profile: None,
        })
        .expect("daemon should start session");
    let session_id = session.session.id.clone();

    client
        .queue_plain_text_session_turn(PlainTextResumeRequest {
            session_id: session_id.clone(),
            profile: profile.clone(),
            prompt: String::from("run through a restart"),
            tool_loop: None,
        })
        .expect("queue turn should be accepted");
    drop(client);
    let running = wait_for_detached_status(
        environment.probe_home(),
        &session_id,
        DetachedSessionStatus::Running,
    );
    assert!(running.active_turn_id.is_some());

    daemon.kill_ungraceful();
    let mut restarted = DaemonProcess::start(environment.probe_home());
    let mut reattached = daemon_client(environment.probe_home());
    let inspected = reattached
        .inspect_detached_session(&session_id)
        .expect("restarted daemon should report failed running turn");
    assert_eq!(inspected.summary.status, DetachedSessionStatus::Failed);
    assert_eq!(
        inspected.summary.recovery_state,
        DetachedSessionRecoveryState::RunningTurnFailedOnRestart
    );
    assert!(
        inspected
            .summary
            .recovery_note
            .as_deref()
            .is_some_and(|note| note.contains("restarted before this running turn completed"))
    );
    assert!(inspected
        .turn_control
        .recent_turns
        .first()
        .and_then(|turn| turn.failure_message.as_deref())
        .is_some_and(|message| message.contains("restarted before this running turn completed")));

    drop(reattached);
    restarted.stop();
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

    fn kill_ungraceful(&mut self) {
        let _ = self.child.kill();
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

fn wait_for_detached_status(
    probe_home: &Path,
    session_id: &probe_protocol::session::SessionId,
    expected: DetachedSessionStatus,
) -> probe_protocol::runtime::DetachedSessionSummary {
    let mut last_summary = None;
    for _ in 0..100 {
        let mut client = daemon_client(probe_home);
        let sessions = client
            .list_detached_sessions()
            .expect("daemon should list detached sessions");
        if let Some(summary) = sessions
            .into_iter()
            .find(|summary| summary.session_id == *session_id)
        {
            if summary.status == expected {
                return summary;
            }
            last_summary = Some(summary);
        }
        drop(client);
        thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for detached status {expected:?}: {last_summary:?}");
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

fn delayed_completion_backend(delay: Duration, assistant_text: &str) -> FakeOpenAiServer {
    let assistant_text = String::from(assistant_text);
    FakeOpenAiServer::from_handler(move |_request| {
        thread::sleep(delay);
        FakeHttpResponse::json_ok(serde_json::json!({
            "id": "chatcmpl_detached_complete",
            "model": TEST_MODEL,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": assistant_text.clone()
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 3,
                "completion_tokens": 3,
                "total_tokens": 6
            }
        }))
    })
}

fn approval_pause_backend(delay: Duration) -> FakeOpenAiServer {
    FakeOpenAiServer::from_handler(move |_request| {
        thread::sleep(delay);
        FakeHttpResponse::json_ok(serde_json::json!({
            "id": "chatcmpl_detached_pause",
            "model": TEST_MODEL,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_patch_1",
                        "type": "function",
                        "function": {
                            "name": "apply_patch",
                            "arguments": "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }))
    })
}

fn approval_pause_tool_loop() -> ToolLoopConfig {
    let mut tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Required, false);
    tool_loop.approval = ToolApprovalConfig {
        allow_write_tools: false,
        allow_network_shell: false,
        allow_destructive_shell: false,
        denied_action: ToolDeniedAction::Pause,
    };
    tool_loop
}
