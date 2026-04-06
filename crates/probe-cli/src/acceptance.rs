use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use probe_core::harness::resolve_harness_profile;
use probe_core::runtime::{
    PlainTextExecOutcome, PlainTextExecRequest, PlainTextResumeRequest, ProbeRuntime,
    ResolvePendingToolApprovalOutcome, ResolvePendingToolApprovalRequest, RuntimeError,
};
use probe_core::tools::{ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction, ToolLoopConfig};
use probe_protocol::backend::{BackendKind, BackendProfile};
use probe_protocol::session::{
    BackendTurnReceipt, CacheSignal, SessionMetadata, ToolApprovalResolution, ToolPolicyDecision,
    TranscriptEvent, TranscriptItemKind, TurnObservability, UsageMeasurement,
};
use serde::{Deserialize, Serialize};

const ACCEPTANCE_REPEAT_RUNS: usize = 2;
const ACCEPTANCE_REPORT_SCHEMA_VERSION: &str = "v3";
const ACCEPTANCE_COMPARISON_REPORT_SCHEMA_VERSION: &str = "v1";
const ACCEPTANCE_MATRIX_REPORT_SCHEMA_VERSION: &str = "v1";
const ACCEPTANCE_TOOL_SET: &str = "coding_bootstrap";
const ACCEPTANCE_HARNESS_PROFILE_NAME: &str = "coding_bootstrap_default";
const ACCEPTANCE_HARNESS_PROFILE_VERSION: &str = "v1";
const ACCEPTANCE_CASE_NAMES: [&str; 6] = [
    "read_file_answer",
    "list_then_read",
    "search_then_read",
    "shell_then_summarize",
    "patch_then_verify",
    "approval_pause_or_refusal",
];
const SELF_TEST_CASE_NAMES: [&str; 10] = [
    "read_file_answer",
    "list_then_read",
    "search_then_read",
    "shell_then_summarize",
    "patch_then_verify",
    "approval_pause_or_refusal",
    "shell_failure_then_summarize",
    "multi_turn_session_resume",
    "approval_pause_then_resume",
    "backend_failure_is_honest",
];
const MATRIX_SCENARIO_NAMES: [&str; 6] = [
    "read_file_answer",
    "search_then_read",
    "patch_then_verify",
    "approval_pause_or_refusal",
    "streaming_reply_stability",
    "multi_turn_session_resume",
];

#[derive(Clone, Debug)]
pub struct AcceptanceHarnessConfig {
    pub probe_home: PathBuf,
    pub report_path: PathBuf,
    pub base_profile: BackendProfile,
}

#[derive(Clone, Debug)]
pub struct AcceptanceComparisonConfig {
    pub probe_home: PathBuf,
    pub report_path: PathBuf,
    pub qwen_profile: BackendProfile,
    pub apple_fm_profile: BackendProfile,
}

