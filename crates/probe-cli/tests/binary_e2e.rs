use assert_cmd::prelude::*;
use insta::{assert_json_snapshot, assert_snapshot};
use probe_core::runtime::ProbeRuntime;
use probe_test_support::{
    ProbeTestEnvironment, configure_snapshot_root, normalize_chat_stderr_for_snapshot,
    normalized_tui_smoke_report_snapshot, probe_cli_command, write_openai_attach_server_config,
};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Output, Stdio};

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
