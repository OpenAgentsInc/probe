use assert_cmd::prelude::*;
use insta::{assert_json_snapshot, assert_snapshot};
use probe_client::{ProbeClient, ProbeClientConfig, ProbeClientTransportConfig};
use probe_core::runtime::{PlainTextResumeRequest, ProbeRuntime};
use probe_core::tools::{ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction, ToolLoopConfig};
use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
use probe_protocol::default_local_daemon_socket_path;
use probe_protocol::runtime::{DetachedSessionStatus, StartSessionRequest};
use probe_test_support::{
    FakeHttpResponse, ProbeTestEnvironment, configure_snapshot_root,
    normalize_chat_stderr_for_snapshot, normalized_tui_smoke_report_snapshot, probe_cli_command,
    write_openai_attach_server_config,
};
use serde_json::{Value, json};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::Duration;

const TEST_MODEL: &str = "tiny-qwen35";

#[test]
fn chat_process_can_create_and_resume_a_session_from_stdin() {
    configure_snapshot_root();
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let server = probe_test_support::FakeOpenAiServer::from_json_responses(vec![
        models_response(),
        json!({
            "id": "chatcmpl_chat_turn_1",
            "model": TEST_MODEL,
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "First chat reply."},
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 5,
                "completion_tokens": 3,
                "total_tokens": 8
            }
        }),
        models_response(),
        json!({
            "id": "chatcmpl_chat_turn_2",
            "model": TEST_MODEL,
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "Resumed chat reply."},
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 6,
                "completion_tokens": 3,
                "total_tokens": 9
            }
        }),
    ]);
    write_openai_attach_server_config(&environment, &server, TEST_MODEL);

    let first_output = probe_cli_command()
        .args([
            "chat",
            "--probe-home",
            environment.probe_home().to_str().expect("probe home utf-8"),
            "--cwd",
            environment.workspace().to_str().expect("workspace utf-8"),
        ])
        .pipe_stdin("hello\n/quit\n");
    assert!(String::from_utf8_lossy(&first_output.stdout).contains("assistant> First chat reply."));

    let runtime = ProbeRuntime::new(environment.probe_home());
    let sessions = runtime
        .session_store()
        .list_sessions()
        .expect("list sessions should succeed");
    assert_eq!(sessions.len(), 1);
    let session_id = sessions[0].id.clone();

    let detached = wait_for_detached_status(
        environment.probe_home(),
        &session_id,
        DetachedSessionStatus::Completed,
    );
    assert_eq!(detached.session_id, session_id);

    let second_output = probe_cli_command()
        .args([
            "chat",
            "--resume",
            session_id.as_str(),
            "--probe-home",
            environment.probe_home().to_str().expect("probe home utf-8"),
        ])
        .pipe_stdin("again\n/quit\n");
    assert!(
        String::from_utf8_lossy(&second_output.stdout).contains("assistant> Resumed chat reply.")
    );

    let transcript = runtime
        .session_store()
        .read_transcript(&session_id)
        .expect("read transcript should succeed");
    assert_eq!(transcript.len(), 2);
    assert_eq!(transcript[1].turn.items[1].text, "Resumed chat reply.");

    let mut daemon = daemon_client(environment.probe_home());
    let attached = daemon
        .inspect_detached_session(&session_id)
        .expect("chat session should remain inspectable through the daemon");
    assert_eq!(attached.summary.status, DetachedSessionStatus::Completed);
    assert_eq!(attached.session.transcript.len(), 2);
    daemon
        .shutdown()
        .expect("chat test daemon should stop cleanly");

    let stderr = format!(
        "{}\n---\n{}",
        normalize_chat_stderr_for_snapshot(
            String::from_utf8_lossy(&first_output.stderr).as_ref(),
            &environment,
        ),
        normalize_chat_stderr_for_snapshot(
            String::from_utf8_lossy(&second_output.stderr).as_ref(),
            &environment,
        ),
    );
    assert_snapshot!("chat_process_resume", stderr);

    let requests = server.finish();
    assert_eq!(requests.len(), 4);
    assert!(requests[0].contains("GET /v1/models HTTP/1.1"));
    assert!(requests[1].contains("hello"));
    assert!(requests[2].contains("GET /v1/models HTTP/1.1"));
    assert!(requests[3].contains("again"));
}