#[derive(Clone, Debug)]
pub struct AcceptanceMatrixConfig {
    pub probe_home: PathBuf,
    pub report_path: PathBuf,
    pub profiles: Vec<BackendProfile>,
    pub models: Vec<String>,
    pub harness_profiles: Vec<String>,
    pub scenarios: Vec<String>,
    pub repetitions: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceRunReport {
    pub run: AcceptanceRunIdentity,
    pub backend: AcceptanceBackendSummary,
    pub harness: AcceptanceHarnessSummary,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub duration_ms: u64,
    pub overall_pass: bool,
    pub counts: AcceptanceRunCounts,
    pub results: Vec<AcceptanceCaseReport>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceComparisonReport {
    pub run: AcceptanceRunIdentity,
    pub qwen_backend: AcceptanceBackendSummary,
    pub apple_fm_backend: AcceptanceBackendSummary,
    pub harness: AcceptanceHarnessSummary,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub duration_ms: u64,
    pub counts: AcceptanceComparisonCounts,
    pub qwen_report_path: PathBuf,
    pub apple_fm_report_path: PathBuf,
    pub cases: Vec<AcceptanceComparisonCaseReport>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceMatrixReport {
    pub run: AcceptanceRunIdentity,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub duration_ms: u64,
    pub repetitions_per_cell: usize,
    pub counts: AcceptanceMatrixCounts,
    pub cells: Vec<AcceptanceMatrixCellReport>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceRunIdentity {
    pub run_id: String,
    pub schema_version: String,
    pub probe_version: String,
    pub git_commit_sha: Option<String>,
    pub git_dirty: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceBackendSummary {
    pub profile_name: String,
    pub base_url: String,
    pub model: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceHarnessSummary {
    pub tool_set: String,
    pub profile_name: String,
    pub profile_version: String,
    pub repeat_runs_per_case: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceRunCounts {
    pub total_cases: usize,
    pub passed_cases: usize,
    pub failed_cases: usize,
    pub total_attempts: usize,
    pub passed_attempts: usize,
    pub failed_attempts: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceComparisonCounts {
    pub total_cases: usize,
    pub comparable_cases: usize,
    pub comparable_passed_cases: usize,
    pub comparable_failed_cases: usize,
    pub unsupported_backend_results: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceMatrixCounts {
    pub total_cells: usize,
    pub passed_cells: usize,
    pub failed_cells: usize,
    pub total_repetitions: usize,
    pub failed_repetitions: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceAttemptReport {
    pub attempt_index: usize,
    pub passed: bool,
    pub failure_category: Option<AcceptanceFailureCategory>,
    pub session_id: Option<String>,
    pub transcript_path: Option<PathBuf>,
    pub assistant_text: Option<String>,
    pub executed_tool_calls: usize,
    pub tool_names: Vec<String>,
    pub policy_counts: AcceptancePolicyCounts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability: Option<AcceptanceObservabilitySummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_receipt: Option<AcceptanceBackendReceiptSummary>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceCaseReport {
    pub case_name: String,
    pub case_index: usize,
    pub passed: bool,
    pub repeat_runs: usize,
    pub passed_attempts: usize,
    pub failed_attempts: usize,
    pub median_elapsed_ms: Option<u64>,
    pub latest_session_id: Option<String>,
    pub latest_transcript_path: Option<PathBuf>,
    pub latest_assistant_text: Option<String>,
    pub latest_executed_tool_calls: usize,
    pub latest_tool_names: Vec<String>,
    pub latest_policy_counts: AcceptancePolicyCounts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_observability: Option<AcceptanceObservabilitySummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_backend_receipt: Option<AcceptanceBackendReceiptSummary>,
    pub failure_category: Option<AcceptanceFailureCategory>,
    pub error: Option<String>,
    pub attempts: Vec<AcceptanceAttemptReport>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceComparisonCaseReport {
    pub case_name: String,
    pub status: AcceptanceComparisonStatus,
    pub qwen: AcceptanceComparisonBackendCaseReport,
    pub apple_fm: AcceptanceComparisonBackendCaseReport,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceMatrixCellReport {
    pub profile_name: String,
    pub model: String,
    pub harness_profile: String,
    pub scenario: String,
    pub passed: bool,
    pub worst_repetition_index: usize,
    pub worst_failure_category: Option<AcceptanceFailureCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worst_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worst_transcript_path: Option<PathBuf>,
    pub repetitions: Vec<AcceptanceAttemptReport>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AcceptancePolicyCounts {
    pub auto_allowed_tool_calls: usize,
    pub approved_tool_calls: usize,
    pub refused_tool_calls: usize,
    pub paused_tool_calls: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceObservabilitySummary {
    pub wallclock_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_output_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_detail: Option<UsageMeasurement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens_detail: Option<UsageMeasurement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens_detail: Option<UsageMeasurement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens_per_second_x1000: Option<u64>,
    pub cache_signal: CacheSignal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceBackendReceiptSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_retryable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_suggestion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal_explanation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability_ready: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability_reason_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability_platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_payload_bytes: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceComparisonBackendCaseReport {
    pub backend_profile_name: String,
    pub status: AcceptanceComparisonBackendCaseStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<AcceptanceCaseReport>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceFailureCategory {
    BackendFailure,
    ToolExecutionFailure,
    PolicyRefusal,
    PolicyPaused,
    VerificationFailure,
    ConfigurationFailure,
    UnknownFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceComparisonBackendCaseStatus {
    Passed,
    Failed,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceComparisonStatus {
    ComparablePass,
    ComparableFail,
    Unsupported,
}

#[derive(Clone, Debug)]
struct AcceptanceCaseSupport {
    unsupported_reason: Option<String>,
}

#[derive(Debug)]
enum AcceptanceExecutionError {
    Runtime(RuntimeError),
    Setup(String),
}

impl std::fmt::Display for AcceptanceExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime(error) => write!(f, "{error}"),
            Self::Setup(error) => write!(f, "{error}"),
        }
    }
}

impl AcceptanceCaseSupport {
    fn supported() -> Self {
        Self {
            unsupported_reason: None,
        }
    }

    fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            unsupported_reason: Some(reason.into()),
        }
    }

    fn is_supported(&self) -> bool {
        self.unsupported_reason.is_none()
    }
}

pub fn run_acceptance_harness(
    config: AcceptanceHarnessConfig,
) -> Result<AcceptanceRunReport, String> {
    run_acceptance_harness_for_case_names(config, retained_acceptance_case_names())
}

pub fn run_self_test_harness(
    config: AcceptanceHarnessConfig,
) -> Result<AcceptanceRunReport, String> {
    run_acceptance_harness_for_case_names(config, retained_self_test_case_names())
}

pub fn run_acceptance_comparison(
    config: AcceptanceComparisonConfig,
) -> Result<AcceptanceComparisonReport, String> {
    run_acceptance_comparison_for_case_names(config, retained_acceptance_case_names())
}

pub fn run_acceptance_matrix(
    config: AcceptanceMatrixConfig,
) -> Result<AcceptanceMatrixReport, String> {
    let started_at_ms = now_ms();
    fs::create_dir_all(config.probe_home.as_path()).map_err(|error| error.to_string())?;
    if let Some(parent) = config.report_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    if config.repetitions == 0 {
        return Err(String::from("matrix repetitions must be at least 1"));
    }
    if config.profiles.is_empty() {
        return Err(String::from("matrix requires at least one profile"));
    }

    let scenarios = if config.scenarios.is_empty() {
        retained_matrix_scenario_names()
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>()
    } else {
        config.scenarios.clone()
    };
    let harness_profiles = if config.harness_profiles.is_empty() {
        vec![String::from(ACCEPTANCE_HARNESS_PROFILE_NAME)]
    } else {
        config.harness_profiles.clone()
    };

    let run_id = format!("acceptance_matrix_{}_{}", started_at_ms, std::process::id());
    let matrix_root = config.probe_home.join("matrix_runs").join(run_id.as_str());
    let mut cells = Vec::new();
    let mut cell_index = 0_usize;

    for profile in &config.profiles {
        let models = if config.models.is_empty() {
            vec![profile.model.clone()]
        } else {
            config.models.clone()
        };
        for model in models {
            for harness_profile in &harness_profiles {
                for scenario in &scenarios {
                    let cell_probe_home = matrix_root.join(format!("cell_{cell_index}"));
                    let runtime = ProbeRuntime::new(cell_probe_home.clone());
                    let mut cell_profile = profile.clone();
                    cell_profile.model = model.clone();
                    let repetitions = (0..config.repetitions)
                        .map(|attempt_index| {
                            run_matrix_scenario_attempt(
                                &runtime,
                                &cell_profile,
                                cell_probe_home.as_path(),
                                scenario.as_str(),
                                harness_profile.as_str(),
                                attempt_index,
                            )
                        })
                        .collect::<Vec<_>>();
                    cells.push(build_matrix_cell_report(
                        profile.name.as_str(),
                        model.as_str(),
                        harness_profile.as_str(),
                        scenario.as_str(),
                        repetitions,
                    ));
                    cell_index += 1;
                }
            }
        }
    }

    let finished_at_ms = now_ms();
    let failed_cells = cells.iter().filter(|cell| !cell.passed).count();
    let failed_repetitions = cells
        .iter()
        .flat_map(|cell| cell.repetitions.iter())
        .filter(|attempt| !attempt.passed)
        .count();
    let git_state = current_probe_git_state();
    let report = AcceptanceMatrixReport {
        run: AcceptanceRunIdentity {
            run_id,
            schema_version: String::from(ACCEPTANCE_MATRIX_REPORT_SCHEMA_VERSION),
            probe_version: String::from(env!("CARGO_PKG_VERSION")),
            git_commit_sha: git_state.git_commit_sha,
            git_dirty: git_state.git_dirty,
        },
        started_at_ms,
        finished_at_ms,
        duration_ms: finished_at_ms.saturating_sub(started_at_ms),
        repetitions_per_cell: config.repetitions,
        counts: AcceptanceMatrixCounts {
            total_cells: cells.len(),
            passed_cells: cells.len().saturating_sub(failed_cells),
            failed_cells,
            total_repetitions: cells.iter().map(|cell| cell.repetitions.len()).sum(),
            failed_repetitions,
        },
        cells,
    };
    let report_json =
        serde_json::to_string_pretty(&report).map_err(|error| format!("json error: {error}"))?;
    fs::write(config.report_path, report_json).map_err(|error| error.to_string())?;
    Ok(report)
}

pub fn default_comparison_report_path(probe_home: &Path) -> PathBuf {
    probe_home
        .join("reports")
        .join(format!("probe_acceptance_compare_{}.json", now_ms()))
}

pub fn default_matrix_report_path(probe_home: &Path) -> PathBuf {
    probe_home
        .join("reports")
        .join(format!("probe_matrix_{}.json", now_ms()))
}

fn retained_acceptance_case_names() -> &'static [&'static str] {
    &ACCEPTANCE_CASE_NAMES
}

fn retained_self_test_case_names() -> &'static [&'static str] {
    &SELF_TEST_CASE_NAMES
}

fn retained_matrix_scenario_names() -> &'static [&'static str] {
    &MATRIX_SCENARIO_NAMES
}

fn run_acceptance_harness_for_case_names(
    config: AcceptanceHarnessConfig,
    case_names: &[&str],
) -> Result<AcceptanceRunReport, String> {
    let started_at_ms = now_ms();
    fs::create_dir_all(config.probe_home.as_path()).map_err(|error| error.to_string())?;
    if let Some(parent) = config.report_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let runtime = ProbeRuntime::new(config.probe_home.clone());
    let mut results = case_names
        .iter()
        .map(|case_name| {
            run_named_case(
                case_name,
                &runtime,
                &config.base_profile,
                config.probe_home.as_path(),
            )
        })
        .collect::<Vec<_>>();
    for (case_index, result) in results.iter_mut().enumerate() {
        result.case_index = case_index;
    }

    let finished_at_ms = now_ms();
    let counts = build_run_counts(results.as_slice());
    let git_state = current_probe_git_state();

    let report = AcceptanceRunReport {
        run: AcceptanceRunIdentity {
            run_id: format!("acceptance_{}_{}", started_at_ms, std::process::id()),
            schema_version: String::from(ACCEPTANCE_REPORT_SCHEMA_VERSION),
            probe_version: String::from(env!("CARGO_PKG_VERSION")),
            git_commit_sha: git_state.git_commit_sha,
            git_dirty: git_state.git_dirty,
        },
        backend: AcceptanceBackendSummary {
            profile_name: config.base_profile.name.clone(),
            base_url: config.base_profile.base_url.clone(),
            model: config.base_profile.model.clone(),
        },
        harness: AcceptanceHarnessSummary {
            tool_set: String::from(ACCEPTANCE_TOOL_SET),
            profile_name: String::from(ACCEPTANCE_HARNESS_PROFILE_NAME),
            profile_version: String::from(ACCEPTANCE_HARNESS_PROFILE_VERSION),
            repeat_runs_per_case: ACCEPTANCE_REPEAT_RUNS,
        },
        started_at_ms,
        finished_at_ms,
        duration_ms: finished_at_ms.saturating_sub(started_at_ms),
        overall_pass: results.iter().all(|result| result.passed),
        counts,
        results,
    };

    let report_json =
        serde_json::to_string_pretty(&report).map_err(|error| format!("json error: {error}"))?;
    fs::write(config.report_path, report_json).map_err(|error| error.to_string())?;
    Ok(report)
}

fn run_acceptance_comparison_for_case_names(
    config: AcceptanceComparisonConfig,
    case_names: &[&str],
) -> Result<AcceptanceComparisonReport, String> {
    let started_at_ms = now_ms();
    fs::create_dir_all(config.probe_home.as_path()).map_err(|error| error.to_string())?;
    if let Some(parent) = config.report_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let run_id = format!(
        "acceptance_compare_{}_{}",
        started_at_ms,
        std::process::id()
    );
    let comparison_root = config
        .probe_home
        .join("comparison_runs")
        .join(run_id.as_str());
    let qwen_probe_home = comparison_root.join("qwen_probe_home");
    let apple_probe_home = comparison_root.join("apple_fm_probe_home");
    let qwen_report_path = comparison_root.join("qwen_acceptance.json");
    let apple_report_path = comparison_root.join("apple_fm_acceptance.json");
    let qwen_case_names = case_names
        .iter()
        .copied()
        .filter(|case_name| {
            comparison_case_support(*case_name, config.qwen_profile.kind).is_supported()
        })
        .collect::<Vec<_>>();
    let apple_case_names = case_names
        .iter()
        .copied()
        .filter(|case_name| {
            comparison_case_support(*case_name, config.apple_fm_profile.kind).is_supported()
        })
        .collect::<Vec<_>>();

    let qwen_report = run_acceptance_harness_for_case_names(
        AcceptanceHarnessConfig {
            probe_home: qwen_probe_home,
            report_path: qwen_report_path.clone(),
            base_profile: config.qwen_profile.clone(),
        },
        qwen_case_names.as_slice(),
    )?;
    let apple_report = run_acceptance_harness_for_case_names(
        AcceptanceHarnessConfig {
            probe_home: apple_probe_home,
            report_path: apple_report_path.clone(),
            base_profile: config.apple_fm_profile.clone(),
        },
        apple_case_names.as_slice(),
    )?;
    let qwen_cases = qwen_report
        .results
        .iter()
        .map(|case| (case.case_name.clone(), case.clone()))
        .collect::<BTreeMap<_, _>>();
    let apple_cases = apple_report
        .results
        .iter()
        .map(|case| (case.case_name.clone(), case.clone()))
        .collect::<BTreeMap<_, _>>();

    let cases = case_names
        .iter()
        .map(|case_name| {
            let qwen = comparison_backend_case_report(
                case_name,
                &config.qwen_profile,
                qwen_cases.get(*case_name).cloned(),
            );
            let apple_fm = comparison_backend_case_report(
                case_name,
                &config.apple_fm_profile,
                apple_cases.get(*case_name).cloned(),
            );
            let status = comparison_case_status(&qwen, &apple_fm);
            AcceptanceComparisonCaseReport {
                case_name: (*case_name).to_string(),
                status,
                qwen,
                apple_fm,
            }
        })
        .collect::<Vec<_>>();
    let finished_at_ms = now_ms();
    let counts = build_comparison_counts(cases.as_slice());
    let git_state = current_probe_git_state();
    let report = AcceptanceComparisonReport {
        run: AcceptanceRunIdentity {
            run_id: run_id.clone(),
            schema_version: String::from(ACCEPTANCE_COMPARISON_REPORT_SCHEMA_VERSION),
            probe_version: String::from(env!("CARGO_PKG_VERSION")),
            git_commit_sha: git_state.git_commit_sha,
            git_dirty: git_state.git_dirty,
        },
        qwen_backend: AcceptanceBackendSummary {
            profile_name: qwen_report.backend.profile_name.clone(),
            base_url: qwen_report.backend.base_url.clone(),
            model: qwen_report.backend.model.clone(),
        },
        apple_fm_backend: AcceptanceBackendSummary {
            profile_name: apple_report.backend.profile_name.clone(),
            base_url: apple_report.backend.base_url.clone(),
            model: apple_report.backend.model.clone(),
        },
        harness: qwen_report.harness.clone(),
        started_at_ms,
        finished_at_ms,
        duration_ms: finished_at_ms.saturating_sub(started_at_ms),
        counts,
        qwen_report_path,
        apple_fm_report_path: apple_report_path,
        cases,
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

pub fn default_self_test_report_path(probe_home: &Path) -> PathBuf {
    probe_home
        .join("reports")
        .join(format!("probe_self_test_{}.json", now_ms()))
}

fn comparison_case_support(case_name: &str, backend_kind: BackendKind) -> AcceptanceCaseSupport {
    match (case_name, backend_kind) {
        (
            "read_file_answer"
            | "list_then_read"
            | "search_then_read"
            | "shell_then_summarize"
            | "patch_then_verify"
            | "approval_pause_or_refusal",
            BackendKind::OpenAiChatCompletions
            | BackendKind::OpenAiCodexSubscription
            | BackendKind::AppleFmBridge,
        ) => AcceptanceCaseSupport::supported(),
        (unknown_case, _) => AcceptanceCaseSupport::unsupported(format!(
            "case `{unknown_case}` is not part of the retained comparison set"
        )),
    }
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
                && attempt.policy_counts.auto_allowed_tool_calls >= 1
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
                && attempt.policy_counts.approved_tool_calls >= 1
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
            attempt.policy_counts.paused_tool_calls >= 1
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

fn run_case_shell_failure_then_summarize(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    probe_home: &Path,
) -> AcceptanceCaseReport {
    run_repeated_case(
        runtime,
        base_profile,
        probe_home,
        "shell_failure_then_summarize",
        |runtime, profile, workspace, title| {
            execute_coding_case(
                runtime,
                profile,
                workspace,
                title,
                "Use shell to run `false`, observe the non-zero exit code, then answer with exactly SHELL_FAILURE_OK.",
                coding_tool_loop(false, false, false, ToolDeniedAction::Refuse),
            )
        },
        |attempt, _workspace| {
            attempt.assistant_text.as_deref() == Some("SHELL_FAILURE_OK")
                && attempt.tool_names.iter().any(|name| name == "shell")
        },
    )
}

fn run_case_multi_turn_session_resume(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    probe_home: &Path,
) -> AcceptanceCaseReport {
    run_repeated_case(
        runtime,
        base_profile,
        probe_home,
        "multi_turn_session_resume",
        |runtime, profile, workspace, title| {
            let first = execute_coding_case(
                runtime,
                profile,
                workspace,
                title,
                "Reply with exactly SELF_TEST_TURN_ONE.",
                coding_tool_loop(false, false, false, ToolDeniedAction::Refuse),
            )?;
            runtime
                .continue_plain_text_session(PlainTextResumeRequest {
                    session_id: first.session.id,
                    profile: profile.clone(),
                    prompt: String::from("Reply with exactly SELF_TEST_TURN_TWO."),
                    tool_loop: None,
                })
                .map_err(AcceptanceExecutionError::Runtime)
        },
        |attempt, _workspace| {
            let Some(transcript_path) = attempt.transcript_path.as_ref() else {
                return false;
            };
            let events = load_transcript_events(transcript_path.as_path());
            attempt.assistant_text.as_deref() == Some("SELF_TEST_TURN_TWO.")
                && events.len() == 2
                && events[0]
                    .turn
                    .items
                    .iter()
                    .any(|item| item.text == "SELF_TEST_TURN_ONE.")
                && events[1]
                    .turn
                    .items
                    .iter()
                    .any(|item| item.text == "SELF_TEST_TURN_TWO.")
        },
    )
}

fn run_case_approval_pause_then_resume(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    probe_home: &Path,
) -> AcceptanceCaseReport {
    run_repeated_case(
        runtime,
        base_profile,
        probe_home,
        "approval_pause_then_resume",
        |runtime, profile, workspace, title| {
            let tool_loop = coding_tool_loop(false, false, false, ToolDeniedAction::Pause);
            match execute_coding_case(
                runtime,
                profile,
                workspace,
                title,
                "Use apply_patch to replace world with probe in hello.txt, then answer with exactly APPROVAL_RESUME_OK.",
                tool_loop.clone(),
            ) {
                Err(AcceptanceExecutionError::Runtime(RuntimeError::ToolApprovalPending {
                    session_id,
                    call_id,
                    ..
                })) => match runtime
                    .resolve_pending_tool_approval(ResolvePendingToolApprovalRequest {
                        session_id,
                        profile: profile.clone(),
                        tool_loop,
                        call_id,
                        resolution: ToolApprovalResolution::Approved,
                    })
                    .map_err(AcceptanceExecutionError::Runtime)?
                {
                    ResolvePendingToolApprovalOutcome::Resumed { outcome } => Ok(outcome),
                    ResolvePendingToolApprovalOutcome::StillPending { .. } => {
                        Err(AcceptanceExecutionError::Setup(String::from(
                            "approval remained pending after approval resolution",
                        )))
                    }
                },
                Err(other) => Err(other),
                Ok(_) => Err(AcceptanceExecutionError::Setup(String::from(
                    "expected approval pause before resume",
                ))),
            }
        },
        |attempt, workspace| {
            attempt.assistant_text.as_deref() == Some("APPROVAL_RESUME_OK")
                && attempt.policy_counts.paused_tool_calls >= 1
                && attempt.policy_counts.approved_tool_calls >= 1
                && fs::read_to_string(workspace.join("hello.txt"))
                    .map(|content| content == "hello probe\n")
                    .unwrap_or(false)
        },
    )
}

fn run_case_backend_failure_is_honest(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    probe_home: &Path,
) -> AcceptanceCaseReport {
    run_repeated_case(
        runtime,
        base_profile,
        probe_home,
        "backend_failure_is_honest",
        |runtime, profile, workspace, title| {
            let mut failing_profile = profile.clone();
            failing_profile.base_url =
                format!("{}/missing", profile.base_url.trim_end_matches('/'));
            runtime
                .exec_plain_text(PlainTextExecRequest {
                    profile: failing_profile,
                    prompt: String::from("Reply with exactly BACKEND_FAILURE_SHOULD_NOT_SUCCEED."),
                    title: Some(String::from(title)),
                    cwd: workspace.to_path_buf(),
                    system_prompt: None,
                    harness_profile: None,
                    tool_loop: None,
                })
                .map_err(AcceptanceExecutionError::Runtime)
        },
        |attempt, _workspace| {
            attempt.assistant_text.is_none()
                && matches!(
                    attempt.failure_category,
                    Some(AcceptanceFailureCategory::BackendFailure)
                )
                && attempt.error.is_some()
                && attempt.session_id.is_some()
        },
    )
}

fn run_named_case(
    case_name: &str,
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    probe_home: &Path,
) -> AcceptanceCaseReport {
    match case_name {
        "read_file_answer" => run_case_read_file_answer(runtime, base_profile, probe_home),
        "list_then_read" => run_case_list_then_read(runtime, base_profile, probe_home),
        "search_then_read" => run_case_search_then_read(runtime, base_profile, probe_home),
        "shell_then_summarize" => run_case_shell_then_summarize(runtime, base_profile, probe_home),
        "patch_then_verify" => run_case_patch_then_verify(runtime, base_profile, probe_home),
        "approval_pause_or_refusal" => {
            run_case_approval_pause_or_refusal(runtime, base_profile, probe_home)
        }
        "shell_failure_then_summarize" => {
            run_case_shell_failure_then_summarize(runtime, base_profile, probe_home)
        }
        "multi_turn_session_resume" => {
            run_case_multi_turn_session_resume(runtime, base_profile, probe_home)
        }
        "approval_pause_then_resume" => {
            run_case_approval_pause_then_resume(runtime, base_profile, probe_home)
        }
        "backend_failure_is_honest" => {
            run_case_backend_failure_is_honest(runtime, base_profile, probe_home)
        }
        other => panic!("unknown acceptance case `{other}`"),
    }
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
    F: FnMut(
        &ProbeRuntime,
        &BackendProfile,
        &Path,
        &str,
    ) -> Result<PlainTextExecOutcome, AcceptanceExecutionError>,
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
        if attempt.passed {
            attempt.failure_category = None;
        } else if attempt.failure_category.is_none() {
            attempt.failure_category = Some(AcceptanceFailureCategory::VerificationFailure);
        }
        attempts.push(attempt);
    }

    build_case_report(case_name, attempts)
}

fn run_matrix_scenario_attempt(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    probe_home: &Path,
    scenario: &str,
    harness_profile_name: &str,
    attempt_index: usize,
) -> AcceptanceAttemptReport {
    let title = format!("matrix-{scenario}-{}", attempt_index + 1);
    let workspace = prepare_acceptance_workspace(probe_home, scenario, attempt_index)
        .unwrap_or_else(|error| {
            panic!("failed to prepare matrix workspace for {scenario}: {error}")
        });
    let outcome = match scenario {
        "read_file_answer" => execute_coding_case_with_harness(
            runtime,
            base_profile,
            workspace.as_path(),
            title.as_str(),
            "Use read_file on README.md and answer with exactly READ_FILE_OK once you confirm the first line says Probe acceptance fixture.",
            harness_profile_name,
            coding_tool_loop(false, false, false, ToolDeniedAction::Refuse),
        ),
        "search_then_read" => execute_coding_case_with_harness(
            runtime,
            base_profile,
            workspace.as_path(),
            title.as_str(),
            "Use code_search for beta_function, then read the matching file, then answer with exactly SEARCH_READ_OK if beta_function exists.",
            harness_profile_name,
            coding_tool_loop(false, false, false, ToolDeniedAction::Refuse),
        ),
        "patch_then_verify" => execute_coding_case_with_harness(
            runtime,
            base_profile,
            workspace.as_path(),
            title.as_str(),
            "Use apply_patch to replace world with probe in hello.txt, then read hello.txt, then answer with exactly PATCH_OK.",
            harness_profile_name,
            coding_tool_loop(true, false, false, ToolDeniedAction::Refuse),
        ),
        "approval_pause_or_refusal" => execute_coding_case_with_harness(
            runtime,
            base_profile,
            workspace.as_path(),
            title.as_str(),
            "Use apply_patch to replace world with blocked in hello.txt.",
            harness_profile_name,
            coding_tool_loop(false, false, false, ToolDeniedAction::Pause),
        ),
        "streaming_reply_stability" => execute_eventful_case_with_harness(
            runtime,
            base_profile,
            workspace.as_path(),
            title.as_str(),
            "Reply with exactly STREAMING_OK.",
            harness_profile_name,
        ),
        "multi_turn_session_resume" => execute_multi_turn_case_with_harness(
            runtime,
            base_profile,
            workspace.as_path(),
            title.as_str(),
            harness_profile_name,
        ),
        other => Err(AcceptanceExecutionError::Setup(format!(
            "unknown matrix scenario `{other}`"
        ))),
    };
    let mut attempt = capture_attempt_report(runtime, title.as_str(), attempt_index, outcome);
    attempt.passed = validate_matrix_scenario(scenario, &attempt, workspace.as_path());
    if attempt.passed {
        attempt.failure_category = None;
    } else if attempt.failure_category.is_none() {
        attempt.failure_category = Some(AcceptanceFailureCategory::VerificationFailure);
    }
    attempt
}

fn validate_matrix_scenario(
    scenario: &str,
    attempt: &AcceptanceAttemptReport,
    workspace: &Path,
) -> bool {
    match scenario {
        "read_file_answer" => {
            attempt.assistant_text.as_deref() == Some("READ_FILE_OK")
                && attempt.tool_names.iter().any(|name| name == "read_file")
        }
        "search_then_read" => {
            attempt.assistant_text.as_deref() == Some("SEARCH_READ_OK")
                && attempt.tool_names.iter().any(|name| name == "code_search")
                && attempt.tool_names.iter().any(|name| name == "read_file")
        }
        "patch_then_verify" => {
            attempt.assistant_text.as_deref() == Some("PATCH_OK")
                && attempt.tool_names.iter().any(|name| name == "apply_patch")
                && fs::read_to_string(workspace.join("hello.txt"))
                    .map(|content| content == "hello probe\n")
                    .unwrap_or(false)
        }
        "approval_pause_or_refusal" => {
            attempt.policy_counts.paused_tool_calls >= 1
                && attempt.tool_names.iter().any(|name| name == "apply_patch")
        }
        "streaming_reply_stability" => attempt.assistant_text.as_deref() == Some("STREAMING_OK."),
        "multi_turn_session_resume" => {
            let Some(transcript_path) = attempt.transcript_path.as_ref() else {
                return false;
            };
            let events = load_transcript_events(transcript_path.as_path());
            attempt.assistant_text.as_deref() == Some("SELF_TEST_TURN_TWO.") && events.len() == 2
        }
        _ => false,
    }
}

fn build_matrix_cell_report(
    profile_name: &str,
    model: &str,
    harness_profile: &str,
    scenario: &str,
    repetitions: Vec<AcceptanceAttemptReport>,
) -> AcceptanceMatrixCellReport {
    let worst_repetition_index = repetitions
        .iter()
        .position(|attempt| !attempt.passed)
        .unwrap_or(0);
    let worst = repetitions
        .get(worst_repetition_index)
        .cloned()
        .expect("matrix cell should have at least one repetition");
    AcceptanceMatrixCellReport {
        profile_name: String::from(profile_name),
        model: String::from(model),
        harness_profile: String::from(harness_profile),
        scenario: String::from(scenario),
        passed: repetitions.iter().all(|attempt| attempt.passed),
        worst_repetition_index,
        worst_failure_category: worst.failure_category.clone(),
        worst_session_id: worst.session_id.clone(),
        worst_transcript_path: worst.transcript_path.clone(),
        repetitions,
    }
}

fn execute_coding_case(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    workspace: &Path,
    title: &str,
    prompt: &str,
    tool_loop: ToolLoopConfig,
) -> Result<PlainTextExecOutcome, AcceptanceExecutionError> {
    execute_coding_case_with_harness(
        runtime,
        base_profile,
        workspace,
        title,
        prompt,
        ACCEPTANCE_HARNESS_PROFILE_NAME,
        tool_loop,
    )
}

fn execute_coding_case_with_harness(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    workspace: &Path,
    title: &str,
    prompt: &str,
    harness_profile_name: &str,
    tool_loop: ToolLoopConfig,
) -> Result<PlainTextExecOutcome, AcceptanceExecutionError> {
    let resolved = resolve_harness_profile(
        Some("coding_bootstrap"),
        Some(harness_profile_name),
        workspace,
        None,
    )
    .map_err(AcceptanceExecutionError::Setup)?
    .ok_or_else(|| {
        AcceptanceExecutionError::Setup(String::from("missing coding bootstrap harness profile"))
    })?;

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
        .map_err(AcceptanceExecutionError::Runtime)
}

fn execute_eventful_case_with_harness(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    workspace: &Path,
    title: &str,
    prompt: &str,
    harness_profile_name: &str,
) -> Result<PlainTextExecOutcome, AcceptanceExecutionError> {
    let resolved = resolve_harness_profile(
        Some("coding_bootstrap"),
        Some(harness_profile_name),
        workspace,
        None,
    )
    .map_err(AcceptanceExecutionError::Setup)?
    .ok_or_else(|| {
        AcceptanceExecutionError::Setup(String::from("missing coding bootstrap harness profile"))
    })?;
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let outcome = runtime
        .exec_plain_text_with_events(
            PlainTextExecRequest {
                profile: base_profile.clone(),
                prompt: String::from(prompt),
                title: Some(String::from(title)),
                cwd: workspace.to_path_buf(),
                system_prompt: Some(resolved.system_prompt),
                harness_profile: Some(resolved.profile),
                tool_loop: None,
            },
            Arc::new(move |event| {
                sink_events
                    .lock()
                    .expect("matrix event collection lock")
                    .push(event);
            }),
        )
        .map_err(AcceptanceExecutionError::Runtime)?;
    let captured = events.lock().expect("matrix event collection lock");
    if !captured.iter().any(|event| {
        matches!(
            event,
            probe_core::runtime::RuntimeEvent::ModelRequestStarted { .. }
        )
    }) || !captured.iter().any(|event| {
        matches!(
            event,
            probe_core::runtime::RuntimeEvent::AssistantTurnCommitted { .. }
        )
    }) {
        return Err(AcceptanceExecutionError::Setup(String::from(
            "streaming stability scenario did not emit the required runtime lifecycle events",
        )));
    }
    Ok(outcome)
}

fn execute_multi_turn_case_with_harness(
    runtime: &ProbeRuntime,
    base_profile: &BackendProfile,
    workspace: &Path,
    title: &str,
    harness_profile_name: &str,
) -> Result<PlainTextExecOutcome, AcceptanceExecutionError> {
    let first = execute_coding_case_with_harness(
        runtime,
        base_profile,
        workspace,
        title,
        "Reply with exactly SELF_TEST_TURN_ONE.",
        harness_profile_name,
        coding_tool_loop(false, false, false, ToolDeniedAction::Refuse),
    )?;
    runtime
        .continue_plain_text_session(PlainTextResumeRequest {
            session_id: first.session.id,
            profile: base_profile.clone(),
            prompt: String::from("Reply with exactly SELF_TEST_TURN_TWO."),
            tool_loop: None,
        })
        .map_err(AcceptanceExecutionError::Runtime)
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
    outcome: Result<PlainTextExecOutcome, AcceptanceExecutionError>,
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
    let failure_category = classify_attempt(transcript_summary.as_ref(), outcome.as_ref());

    let (assistant_text, executed_tool_calls, error) = match outcome {
        Ok(outcome) => (
            Some(outcome.assistant_text),
            outcome.executed_tool_calls,
            None,
        ),
        Err(error) => {
            let executed = transcript_summary
                .as_ref()
                .map(|summary| {
                    summary.policy_counts.auto_allowed_tool_calls
                        + summary.policy_counts.approved_tool_calls
                })
                .unwrap_or(0);
            (None, executed, Some(error.to_string()))
        }
    };

    AcceptanceAttemptReport {
        attempt_index,
        passed: false,
        failure_category,
        session_id: session_metadata
            .as_ref()
            .map(|metadata| metadata.id.as_str().to_string()),
        transcript_path: session_metadata
            .as_ref()
            .map(|metadata| metadata.transcript_path.clone()),
        assistant_text,
        executed_tool_calls,
        tool_names: transcript_summary
            .as_ref()
            .map(|summary| summary.tool_names.clone())
            .unwrap_or_default(),
        policy_counts: transcript_summary
            .as_ref()
            .map(|summary| summary.policy_counts.clone())
            .unwrap_or_default(),
        observability: transcript_summary
            .as_ref()
            .and_then(|summary| summary.final_observability.clone()),
        backend_receipt: transcript_summary
            .as_ref()
            .and_then(|summary| summary.final_backend_receipt.clone()),
        error,
    }
}

fn build_case_report(
    case_name: &str,
    attempts: Vec<AcceptanceAttemptReport>,
) -> AcceptanceCaseReport {
    let passed = attempts.iter().all(|attempt| attempt.passed);
    let median_elapsed_ms = median(
        attempts
            .iter()
            .filter_map(|attempt| attempt.observability.as_ref())
            .map(|observability| observability.wallclock_ms)
            .collect(),
    );
    let passed_attempts = attempts.iter().filter(|attempt| attempt.passed).count();
    let failed_attempts = attempts.len().saturating_sub(passed_attempts);
    let summary_attempt = attempts.last().cloned();

    AcceptanceCaseReport {
        case_name: String::from(case_name),
        case_index: 0,
        passed,
        repeat_runs: attempts.len(),
        passed_attempts,
        failed_attempts,
        median_elapsed_ms,
        latest_session_id: summary_attempt
            .as_ref()
            .and_then(|attempt| attempt.session_id.clone()),
        latest_transcript_path: summary_attempt
            .as_ref()
            .and_then(|attempt| attempt.transcript_path.clone()),
        latest_assistant_text: summary_attempt
            .as_ref()
            .and_then(|attempt| attempt.assistant_text.clone()),
        latest_executed_tool_calls: summary_attempt
            .as_ref()
            .map(|attempt| attempt.executed_tool_calls)
            .unwrap_or(0),
        latest_tool_names: summary_attempt
            .as_ref()
            .map(|attempt| attempt.tool_names.clone())
            .unwrap_or_default(),
        latest_policy_counts: summary_attempt
            .as_ref()
            .map(|attempt| attempt.policy_counts.clone())
            .unwrap_or_default(),
        latest_observability: summary_attempt
            .as_ref()
            .and_then(|attempt| attempt.observability.clone()),
        latest_backend_receipt: summary_attempt
            .as_ref()
            .and_then(|attempt| attempt.backend_receipt.clone()),
        failure_category: attempts
            .iter()
            .find(|attempt| !attempt.passed)
            .and_then(|attempt| attempt.failure_category.clone()),
        error: attempts.iter().find_map(|attempt| attempt.error.clone()),
        attempts,
    }
}

fn build_run_counts(results: &[AcceptanceCaseReport]) -> AcceptanceRunCounts {
    let total_cases = results.len();
    let passed_cases = results.iter().filter(|result| result.passed).count();
    let failed_cases = total_cases.saturating_sub(passed_cases);
    let total_attempts: usize = results.iter().map(|result| result.attempts.len()).sum();
    let passed_attempts = results
        .iter()
        .flat_map(|result| result.attempts.iter())
        .filter(|attempt| attempt.passed)
        .count();
    let failed_attempts = total_attempts.saturating_sub(passed_attempts);

    AcceptanceRunCounts {
        total_cases,
        passed_cases,
        failed_cases,
        total_attempts,
        passed_attempts,
        failed_attempts,
    }
}

fn comparison_backend_case_report(
    case_name: &str,
    profile: &BackendProfile,
    case: Option<AcceptanceCaseReport>,
) -> AcceptanceComparisonBackendCaseReport {
    let support = comparison_case_support(case_name, profile.kind);
    let status = if !support.is_supported() {
        AcceptanceComparisonBackendCaseStatus::Unsupported
    } else if case.as_ref().is_some_and(|case| case.passed) {
        AcceptanceComparisonBackendCaseStatus::Passed
    } else {
        AcceptanceComparisonBackendCaseStatus::Failed
    };
    AcceptanceComparisonBackendCaseReport {
        backend_profile_name: profile.name.clone(),
        status,
        unsupported_reason: support.unsupported_reason,
        case,
    }
}

fn comparison_case_status(
    qwen: &AcceptanceComparisonBackendCaseReport,
    apple_fm: &AcceptanceComparisonBackendCaseReport,
) -> AcceptanceComparisonStatus {
    if matches!(
        qwen.status,
        AcceptanceComparisonBackendCaseStatus::Unsupported
    ) || matches!(
        apple_fm.status,
        AcceptanceComparisonBackendCaseStatus::Unsupported
    ) {
        AcceptanceComparisonStatus::Unsupported
    } else if matches!(qwen.status, AcceptanceComparisonBackendCaseStatus::Passed)
        && matches!(
            apple_fm.status,
            AcceptanceComparisonBackendCaseStatus::Passed
        )
    {
        AcceptanceComparisonStatus::ComparablePass
    } else {
        AcceptanceComparisonStatus::ComparableFail
    }
}

fn build_comparison_counts(cases: &[AcceptanceComparisonCaseReport]) -> AcceptanceComparisonCounts {
    let total_cases = cases.len();
    let comparable_cases = cases
        .iter()
        .filter(|case| !matches!(case.status, AcceptanceComparisonStatus::Unsupported))
        .count();
    let comparable_passed_cases = cases
        .iter()
        .filter(|case| matches!(case.status, AcceptanceComparisonStatus::ComparablePass))
        .count();
    let comparable_failed_cases = cases
        .iter()
        .filter(|case| matches!(case.status, AcceptanceComparisonStatus::ComparableFail))
        .count();
    let unsupported_backend_results = cases
        .iter()
        .map(|case| {
            usize::from(matches!(
                case.qwen.status,
                AcceptanceComparisonBackendCaseStatus::Unsupported
            )) + usize::from(matches!(
                case.apple_fm.status,
                AcceptanceComparisonBackendCaseStatus::Unsupported
            ))
        })
        .sum();

    AcceptanceComparisonCounts {
        total_cases,
        comparable_cases,
        comparable_passed_cases,
        comparable_failed_cases,
        unsupported_backend_results,
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

fn load_transcript_events(path: &Path) -> Vec<TranscriptEvent> {
    fs::read_to_string(path)
        .expect("read transcript")
        .lines()
        .map(|line| serde_json::from_str(line).expect("decode transcript event"))
        .collect()
}

#[derive(Clone, Debug, Default)]
struct TranscriptSummary {
    tool_names: Vec<String>,
    policy_counts: AcceptancePolicyCounts,
    final_observability: Option<AcceptanceObservabilitySummary>,
    final_backend_receipt: Option<AcceptanceBackendReceiptSummary>,
}

fn summarize_transcript(events: &[TranscriptEvent]) -> TranscriptSummary {
    let mut summary = TranscriptSummary::default();
    for event in events {
        if let Some(observability) = event.turn.observability.clone() {
            summary.final_observability = Some(observability_summary(&observability));
        }
        if let Some(receipt) = event.turn.backend_receipt.as_ref() {
            summary.final_backend_receipt = Some(backend_receipt_summary(receipt));
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
                    ToolPolicyDecision::AutoAllow => {
                        summary.policy_counts.auto_allowed_tool_calls += 1;
                    }
                    ToolPolicyDecision::Approved => {
                        summary.policy_counts.approved_tool_calls += 1;
                    }
                    ToolPolicyDecision::Refused => {
                        summary.policy_counts.refused_tool_calls += 1;
                    }
                    ToolPolicyDecision::Paused => {
                        summary.policy_counts.paused_tool_calls += 1;
                    }
                }
            }
        }
    }
    summary
}

fn classify_attempt(
    transcript_summary: Option<&TranscriptSummary>,
    outcome: Result<&PlainTextExecOutcome, &AcceptanceExecutionError>,
) -> Option<AcceptanceFailureCategory> {
    match outcome {
        Ok(_) => None,
        Err(error) => {
            if transcript_summary
                .map(|summary| summary.policy_counts.paused_tool_calls > 0)
                .unwrap_or(false)
                || matches!(
                    error,
                    AcceptanceExecutionError::Runtime(RuntimeError::ToolApprovalPending { .. })
                )
            {
                return Some(AcceptanceFailureCategory::PolicyPaused);
            }
            if transcript_summary
                .map(|summary| summary.policy_counts.refused_tool_calls > 0)
                .unwrap_or(false)
            {
                return Some(AcceptanceFailureCategory::PolicyRefusal);
            }
            match error {
                AcceptanceExecutionError::Setup(_) => {
                    Some(AcceptanceFailureCategory::ConfigurationFailure)
                }
                AcceptanceExecutionError::Runtime(RuntimeError::ProviderRequest { .. })
                | AcceptanceExecutionError::Runtime(RuntimeError::MissingAssistantMessage {
                    ..
                })
                | AcceptanceExecutionError::Runtime(RuntimeError::UnsupportedBackendFeature {
                    ..
                }) => Some(AcceptanceFailureCategory::BackendFailure),
                AcceptanceExecutionError::Runtime(RuntimeError::MaxToolRoundTrips { .. })
                | AcceptanceExecutionError::Runtime(RuntimeError::MalformedTranscript(_))
                | AcceptanceExecutionError::Runtime(RuntimeError::PendingToolApprovalNotFound {
                    ..
                })
                | AcceptanceExecutionError::Runtime(
                    RuntimeError::PendingToolApprovalAlreadyResolved { .. },
                ) => Some(AcceptanceFailureCategory::ToolExecutionFailure),
                AcceptanceExecutionError::Runtime(RuntimeError::ProbeHomeUnavailable)
                | AcceptanceExecutionError::Runtime(RuntimeError::CurrentDir(_))
                | AcceptanceExecutionError::Runtime(RuntimeError::SessionStore(_)) => {
                    Some(AcceptanceFailureCategory::ConfigurationFailure)
                }
                AcceptanceExecutionError::Runtime(RuntimeError::ToolApprovalPending { .. }) => {
                    Some(AcceptanceFailureCategory::PolicyPaused)
                }
            }
        }
    }
}

fn median(mut values: Vec<u64>) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[values.len() / 2])
}

#[derive(Clone, Debug)]
struct ProbeGitState {
    git_commit_sha: Option<String>,
    git_dirty: Option<bool>,
}

fn current_probe_git_state() -> ProbeGitState {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf);

    let Some(repo_root) = repo_root else {
        return ProbeGitState {
            git_commit_sha: None,
            git_dirty: None,
        };
    };

    let git_commit_sha = run_git(repo_root.as_path(), ["rev-parse", "HEAD"]);
    let git_dirty =
        run_git(repo_root.as_path(), ["status", "--porcelain"]).map(|output| !output.is_empty());

    ProbeGitState {
        git_commit_sha,
        git_dirty,
    }
}

fn run_git<const N: usize>(repo_root: &Path, args: [&str; N]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn observability_summary(observability: &TurnObservability) -> AcceptanceObservabilitySummary {
    AcceptanceObservabilitySummary {
        wallclock_ms: observability.wallclock_ms,
        model_output_ms: observability.model_output_ms,
        prompt_tokens: observability.prompt_tokens,
        prompt_tokens_detail: observability.prompt_tokens_detail.clone(),
        completion_tokens: observability.completion_tokens,
        completion_tokens_detail: observability.completion_tokens_detail.clone(),
        total_tokens: observability.total_tokens,
        total_tokens_detail: observability.total_tokens_detail.clone(),
        completion_tokens_per_second_x1000: observability.completion_tokens_per_second_x1000,
        cache_signal: observability.cache_signal,
    }
}

fn backend_receipt_summary(receipt: &BackendTurnReceipt) -> AcceptanceBackendReceiptSummary {
    AcceptanceBackendReceiptSummary {
        failure_family: receipt
            .failure
            .as_ref()
            .map(|failure| failure.family.clone()),
        failure_code: receipt
            .failure
            .as_ref()
            .and_then(|failure| failure.code.clone()),
        failure_message: receipt
            .failure
            .as_ref()
            .map(|failure| failure.message.clone()),
        failure_retryable: receipt
            .failure
            .as_ref()
            .and_then(|failure| failure.retryable),
        failure_reason: receipt
            .failure
            .as_ref()
            .and_then(|failure| failure.failure_reason.clone()),
        recovery_suggestion: receipt
            .failure
            .as_ref()
            .and_then(|failure| failure.recovery_suggestion.clone()),
        refusal_explanation: receipt
            .failure
            .as_ref()
            .and_then(|failure| failure.refusal_explanation.clone()),
        tool_name: receipt
            .failure
            .as_ref()
            .and_then(|failure| failure.tool_name.clone()),
        availability_ready: receipt
            .availability
            .as_ref()
            .map(|availability| availability.ready),
        availability_reason_code: receipt
            .availability
            .as_ref()
            .and_then(|availability| availability.reason_code.clone()),
        availability_message: receipt
            .availability
            .as_ref()
            .and_then(|availability| availability.message.clone()),
        availability_platform: receipt
            .availability
            .as_ref()
            .and_then(|availability| availability.platform.clone()),
        transcript_format: receipt
            .transcript
            .as_ref()
            .map(|transcript| transcript.format.clone()),
        transcript_payload_bytes: receipt
            .transcript
            .as_ref()
            .map(|transcript| transcript.payload.len()),
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
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use probe_core::backend_profiles::{psionic_apple_fm_bridge, psionic_qwen35_2b_q8_registry};
    use probe_test_support::{
        FakeAppleFmServer, FakeHttpRequest, FakeHttpResponse, FakeOpenAiServer,
    };
    use serde_json::json;

    use super::{
        AcceptanceComparisonBackendCaseStatus, AcceptanceComparisonConfig,
        AcceptanceComparisonStatus, AcceptanceHarnessConfig, AcceptanceMatrixConfig,
        default_comparison_report_path, default_matrix_report_path, default_report_path,
        default_self_test_report_path, run_acceptance_comparison_for_case_names,
        run_acceptance_harness, run_acceptance_harness_for_case_names, run_acceptance_matrix,
    };

    #[derive(Default)]
    struct AppleComparisonBridgeState {
        callback_url: String,
        session_token: String,
        next_session_index: usize,
    }

    struct ToolCallbackResponse {
        status_code: u16,
        body: String,
    }

    fn record_apple_comparison_session(
        state: &Arc<Mutex<AppleComparisonBridgeState>>,
        request: &FakeHttpRequest,
    ) -> FakeHttpResponse {
        let request_json: serde_json::Value =
            serde_json::from_str(request.body.as_str()).expect("session create json");
        let callback_url = request_json["tool_callback"]["url"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let session_token = request_json["tool_callback"]["session_token"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let session_id = {
            let mut guard = state.lock().expect("apple comparison state lock");
            guard.callback_url = callback_url;
            guard.session_token = session_token;
            guard.next_session_index += 1;
            format!("sess-apple-compare-{}", guard.next_session_index)
        };
        FakeHttpResponse::json_ok(json!({
            "session": {
                "id": session_id,
                "instructions": request_json["instructions"],
                "model": {
                    "id": "apple-foundation-model",
                    "use_case": "general",
                    "guardrails": "default"
                },
                "tools": request_json["tools"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|tool| json!({
                        "name": tool["name"],
                        "description": tool["description"]
                    }))
                    .collect::<Vec<_>>(),
                "is_responding": false,
                "transcript_json": serde_json::to_string(&request_json["transcript"])
                    .unwrap_or_else(|_| String::from("{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"))
            }
        }))
    }

    fn invoke_tool_callback(
        state: &Arc<Mutex<AppleComparisonBridgeState>>,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> ToolCallbackResponse {
        let (callback_url, session_token) = {
            let guard = state.lock().expect("apple comparison state lock");
            (guard.callback_url.clone(), guard.session_token.clone())
        };
        let url = callback_url
            .strip_prefix("http://")
            .expect("callback url should be http");
        let (authority, path) = url
            .split_once('/')
            .expect("callback url should include path");
        let body = json!({
            "session_token": session_token,
            "tool_name": tool_name,
            "arguments": {
                "content": arguments,
                "is_complete": true
            }
        })
        .to_string();
        let mut stream = TcpStream::connect(authority).expect("connect tool callback");
        let request = format!(
            "POST /{} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            path,
            authority,
            body.len(),
            body
        );
        stream
            .write_all(request.as_bytes())
            .expect("write tool callback request");
        stream.flush().expect("flush tool callback request");
        stream
            .shutdown(Shutdown::Write)
            .expect("close tool callback request writer");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read tool callback response");
        let (head, body) = response
            .split_once("\r\n\r\n")
            .expect("tool callback response should include body");
        let status_code = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u16>().ok())
            .expect("tool callback status code");
        ToolCallbackResponse {
            status_code,
            body: body.to_string(),
        }
    }

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

        assert!(
            report.overall_pass,
            "failed self-test cases: {:?}",
            report
                .results
                .iter()
                .filter(|result| !result.passed)
                .map(|result| (
                    result.case_name.clone(),
                    result.failure_category.clone(),
                    result.error.clone(),
                ))
                .collect::<Vec<_>>()
        );
        assert_eq!(report.results.len(), 6);
        assert_eq!(report.harness.repeat_runs_per_case, 2);
        assert_eq!(report.counts.total_cases, 6);
        assert_eq!(report.counts.passed_cases, 6);
        assert_eq!(report.counts.failed_cases, 0);
        assert_eq!(report.backend.profile_name, "psionic-qwen35-2b-q8-registry");
        assert_eq!(report.harness.tool_set, "coding_bootstrap");
        assert_eq!(report.run.schema_version, "v3");
        assert_eq!(report.results[5].failure_category, None);
        assert_eq!(report.results[5].latest_policy_counts.paused_tool_calls, 1);
        assert!(report.results[0].latest_transcript_path.is_some());
        assert!(
            report.results[0].attempts[0]
                .observability
                .as_ref()
                .is_some()
        );
        assert!(report_path.exists());

        handle.join().expect("server thread should exit cleanly");
    }

    #[test]
    fn self_test_extension_cases_write_report_against_mock_server() {
        let responses = retained_self_test_extension_responses();
        let server = FakeOpenAiServer::from_responses(responses);
        let temp = tempfile::tempdir().expect("temp dir");
        let probe_home = temp.path().join(".probe");
        let report_path = default_self_test_report_path(probe_home.as_path());
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = server.base_url().to_string();

        let report = run_acceptance_harness_for_case_names(
            AcceptanceHarnessConfig {
                probe_home,
                report_path: report_path.clone(),
                base_profile: profile,
            },
            &[
                "shell_failure_then_summarize",
                "multi_turn_session_resume",
                "approval_pause_then_resume",
                "backend_failure_is_honest",
            ],
        )
        .expect("self-test extension cases should succeed");

        assert!(
            report.overall_pass,
            "failed self-test cases: {:?}",
            report
                .results
                .iter()
                .filter(|result| !result.passed)
                .map(|result| (
                    result.case_name.clone(),
                    result.failure_category.clone(),
                    result.error.clone(),
                ))
                .collect::<Vec<_>>()
        );
        assert_eq!(report.results.len(), 4);
        assert_eq!(report.counts.total_cases, 4);
        assert_eq!(report.counts.passed_cases, 4);
        assert_eq!(
            report.results[0].latest_tool_names,
            vec![String::from("shell")]
        );
        assert_eq!(
            report.results[1].latest_assistant_text.as_deref(),
            Some("SELF_TEST_TURN_TWO.")
        );
        assert!(report.results[2].latest_policy_counts.paused_tool_calls >= 1);
        assert!(report.results[2].latest_policy_counts.approved_tool_calls >= 1);
        assert_eq!(report.results[3].latest_assistant_text, None);
        assert!(
            report.results[3]
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("missing route")
        );
        assert!(report_path.exists());
    }

    fn retained_self_test_extension_responses() -> Vec<FakeHttpResponse> {
        let mut responses = Vec::new();

        for attempt in 0..2 {
            responses.push(FakeHttpResponse::json_ok(serde_json::json!({
                "id": format!("shell_failure_tool_{}", attempt + 1),
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": format!("call_shell_failure_{}", attempt + 1),
                            "type": "function",
                            "function": {
                                "name": "shell",
                                "arguments": "{\"command\":\"false\",\"timeout_secs\":2}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })));
            responses.push(FakeHttpResponse::json_ok(serde_json::json!({
                "id": format!("shell_failure_final_{}", attempt + 1),
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "SHELL_FAILURE_OK"},
                    "finish_reason": "stop"
                }]
            })));
        }

        for attempt in 0..2 {
            responses.push(FakeHttpResponse::json_ok(serde_json::json!({
                "id": format!("multi_turn_first_{}", attempt + 1),
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "SELF_TEST_TURN_ONE."},
                    "finish_reason": "stop"
                }]
            })));
            responses.push(FakeHttpResponse::json_ok(serde_json::json!({
                "id": format!("multi_turn_second_{}", attempt + 1),
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "SELF_TEST_TURN_TWO."},
                    "finish_reason": "stop"
                }]
            })));
        }

        for attempt in 0..2 {
            responses.push(FakeHttpResponse::json_ok(serde_json::json!({
                "id": format!("approval_resume_pause_{}", attempt + 1),
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": format!("call_resume_patch_{}", attempt + 1),
                            "type": "function",
                            "function": {
                                "name": "apply_patch",
                                "arguments": "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })));
            responses.push(FakeHttpResponse::json_ok(serde_json::json!({
                "id": format!("approval_resume_final_{}", attempt + 1),
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "APPROVAL_RESUME_OK"},
                    "finish_reason": "stop"
                }]
            })));
        }

        for _attempt in 0..2 {
            responses.push(FakeHttpResponse::json_status(
                404,
                serde_json::json!({"error": "missing route"}),
            ));
        }

        responses
    }

    #[test]
    fn matrix_runner_keeps_the_worst_repetition_for_each_cell() {
        let server = FakeOpenAiServer::from_responses(vec![
            FakeHttpResponse::json_ok(serde_json::json!({
                "id": "matrix_stream_ok_1",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "STREAMING_OK."},
                    "finish_reason": "stop"
                }]
            })),
            FakeHttpResponse::json_status(404, serde_json::json!({"error": "missing route"})),
            FakeHttpResponse::json_ok(serde_json::json!({
                "id": "matrix_stream_ok_3",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "STREAMING_OK."},
                    "finish_reason": "stop"
                }]
            })),
        ]);
        let temp = tempfile::tempdir().expect("temp dir");
        let probe_home = temp.path().join(".probe");
        let report_path = default_matrix_report_path(probe_home.as_path());
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = server.base_url().to_string();

        let report = run_acceptance_matrix(AcceptanceMatrixConfig {
            probe_home,
            report_path: report_path.clone(),
            profiles: vec![profile],
            models: Vec::new(),
            harness_profiles: vec![String::from("coding_bootstrap_default")],
            scenarios: vec![String::from("streaming_reply_stability")],
            repetitions: 3,
        })
        .expect("matrix runner should succeed");

        assert_eq!(report.counts.total_cells, 1);
        assert_eq!(report.counts.failed_cells, 1);
        assert_eq!(report.counts.failed_repetitions, 1);
        assert!(!report.cells[0].passed);
        assert_eq!(report.cells[0].worst_repetition_index, 1);
        assert_eq!(
            report.cells[0].worst_failure_category,
            Some(super::AcceptanceFailureCategory::BackendFailure)
        );
        assert!(report_path.exists());
    }

    #[test]
    fn acceptance_comparison_writes_report_against_mock_qwen_and_apple_backends() {
        let qwen_server = FakeOpenAiServer::from_json_responses(vec![
            json!({
                "id": "qwen_read_file_tool_1",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": "call_readme_1", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"README.md\"}"}}]}, "finish_reason": "tool_calls"}]
            }),
            json!({
                "id": "qwen_read_file_final_1",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "READ_FILE_OK"}, "finish_reason": "stop"}]
            }),
            json!({
                "id": "qwen_read_file_tool_2",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [{"id": "call_readme_2", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"README.md\"}"}}]}, "finish_reason": "tool_calls"}]
            }),
            json!({
                "id": "qwen_read_file_final_2",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "READ_FILE_OK"}, "finish_reason": "stop"}]
            }),
        ]);
        let apple_state = Arc::new(Mutex::new(AppleComparisonBridgeState::default()));
        let captured_state = Arc::clone(&apple_state);
        let apple_server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => {
                    record_apple_comparison_session(&captured_state, &request)
                }
                ("POST", path) if path.starts_with("/v1/sessions/sess-apple-compare-") => {
                    let callback_response = invoke_tool_callback(
                        &captured_state,
                        "read_file",
                        json!({
                            "path": "README.md",
                            "start_line": 1,
                            "max_lines": 10
                        }),
                    );
                    assert_eq!(callback_response.status_code, 200);
                    let callback_json: serde_json::Value =
                        serde_json::from_str(callback_response.body.as_str())
                            .expect("callback json");
                    assert!(
                        callback_json["output"]
                            .as_str()
                            .unwrap_or_default()
                            .contains("Probe acceptance fixture")
                    );
                    FakeHttpResponse::json_ok(json!({
                        "session": {
                            "id": path
                                .trim_start_matches("/v1/sessions/")
                                .trim_end_matches("/responses"),
                            "instructions": "coding_bootstrap acceptance harness profile v1",
                            "model": {
                                "id": "apple-foundation-model",
                                "use_case": "general",
                                "guardrails": "default"
                            },
                            "tools": [{"name": "read_file"}],
                            "is_responding": false,
                            "transcript_json": "{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"
                        },
                        "model": "apple-foundation-model",
                        "output": "READ_FILE_OK",
                        "usage": {
                            "total_tokens_detail": {"value": 11, "truth": "estimated"}
                        }
                    }))
                }
                ("DELETE", path) if path.starts_with("/v1/sessions/sess-apple-compare-") => {
                    FakeHttpResponse::json_ok(json!({}))
                }
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let temp = tempfile::tempdir().expect("temp dir");
        let probe_home = temp.path().join(".probe");
        let report_path = default_comparison_report_path(probe_home.as_path());
        let mut qwen_profile = psionic_qwen35_2b_q8_registry();
        qwen_profile.base_url = qwen_server.base_url().to_string();
        let mut apple_profile = psionic_apple_fm_bridge();
        apple_profile.base_url = apple_server.base_url().to_string();

        let report = run_acceptance_comparison_for_case_names(
            AcceptanceComparisonConfig {
                probe_home,
                report_path: report_path.clone(),
                qwen_profile,
                apple_fm_profile: apple_profile,
            },
            &["read_file_answer"],
        )
        .expect("comparison should succeed");

        assert_eq!(report.run.schema_version, "v1");
        assert_eq!(report.cases.len(), 1);
        assert_eq!(report.counts.comparable_cases, 1);
        assert_eq!(report.counts.comparable_passed_cases, 1);
        assert_eq!(report.counts.comparable_failed_cases, 0);
        assert_eq!(
            report.cases[0].status,
            AcceptanceComparisonStatus::ComparablePass
        );
        assert_eq!(
            report.cases[0].qwen.status,
            AcceptanceComparisonBackendCaseStatus::Passed
        );
        assert_eq!(
            report.cases[0].apple_fm.status,
            AcceptanceComparisonBackendCaseStatus::Passed
        );
        assert!(report.qwen_report_path.exists());
        assert!(report.apple_fm_report_path.exists());
        assert!(report_path.exists());
        let qwen_requests = qwen_server.finish();
        assert_eq!(qwen_requests.len(), 4);
        let apple_requests = apple_server.finish();
        assert_eq!(apple_requests.len(), 6);
    }
}
