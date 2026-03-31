use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use probe_core::runtime::{PlainTextExecRequest, PlainTextResumeRequest, ProbeRuntime};
use probe_core::tools::{ProbeToolChoice, ToolLoopConfig};
use probe_protocol::backend::BackendProfile;
use serde::Serialize;

#[derive(Clone, Debug)]
pub struct AcceptanceHarnessConfig {
    pub probe_home: PathBuf,
    pub report_path: PathBuf,
    pub base_profile: BackendProfile,
}

#[derive(Clone, Debug, Serialize)]
pub struct AcceptanceRunReport {
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub base_url: String,
    pub model: String,
    pub overall_pass: bool,
    pub results: Vec<AcceptanceCaseReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AcceptanceCaseReport {
    pub case_name: String,
    pub passed: bool,
    pub session_id: Option<String>,
    pub assistant_text: Option<String>,
    pub executed_tool_calls: usize,
    pub error: Option<String>,
}

pub fn run_acceptance_harness(
    config: AcceptanceHarnessConfig,
) -> Result<AcceptanceRunReport, String> {
    let started_at_ms = now_ms();
    fs::create_dir_all(config.probe_home.as_path()).map_err(|error| error.to_string())?;
    if let Some(parent) = config.report_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let runtime = ProbeRuntime::new(config.probe_home.clone());
    let mut results = Vec::new();

    results.push(run_case_plain_answer(&runtime, &config.base_profile));
    results.push(run_case_required_single_tool(
        &runtime,
        &config.base_profile,
    ));
    results.push(run_case_multi_turn_tool_continuation(
        &runtime,
        &config.base_profile,
    ));
    results.push(run_case_parallel_tool_batch(&runtime, &config.base_profile));

    let report = AcceptanceRunReport {
        started_at_ms,
        finished_at_ms: now_ms(),
        base_url: config.base_profile.base_url.clone(),
        model: config.base_profile.model.clone(),
        overall_pass: results.iter().all(|result| result.passed),
        results,
    };

    let report_json =
        serde_json::to_string_pretty(&report).map_err(|error| format!("json error: {error}"))?;
    fs::write(config.report_path, report_json).map_err(|error| error.to_string())?;
    Ok(report)
}

pub fn default_report_path(probe_home: &Path) -> PathBuf {
    probe_home
        .join("reports")
        .join(format!("probe_acceptance_{}.json", now_ms()))
}

fn run_case_plain_answer(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
) -> AcceptanceCaseReport {
    let outcome = runtime.exec_plain_text(PlainTextExecRequest {
        profile: base_profile.clone(),
        prompt: String::from("Reply with exactly PLAIN_OK and do not use tools."),
        title: Some(String::from("acceptance-plain")),
        cwd: PathBuf::from("."),
        system_prompt: None,
        tool_loop: None,
    });

    match outcome {
        Ok(outcome) => AcceptanceCaseReport {
            case_name: String::from("no_tool_plain_answer"),
            passed: outcome.assistant_text.contains("PLAIN_OK") && outcome.executed_tool_calls == 0,
            session_id: Some(outcome.session.id.as_str().to_string()),
            assistant_text: Some(outcome.assistant_text),
            executed_tool_calls: outcome.executed_tool_calls,
            error: None,
        },
        Err(error) => failed_case("no_tool_plain_answer", error.to_string()),
    }
}

fn run_case_required_single_tool(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
) -> AcceptanceCaseReport {
    let outcome = runtime.exec_plain_text(PlainTextExecRequest {
        profile: base_profile.clone(),
        prompt: String::from(
            "Use the `lookup_weather` tool and answer: what is the weather in Paris?",
        ),
        title: Some(String::from("acceptance-required-tool")),
        cwd: PathBuf::from("."),
        system_prompt: None,
        tool_loop: Some(ToolLoopConfig::weather_demo(
            ProbeToolChoice::Required,
            false,
        )),
    });

    match outcome {
        Ok(outcome) => AcceptanceCaseReport {
            case_name: String::from("required_single_tool_turn"),
            passed: outcome.executed_tool_calls == 1
                && outcome.assistant_text.contains("Paris")
                && outcome.assistant_text.contains("18"),
            session_id: Some(outcome.session.id.as_str().to_string()),
            assistant_text: Some(outcome.assistant_text),
            executed_tool_calls: outcome.executed_tool_calls,
            error: None,
        },
        Err(error) => failed_case("required_single_tool_turn", error.to_string()),
    }
}

fn run_case_multi_turn_tool_continuation(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
) -> AcceptanceCaseReport {
    let first = runtime.exec_plain_text(PlainTextExecRequest {
        profile: base_profile.clone(),
        prompt: String::from(
            "Use the `lookup_weather` tool and answer: what is the weather in Paris?",
        ),
        title: Some(String::from("acceptance-multi-turn")),
        cwd: PathBuf::from("."),
        system_prompt: None,
        tool_loop: Some(ToolLoopConfig::weather_demo(
            ProbeToolChoice::Required,
            false,
        )),
    });

    let first = match first {
        Ok(first) => first,
        Err(error) => return failed_case("multi_turn_tool_continuation", error.to_string()),
    };

    let second = runtime.continue_plain_text_session(PlainTextResumeRequest {
        session_id: first.session.id.clone(),
        profile: base_profile.clone(),
        prompt: String::from(
            "Now use the `lookup_weather` tool and answer: what is the weather in Tokyo?",
        ),
        tool_loop: Some(ToolLoopConfig::weather_demo(
            ProbeToolChoice::Required,
            false,
        )),
    });

    match second {
        Ok(outcome) => AcceptanceCaseReport {
            case_name: String::from("multi_turn_tool_continuation"),
            passed: outcome.executed_tool_calls == 1
                && outcome.assistant_text.contains("Tokyo")
                && outcome.assistant_text.contains("12"),
            session_id: Some(outcome.session.id.as_str().to_string()),
            assistant_text: Some(outcome.assistant_text),
            executed_tool_calls: outcome.executed_tool_calls,
            error: None,
        },
        Err(error) => failed_case("multi_turn_tool_continuation", error.to_string()),
    }
}

fn run_case_parallel_tool_batch(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
) -> AcceptanceCaseReport {
    let outcome = runtime.exec_plain_text(PlainTextExecRequest {
        profile: base_profile.clone(),
        prompt: String::from(
            "Use the `lookup_weather` tool for Paris and Tokyo in the same turn, then answer with both results.",
        ),
        title: Some(String::from("acceptance-parallel-tools")),
        cwd: PathBuf::from("."),
        system_prompt: None,
        tool_loop: Some(ToolLoopConfig::weather_demo(
            ProbeToolChoice::Required,
            true,
        )),
    });

    match outcome {
        Ok(outcome) => AcceptanceCaseReport {
            case_name: String::from("same_turn_two_tool_batch"),
            passed: outcome.executed_tool_calls == 2
                && outcome.assistant_text.contains("Paris")
                && outcome.assistant_text.contains("Tokyo"),
            session_id: Some(outcome.session.id.as_str().to_string()),
            assistant_text: Some(outcome.assistant_text),
            executed_tool_calls: outcome.executed_tool_calls,
            error: None,
        },
        Err(error) => failed_case("same_turn_two_tool_batch", error.to_string()),
    }
}

fn failed_case(case_name: &str, error: String) -> AcceptanceCaseReport {
    AcceptanceCaseReport {
        case_name: String::from(case_name),
        passed: false,
        session_id: None,
        assistant_text: None,
        executed_tool_calls: 0,
        error: Some(error),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use probe_core::backend_profiles::psionic_qwen35_2b_q8_registry;

    use super::{AcceptanceHarnessConfig, default_report_path, run_acceptance_harness};

    #[test]
    fn acceptance_harness_writes_report_against_mock_server() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            let responses = vec![
                serde_json::json!({
                    "id": "plain",
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "PLAIN_OK"}, "finish_reason": "stop"}]
                }),
                serde_json::json!({
                    "id": "required_tool_turn",
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": "call_paris", "type": "function", "function": {"name": "lookup_weather", "arguments": "{\"city\":\"Paris\"}"}}]}, "finish_reason": "tool_calls"}]
                }),
                serde_json::json!({
                    "id": "required_tool_final",
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "Paris is sunny at 18C."}, "finish_reason": "stop"}]
                }),
                serde_json::json!({
                    "id": "multi_turn_first_tool",
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": "call_paris_2", "type": "function", "function": {"name": "lookup_weather", "arguments": "{\"city\":\"Paris\"}"}}]}, "finish_reason": "tool_calls"}]
                }),
                serde_json::json!({
                    "id": "multi_turn_first_final",
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "Paris is sunny at 18C."}, "finish_reason": "stop"}]
                }),
                serde_json::json!({
                    "id": "multi_turn_second_tool",
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": "call_tokyo", "type": "function", "function": {"name": "lookup_weather", "arguments": "{\"city\":\"Tokyo\"}"}}]}, "finish_reason": "tool_calls"}]
                }),
                serde_json::json!({
                    "id": "multi_turn_second_final",
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "Tokyo is rainy at 12C."}, "finish_reason": "stop"}]
                }),
                serde_json::json!({
                    "id": "parallel_tool_turn",
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [
                        {"id": "call_weather_paris", "type": "function", "function": {"name": "lookup_weather", "arguments": "{\"city\":\"Paris\"}"}},
                        {"id": "call_weather_tokyo", "type": "function", "function": {"name": "lookup_weather", "arguments": "{\"city\":\"Tokyo\"}"}}
                    ]}, "finish_reason": "tool_calls"}]
                }),
                serde_json::json!({
                    "id": "parallel_tool_final",
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "Paris is sunny at 18C. Tokyo is rainy at 12C."}, "finish_reason": "stop"}]
                }),
            ];

            for body in responses {
                let (mut stream, _) = listener.accept().expect("accept connection");
                let mut buffer = [0_u8; 8192];
                let _ = stream.read(&mut buffer).expect("read request");
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });

        let temp = tempfile::tempdir().expect("temp dir");
        let probe_home = temp.path().join(".probe");
        let report_path = default_report_path(probe_home.as_path());
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = format!("http://{address}/v1");

        let report = run_acceptance_harness(AcceptanceHarnessConfig {
            probe_home,
            report_path: report_path.clone(),
            base_profile: profile,
        })
        .expect("acceptance harness should succeed");

        assert!(report.overall_pass);
        assert_eq!(report.results.len(), 4);
        assert!(report_path.exists());

        handle.join().expect("server thread should exit cleanly");
    }
}