#[test]
fn tui_process_can_resume_detached_daemon_session() {
    configure_snapshot_root();
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let first_report_path = environment
        .probe_home()
        .join("reports/tui_resume_first.json");
    let second_report_path = environment
        .probe_home()
        .join("reports/tui_resume_attach_only.json");
    let server = probe_test_support::FakeOpenAiServer::from_json_responses(vec![
        json!({
            "id": "chatcmpl_probe_tui_tool_resume_1",
            "model": TEST_MODEL,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_readme_resume_1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"README.md\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
        json!({
            "id": "chatcmpl_probe_tui_final_resume_1",
            "model": TEST_MODEL,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Probe inspected README.md through the real runtime."
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 21,
                "completion_tokens": 9,
                "total_tokens": 30
            }
        }),
    ]);
    write_openai_attach_server_config(&environment, &server, TEST_MODEL);

    probe_cli_command()
        .arg("tui")
        .arg("--probe-home")
        .arg(environment.probe_home())
        .arg("--cwd")
        .arg(environment.workspace())
        .arg("--smoke-prompt")
        .arg("hello")
        .arg("--smoke-wait-for-text")
        .arg("Probe inspected README.md through the real runtime.")
        .arg("--smoke-wait-for-worker-event")
        .arg("pending approvals cleared")
        .arg("--smoke-report-path")
        .arg(first_report_path.as_path())
        .assert()
        .success();

    let first_report = read_smoke_report(first_report_path.as_path());
    let session_id = first_report["runtime_session_id"]
        .as_str()
        .expect("first tui run should report a runtime session id")
        .to_string();
    let mut daemon = daemon_client(environment.probe_home());
    let first_attached = daemon
        .inspect_detached_session(&probe_protocol::session::SessionId::new(session_id.clone()))
        .expect("first tui session should be inspectable");
    let first_turn_count = first_attached.session.transcript.len();
    drop(daemon);

    probe_cli_command()
        .arg("tui")
        .arg("--resume")
        .arg(session_id.as_str())
        .arg("--probe-home")
        .arg(environment.probe_home())
        .arg("--smoke-attach-only")
        .arg("--smoke-wait-for-text")
        .arg("Probe inspected README.md through the real runtime.")
        .arg("--smoke-wait-for-worker-event")
        .arg("runtime session ready:")
        .arg("--smoke-report-path")
        .arg(second_report_path.as_path())
        .assert()
        .success();

    let second_report = read_smoke_report(second_report_path.as_path());
    assert_eq!(
        second_report["runtime_session_id"].as_str(),
        Some(session_id.as_str())
    );
    assert!(
        second_report["final_render"].as_str().is_some_and(
            |render| render.contains("Probe inspected README.md through the real runtime.")
        )
    );

    let mut daemon = daemon_client(environment.probe_home());
    let attached = daemon
        .inspect_detached_session(&probe_protocol::session::SessionId::new(session_id.clone()))
        .expect("attached tui session should remain inspectable");
    assert_eq!(attached.summary.status, DetachedSessionStatus::Completed);
    assert_eq!(attached.session.transcript.len(), first_turn_count);
    daemon
        .shutdown()
        .expect("tui test daemon should stop cleanly");

    let requests = server.finish();
    assert_eq!(requests.len(), 2);
}

#[test]
fn tui_process_smoke_drives_a_real_background_turn() {
    configure_snapshot_root();
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let report_path = environment.probe_home().join("reports/tui_smoke.json");
    let server = probe_test_support::FakeOpenAiServer::from_json_responses(vec![
        json!({
            "id": "chatcmpl_probe_tui_tool_1",
            "model": TEST_MODEL,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_readme_1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"README.md\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
        json!({
            "id": "chatcmpl_probe_tui_final_1",
            "model": TEST_MODEL,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Probe inspected README.md through the real runtime."
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 21,
                "completion_tokens": 9,
                "total_tokens": 30
            }
        }),
    ]);
    write_openai_attach_server_config(&environment, &server, TEST_MODEL);

    probe_cli_command()
        .arg("tui")
        .arg("--probe-home")
        .arg(environment.probe_home())
        .arg("--cwd")
        .arg(environment.workspace())
        .arg("--smoke-prompt")
        .arg("hello")
        .arg("--smoke-wait-for-text")
        .arg("Probe inspected README.md through the real runtime.")
        .arg("--smoke-wait-for-worker-event")
        .arg("pending approvals cleared")
        .arg("--smoke-report-path")
        .arg(report_path.as_path())
        .assert()
        .success();

    let report = normalized_tui_smoke_report_snapshot(report_path.as_path(), &environment);
    assert_json_snapshot!("tui_smoke_report", report);

    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("read_file"));
    assert!(requests[1].contains("Probe acceptance fixture"));
}

#[test]
fn daemon_operator_commands_manage_detached_sessions() {
    configure_snapshot_root();
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let server = probe_test_support::FakeOpenAiServer::from_handler(move |_request| {
        thread::sleep(Duration::from_millis(50));
        FakeHttpResponse::json_ok(json!({
            "id": "chatcmpl_daemon_cli_pause",
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
    });
    let profile = test_profile(server.base_url());
    let _daemon = DaemonProcess::start(environment.probe_home());
    let mut client = daemon_client(environment.probe_home());
    let session = client
        .start_session(StartSessionRequest {
            title: Some(String::from("daemon cli approval pause")),
            cwd: environment.workspace().to_path_buf(),
            profile: profile.clone(),
            system_prompt: None,
            harness_profile: None,
            workspace_state: None,
        })
        .expect("daemon should start a session");
    let session_id = session.session.id.clone();
    client
        .queue_plain_text_session_turn(PlainTextResumeRequest {
            session_id: session_id.clone(),
            profile: profile.clone(),
            prompt: String::from("pause for daemon cli"),
            tool_loop: Some(approval_pause_tool_loop()),
        })
        .expect("queued turn should be accepted");
    drop(client);

    let paused = wait_for_detached_status(
        environment.probe_home(),
        &session_id,
        DetachedSessionStatus::ApprovalPaused,
    );
    assert_eq!(paused.pending_approval_count, 1);

    let ps_output = probe_cli_command()
        .arg("ps")
        .arg("--probe-home")
        .arg(environment.probe_home())
        .assert()
        .success()
        .get_output()
        .clone();
    let attach_output = probe_cli_command()
        .arg("attach")
        .arg(session_id.as_str())
        .arg("--probe-home")
        .arg(environment.probe_home())
        .assert()
        .success()
        .get_output()
        .clone();
    let logs_output = probe_cli_command()
        .arg("logs")
        .arg(session_id.as_str())
        .arg("--probe-home")
        .arg(environment.probe_home())
        .arg("--limit")
        .arg("8")
        .assert()
        .success()
        .get_output()
        .clone();
    let stop_output = probe_cli_command()
        .arg("stop")
        .arg(session_id.as_str())
        .arg("--probe-home")
        .arg(environment.probe_home())
        .assert()
        .success()
        .get_output()
        .clone();

    let mut inspect_client = daemon_client(environment.probe_home());
    let cancelled = inspect_client
        .inspect_detached_session(&session_id)
        .expect("cancelled detached session should be inspectable");
    assert_eq!(cancelled.summary.status, DetachedSessionStatus::Cancelled);
    drop(inspect_client);

    let daemon_stop_output = probe_cli_command()
        .arg("daemon")
        .arg("stop")
        .arg("--probe-home")
        .arg(environment.probe_home())
        .assert()
        .success()
        .get_output()
        .clone();

    let combined = format!(
        "PS\n{}\nATTACH\n{}\nLOGS\n{}\nSTOP\n{}\nDAEMON_STOP\n{}",
        String::from_utf8_lossy(&ps_output.stdout),
        String::from_utf8_lossy(&attach_output.stdout),
        stable_log_snapshot(String::from_utf8_lossy(&logs_output.stdout).as_ref()),
        String::from_utf8_lossy(&stop_output.stdout),
        String::from_utf8_lossy(&daemon_stop_output.stdout),
    );
    assert_snapshot!(
        "daemon_operator_commands",
        normalize_daemon_cli_snapshot(&combined, &environment, session_id.as_str())
    );

    let requests = server.finish();
    assert_eq!(requests.len(), 1);
}

fn models_response() -> Value {
    json!({
        "object": "list",
        "data": [
            {
                "id": TEST_MODEL,
                "object": "model",
                "owned_by": "probe-test"
            }
        ]
    })
}

trait CommandPipeExt {
    fn pipe_stdin(&mut self, input: &str) -> Output;
}

impl CommandPipeExt for Command {
    fn pipe_stdin(&mut self, input: &str) -> Output {
        let mut child = self
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn probe cli child");
        child
            .stdin
            .as_mut()
            .expect("child stdin")
            .write_all(input.as_bytes())
            .expect("write child stdin");
        let output = child.wait_with_output().expect("wait for child output");
        assert!(
            output.status.success(),
            "probe cli child failed: stdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        output
    }
}

struct DaemonProcess {
    child: Child,
    probe_home: PathBuf,
}

impl DaemonProcess {
    fn start(probe_home: &Path) -> Self {
        let daemon_binary = std::env::var("CARGO_BIN_EXE_probe-daemon")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let current_exe =
                    std::env::current_exe().expect("current test binary path should resolve");
                current_exe
                    .parent()
                    .and_then(Path::parent)
                    .expect("test binary should live under target/debug/deps")
                    .join("probe-daemon")
            });
        let mut child = Command::new(daemon_binary)
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
        let graceful_shutdown = try_daemon_client(self.probe_home.as_path())
            .and_then(|mut client| client.shutdown())
            .is_ok();
        if !graceful_shutdown {
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

fn read_smoke_report(path: &Path) -> Value {
    serde_json::from_str(
        std::fs::read_to_string(path)
            .expect("smoke report should be readable")
            .as_str(),
    )
    .expect("smoke report should parse as json")
}

fn try_daemon_client(probe_home: &Path) -> Result<ProbeClient, probe_client::ProbeClientError> {
    let mut config = ProbeClientConfig::new(probe_home.to_path_buf(), "probe-cli-test");
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
        let sessions = match client.list_detached_sessions() {
            Ok(sessions) => sessions,
            Err(_) => {
                drop(client);
                thread::sleep(Duration::from_millis(25));
                continue;
            }
        };
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

fn test_profile(base_url: &str) -> BackendProfile {
    BackendProfile {
        name: String::from("daemon-cli-test"),
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

fn normalize_daemon_cli_snapshot(
    value: &str,
    environment: &ProbeTestEnvironment,
    session_id: &str,
) -> String {
    let normalized = value
        .replace(
            environment.probe_home().to_str().expect("probe home utf-8"),
            "<PROBE_HOME>",
        )
        .replace(
            environment.workspace().to_str().expect("workspace utf-8"),
            "<WORKSPACE>",
        )
        .replace(session_id, "<SESSION_ID>");
    let normalized = normalize_key_value_field(normalized.as_str(), "requested_at_ms", "<TS>");
    let normalized = normalize_key_value_field(normalized.as_str(), "started_at_ms", "<TS>");
    let normalized = normalize_key_value_field(normalized.as_str(), "last_progress_at_ms", "<TS>");
    let normalized =
        normalize_key_value_field(normalized.as_str(), "execution_timeout_at_ms", "<TS>");
    normalize_key_value_field(normalized.as_str(), "cursor", "<CURSOR>")
}

fn stable_log_snapshot(value: &str) -> String {
    let mut lines = Vec::new();
    let mut saw_pending = false;
    let mut saw_paused_summary = false;
    for line in value.lines() {
        if !saw_pending && line.contains("kind=pending_approvals_updated") {
            lines.push(line.to_string());
            saw_pending = true;
            continue;
        }
        if !saw_paused_summary && line.contains("kind=summary_updated status=approval_paused") {
            lines.push(line.to_string());
            saw_paused_summary = true;
        }
    }
    lines.join("\n")
}

fn normalize_key_value_field(value: &str, key: &str, replacement: &str) -> String {
    let needle = format!("{key}=");
    value
        .lines()
        .map(|line| {
            let Some(start) = line.find(needle.as_str()) else {
                return line.to_string();
            };
            let value_start = start + needle.len();
            let value_end = line[value_start..]
                .find(' ')
                .map(|offset| value_start + offset)
                .unwrap_or(line.len());
            format!(
                "{}{}{}",
                &line[..value_start],
                replacement,
                &line[value_end..]
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
