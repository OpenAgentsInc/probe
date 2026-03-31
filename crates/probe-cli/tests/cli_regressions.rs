use std::fs;
use std::path::Path;
use std::process::Command;

use assert_cmd::cargo::CommandCargoExt;
use assert_cmd::prelude::*;
use insta::{assert_json_snapshot, assert_snapshot};
use predicates::prelude::*;
use probe_test_support::{FakeOpenAiServer, ProbeTestEnvironment, normalize_workspace_path};
use serde_json::{Value, json};

const TEST_MODEL: &str = "tiny-qwen35";

#[test]
fn exec_process_renders_stable_stderr_and_persists_selected_transcript_event() {
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
    write_attach_server_config(&environment, &server, TEST_MODEL);

    let output = probe_command()
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

    let stderr = normalize_exec_stderr(
        String::from_utf8_lossy(&output.stderr).as_ref(),
        &environment,
    );
    assert_snapshot!("exec_stderr_summary", stderr);

    let transcript_event = selected_transcript_event(environment.probe_home());
    assert_json_snapshot!("exec_transcript_event", transcript_event);
}

#[test]
fn exec_rejects_incompatible_harness_and_tool_set() {
    let environment = ProbeTestEnvironment::new();
    let server = FakeOpenAiServer::from_json_responses(vec![models_response()]);
    write_attach_server_config(&environment, &server, TEST_MODEL);

    probe_command()
        .arg("exec")
        .arg("--probe-home")
        .arg(environment.probe_home())
        .arg("--tool-set")
        .arg("weather")
        .arg("--harness-profile")
        .arg("coding_bootstrap_default")
        .arg("Hello")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "harness profile `coding_bootstrap_default` is not available for the `weather` tool set",
        ));

    let requests = server.finish();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("/v1/models"));
}

#[test]
fn chat_resume_rejects_prompt_overrides() {
    probe_command()
        .args(["chat", "--resume", "sess_fake", "--title", "Nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "resume does not accept --title, --system, or --harness-profile overrides",
        ));
}

#[test]
fn accept_process_emits_stable_report_shape() {
    let environment = ProbeTestEnvironment::new();
    let report_path = environment.probe_home().join("reports/acceptance.json");
    let server = FakeOpenAiServer::from_json_responses(acceptance_response_sequence());
    write_attach_server_config(&environment, &server, TEST_MODEL);

    probe_command()
        .arg("accept")
        .arg("--probe-home")
        .arg(environment.probe_home())
        .arg("--report-path")
        .arg(report_path.as_path())
        .arg("--model")
        .arg(TEST_MODEL)
        .assert()
        .success()
        .stderr(predicate::str::contains("acceptance overall_pass=true"));

    let requests = server.finish();
    assert_eq!(requests.len(), 29);
    assert!(requests[0].contains("GET /v1/models HTTP/1.1"));
    assert!(requests[1].contains("POST /v1/chat/completions HTTP/1.1"));

    let report = normalized_acceptance_report(report_path.as_path());
    assert_json_snapshot!("accept_report", report);
}

fn probe_command() -> Command {
    Command::cargo_bin("probe-cli").expect("probe-cli binary should build for tests")
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

fn write_attach_server_config(
    environment: &ProbeTestEnvironment,
    server: &FakeOpenAiServer,
    model_id: &str,
) {
    let address = server
        .base_url()
        .strip_prefix("http://")
        .expect("base url should start with http://")
        .strip_suffix("/v1")
        .expect("base url should end with /v1");
    let (host, port) = address.rsplit_once(':').expect("host:port pair");
    let port = port.parse::<u16>().expect("port should parse");
    let config_path = environment.probe_home().join("server/psionic-local.json");
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).expect("create server config directory");
    }
    fs::write(
        &config_path,
        serde_json::to_string_pretty(&json!({
            "mode": "attach",
            "host": host,
            "port": port,
            "backend": "cpu",
            "binary_path": null,
            "model_path": null,
            "model_id": model_id,
            "reasoning_budget": null
        }))
        .expect("encode server config"),
    )
    .expect("write server config");
}

fn normalize_exec_stderr(raw: &str, environment: &ProbeTestEnvironment) -> String {
    raw.lines()
        .map(|line| {
            if line.starts_with("session=") {
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

fn normalize_test_paths(value: &str, environment: &ProbeTestEnvironment) -> String {
    let temp_root = environment.temp_root().display().to_string();
    let replaced = normalize_workspace_path(value.replace(temp_root.as_str(), "$TEST_ROOT"));
    normalize_session_path_segments(replaced.as_str())
}

fn selected_transcript_event(probe_home: &Path) -> Value {
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
    json!({
        "turn_index": event["turn"]["index"],
        "observability": {
            "cache_signal": event["turn"]["observability"]["cache_signal"],
            "prompt_tokens": event["turn"]["observability"]["prompt_tokens"],
            "completion_tokens": event["turn"]["observability"]["completion_tokens"],
            "total_tokens": event["turn"]["observability"]["total_tokens"]
        },
        "items": event["turn"]["items"]
            .as_array()
            .expect("items array")
            .iter()
            .map(|item| {
                json!({
                    "kind": item["kind"],
                    "name": item["name"],
                    "text": item["text"],
                    "tool_execution": item["tool_execution"]
                })
            })
            .collect::<Vec<_>>()
    })
}

fn normalized_acceptance_report(report_path: &Path) -> Value {
    let mut value: Value =
        serde_json::from_str(&fs::read_to_string(report_path).expect("read acceptance report"))
            .expect("decode acceptance report");
    value["started_at_ms"] = json!("<ms>");
    value["finished_at_ms"] = json!("<ms>");
    value["base_url"] = json!("<base-url>");
    for result in value["results"].as_array_mut().expect("results array") {
        if result["median_wallclock_ms"].is_number() {
            result["median_wallclock_ms"] = json!("<ms>");
        }
        if result["session_id"].is_string() {
            result["session_id"] = json!("<session-id>");
        }
        if let Some(error) = result["error"].as_str() {
            result["error"] = json!(normalize_session_words(error));
        }
        for attempt in result["attempts"].as_array_mut().expect("attempts array") {
            if attempt["session_id"].is_string() {
                attempt["session_id"] = json!("<session-id>");
            }
            if attempt["final_wallclock_ms"].is_number() {
                attempt["final_wallclock_ms"] = json!("<ms>");
            }
            if let Some(error) = attempt["error"].as_str() {
                attempt["error"] = json!(normalize_session_words(error));
            }
        }
    }
    value
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
            if word.starts_with("sess_") {
                String::from("<session-id>")
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
