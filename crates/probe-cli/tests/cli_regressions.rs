use assert_cmd::prelude::*;
use insta::{assert_json_snapshot, assert_snapshot};
use predicates::prelude::*;
use probe_test_support::{
    FakeOpenAiServer, ProbeTestEnvironment, configure_snapshot_root,
    normalize_exec_stderr_for_snapshot, normalized_acceptance_report_snapshot, probe_cli_command,
    selected_transcript_event_snapshot, write_openai_attach_server_config,
};
use serde_json::{Value, json};

const TEST_MODEL: &str = "tiny-qwen35";

#[test]
fn exec_process_renders_stable_stderr_and_persists_selected_transcript_event() {
    configure_snapshot_root();
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();

    let server = FakeOpenAiServer::from_json_responses(vec![
        models_response(),
        json!({
            "id": "chatcmpl_exec_success",
            "model": TEST_MODEL,
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "EXEC_OK"},
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 42,
                "completion_tokens": 3,
                "total_tokens": 45
            }
        }),
    ]);
    write_openai_attach_server_config(&environment, &server, TEST_MODEL);

    let output = probe_cli_command()
        .arg("exec")
        .arg("--probe-home")
        .arg(environment.probe_home())
        .arg("--cwd")
        .arg(environment.workspace())
        .arg("--tool-set")
        .arg("coding_bootstrap")
        .arg("--harness-profile")
        .arg("coding_bootstrap_default")
        .arg("Reply with exactly EXEC_OK.")
        .assert()
        .success()
        .get_output()
        .clone();

    assert_eq!(String::from_utf8_lossy(&output.stdout), "EXEC_OK\n");
    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("GET /v1/models HTTP/1.1"));
    assert!(requests[1].contains("POST /v1/chat/completions HTTP/1.1"));
    assert!(requests[1].contains("coding_bootstrap harness profile v1"));

    let stderr = normalize_exec_stderr_for_snapshot(
        String::from_utf8_lossy(&output.stderr).as_ref(),
        &environment,
    );
    assert_snapshot!("exec_stderr_summary", stderr);

    let transcript_event = selected_transcript_event_snapshot(environment.probe_home());
    assert_json_snapshot!("exec_transcript_event", transcript_event);
}

#[test]
fn chat_resume_rejects_prompt_overrides() {
    probe_cli_command()
        .args(["chat", "--resume", "sess_fake", "--title", "Nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "resume does not accept --title, --system, or --harness-profile overrides",
        ));
}

#[test]
fn tui_help_is_available() {
    probe_cli_command()
        .args(["tui", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Launch the current Probe terminal UI",
        ));
}

#[test]
fn accept_process_emits_stable_report_shape() {
    configure_snapshot_root();
    let environment = ProbeTestEnvironment::new();
    let report_path = environment.probe_home().join("reports/acceptance.json");
    let server = FakeOpenAiServer::from_json_responses(acceptance_response_sequence());
    write_openai_attach_server_config(&environment, &server, TEST_MODEL);

    probe_cli_command()
        .arg("accept")
        .arg("--probe-home")
        .arg(environment.probe_home())
        .arg("--report-path")
        .arg(report_path.as_path())
        .arg("--model")
        .arg(TEST_MODEL)
        .assert()
        .success()
        .stderr(predicate::str::contains("overall_pass=true"));

    let requests = server.finish();
    assert_eq!(requests.len(), 29);
    assert!(requests[0].contains("/models"));
    assert!(requests[1].contains("POST /v1/chat/completions HTTP/1.1"));

    let report = normalized_acceptance_report_snapshot(report_path.as_path(), &environment);
    assert_json_snapshot!("accept_report", report);
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

fn acceptance_response_sequence() -> Vec<Value> {
    let mut responses = vec![models_response()];

    for attempt in 0..2 {
        let call_id = format!("call_readme_{}", attempt + 1);
        responses.push(json!({
            "id": format!("read_file_tool_{}", attempt + 1),
            "model": TEST_MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": call_id, "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"README.md\"}"}}]}, "finish_reason": "tool_calls"}]
        }));
        responses.push(json!({
            "id": format!("read_file_final_{}", attempt + 1),
            "model": TEST_MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "READ_FILE_OK"}, "finish_reason": "stop"}]
        }));
    }

    for attempt in 0..2 {
        responses.push(json!({
            "id": format!("list_then_read_list_{}", attempt + 1),
            "model": TEST_MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_list_{}", attempt + 1), "type": "function", "function": {"name": "list_files", "arguments": "{\"path\":\"src\"}"}}]}, "finish_reason": "tool_calls"}]
        }));
        responses.push(json!({
            "id": format!("list_then_read_read_{}", attempt + 1),
            "model": TEST_MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_read_main_{}", attempt + 1), "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"src/main.rs\"}"}}]}, "finish_reason": "tool_calls"}]
        }));
        responses.push(json!({
            "id": format!("list_then_read_final_{}", attempt + 1),
            "model": TEST_MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "LIST_READ_OK"}, "finish_reason": "stop"}]
        }));
    }

    for attempt in 0..2 {
        responses.push(json!({
            "id": format!("search_then_read_search_{}", attempt + 1),
            "model": TEST_MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_search_{}", attempt + 1), "type": "function", "function": {"name": "code_search", "arguments": "{\"pattern\":\"beta_function\",\"path\":\"src\"}"}}]}, "finish_reason": "tool_calls"}]
        }));
        responses.push(json!({
            "id": format!("search_then_read_read_{}", attempt + 1),
            "model": TEST_MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_read_lib_{}", attempt + 1), "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"src/lib.rs\"}"}}]}, "finish_reason": "tool_calls"}]
        }));
        responses.push(json!({
            "id": format!("search_then_read_final_{}", attempt + 1),
            "model": TEST_MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "SEARCH_READ_OK"}, "finish_reason": "stop"}]
        }));
    }

    for attempt in 0..2 {
        responses.push(json!({
            "id": format!("shell_then_summarize_tool_{}", attempt + 1),
            "model": TEST_MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_shell_{}", attempt + 1), "type": "function", "function": {"name": "shell", "arguments": "{\"command\":\"pwd\",\"timeout_secs\":2}"}}]}, "finish_reason": "tool_calls"}]
        }));
        responses.push(json!({
            "id": format!("shell_then_summarize_final_{}", attempt + 1),
            "model": TEST_MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "SHELL_OK"}, "finish_reason": "stop"}]
        }));
    }

    for attempt in 0..2 {
        responses.push(json!({
            "id": format!("patch_then_verify_patch_{}", attempt + 1),
            "model": TEST_MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_patch_{}", attempt + 1), "type": "function", "function": {"name": "apply_patch", "arguments": "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}"}}]}, "finish_reason": "tool_calls"}]
        }));
        responses.push(json!({
            "id": format!("patch_then_verify_read_{}", attempt + 1),
            "model": TEST_MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_verify_{}", attempt + 1), "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"hello.txt\"}"}}]}, "finish_reason": "tool_calls"}]
        }));
        responses.push(json!({
            "id": format!("patch_then_verify_final_{}", attempt + 1),
            "model": TEST_MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "PATCH_OK"}, "finish_reason": "stop"}]
        }));
    }

    for attempt in 0..2 {
        responses.push(json!({
            "id": format!("approval_pause_{}", attempt + 1),
            "model": TEST_MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_blocked_patch_{}", attempt + 1), "type": "function", "function": {"name": "apply_patch", "arguments": "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"blocked\"}"}}]}, "finish_reason": "tool_calls"}]
        }));
    }

    responses
}
