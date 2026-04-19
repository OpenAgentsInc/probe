use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use forge_eval::fetch_issue_thread;
use forge_policy::ExecutionPolicyBundle;
use forge_rlm_core::{CountMismatch, IssueThreadCorpus};
use forge_runtime_protocol::{
    CorpusKind, CorpusLocator, ExecutionBudget, OutputSchema, RuntimeAssignment,
};
use probe_protocol::backend::BackendProfile;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paper_rlm::{PaperRlmCorpus, PaperRlmExecutionRequest, execute_paper_rlm_request};
use crate::provider::{
    OpenAiRequestContext, PlainTextMessage, ProviderError, complete_plain_text_with_context,
};

pub const HEURISTIC_RLM_TRIGGER_ID: &str = "heuristic_rlm_trigger_v1";
pub const DIRECT_ISSUE_THREAD_STRATEGY_ID: &str = "issue_thread_direct_v1";
pub const PAPER_RLM_ISSUE_THREAD_STRATEGY_ID: &str = "paper_rlm_issue_thread_v1";
pub const COMPACT_LONG_CONTEXT_STRATEGY_ID: &str = "compact_long_context_v1";

const DEFAULT_DIRECT_SYSTEM_PROMPT: &str = "You analyze GitHub issue threads. Return only strict \
JSON with keys `answer` and `evidence_item_refs`. `evidence_item_refs` must be an array of \
corpus item refs like `issue-body` or `comment-12`. Do not include markdown fences.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LongContextStrategy {
    Direct,
    Compact,
    Rlm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueThreadStrategyMode {
    Auto,
    Direct,
    Rlm,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubIssueThreadHandle {
    pub repo_owner: String,
    pub repo_name: String,
    pub issue_number: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_url: Option<String>,
}

impl GithubIssueThreadHandle {
    #[must_use]
    pub fn storage_ref(&self) -> String {
        format!(
            "github://{}/{}/issues/{}",
            self.repo_owner, self.repo_name, self.issue_number
        )
    }

    #[must_use]
    pub fn display_label(&self) -> String {
        format!(
            "{}/{}#{}",
            self.repo_owner, self.repo_name, self.issue_number
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueThreadStrategyBudget {
    pub max_iterations: u32,
    pub max_loaded_chunks: u32,
    pub max_loaded_bytes: u64,
    pub max_sub_lm_calls: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RlmTriggerContext {
    pub operator_strategy: IssueThreadStrategyMode,
    pub requested_task_kind: String,
    pub has_explicit_issue_reference: bool,
    pub long_context_should_escalate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_issue: Option<GithubIssueThreadHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corpus_total_items: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corpus_total_chars: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RlmTriggerDecision {
    pub trigger_id: String,
    pub selected_strategy: LongContextStrategy,
    pub execution_strategy_id: String,
    pub confidence_bps: u16,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corpus_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<IssueThreadStrategyBudget>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IssueThreadCorpusSource {
    GithubIssue { handle: GithubIssueThreadHandle },
    LocalPath { path: String },
}

impl IssueThreadCorpusSource {
    #[must_use]
    pub fn display_ref(&self) -> String {
        match self {
            Self::GithubIssue { handle } => handle.storage_ref(),
            Self::LocalPath { path } => path.clone(),
        }
    }

    #[must_use]
    pub fn corpus_kind(&self) -> CorpusKind {
        match self {
            Self::GithubIssue { .. } => CorpusKind::GithubIssueThread,
            Self::LocalPath { .. } => CorpusKind::LocalPath,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueThreadAnalysisOutput {
    pub question: String,
    pub answer: String,
    pub evidence_item_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueThreadCorpusStats {
    pub source_ref: String,
    pub total_items: usize,
    pub total_chars: usize,
    pub flattened_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueThreadAnalysisPlan {
    pub question: String,
    pub source: IssueThreadCorpusSource,
    pub corpus_stats: IssueThreadCorpusStats,
    pub strategy_decision: RlmTriggerDecision,
}

#[derive(Clone, Debug)]
pub struct IssueThreadAnalysisRequest {
    pub source: IssueThreadCorpusSource,
    pub question: String,
    pub strategy_mode: IssueThreadStrategyMode,
    pub has_explicit_issue_reference: bool,
    pub direct_profile: BackendProfile,
    pub controller_profile: BackendProfile,
    pub sub_lm_profile: BackendProfile,
    pub probe_home: Option<PathBuf>,
    pub output_root: PathBuf,
    pub github_token: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueThreadAnalysisOutcome {
    pub plan: IssueThreadAnalysisPlan,
    pub output: IssueThreadAnalysisOutput,
    pub iterations: u32,
    pub sub_lm_calls: u32,
    pub output_dir: String,
    pub artifact_refs: Vec<String>,
}

#[derive(Debug)]
pub enum IssueThreadAnalysisError {
    MissingSourcePath(String),
    CountMismatch(CountMismatch),
    Io(std::io::Error),
    Json(serde_json::Error),
    Eval(forge_eval::EvalError),
    Provider(ProviderError),
    PaperRlm(crate::paper_rlm::PaperRlmExecutionError),
    InvalidOutput(String),
}

impl Display for IssueThreadAnalysisError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSourcePath(path) => {
                write!(f, "issue thread corpus path does not exist: {path}")
            }
            Self::CountMismatch(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::Eval(error) => write!(f, "{error}"),
            Self::Provider(error) => write!(f, "{error}"),
            Self::PaperRlm(error) => write!(f, "{error}"),
            Self::InvalidOutput(detail) => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for IssueThreadAnalysisError {}

impl From<CountMismatch> for IssueThreadAnalysisError {
    fn from(value: CountMismatch) -> Self {
        Self::CountMismatch(value)
    }
}

impl From<std::io::Error> for IssueThreadAnalysisError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for IssueThreadAnalysisError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<forge_eval::EvalError> for IssueThreadAnalysisError {
    fn from(value: forge_eval::EvalError) -> Self {
        Self::Eval(value)
    }
}

impl From<ProviderError> for IssueThreadAnalysisError {
    fn from(value: ProviderError) -> Self {
        Self::Provider(value)
    }
}

impl From<crate::paper_rlm::PaperRlmExecutionError> for IssueThreadAnalysisError {
    fn from(value: crate::paper_rlm::PaperRlmExecutionError) -> Self {
        Self::PaperRlm(value)
    }
}

#[must_use]
pub fn heuristic_rlm_trigger(input: &RlmTriggerContext) -> RlmTriggerDecision {
    let budget = default_paper_rlm_budget();
    let corpus_ref = input
        .selected_issue
        .as_ref()
        .map(GithubIssueThreadHandle::storage_ref);
    let total_items = input.corpus_total_items.unwrap_or(0);
    let total_chars = input.corpus_total_chars.unwrap_or(0);

    match input.operator_strategy {
        IssueThreadStrategyMode::Direct => {
            return RlmTriggerDecision {
                trigger_id: String::from(HEURISTIC_RLM_TRIGGER_ID),
                selected_strategy: LongContextStrategy::Direct,
                execution_strategy_id: String::from(DIRECT_ISSUE_THREAD_STRATEGY_ID),
                confidence_bps: 9_900,
                reason: String::from("operator forced the direct issue-thread lane"),
                corpus_ref,
                budget: None,
            };
        }
        IssueThreadStrategyMode::Rlm => {
            return RlmTriggerDecision {
                trigger_id: String::from(HEURISTIC_RLM_TRIGGER_ID),
                selected_strategy: LongContextStrategy::Rlm,
                execution_strategy_id: String::from(PAPER_RLM_ISSUE_THREAD_STRATEGY_ID),
                confidence_bps: 9_900,
                reason: String::from("operator forced the paper RLM lane"),
                corpus_ref,
                budget: Some(budget),
            };
        }
        IssueThreadStrategyMode::Auto => {}
    }

    if input.selected_issue.is_none() {
        if input.long_context_should_escalate {
            return RlmTriggerDecision {
                trigger_id: String::from(HEURISTIC_RLM_TRIGGER_ID),
                selected_strategy: LongContextStrategy::Compact,
                execution_strategy_id: String::from(COMPACT_LONG_CONTEXT_STRATEGY_ID),
                confidence_bps: 7_600,
                reason: String::from(
                    "the task shows long-context pressure, but there is no concrete issue-thread corpus handle yet",
                ),
                corpus_ref: None,
                budget: None,
            };
        }
        return RlmTriggerDecision {
            trigger_id: String::from(HEURISTIC_RLM_TRIGGER_ID),
            selected_strategy: LongContextStrategy::Direct,
            execution_strategy_id: String::from(DIRECT_ISSUE_THREAD_STRATEGY_ID),
            confidence_bps: 8_600,
            reason: String::from(
                "stay on the direct lane until a concrete issue-thread corpus handle exists",
            ),
            corpus_ref: None,
            budget: None,
        };
    }

    if input.has_explicit_issue_reference
        && (input.corpus_total_items.is_none() || total_items >= 8 || total_chars >= 6_000)
    {
        return RlmTriggerDecision {
            trigger_id: String::from(HEURISTIC_RLM_TRIGGER_ID),
            selected_strategy: LongContextStrategy::Rlm,
            execution_strategy_id: String::from(PAPER_RLM_ISSUE_THREAD_STRATEGY_ID),
            confidence_bps: 8_900,
            reason: String::from(
                "an explicit GitHub issue reference selected a concrete issue-thread corpus handle",
            ),
            corpus_ref,
            budget: Some(budget),
        };
    }

    if total_items >= 24 || total_chars >= 18_000 {
        return RlmTriggerDecision {
            trigger_id: String::from(HEURISTIC_RLM_TRIGGER_ID),
            selected_strategy: LongContextStrategy::Rlm,
            execution_strategy_id: String::from(PAPER_RLM_ISSUE_THREAD_STRATEGY_ID),
            confidence_bps: 8_400,
            reason: String::from(
                "the selected issue thread is large enough that the paper RLM lane is the safer strategy",
            ),
            corpus_ref,
            budget: Some(budget),
        };
    }

    if input.long_context_should_escalate && (total_items >= 8 || total_chars >= 6_000) {
        return RlmTriggerDecision {
            trigger_id: String::from(HEURISTIC_RLM_TRIGGER_ID),
            selected_strategy: LongContextStrategy::Rlm,
            execution_strategy_id: String::from(PAPER_RLM_ISSUE_THREAD_STRATEGY_ID),
            confidence_bps: 7_900,
            reason: String::from(
                "the selected issue thread already shows enough context pressure to justify the paper RLM lane",
            ),
            corpus_ref,
            budget: Some(budget),
        };
    }

    RlmTriggerDecision {
        trigger_id: String::from(HEURISTIC_RLM_TRIGGER_ID),
        selected_strategy: LongContextStrategy::Direct,
        execution_strategy_id: String::from(DIRECT_ISSUE_THREAD_STRATEGY_ID),
        confidence_bps: 8_100,
        reason: String::from("the selected issue thread is still small enough for the direct lane"),
        corpus_ref,
        budget: None,
    }
}

pub fn plan_issue_thread_analysis(
    request: &IssueThreadAnalysisRequest,
) -> Result<IssueThreadAnalysisPlan, IssueThreadAnalysisError> {
    let (corpus, handle) =
        materialize_issue_thread_corpus(&request.source, request.github_token.as_deref())?;
    let corpus_stats = IssueThreadCorpusStats {
        source_ref: request.source.display_ref(),
        total_items: corpus.total_items(),
        total_chars: corpus_text_total_chars(&corpus),
        flattened_bytes: corpus.render_markdown_snapshot().len(),
    };
    let strategy_decision = heuristic_rlm_trigger(&RlmTriggerContext {
        operator_strategy: request.strategy_mode,
        requested_task_kind: String::from("issue_thread_analysis"),
        has_explicit_issue_reference: request.has_explicit_issue_reference,
        long_context_should_escalate: corpus_stats.total_items >= 8
            || corpus_stats.total_chars >= 6_000,
        selected_issue: handle,
        corpus_total_items: Some(corpus_stats.total_items),
        corpus_total_chars: Some(corpus_stats.total_chars),
    });
    Ok(IssueThreadAnalysisPlan {
        question: request.question.clone(),
        source: request.source.clone(),
        corpus_stats,
        strategy_decision,
    })
}

pub fn execute_issue_thread_analysis(
    request: IssueThreadAnalysisRequest,
) -> Result<IssueThreadAnalysisOutcome, IssueThreadAnalysisError> {
    let (corpus, handle) =
        materialize_issue_thread_corpus(&request.source, request.github_token.as_deref())?;
    let corpus_stats = IssueThreadCorpusStats {
        source_ref: request.source.display_ref(),
        total_items: corpus.total_items(),
        total_chars: corpus_text_total_chars(&corpus),
        flattened_bytes: corpus.render_markdown_snapshot().len(),
    };
    let strategy_decision = heuristic_rlm_trigger(&RlmTriggerContext {
        operator_strategy: request.strategy_mode,
        requested_task_kind: String::from("issue_thread_analysis"),
        has_explicit_issue_reference: request.has_explicit_issue_reference,
        long_context_should_escalate: corpus_stats.total_items >= 8
            || corpus_stats.total_chars >= 6_000,
        selected_issue: handle,
        corpus_total_items: Some(corpus_stats.total_items),
        corpus_total_chars: Some(corpus_stats.total_chars),
    });
    let plan = IssueThreadAnalysisPlan {
        question: request.question.clone(),
        source: request.source.clone(),
        corpus_stats,
        strategy_decision: strategy_decision.clone(),
    };
    let output_dir = request
        .output_root
        .join(issue_thread_run_directory_name(&plan));
    fs::create_dir_all(output_dir.as_path())?;
    write_json_file(output_dir.join("analysis_plan.json").as_path(), &plan)?;

    match strategy_decision.selected_strategy {
        LongContextStrategy::Direct => {
            execute_direct_issue_thread_analysis(&request, corpus, plan, output_dir)
        }
        LongContextStrategy::Rlm => {
            execute_paper_issue_thread_analysis(&request, corpus, plan, output_dir)
        }
        LongContextStrategy::Compact => Err(IssueThreadAnalysisError::InvalidOutput(String::from(
            "compact issue-thread execution is not implemented; re-run with a concrete issue handle or force direct/rlm",
        ))),
    }
}

fn execute_direct_issue_thread_analysis(
    request: &IssueThreadAnalysisRequest,
    corpus: IssueThreadCorpus,
    plan: IssueThreadAnalysisPlan,
    output_dir: PathBuf,
) -> Result<IssueThreadAnalysisOutcome, IssueThreadAnalysisError> {
    let paper_corpus = PaperRlmCorpus::from_issue_thread(&corpus);
    let manifest = paper_corpus.manifest();
    let prompt = format!(
        "Question:\n{}\n\nCorpus manifest:\n{}\n\nFull issue thread:\n{}",
        request.question,
        render_manifest_lines(&manifest),
        corpus.render_markdown_snapshot()
    );
    write_json_file(output_dir.join("corpus_manifest.json").as_path(), &manifest)?;
    fs::write(
        output_dir.join("corpus.md"),
        corpus.render_markdown_snapshot(),
    )?;
    fs::write(output_dir.join("direct_prompt.txt"), prompt.as_bytes())?;

    let response = complete_plain_text_with_context(
        &request.direct_profile,
        vec![
            PlainTextMessage::system(DEFAULT_DIRECT_SYSTEM_PROMPT),
            PlainTextMessage::user(prompt.clone()),
        ],
        OpenAiRequestContext {
            probe_home: request.probe_home.as_deref(),
            session_id: None,
        },
    )?;
    let assistant_text = response.assistant_text.unwrap_or_default();
    fs::write(
        output_dir.join("direct_response.txt"),
        assistant_text.as_bytes(),
    )?;
    let parsed = parse_issue_thread_analysis_output(
        extract_json_value(assistant_text.as_str())?,
        request.question.as_str(),
    )?;
    write_json_file(output_dir.join("analysis_output.json").as_path(), &parsed)?;

    let artifact_refs = vec![
        display_path(output_dir.join("analysis_plan.json").as_path()),
        display_path(output_dir.join("corpus_manifest.json").as_path()),
        display_path(output_dir.join("corpus.md").as_path()),
        display_path(output_dir.join("direct_prompt.txt").as_path()),
        display_path(output_dir.join("direct_response.txt").as_path()),
        display_path(output_dir.join("analysis_output.json").as_path()),
    ];
    let outcome = IssueThreadAnalysisOutcome {
        plan,
        output: parsed,
        iterations: 1,
        sub_lm_calls: 0,
        output_dir: display_path(output_dir.as_path()),
        artifact_refs: artifact_refs.clone(),
    };
    write_json_file(output_dir.join("analysis_outcome.json").as_path(), &outcome)?;
    Ok(IssueThreadAnalysisOutcome {
        artifact_refs: {
            let mut refs = artifact_refs;
            refs.push(display_path(
                output_dir.join("analysis_outcome.json").as_path(),
            ));
            refs
        },
        ..outcome
    })
}

fn execute_paper_issue_thread_analysis(
    request: &IssueThreadAnalysisRequest,
    corpus: IssueThreadCorpus,
    plan: IssueThreadAnalysisPlan,
    output_dir: PathBuf,
) -> Result<IssueThreadAnalysisOutcome, IssueThreadAnalysisError> {
    let bundle = ExecutionPolicyBundle::issue_thread_paper_rlm_default();
    let runtime_assignment = RuntimeAssignment {
        assignment_id: format!(
            "paper-issue-thread-{}",
            sanitize_path_fragment(plan.corpus_stats.source_ref.as_str())
        ),
        strategy_family: bundle.strategy_family,
        policy_bundle: bundle.policy_ref,
        corpus: CorpusLocator {
            kind: request.source.corpus_kind(),
            storage_ref: request.source.display_ref(),
            content_hash: None,
            expected_item_count: Some(plan.corpus_stats.total_items),
        },
        budget: ExecutionBudget {
            max_iterations: bundle.max_iterations,
            max_loaded_chunks: bundle.max_loaded_chunks,
            max_duration_seconds: 300,
            max_sub_lm_calls: bundle
                .repl_policy
                .as_ref()
                .map(|policy| policy.max_sub_lm_calls)
                .unwrap_or(24),
            max_loaded_bytes: 2_000_000,
            max_stdout_bytes: 16_384,
            max_observation_bytes: 65_536,
        },
        model_roles: bundle.model_roles,
        repl_policy: bundle.repl_policy,
        output_schema: OutputSchema::RlmFinalJsonV1,
    };
    let paper_request = PaperRlmExecutionRequest {
        assignment: runtime_assignment,
        query: format!(
            "Answer the GitHub issue-thread question below. Return strict JSON only with keys \
`answer` and `evidence_item_refs`.\n\nQuestion:\n{}",
            request.question
        ),
        corpus: PaperRlmCorpus::from_issue_thread(&corpus),
        controller_profile: request.controller_profile.clone(),
        sub_lm_profile: Some(request.sub_lm_profile.clone()),
        probe_home: request.probe_home.clone(),
        output_root: output_dir.clone(),
    };
    let runtime_outcome = execute_paper_rlm_request(paper_request)?;
    let final_output = runtime_outcome.final_output.ok_or_else(|| {
        IssueThreadAnalysisError::InvalidOutput(
            runtime_outcome
                .failure_reason
                .unwrap_or_else(|| String::from("paper RLM runtime did not produce final output")),
        )
    })?;
    let parsed = parse_issue_thread_analysis_output(final_output, request.question.as_str())?;
    write_json_file(output_dir.join("analysis_output.json").as_path(), &parsed)?;

    let mut artifact_refs = runtime_outcome.runtime_result.artifact_refs.clone();
    artifact_refs.push(display_path(
        output_dir.join("analysis_plan.json").as_path(),
    ));
    artifact_refs.push(display_path(
        output_dir.join("analysis_output.json").as_path(),
    ));
    let outcome = IssueThreadAnalysisOutcome {
        plan,
        output: parsed,
        iterations: runtime_outcome
            .trajectory
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    crate::paper_rlm::PaperRlmTraceEvent::ControllerTurnRequested { .. }
                )
            })
            .count() as u32,
        sub_lm_calls: runtime_outcome.subcall_receipts.len() as u32,
        output_dir: display_path(output_dir.as_path()),
        artifact_refs: artifact_refs.clone(),
    };
    write_json_file(output_dir.join("analysis_outcome.json").as_path(), &outcome)?;
    artifact_refs.push(display_path(
        output_dir.join("analysis_outcome.json").as_path(),
    ));
    Ok(IssueThreadAnalysisOutcome {
        artifact_refs,
        ..outcome
    })
}

pub(crate) fn materialize_issue_thread_corpus(
    source: &IssueThreadCorpusSource,
    github_token: Option<&str>,
) -> Result<(IssueThreadCorpus, Option<GithubIssueThreadHandle>), IssueThreadAnalysisError> {
    match source {
        IssueThreadCorpusSource::GithubIssue { handle } => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let corpus = runtime.block_on(fetch_issue_thread(
                handle.repo_owner.as_str(),
                handle.repo_name.as_str(),
                handle.issue_number,
                github_token,
            ))?;
            Ok((corpus, Some(handle.clone())))
        }
        IssueThreadCorpusSource::LocalPath { path } => {
            let path = PathBuf::from(path);
            if !path.exists() {
                return Err(IssueThreadAnalysisError::MissingSourcePath(
                    path.display().to_string(),
                ));
            }
            let raw = fs::read_to_string(path.as_path())?;
            let corpus: IssueThreadCorpus = serde_json::from_str(&raw)?;
            corpus.validate_comment_count()?;
            let handle = GithubIssueThreadHandle {
                repo_owner: corpus.repository_owner.clone(),
                repo_name: corpus.repository_name.clone(),
                issue_number: corpus.issue_number,
                issue_url: Some(corpus.issue_url.clone()),
            };
            Ok((corpus, Some(handle)))
        }
    }
}

fn parse_issue_thread_analysis_output(
    value: Value,
    question: &str,
) -> Result<IssueThreadAnalysisOutput, IssueThreadAnalysisError> {
    let answer = value
        .get("answer")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            IssueThreadAnalysisError::InvalidOutput(String::from(
                "issue-thread analysis output must include a non-empty `answer` string",
            ))
        })?
        .to_string();
    let evidence_item_refs = value
        .get("evidence_item_refs")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            IssueThreadAnalysisError::InvalidOutput(String::from(
                "issue-thread analysis output must include `evidence_item_refs`",
            ))
        })?
        .iter()
        .filter_map(|value| value.as_str().map(ToString::to_string))
        .collect::<Vec<_>>();
    if evidence_item_refs.is_empty() {
        return Err(IssueThreadAnalysisError::InvalidOutput(String::from(
            "issue-thread analysis output must include at least one evidence item ref",
        )));
    }
    Ok(IssueThreadAnalysisOutput {
        question: question.to_string(),
        answer,
        evidence_item_refs,
    })
}

fn extract_json_value(text: &str) -> Result<Value, IssueThreadAnalysisError> {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(value);
    }

    if let Some(inner) = trimmed
        .strip_prefix("```json")
        .and_then(|value| value.strip_suffix("```"))
    {
        return serde_json::from_str(inner.trim()).map_err(IssueThreadAnalysisError::from);
    }

    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        return serde_json::from_str(&trimmed[start..=end]).map_err(IssueThreadAnalysisError::from);
    }

    Err(IssueThreadAnalysisError::InvalidOutput(String::from(
        "failed to parse JSON issue-thread analysis output",
    )))
}

fn render_manifest_lines(manifest: &crate::paper_rlm::PaperRlmCorpusManifest) -> String {
    manifest
        .items
        .iter()
        .map(|item| {
            format!(
                "- {} [{}] {} chars={} created_at={}",
                item.item_ref,
                item.item_kind,
                item.label,
                item.char_count,
                item.created_at.as_deref().unwrap_or("unknown")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn default_paper_rlm_budget() -> IssueThreadStrategyBudget {
    let bundle = ExecutionPolicyBundle::issue_thread_paper_rlm_default();
    IssueThreadStrategyBudget {
        max_iterations: bundle.max_iterations,
        max_loaded_chunks: bundle.max_loaded_chunks,
        max_loaded_bytes: 2_000_000,
        max_sub_lm_calls: bundle
            .repl_policy
            .as_ref()
            .map(|policy| policy.max_sub_lm_calls)
            .unwrap_or(24),
    }
}

fn corpus_text_total_chars(corpus: &IssueThreadCorpus) -> usize {
    corpus.issue_body.body.chars().count()
        + corpus
            .comments
            .iter()
            .map(|comment| comment.body.chars().count())
            .sum::<usize>()
}

fn write_json_file(path: &Path, value: &impl Serialize) -> Result<(), IssueThreadAnalysisError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(path, bytes)?;
    Ok(())
}

fn issue_thread_run_directory_name(plan: &IssueThreadAnalysisPlan) -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let source_label = match &plan.source {
        IssueThreadCorpusSource::GithubIssue { handle } => format!(
            "{}-{}-{}",
            handle.repo_owner.to_ascii_lowercase(),
            handle.repo_name.to_ascii_lowercase(),
            handle.issue_number
        ),
        IssueThreadCorpusSource::LocalPath { path } => sanitize_path_fragment(path.as_str()),
    };
    format!(
        "{}-{}-{seconds}",
        source_label, plan.strategy_decision.execution_strategy_id
    )
}

fn sanitize_path_fragment(value: &str) -> String {
    let mut rendered = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while rendered.contains("--") {
        rendered = rendered.replace("--", "-");
    }
    rendered.trim_matches('-').to_string()
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        GithubIssueThreadHandle, IssueThreadAnalysisOutput, IssueThreadAnalysisRequest,
        IssueThreadCorpusSource, IssueThreadStrategyMode, LongContextStrategy,
        execute_issue_thread_analysis, heuristic_rlm_trigger,
    };
    use crate::backend_profiles::openai_codex_subscription;
    use forge_rlm_core::{IssueBody, IssueComment, IssueThreadCorpus};
    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use probe_test_support::FakeOpenAiServer;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn heuristic_trigger_can_choose_compact_without_a_corpus_handle() {
        let decision = heuristic_rlm_trigger(&super::RlmTriggerContext {
            operator_strategy: IssueThreadStrategyMode::Auto,
            requested_task_kind: String::from("architecture_summary"),
            has_explicit_issue_reference: false,
            long_context_should_escalate: true,
            selected_issue: None,
            corpus_total_items: None,
            corpus_total_chars: None,
        });
        assert_eq!(decision.selected_strategy, LongContextStrategy::Compact);
        assert_eq!(decision.execution_strategy_id, "compact_long_context_v1");
    }

    #[test]
    fn issue_thread_analysis_runs_under_both_direct_and_rlm_strategies() {
        unsafe {
            std::env::set_var("PROBE_OPENAI_API_KEY", "test-token");
        }
        let tempdir = tempdir().expect("tempdir");
        let corpus_path = tempdir.path().join("corpus.json");
        let output_root = tempdir.path().join("out");
        std::fs::write(
            corpus_path.as_path(),
            serde_json::to_vec_pretty(&synthetic_issue_thread()).expect("serialize corpus"),
        )
        .expect("write corpus");

        let direct_server = FakeOpenAiServer::from_json_responses(vec![json!({
            "id": "chatcmpl_direct_issue_thread_1",
            "model": "direct-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "{\"answer\":\"The blocker is payout dispatch gating.\",\"evidence_item_refs\":[\"comment-1\"]}"
                },
                "finish_reason": "stop"
            }]
        })]);
        let direct_outcome = execute_issue_thread_analysis(IssueThreadAnalysisRequest {
            source: IssueThreadCorpusSource::LocalPath {
                path: corpus_path.display().to_string(),
            },
            question: String::from("What is the blocker?"),
            strategy_mode: IssueThreadStrategyMode::Direct,
            has_explicit_issue_reference: true,
            direct_profile: test_profile("direct", direct_server.base_url().to_string()),
            controller_profile: openai_codex_subscription(),
            sub_lm_profile: openai_codex_subscription(),
            probe_home: None,
            output_root: output_root.join("direct"),
            github_token: None,
        })
        .expect("direct issue-thread analysis");
        assert_eq!(
            direct_outcome.output,
            IssueThreadAnalysisOutput {
                question: String::from("What is the blocker?"),
                answer: String::from("The blocker is payout dispatch gating."),
                evidence_item_refs: vec![String::from("comment-1")],
            }
        );
        assert_eq!(
            direct_outcome.plan.strategy_decision.execution_strategy_id,
            "issue_thread_direct_v1"
        );

        let controller_server = FakeOpenAiServer::from_json_responses(vec![json!({
            "id": "chatcmpl_controller_issue_thread_1",
            "model": "controller-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "```rhai\nlet body = context_load(\"comment-1\");\nlet answer = llm_query(\"Answer the question with strict JSON and evidence_item_refs.\\nQuestion: What is the blocker?\\nCorpus:\\n\" + body);\nFINAL_VAR(answer);\n```"
                },
                "finish_reason": "stop"
            }]
        })]);
        let sub_lm_server = FakeOpenAiServer::from_json_responses(vec![json!({
            "id": "chatcmpl_sub_issue_thread_1",
            "model": "sub-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "{\"answer\":\"The blocker is payout dispatch gating.\",\"evidence_item_refs\":[\"comment-1\"]}"
                },
                "finish_reason": "stop"
            }]
        })]);
        let rlm_outcome = execute_issue_thread_analysis(IssueThreadAnalysisRequest {
            source: IssueThreadCorpusSource::LocalPath {
                path: corpus_path.display().to_string(),
            },
            question: String::from("What is the blocker?"),
            strategy_mode: IssueThreadStrategyMode::Rlm,
            has_explicit_issue_reference: true,
            direct_profile: openai_codex_subscription(),
            controller_profile: test_profile(
                "controller",
                controller_server.base_url().to_string(),
            ),
            sub_lm_profile: test_profile("sub-lm", sub_lm_server.base_url().to_string()),
            probe_home: None,
            output_root: output_root.join("rlm"),
            github_token: None,
        })
        .expect("paper rlm issue-thread analysis");
        assert_eq!(rlm_outcome.output, direct_outcome.output);
        assert_eq!(
            rlm_outcome.plan.strategy_decision.execution_strategy_id,
            "paper_rlm_issue_thread_v1"
        );
        assert_eq!(rlm_outcome.sub_lm_calls, 1);
        assert!(
            rlm_outcome
                .artifact_refs
                .iter()
                .any(|path| path.ends_with("analysis_output.json"))
        );
    }

    #[test]
    fn auto_strategy_prefers_rlm_for_large_explicit_issue_threads() {
        let decision = heuristic_rlm_trigger(&super::RlmTriggerContext {
            operator_strategy: IssueThreadStrategyMode::Auto,
            requested_task_kind: String::from("issue_thread_analysis"),
            has_explicit_issue_reference: true,
            long_context_should_escalate: true,
            selected_issue: Some(GithubIssueThreadHandle {
                repo_owner: String::from("OpenAgentsInc"),
                repo_name: String::from("openagents"),
                issue_number: 4368,
                issue_url: None,
            }),
            corpus_total_items: Some(140),
            corpus_total_chars: Some(75_000),
        });
        assert_eq!(decision.selected_strategy, LongContextStrategy::Rlm);
        assert_eq!(decision.execution_strategy_id, "paper_rlm_issue_thread_v1");
        assert!(decision.budget.is_some());
    }

    fn synthetic_issue_thread() -> IssueThreadCorpus {
        IssueThreadCorpus {
            repository_owner: String::from("OpenAgentsInc"),
            repository_name: String::from("openagents"),
            issue_number: 4368,
            issue_title: String::from("Finish distributed CS336/Ep224 homework run"),
            issue_state: String::from("open"),
            issue_url: String::from("https://github.com/OpenAgentsInc/openagents/issues/4368"),
            issue_body: IssueBody {
                author: String::from("AtlantisPleb"),
                created_at: String::from("2026-04-17T00:00:00Z"),
                body: String::from("### Objective\n\nProve the full live homework run."),
            },
            comment_count_from_metadata: 1,
            comments: vec![IssueComment {
                comment_id: 1,
                author: String::from("AtlantisPleb"),
                created_at: String::from("2026-04-17T01:00:00Z"),
                edited: false,
                minimized: false,
                body: String::from(
                    "The current blocker is payout dispatch gating after reconcile.",
                ),
            }],
        }
    }

    fn test_profile(name: &str, base_url: String) -> BackendProfile {
        BackendProfile {
            name: name.to_string(),
            kind: BackendKind::OpenAiChatCompletions,
            base_url,
            model: String::from("test-model"),
            reasoning_level: None,
            service_tier: None,
            api_key_env: String::from("PROBE_OPENAI_API_KEY"),
            timeout_secs: 30,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
            control_plane: None,
            psionic_mesh: None,
        }
    }
}
