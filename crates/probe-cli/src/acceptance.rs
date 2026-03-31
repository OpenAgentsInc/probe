use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use probe_core::harness::resolve_harness_profile;
use probe_core::runtime::{PlainTextExecOutcome, PlainTextExecRequest, ProbeRuntime};
use probe_core::tools::{ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction, ToolLoopConfig};
use probe_protocol::backend::BackendProfile;
use probe_protocol::session::{
    CacheSignal, SessionMetadata, ToolPolicyDecision, TranscriptEvent, TranscriptItemKind,
    TurnObservability,
};
use serde::Serialize;

const ACCEPTANCE_REPEAT_RUNS: usize = 2;

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
    pub repeat_runs_per_case: usize,
    pub results: Vec<AcceptanceCaseReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AcceptanceCaseReport {
    pub case_name: String,
    pub passed: bool,
    pub repeat_runs: usize,
    pub median_wallclock_ms: Option<u64>,
    pub session_id: Option<String>,
    pub assistant_text: Option<String>,
    pub executed_tool_calls: usize,
    pub error: Option<String>,
    pub attempts: Vec<AcceptanceAttemptReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AcceptanceAttemptReport {
    pub attempt_index: usize,
    pub passed: bool,
    pub session_id: Option<String>,
    pub assistant_text: Option<String>,
    pub executed_tool_calls: usize,
    pub tool_names: Vec<String>,
    pub auto_allowed_tool_calls: usize,
    pub approved_tool_calls: usize,
    pub refused_tool_calls: usize,
    pub paused_tool_calls: usize,
    pub final_wallclock_ms: Option<u64>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    pub cache_signal: Option<String>,
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
    let results = vec![
        run_case_read_file_answer(&runtime, &config.base_profile, config.probe_home.as_path()),
        run_case_list_then_read(&runtime, &config.base_profile, config.probe_home.as_path()),
        run_case_search_then_read(&runtime, &config.base_profile, config.probe_home.as_path()),
        run_case_shell_then_summarize(&runtime, &config.base_profile, config.probe_home.as_path()),
        run_case_patch_then_verify(&runtime, &config.base_profile, config.probe_home.as_path()),
        run_case_approval_pause_or_refusal(
            &runtime,
            &config.base_profile,
            config.probe_home.as_path(),
        ),
    ];

    let report = AcceptanceRunReport {
        started_at_ms,
        finished_at_ms: now_ms(),
        base_url: config.base_profile.base_url.clone(),
        model: config.base_profile.model.clone(),
        overall_pass: results.iter().all(|result| result.passed),
        repeat_runs_per_case: ACCEPTANCE_REPEAT_RUNS,
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

fn run_case_read_file_answer(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    probe_home: &Path,
) -> AcceptanceCaseReport {
    run_repeated_case(
        runtime,
        base_profile,
        probe_home,
        "read_file_answer",
        |runtime, profile, workspace, title| {
            execute_coding_case(
                runtime,
                profile,
                workspace,
                title,
                "Use read_file on README.md and answer with exactly READ_FILE_OK once you confirm the first line says Probe acceptance fixture.",
                coding_tool_loop(false, false, false, ToolDeniedAction::Refuse),
            )
        },
        |attempt, _workspace| {
            attempt.assistant_text.as_deref() == Some("READ_FILE_OK")
                && attempt.executed_tool_calls >= 1
                && attempt.tool_names.iter().any(|name| name == "read_file")
        },
    )
}

fn run_case_list_then_read(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    probe_home: &Path,
) -> AcceptanceCaseReport {
    run_repeated_case(
        runtime,
        base_profile,
        probe_home,
        "list_then_read",
        |runtime, profile, workspace, title| {
            execute_coding_case(
                runtime,
                profile,
                workspace,
                title,
                "Use list_files on src, then read src/main.rs, then answer with exactly LIST_READ_OK if the file prints PROBE_FIXTURE_MAIN.",
                coding_tool_loop(false, false, false, ToolDeniedAction::Refuse),
            )
        },
        |attempt, _workspace| {
            attempt.assistant_text.as_deref() == Some("LIST_READ_OK")
                && attempt.tool_names.iter().any(|name| name == "list_files")
                && attempt.tool_names.iter().any(|name| name == "read_file")
        },
    )
}

fn run_case_search_then_read(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    probe_home: &Path,
) -> AcceptanceCaseReport {
    run_repeated_case(
        runtime,
        base_profile,
        probe_home,
        "search_then_read",
        |runtime, profile, workspace, title| {
            execute_coding_case(
                runtime,
                profile,
                workspace,
                title,
                "Use code_search for beta_function, then read the matching file, then answer with exactly SEARCH_READ_OK if beta_function exists.",
                coding_tool_loop(false, false, false, ToolDeniedAction::Refuse),
            )
        },
        |attempt, _workspace| {
            attempt.assistant_text.as_deref() == Some("SEARCH_READ_OK")
                && attempt.tool_names.iter().any(|name| name == "code_search")
                && attempt.tool_names.iter().any(|name| name == "read_file")
        },
    )
}

fn run_case_shell_then_summarize(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    probe_home: &Path,
) -> AcceptanceCaseReport {
    run_repeated_case(
        runtime,
        base_profile,
        probe_home,
        "shell_then_summarize",
        |runtime, profile, workspace, title| {
            execute_coding_case(
                runtime,
                profile,
                workspace,
                title,
                "Use a read-only shell command to print the current working directory, then answer with exactly SHELL_OK.",
                coding_tool_loop(false, false, false, ToolDeniedAction::Refuse),
            )
        },
        |attempt, _workspace| {
            attempt.assistant_text.as_deref() == Some("SHELL_OK")
                && attempt.tool_names.iter().any(|name| name == "shell")
                && attempt.auto_allowed_tool_calls >= 1
        },
    )
}

fn run_case_patch_then_verify(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    probe_home: &Path,
) -> AcceptanceCaseReport {
    run_repeated_case(
        runtime,
        base_profile,
        probe_home,
        "patch_then_verify",
        |runtime, profile, workspace, title| {
            execute_coding_case(
                runtime,
                profile,
                workspace,
                title,
                "Use apply_patch to replace world with probe in hello.txt, then read hello.txt, then answer with exactly PATCH_OK.",
                coding_tool_loop(true, false, false, ToolDeniedAction::Refuse),
            )
        },
        |attempt, workspace| {
            attempt.assistant_text.as_deref() == Some("PATCH_OK")
                && attempt.tool_names.iter().any(|name| name == "apply_patch")
                && attempt.tool_names.iter().any(|name| name == "read_file")
                && attempt.approved_tool_calls >= 1
                && fs::read_to_string(workspace.join("hello.txt"))
                    .map(|content| content == "hello probe\n")
                    .unwrap_or(false)
        },
    )
}

fn run_case_approval_pause_or_refusal(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    probe_home: &Path,
) -> AcceptanceCaseReport {
    run_repeated_case(
        runtime,
        base_profile,
        probe_home,
        "approval_pause_or_refusal",
        |runtime, profile, workspace, title| {
            execute_coding_case(
                runtime,
                profile,
                workspace,
                title,
                "Use apply_patch to replace world with blocked in hello.txt.",
                coding_tool_loop(false, false, false, ToolDeniedAction::Pause),
            )
        },
        |attempt, workspace| {
            attempt.paused_tool_calls >= 1
                && attempt.tool_names.iter().any(|name| name == "apply_patch")
                && attempt
                    .error
                    .as_deref()
                    .unwrap_or_default()
                    .contains("paused for approval")
                && fs::read_to_string(workspace.join("hello.txt"))
                    .map(|content| content == "hello world\n")
                    .unwrap_or(false)
        },
    )
}

fn run_repeated_case<F, G>(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    probe_home: &Path,
    case_name: &str,
    mut runner: F,
    mut validator: G,
) -> AcceptanceCaseReport
where
    F: FnMut(&ProbeRuntime, &BackendProfile, &Path, &str) -> Result<PlainTextExecOutcome, String>,
    G: FnMut(&AcceptanceAttemptReport, &Path) -> bool,
{
    let mut attempts = Vec::new();
    for attempt_index in 0..ACCEPTANCE_REPEAT_RUNS {
        let title = format!("acceptance-{case_name}-{}", attempt_index + 1);
        let workspace = prepare_acceptance_workspace(probe_home, case_name, attempt_index)
            .unwrap_or_else(|error| {
                panic!("failed to prepare acceptance workspace for {case_name}: {error}")
            });
        let outcome = runner(runtime, base_profile, workspace.as_path(), title.as_str());
        let mut attempt = capture_attempt_report(runtime, title.as_str(), attempt_index, outcome);
        attempt.passed = validator(&attempt, workspace.as_path());
        attempts.push(attempt);
    }

    build_case_report(case_name, attempts)
}

fn execute_coding_case(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    workspace: &Path,
    title: &str,
    prompt: &str,
    tool_loop: ToolLoopConfig,
) -> Result<PlainTextExecOutcome, String> {
    let resolved = resolve_harness_profile(
        Some("coding_bootstrap"),
        Some("coding_bootstrap_default"),
        workspace,
        None,
    )?
    .ok_or_else(|| String::from("missing coding bootstrap harness profile"))?;

    runtime
        .exec_plain_text(PlainTextExecRequest {
            profile: base_profile.clone(),
            prompt: String::from(prompt),
            title: Some(String::from(title)),
            cwd: workspace.to_path_buf(),
            system_prompt: Some(resolved.system_prompt),
            harness_profile: Some(resolved.profile),
            tool_loop: Some(tool_loop),
        })
        .map_err(|error| error.to_string())
}

fn coding_tool_loop(
    allow_write_tools: bool,
    allow_network_shell: bool,
    allow_destructive_shell: bool,
    denied_action: ToolDeniedAction,
) -> ToolLoopConfig {
    let mut tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Required, false);
    tool_loop.approval = ToolApprovalConfig {
        allow_write_tools,
        allow_network_shell,
        allow_destructive_shell,
        denied_action,
    };
    tool_loop
}

fn capture_attempt_report(
    runtime: &ProbeRuntime,
    title: &str,
    attempt_index: usize,
    outcome: Result<PlainTextExecOutcome, String>,
) -> AcceptanceAttemptReport {
    let session_metadata = match &outcome {
        Ok(outcome) => Some(outcome.session.clone()),
        Err(_) => find_session_by_title(runtime, title),
    };
    let transcript = session_metadata.as_ref().and_then(|metadata| {
        runtime
            .session_store()
            .read_transcript(&metadata.id)
            .ok()
            .map(|events| (metadata.clone(), events))
    });
    let transcript_summary = transcript
        .as_ref()
        .map(|(_, events)| summarize_transcript(events.as_slice()));

    let (assistant_text, executed_tool_calls, error) = match outcome {
        Ok(outcome) => (
            Some(outcome.assistant_text),
            outcome.executed_tool_calls,
            None,
        ),
        Err(error) => {
            let executed = transcript_summary
                .as_ref()
                .map(|summary| summary.auto_allowed_tool_calls + summary.approved_tool_calls)
                .unwrap_or(0);
            (None, executed, Some(error))
        }
    };

    AcceptanceAttemptReport {
        attempt_index,
        passed: false,
        session_id: session_metadata
            .as_ref()
            .map(|metadata| metadata.id.as_str().to_string()),
        assistant_text,
        executed_tool_calls,
        tool_names: transcript_summary
            .as_ref()
            .map(|summary| summary.tool_names.clone())
            .unwrap_or_default(),
        auto_allowed_tool_calls: transcript_summary
            .as_ref()
            .map(|summary| summary.auto_allowed_tool_calls)
            .unwrap_or(0),
        approved_tool_calls: transcript_summary
            .as_ref()
            .map(|summary| summary.approved_tool_calls)
            .unwrap_or(0),
        refused_tool_calls: transcript_summary
            .as_ref()
            .map(|summary| summary.refused_tool_calls)
            .unwrap_or(0),
        paused_tool_calls: transcript_summary
            .as_ref()
            .map(|summary| summary.paused_tool_calls)
            .unwrap_or(0),
        final_wallclock_ms: transcript_summary
            .as_ref()
            .and_then(|summary| summary.final_observability.as_ref())
            .map(|observability| observability.wallclock_ms),
        prompt_tokens: transcript_summary
            .as_ref()
            .and_then(|summary| summary.final_observability.as_ref())
            .and_then(|observability| observability.prompt_tokens),
        completion_tokens: transcript_summary
            .as_ref()
            .and_then(|summary| summary.final_observability.as_ref())
            .and_then(|observability| observability.completion_tokens),
        total_tokens: transcript_summary
            .as_ref()
            .and_then(|summary| summary.final_observability.as_ref())
            .and_then(|observability| observability.total_tokens),
        cache_signal: transcript_summary
            .as_ref()
            .and_then(|summary| summary.final_observability.as_ref())
            .map(|observability| render_cache_signal(observability.cache_signal).to_string()),
        error,
    }
}

fn build_case_report(
    case_name: &str,
    attempts: Vec<AcceptanceAttemptReport>,
) -> AcceptanceCaseReport {
    let passed = attempts.iter().all(|attempt| attempt.passed);
    let median_wallclock_ms = median(
        attempts
            .iter()
            .filter_map(|attempt| attempt.final_wallclock_ms)
            .collect(),
    );
    let summary_attempt = attempts.last();

    AcceptanceCaseReport {
        case_name: String::from(case_name),
        passed,
        repeat_runs: attempts.len(),
        median_wallclock_ms,
        session_id: summary_attempt.and_then(|attempt| attempt.session_id.clone()),
        assistant_text: summary_attempt.and_then(|attempt| attempt.assistant_text.clone()),
        executed_tool_calls: summary_attempt
            .map(|attempt| attempt.executed_tool_calls)
            .unwrap_or(0),
        error: attempts.iter().find_map(|attempt| attempt.error.clone()),
        attempts,
    }
}

fn prepare_acceptance_workspace(
    probe_home: &Path,
    case_name: &str,
    attempt_index: usize,
) -> Result<PathBuf, String> {
    let workspace = probe_home
        .join("acceptance_workspaces")
        .join(case_name)
        .join(format!("attempt_{}", attempt_index + 1));
    if workspace.exists() {
        fs::remove_dir_all(&workspace).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(workspace.join("src")).map_err(|error| error.to_string())?;
    fs::create_dir_all(workspace.join("notes")).map_err(|error| error.to_string())?;

    fs::write(
        workspace.join("README.md"),
        "Probe acceptance fixture\nThis workspace exists for coding-lane acceptance.\n",
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        workspace.join("src/main.rs"),
        "fn main() {\n    println!(\"PROBE_FIXTURE_MAIN\");\n}\n",
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        workspace.join("src/lib.rs"),
        "pub fn alpha_function() {}\npub fn beta_function() {}\n",
    )
    .map_err(|error| error.to_string())?;
    fs::write(workspace.join("hello.txt"), "hello world\n").map_err(|error| error.to_string())?;
    fs::write(
        workspace.join("notes/summary.txt"),
        "acceptance harness fixture\n",
    )
    .map_err(|error| error.to_string())?;

    Ok(workspace)
}

fn find_session_by_title(runtime: &ProbeRuntime, title: &str) -> Option<SessionMetadata> {
    runtime
        .session_store()
        .list_sessions()
        .ok()?
        .into_iter()
        .find(|metadata| metadata.title == title)
}

#[derive(Clone, Debug, Default)]
struct TranscriptSummary {
    tool_names: Vec<String>,
    auto_allowed_tool_calls: usize,
    approved_tool_calls: usize,
    refused_tool_calls: usize,
    paused_tool_calls: usize,
    final_observability: Option<TurnObservability>,
}

fn summarize_transcript(events: &[TranscriptEvent]) -> TranscriptSummary {
    let mut summary = TranscriptSummary::default();
    for event in events {
        if let Some(observability) = event.turn.observability.clone() {
            summary.final_observability = Some(observability);
        }
        for item in &event.turn.items {
            if item.kind != TranscriptItemKind::ToolResult {
                continue;
            }
            if let Some(name) = item.name.as_ref() {
                summary.tool_names.push(name.clone());
            }
            if let Some(tool_execution) = item.tool_execution.as_ref() {
                match tool_execution.policy_decision {
                    ToolPolicyDecision::AutoAllow => summary.auto_allowed_tool_calls += 1,
                    ToolPolicyDecision::Approved => summary.approved_tool_calls += 1,
                    ToolPolicyDecision::Refused => summary.refused_tool_calls += 1,
                    ToolPolicyDecision::Paused => summary.paused_tool_calls += 1,
                }
            }
        }
    }
    summary
}

fn median(mut values: Vec<u64>) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[values.len() / 2])
}

fn render_cache_signal(signal: CacheSignal) -> &'static str {
    match signal {
        CacheSignal::Unknown => "unknown",
        CacheSignal::ColdStart => "cold_start",
        CacheSignal::LikelyWarm => "likely_warm",
        CacheSignal::NoClearSignal => "no_clear_signal",
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
            let mut responses = Vec::new();

            for attempt in 0..2 {
                let call_id = format!("call_readme_{}", attempt + 1);
                responses.push(serde_json::json!({
                    "id": format!("read_file_tool_{}", attempt + 1),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": call_id, "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"README.md\"}"}}]}, "finish_reason": "tool_calls"}]
                }));
                responses.push(serde_json::json!({
                    "id": format!("read_file_final_{}", attempt + 1),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "READ_FILE_OK"}, "finish_reason": "stop"}]
                }));
            }

            for attempt in 0..2 {
                responses.push(serde_json::json!({
                    "id": format!("list_then_read_list_{}", attempt + 1),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_list_{}", attempt + 1), "type": "function", "function": {"name": "list_files", "arguments": "{\"path\":\"src\"}"}}]}, "finish_reason": "tool_calls"}]
                }));
                responses.push(serde_json::json!({
                    "id": format!("list_then_read_read_{}", attempt + 1),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_read_main_{}", attempt + 1), "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"src/main.rs\"}"}}]}, "finish_reason": "tool_calls"}]
                }));
                responses.push(serde_json::json!({
                    "id": format!("list_then_read_final_{}", attempt + 1),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "LIST_READ_OK"}, "finish_reason": "stop"}]
                }));
            }

            for attempt in 0..2 {
                responses.push(serde_json::json!({
                    "id": format!("search_then_read_search_{}", attempt + 1),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_search_{}", attempt + 1), "type": "function", "function": {"name": "code_search", "arguments": "{\"pattern\":\"beta_function\",\"path\":\"src\"}"}}]}, "finish_reason": "tool_calls"}]
                }));
                responses.push(serde_json::json!({
                    "id": format!("search_then_read_read_{}", attempt + 1),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_read_lib_{}", attempt + 1), "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"src/lib.rs\"}"}}]}, "finish_reason": "tool_calls"}]
                }));
                responses.push(serde_json::json!({
                    "id": format!("search_then_read_final_{}", attempt + 1),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "SEARCH_READ_OK"}, "finish_reason": "stop"}]
                }));
            }

            for attempt in 0..2 {
                responses.push(serde_json::json!({
                    "id": format!("shell_then_summarize_tool_{}", attempt + 1),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_shell_{}", attempt + 1), "type": "function", "function": {"name": "shell", "arguments": "{\"command\":\"pwd\",\"timeout_secs\":2}"}}]}, "finish_reason": "tool_calls"}]
                }));
                responses.push(serde_json::json!({
                    "id": format!("shell_then_summarize_final_{}", attempt + 1),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "SHELL_OK"}, "finish_reason": "stop"}]
                }));
            }

            for attempt in 0..2 {
                responses.push(serde_json::json!({
                    "id": format!("patch_then_verify_patch_{}", attempt + 1),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_patch_{}", attempt + 1), "type": "function", "function": {"name": "apply_patch", "arguments": "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}"}}]}, "finish_reason": "tool_calls"}]
                }));
                responses.push(serde_json::json!({
                    "id": format!("patch_then_verify_read_{}", attempt + 1),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_verify_{}", attempt + 1), "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"hello.txt\"}"}}]}, "finish_reason": "tool_calls"}]
                }));
                responses.push(serde_json::json!({
                    "id": format!("patch_then_verify_final_{}", attempt + 1),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "PATCH_OK"}, "finish_reason": "stop"}]
                }));
            }

            for attempt in 0..2 {
                responses.push(serde_json::json!({
                    "id": format!("approval_pause_{}", attempt + 1),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": format!("call_blocked_patch_{}", attempt + 1), "type": "function", "function": {"name": "apply_patch", "arguments": "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"blocked\"}"}}]}, "finish_reason": "tool_calls"}]
                }));
            }

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
        assert_eq!(report.results.len(), 6);
        assert_eq!(report.repeat_runs_per_case, 2);
        assert!(report_path.exists());

        handle.join().expect("server thread should exit cleanly");
    }
}
