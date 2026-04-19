use std::env;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use forge_eval::{IssueThreadEvalReport, fetch_issue_thread, run_issue_thread_eval};
use forge_policy::ExecutionPolicyBundle;
use forge_rlm_core::{IssueBody, IssueComment, IssueThreadCorpus};
use forge_runtime_protocol::{
    CorpusKind, CorpusLocator, ExecutionBudget, ExecutionStatus, OutputSchema, RuntimeAssignment,
    RuntimeExecutionResult,
};
use forge_signatures::StrategyFamily;
use serde::{Deserialize, Serialize};

const DEFAULT_MAX_DURATION_SECONDS: u64 = 300;
const DEFAULT_MAX_LINES_PER_CHUNK: usize = 48;
const REQUIRED_ARTIFACT_NAMES: &[&str] = &[
    "assignment.json",
    "corpus.json",
    "corpus.md",
    "chunk_manifest.json",
    "report.json",
    "trace.json",
    "events.json",
    "runtime_result.json",
    "brief.md",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeRlmExecutionPlan {
    pub assignment: RuntimeAssignment,
    pub workspace_ref: Option<String>,
    pub publication_label: Option<String>,
    pub required_artifacts: Vec<String>,
}

impl ForgeRlmExecutionPlan {
    #[must_use]
    pub fn openagents_4368_issue_thread_proof() -> Self {
        let bundle = ExecutionPolicyBundle::issue_thread_eval_default();
        Self {
            assignment: RuntimeAssignment {
                assignment_id: String::from("forge-rlm-openagents-4368"),
                strategy_family: bundle.strategy_family,
                policy_bundle: bundle.policy_ref,
                corpus: CorpusLocator {
                    kind: CorpusKind::GithubIssueThread,
                    storage_ref: String::from("github://OpenAgentsInc/openagents/issues/4368"),
                    content_hash: None,
                    expected_item_count: None,
                },
                budget: ExecutionBudget {
                    max_iterations: bundle.max_iterations,
                    max_loaded_chunks: bundle.max_loaded_chunks,
                    max_duration_seconds: DEFAULT_MAX_DURATION_SECONDS,
                },
                output_schema: OutputSchema::IssueThreadAnalysisV1,
            },
            workspace_ref: None,
            publication_label: Some(String::from("openagents-4368")),
            required_artifacts: default_required_artifacts(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueThreadChunkManifestEntry {
    pub chunk_index: usize,
    pub item_refs: Vec<String>,
    pub item_kinds: Vec<String>,
    pub line_count: usize,
    pub byte_count: usize,
    pub first_created_at: Option<String>,
    pub last_created_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ForgeRlmExecutionEvent {
    AssignmentAccepted {
        assignment_id: String,
        strategy_family: String,
        output_schema: String,
    },
    CorpusMaterialized {
        storage_ref: String,
        corpus_kind: String,
        total_items: usize,
        comment_count: usize,
    },
    ChunkPlanBuilt {
        chunk_count: usize,
        max_loaded_chunks: u32,
        max_lines_per_chunk: usize,
    },
    AnalysisCompleted {
        passed: bool,
        check_count: usize,
        phase_count: usize,
        code_seam_count: usize,
    },
    ArtifactsWritten {
        output_dir: String,
        artifact_count: usize,
    },
}

pub trait ForgeRlmEventSink: Send + Sync {
    fn emit(&self, event: ForgeRlmExecutionEvent);
}

impl<F> ForgeRlmEventSink for F
where
    F: Fn(ForgeRlmExecutionEvent) + Send + Sync,
{
    fn emit(&self, event: ForgeRlmExecutionEvent) {
        self(event);
    }
}

#[derive(Clone, Debug)]
pub struct ForgeRlmExecutionRequest {
    pub plan: ForgeRlmExecutionPlan,
    pub output_root: PathBuf,
    pub github_token: Option<String>,
    pub max_lines_per_chunk: usize,
}

impl ForgeRlmExecutionRequest {
    #[must_use]
    pub fn with_defaults(plan: ForgeRlmExecutionPlan, output_root: impl Into<PathBuf>) -> Self {
        Self {
            plan,
            output_root: output_root.into(),
            github_token: resolve_github_token(),
            max_lines_per_chunk: DEFAULT_MAX_LINES_PER_CHUNK,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeRlmArtifactPaths {
    pub output_dir: String,
    pub assignment_path: String,
    pub corpus_json_path: String,
    pub corpus_markdown_path: String,
    pub chunk_manifest_path: String,
    pub report_path: String,
    pub trace_path: String,
    pub event_log_path: String,
    pub runtime_result_path: String,
    pub brief_path: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForgeRlmExecutionOutcome {
    pub plan: ForgeRlmExecutionPlan,
    pub report: IssueThreadEvalReport,
    pub chunk_manifest: Vec<IssueThreadChunkManifestEntry>,
    pub event_log: Vec<ForgeRlmExecutionEvent>,
    pub runtime_result: RuntimeExecutionResult,
    pub artifacts: ForgeRlmArtifactPaths,
}

#[derive(Debug)]
pub enum ForgeRlmExecutionError {
    UnsupportedCorpusKind(CorpusKind),
    UnsupportedOutputSchema(OutputSchema),
    InvalidGithubStorageRef(String),
    InvalidChunkBudget {
        chunk_count: usize,
        max_loaded_chunks: u32,
    },
    InvalidMaxLinesPerChunk,
    MissingRequiredArtifacts(Vec<String>),
    ExpectedItemCountMismatch {
        expected: usize,
        actual: usize,
    },
    Io(std::io::Error),
    Json(serde_json::Error),
    Eval(forge_eval::EvalError),
    Tokio(std::io::Error),
}

impl Display for ForgeRlmExecutionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedCorpusKind(kind) => {
                write!(f, "unsupported Forge RLM corpus kind: {kind:?}")
            }
            Self::UnsupportedOutputSchema(schema) => {
                write!(f, "unsupported Forge RLM output schema: {schema:?}")
            }
            Self::InvalidGithubStorageRef(storage_ref) => {
                write!(f, "invalid GitHub issue storage ref: {storage_ref}")
            }
            Self::InvalidChunkBudget {
                chunk_count,
                max_loaded_chunks,
            } => write!(
                f,
                "chunk plan exceeds assignment budget: chunk_count={chunk_count} max_loaded_chunks={max_loaded_chunks}"
            ),
            Self::InvalidMaxLinesPerChunk => {
                f.write_str("max_lines_per_chunk must be greater than zero")
            }
            Self::MissingRequiredArtifacts(missing) => {
                write!(
                    f,
                    "required artifacts were not published: {}",
                    missing.join(", ")
                )
            }
            Self::ExpectedItemCountMismatch { expected, actual } => write!(
                f,
                "materialized corpus item count mismatch: expected={expected} actual={actual}"
            ),
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::Eval(error) => write!(f, "{error}"),
            Self::Tokio(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ForgeRlmExecutionError {}

impl From<std::io::Error> for ForgeRlmExecutionError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ForgeRlmExecutionError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<forge_eval::EvalError> for ForgeRlmExecutionError {
    fn from(value: forge_eval::EvalError) -> Self {
        Self::Eval(value)
    }
}

#[must_use]
pub fn default_required_artifacts() -> Vec<String> {
    REQUIRED_ARTIFACT_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

#[must_use]
pub fn resolve_github_token() -> Option<String> {
    env::var("GITHUB_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("GH_TOKEN")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
}

pub fn execute_forge_rlm_plan(
    request: ForgeRlmExecutionRequest,
) -> Result<ForgeRlmExecutionOutcome, ForgeRlmExecutionError> {
    execute_forge_rlm_plan_with_events(request, None)
}

pub fn execute_forge_rlm_plan_with_events(
    request: ForgeRlmExecutionRequest,
    event_sink: Option<Arc<dyn ForgeRlmEventSink>>,
) -> Result<ForgeRlmExecutionOutcome, ForgeRlmExecutionError> {
    if request.max_lines_per_chunk == 0 {
        return Err(ForgeRlmExecutionError::InvalidMaxLinesPerChunk);
    }

    if request.plan.assignment.output_schema != OutputSchema::IssueThreadAnalysisV1 {
        return Err(ForgeRlmExecutionError::UnsupportedOutputSchema(
            request.plan.assignment.output_schema.clone(),
        ));
    }

    let mut event_log = Vec::new();
    record_event(
        &mut event_log,
        event_sink.as_ref(),
        ForgeRlmExecutionEvent::AssignmentAccepted {
            assignment_id: request.plan.assignment.assignment_id.clone(),
            strategy_family: strategy_family_name(request.plan.assignment.strategy_family)
                .to_string(),
            output_schema: output_schema_name(&request.plan.assignment.output_schema).to_string(),
        },
    );

    let corpus = materialize_corpus(
        &request.plan.assignment.corpus,
        request.github_token.as_deref(),
    )?;
    if let Some(expected) = request.plan.assignment.corpus.expected_item_count {
        let actual = corpus.total_items();
        if expected != actual {
            return Err(ForgeRlmExecutionError::ExpectedItemCountMismatch { expected, actual });
        }
    }

    record_event(
        &mut event_log,
        event_sink.as_ref(),
        ForgeRlmExecutionEvent::CorpusMaterialized {
            storage_ref: request.plan.assignment.corpus.storage_ref.clone(),
            corpus_kind: corpus_kind_name(&request.plan.assignment.corpus.kind).to_string(),
            total_items: corpus.total_items(),
            comment_count: corpus.comments.len(),
        },
    );

    let chunk_manifest = build_chunk_manifest(&corpus, request.max_lines_per_chunk);
    if chunk_manifest.len() as u32 > request.plan.assignment.budget.max_loaded_chunks {
        return Err(ForgeRlmExecutionError::InvalidChunkBudget {
            chunk_count: chunk_manifest.len(),
            max_loaded_chunks: request.plan.assignment.budget.max_loaded_chunks,
        });
    }

    record_event(
        &mut event_log,
        event_sink.as_ref(),
        ForgeRlmExecutionEvent::ChunkPlanBuilt {
            chunk_count: chunk_manifest.len(),
            max_loaded_chunks: request.plan.assignment.budget.max_loaded_chunks,
            max_lines_per_chunk: request.max_lines_per_chunk,
        },
    );

    let report = run_issue_thread_eval(&corpus)?;
    record_event(
        &mut event_log,
        event_sink.as_ref(),
        ForgeRlmExecutionEvent::AnalysisCompleted {
            passed: report.summary.passed,
            check_count: report.summary.checks.len(),
            phase_count: report.analysis.phase_map.len(),
            code_seam_count: report.analysis.code_seams.len(),
        },
    );

    let output_dir = request.output_root.join(run_directory_name(&request.plan));
    fs::create_dir_all(output_dir.as_path())?;

    let artifacts = ForgeRlmArtifactPaths {
        output_dir: display_path(output_dir.as_path()),
        assignment_path: display_path(output_dir.join("assignment.json").as_path()),
        corpus_json_path: display_path(output_dir.join("corpus.json").as_path()),
        corpus_markdown_path: display_path(output_dir.join("corpus.md").as_path()),
        chunk_manifest_path: display_path(output_dir.join("chunk_manifest.json").as_path()),
        report_path: display_path(output_dir.join("report.json").as_path()),
        trace_path: display_path(output_dir.join("trace.json").as_path()),
        event_log_path: display_path(output_dir.join("events.json").as_path()),
        runtime_result_path: display_path(output_dir.join("runtime_result.json").as_path()),
        brief_path: display_path(output_dir.join("brief.md").as_path()),
    };

    verify_required_artifacts(&request.plan.required_artifacts, &artifacts)?;

    write_json_file(Path::new(&artifacts.assignment_path), &request.plan)?;
    write_json_file(Path::new(&artifacts.corpus_json_path), &corpus)?;
    fs::write(
        Path::new(&artifacts.corpus_markdown_path),
        corpus.render_markdown_snapshot(),
    )?;
    write_json_file(Path::new(&artifacts.chunk_manifest_path), &chunk_manifest)?;
    write_json_file(Path::new(&artifacts.report_path), &report)?;
    write_json_file(Path::new(&artifacts.trace_path), &report.analysis.trace)?;
    fs::write(
        Path::new(&artifacts.brief_path),
        report.analysis.follow_up_issue_brief.as_bytes(),
    )?;

    let runtime_result = RuntimeExecutionResult {
        assignment_id: request.plan.assignment.assignment_id.clone(),
        status: if report.summary.passed {
            ExecutionStatus::Succeeded
        } else {
            ExecutionStatus::Failed
        },
        output: Some(serde_json::to_value(&report.analysis)?),
        artifact_refs: vec![
            artifacts.assignment_path.clone(),
            artifacts.corpus_json_path.clone(),
            artifacts.corpus_markdown_path.clone(),
            artifacts.chunk_manifest_path.clone(),
            artifacts.report_path.clone(),
            artifacts.trace_path.clone(),
            artifacts.event_log_path.clone(),
            artifacts.runtime_result_path.clone(),
            artifacts.brief_path.clone(),
        ],
        summary: Some(build_execution_summary(&report, chunk_manifest.len())),
    };

    record_event(
        &mut event_log,
        event_sink.as_ref(),
        ForgeRlmExecutionEvent::ArtifactsWritten {
            output_dir: artifacts.output_dir.clone(),
            artifact_count: runtime_result.artifact_refs.len(),
        },
    );

    write_json_file(Path::new(&artifacts.event_log_path), &event_log)?;
    write_json_file(Path::new(&artifacts.runtime_result_path), &runtime_result)?;

    Ok(ForgeRlmExecutionOutcome {
        plan: request.plan,
        report,
        chunk_manifest,
        event_log,
        runtime_result,
        artifacts,
    })
}

fn materialize_corpus(
    locator: &CorpusLocator,
    github_token: Option<&str>,
) -> Result<IssueThreadCorpus, ForgeRlmExecutionError> {
    match locator.kind {
        CorpusKind::GithubIssueThread => {
            let (owner, repo, issue_number) = parse_github_issue_ref(locator.storage_ref.as_str())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(ForgeRlmExecutionError::Tokio)?;
            runtime
                .block_on(fetch_issue_thread(
                    owner.as_str(),
                    repo.as_str(),
                    issue_number,
                    github_token,
                ))
                .map_err(ForgeRlmExecutionError::Eval)
        }
        CorpusKind::LocalPath => {
            let raw = fs::read_to_string(locator.storage_ref.as_str())?;
            let corpus: IssueThreadCorpus = serde_json::from_str(&raw)?;
            corpus
                .validate_comment_count()
                .map_err(|error| ForgeRlmExecutionError::Eval(error.into()))?;
            Ok(corpus)
        }
        CorpusKind::RepoEvidenceBundle => Err(ForgeRlmExecutionError::UnsupportedCorpusKind(
            locator.kind.clone(),
        )),
    }
}

fn parse_github_issue_ref(
    storage_ref: &str,
) -> Result<(String, String, u64), ForgeRlmExecutionError> {
    if let Some(rest) = storage_ref.strip_prefix("github://") {
        let parts = rest.split('/').collect::<Vec<_>>();
        if parts.len() == 4 && parts[2] == "issues" {
            let issue_number = parts[3].parse::<u64>().map_err(|_| {
                ForgeRlmExecutionError::InvalidGithubStorageRef(storage_ref.to_string())
            })?;
            return Ok((parts[0].to_string(), parts[1].to_string(), issue_number));
        }
    }

    if let Some(rest) = storage_ref.strip_prefix("https://github.com/") {
        let parts = rest.split('/').collect::<Vec<_>>();
        if parts.len() >= 4 && parts[2] == "issues" {
            let issue_number = parts[3].parse::<u64>().map_err(|_| {
                ForgeRlmExecutionError::InvalidGithubStorageRef(storage_ref.to_string())
            })?;
            return Ok((parts[0].to_string(), parts[1].to_string(), issue_number));
        }
    }

    Err(ForgeRlmExecutionError::InvalidGithubStorageRef(
        storage_ref.to_string(),
    ))
}

fn build_chunk_manifest(
    corpus: &IssueThreadCorpus,
    max_lines_per_chunk: usize,
) -> Vec<IssueThreadChunkManifestEntry> {
    let mut items = Vec::with_capacity(corpus.total_items());
    items.push(CorpusItem::issue_body(&corpus.issue_body));
    items.extend(corpus.comments.iter().map(CorpusItem::comment));

    let mut chunks = Vec::new();
    let mut current = ChunkAccumulator::default();

    for item in items {
        if !current.item_refs.is_empty()
            && current.line_count + item.line_count > max_lines_per_chunk
        {
            chunks.push(current.finish(chunks.len()));
            current = ChunkAccumulator::default();
        }

        current.push(item);
    }

    if !current.item_refs.is_empty() {
        chunks.push(current.finish(chunks.len()));
    }

    chunks
}

fn build_execution_summary(report: &IssueThreadEvalReport, chunk_count: usize) -> String {
    let failed_checks = report
        .summary
        .checks
        .iter()
        .filter(|check| !check.passed)
        .map(|check| check.name.as_str())
        .collect::<Vec<_>>();
    if failed_checks.is_empty() {
        format!(
            "assignment passed {} checks across {} chunks and {} corpus items",
            report.summary.checks.len(),
            chunk_count,
            report.corpus_stats.total_items
        )
    } else {
        format!("assignment failed checks: {}", failed_checks.join(", "))
    }
}

fn verify_required_artifacts(
    required_artifacts: &[String],
    artifacts: &ForgeRlmArtifactPaths,
) -> Result<(), ForgeRlmExecutionError> {
    let published = [
        ("assignment.json", artifacts.assignment_path.as_str()),
        ("corpus.json", artifacts.corpus_json_path.as_str()),
        ("corpus.md", artifacts.corpus_markdown_path.as_str()),
        (
            "chunk_manifest.json",
            artifacts.chunk_manifest_path.as_str(),
        ),
        ("report.json", artifacts.report_path.as_str()),
        ("trace.json", artifacts.trace_path.as_str()),
        ("events.json", artifacts.event_log_path.as_str()),
        (
            "runtime_result.json",
            artifacts.runtime_result_path.as_str(),
        ),
        ("brief.md", artifacts.brief_path.as_str()),
    ];

    let missing = required_artifacts
        .iter()
        .filter(|required| !published.iter().any(|(name, _)| name == &required.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(ForgeRlmExecutionError::MissingRequiredArtifacts(missing))
    }
}

fn record_event(
    event_log: &mut Vec<ForgeRlmExecutionEvent>,
    event_sink: Option<&Arc<dyn ForgeRlmEventSink>>,
    event: ForgeRlmExecutionEvent,
) {
    if let Some(sink) = event_sink {
        sink.emit(event.clone());
    }
    event_log.push(event);
}

fn run_directory_name(plan: &ForgeRlmExecutionPlan) -> String {
    let prefix = plan
        .publication_label
        .as_deref()
        .unwrap_or(plan.assignment.assignment_id.as_str());
    format!("{}-{}", slugify(prefix), unix_timestamp_ms())
}

fn write_json_file<T: Serialize>(path: &Path, value: &T) -> Result<(), ForgeRlmExecutionError> {
    let rendered = serde_json::to_string_pretty(value)?;
    fs::write(path, rendered)?;
    Ok(())
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn unix_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn slugify(input: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for character in input.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }

    slug.trim_matches('-').to_string()
}

fn strategy_family_name(strategy_family: StrategyFamily) -> &'static str {
    strategy_family.as_str()
}

fn output_schema_name(output_schema: &OutputSchema) -> &'static str {
    match output_schema {
        OutputSchema::IssueThreadAnalysisV1 => "issue_thread_analysis_v1",
    }
}

fn corpus_kind_name(kind: &CorpusKind) -> &'static str {
    match kind {
        CorpusKind::GithubIssueThread => "github_issue_thread",
        CorpusKind::RepoEvidenceBundle => "repo_evidence_bundle",
        CorpusKind::LocalPath => "local_path",
    }
}

#[derive(Clone, Debug)]
struct CorpusItem {
    item_ref: String,
    item_kind: String,
    created_at: Option<String>,
    line_count: usize,
    byte_count: usize,
}

impl CorpusItem {
    fn issue_body(issue_body: &IssueBody) -> Self {
        let body = issue_body.body.trim();
        Self {
            item_ref: String::from("issue-body"),
            item_kind: String::from("issue_body"),
            created_at: Some(issue_body.created_at.clone()),
            line_count: body.lines().count().max(1) + 2,
            byte_count: body.len(),
        }
    }

    fn comment(comment: &IssueComment) -> Self {
        let body = comment.body.trim();
        Self {
            item_ref: format!("comment-{}", comment.comment_id),
            item_kind: String::from("issue_comment"),
            created_at: Some(comment.created_at.clone()),
            line_count: body.lines().count().max(1) + 2,
            byte_count: body.len(),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct ChunkAccumulator {
    item_refs: Vec<String>,
    item_kinds: Vec<String>,
    line_count: usize,
    byte_count: usize,
    first_created_at: Option<String>,
    last_created_at: Option<String>,
}

impl ChunkAccumulator {
    fn push(&mut self, item: CorpusItem) {
        if self.first_created_at.is_none() {
            self.first_created_at = item.created_at.clone();
        }
        self.last_created_at = item.created_at.clone();
        self.line_count += item.line_count;
        self.byte_count += item.byte_count;
        self.item_refs.push(item.item_ref);
        self.item_kinds.push(item.item_kind);
    }

    fn finish(self, chunk_index: usize) -> IssueThreadChunkManifestEntry {
        IssueThreadChunkManifestEntry {
            chunk_index,
            item_refs: self.item_refs,
            item_kinds: self.item_kinds,
            line_count: self.line_count,
            byte_count: self.byte_count,
            first_created_at: self.first_created_at,
            last_created_at: self.last_created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use forge_policy::ExecutionPolicyBundle;
    use forge_rlm_core::{IssueBody, IssueComment, IssueThreadCorpus};
    use forge_runtime_protocol::{
        CorpusKind, CorpusLocator, ExecutionBudget, OutputSchema, RuntimeAssignment,
    };
    use tempfile::tempdir;

    use super::{
        ForgeRlmExecutionError, ForgeRlmExecutionPlan, ForgeRlmExecutionRequest,
        default_required_artifacts, execute_forge_rlm_plan, resolve_github_token,
    };

    #[test]
    fn local_issue_thread_plan_executes_and_writes_artifacts() {
        let tempdir = tempdir().expect("tempdir");
        let corpus_path = tempdir.path().join("corpus.json");
        let output_root = tempdir.path().join("out");
        std::fs::write(
            corpus_path.as_path(),
            serde_json::to_vec_pretty(&synthetic_issue_thread()).expect("serialize corpus"),
        )
        .expect("write corpus");

        let bundle = ExecutionPolicyBundle::issue_thread_eval_default();
        let plan = ForgeRlmExecutionPlan {
            assignment: RuntimeAssignment {
                assignment_id: String::from("forge-rlm-local-proof"),
                strategy_family: bundle.strategy_family,
                policy_bundle: bundle.policy_ref,
                corpus: CorpusLocator {
                    kind: CorpusKind::LocalPath,
                    storage_ref: corpus_path.to_string_lossy().into_owned(),
                    content_hash: None,
                    expected_item_count: Some(10),
                },
                budget: ExecutionBudget {
                    max_iterations: 24,
                    max_loaded_chunks: 8,
                    max_duration_seconds: 300,
                },
                output_schema: OutputSchema::IssueThreadAnalysisV1,
            },
            workspace_ref: Some(String::from("workspace://probe/test")),
            publication_label: Some(String::from("local-issue-thread-proof")),
            required_artifacts: default_required_artifacts(),
        };
        let request = ForgeRlmExecutionRequest {
            plan: plan.clone(),
            output_root,
            github_token: None,
            max_lines_per_chunk: 12,
        };

        let outcome = execute_forge_rlm_plan(request).expect("execution should succeed");
        assert_eq!(outcome.plan, plan);
        assert!(outcome.report.summary.passed);
        assert_eq!(
            outcome.runtime_result.assignment_id,
            "forge-rlm-local-proof"
        );
        assert_eq!(
            outcome.runtime_result.status,
            forge_runtime_protocol::ExecutionStatus::Succeeded
        );
        assert!(outcome.chunk_manifest.len() > 1);
        assert_eq!(outcome.event_log.len(), 5);
        assert!(PathBuf::from(&outcome.artifacts.report_path).exists());
        assert!(PathBuf::from(&outcome.artifacts.event_log_path).exists());
        assert!(PathBuf::from(&outcome.artifacts.runtime_result_path).exists());
        assert!(
            outcome
                .runtime_result
                .summary
                .as_deref()
                .unwrap_or_default()
                .contains("passed")
        );
    }

    #[test]
    fn truncated_local_issue_thread_fails_honestly() {
        let tempdir = tempdir().expect("tempdir");
        let corpus_path = tempdir.path().join("corpus.json");
        let output_root = tempdir.path().join("out");
        let mut corpus = synthetic_issue_thread();
        corpus.comment_count_from_metadata += 1;
        std::fs::write(
            corpus_path.as_path(),
            serde_json::to_vec_pretty(&corpus).expect("serialize corpus"),
        )
        .expect("write corpus");

        let bundle = ExecutionPolicyBundle::issue_thread_eval_default();
        let request = ForgeRlmExecutionRequest {
            plan: ForgeRlmExecutionPlan {
                assignment: RuntimeAssignment {
                    assignment_id: String::from("forge-rlm-truncated"),
                    strategy_family: bundle.strategy_family,
                    policy_bundle: bundle.policy_ref,
                    corpus: CorpusLocator {
                        kind: CorpusKind::LocalPath,
                        storage_ref: corpus_path.to_string_lossy().into_owned(),
                        content_hash: None,
                        expected_item_count: None,
                    },
                    budget: ExecutionBudget {
                        max_iterations: 24,
                        max_loaded_chunks: 8,
                        max_duration_seconds: 300,
                    },
                    output_schema: OutputSchema::IssueThreadAnalysisV1,
                },
                workspace_ref: None,
                publication_label: None,
                required_artifacts: default_required_artifacts(),
            },
            output_root,
            github_token: None,
            max_lines_per_chunk: 24,
        };

        let error = execute_forge_rlm_plan(request).expect_err("truncated corpus should fail");
        assert!(matches!(error, ForgeRlmExecutionError::Eval(_)));
        assert!(error.to_string().contains("comment"));
    }

    #[test]
    fn chunk_budget_failures_are_explicit() {
        let tempdir = tempdir().expect("tempdir");
        let corpus_path = tempdir.path().join("corpus.json");
        std::fs::write(
            corpus_path.as_path(),
            serde_json::to_vec_pretty(&synthetic_issue_thread()).expect("serialize corpus"),
        )
        .expect("write corpus");

        let bundle = ExecutionPolicyBundle::issue_thread_eval_default();
        let request = ForgeRlmExecutionRequest {
            plan: ForgeRlmExecutionPlan {
                assignment: RuntimeAssignment {
                    assignment_id: String::from("forge-rlm-budget"),
                    strategy_family: bundle.strategy_family,
                    policy_bundle: bundle.policy_ref,
                    corpus: CorpusLocator {
                        kind: CorpusKind::LocalPath,
                        storage_ref: corpus_path.to_string_lossy().into_owned(),
                        content_hash: None,
                        expected_item_count: Some(10),
                    },
                    budget: ExecutionBudget {
                        max_iterations: 24,
                        max_loaded_chunks: 1,
                        max_duration_seconds: 300,
                    },
                    output_schema: OutputSchema::IssueThreadAnalysisV1,
                },
                workspace_ref: None,
                publication_label: None,
                required_artifacts: default_required_artifacts(),
            },
            output_root: tempdir.path().join("out"),
            github_token: None,
            max_lines_per_chunk: 8,
        };

        let error = execute_forge_rlm_plan(request).expect_err("budget failure expected");
        assert!(matches!(
            error,
            ForgeRlmExecutionError::InvalidChunkBudget { .. }
        ));
        assert!(error.to_string().contains("chunk_count"));
    }

    #[test]
    fn proof_plan_defaults_to_live_openagents_issue() {
        let plan = ForgeRlmExecutionPlan::openagents_4368_issue_thread_proof();
        assert_eq!(plan.assignment.assignment_id, "forge-rlm-openagents-4368");
        assert_eq!(plan.assignment.corpus.kind, CorpusKind::GithubIssueThread);
        assert_eq!(
            plan.assignment.corpus.storage_ref,
            "github://OpenAgentsInc/openagents/issues/4368"
        );
        assert_eq!(plan.required_artifacts, default_required_artifacts());
    }

    #[test]
    #[ignore = "requires GH_TOKEN or GITHUB_TOKEN and live GitHub access"]
    fn live_openagents_4368_plan_executes_full_thread() {
        let github_token = resolve_github_token();
        assert!(github_token.is_some(), "set GH_TOKEN or GITHUB_TOKEN");

        let tempdir = tempdir().expect("tempdir");
        let request = ForgeRlmExecutionRequest {
            plan: ForgeRlmExecutionPlan::openagents_4368_issue_thread_proof(),
            output_root: tempdir.path().join("out"),
            github_token,
            max_lines_per_chunk: 48,
        };

        let outcome = execute_forge_rlm_plan(request).expect("live issue proof should execute");
        assert!(outcome.report.summary.passed);
        assert!(outcome.report.corpus_stats.total_items > 130);
        assert!(outcome.chunk_manifest.len() <= 128);
        assert!(outcome.runtime_result.summary.is_some());
        assert!(
            outcome
                .report
                .analysis
                .current_remaining_gap
                .contains("rewarded")
        );
        assert!(PathBuf::from(outcome.artifacts.report_path).exists());
    }

    fn synthetic_issue_thread() -> IssueThreadCorpus {
        IssueThreadCorpus {
            repository_owner: String::from("OpenAgentsInc"),
            repository_name: String::from("openagents"),
            issue_number: 4368,
            issue_title: String::from("Live payout proof"),
            issue_state: String::from("open"),
            issue_url: String::from("https://github.com/OpenAgentsInc/openagents/issues/4368"),
            issue_body: IssueBody {
                author: String::from("AtlantisPleb"),
                created_at: String::from("2026-04-17T00:00:00Z"),
                body: String::from(
                    "### Objective\n\nImplement an end-to-end launch lane that allows us to trigger a homework round via authenticated API, target **all updated online pylons**, assign them the homework workload, collect verifiable completions, and release Bitcoin payouts only for accepted completions.\n",
                ),
            },
            comment_count_from_metadata: 9,
            comments: vec![
                IssueComment {
                    comment_id: 1,
                    author: String::from("AtlantisPleb"),
                    edited: false,
                    created_at: String::from("2026-04-17T01:00:00Z"),
                    minimized: false,
                    body: String::from(
                        "I’m fixing the launch contract so that a fresh homework launch does not stop at run creation. The new path will materialize the first window, persist the assignment plan, and return the matched/assigned pylons directly in the admin response.",
                    ),
                },
                IssueComment {
                    comment_id: 2,
                    author: String::from("AtlantisPleb"),
                    edited: false,
                    created_at: String::from("2026-04-17T02:00:00Z"),
                    minimized: false,
                    body: String::from(
                        "The focused homework integration test now makes it all the way through checkpoint publish, seal, reconcile, and payout generation.",
                    ),
                },
                IssueComment {
                    comment_id: 3,
                    author: String::from("AtlantisPleb"),
                    edited: false,
                    created_at: String::from("2026-04-17T03:00:00Z"),
                    minimized: false,
                    body: String::from(
                        "The nested failure is checkpoint_manifest.json upload to storage.googleapis.com through the production signed URL.",
                    ),
                },
                IssueComment {
                    comment_id: 4,
                    author: String::from("AtlantisPleb"),
                    edited: false,
                    created_at: String::from("2026-04-17T04:00:00Z"),
                    minimized: false,
                    body: String::from(
                        "Worker-side retained closeout is still real and the transport path now replays the authority-side terminal closeout cleanly.",
                    ),
                },
                IssueComment {
                    comment_id: 5,
                    author: String::from("AtlantisPleb"),
                    edited: false,
                    created_at: String::from("2026-04-17T05:00:00Z"),
                    minimized: false,
                    body: String::from(
                        "Another real gap from the codebase itself:\n- `POST /api/training/assignments/ack`",
                    ),
                },
                IssueComment {
                    comment_id: 6,
                    author: String::from("AtlantisPleb"),
                    edited: false,
                    created_at: String::from("2026-04-17T06:00:00Z"),
                    minimized: false,
                    body: String::from(
                        "Found the treasury-side dispatch bug. prepare_due_payouts() currently returns early when config.treasury.payout_sats_per_window == 0, which blocks dispatch of already-queued accepted-work payouts.",
                    ),
                },
                IssueComment {
                    comment_id: 7,
                    author: String::from("AtlantisPleb"),
                    edited: false,
                    created_at: String::from("2026-04-17T07:00:00Z"),
                    minimized: false,
                    body: String::from(
                        "Comprehensive closeout dossier for the exact 4368 proof lane.\n`reconcile_training_window` was persisting / snapshotting full compute-authority state twice inside the replayed terminal closeout path.\n- `f97b97b` replay-safe idempotency for reconciled refused closeouts from retained artifacts\n- `bf7d2bb` deferred/batched projection for replayed terminal closeout reconcile so the replay path does one terminal projection flush instead of persisting the full authority state twice inside the same POST\n- The authority-side seam on main was the reconciled-closeout replacement path. That was patched in `2270be4646e46438263058b1e0fa41deebdf6921` and is live-proven enough to stay on the lane.",
                    ),
                },
                IssueComment {
                    comment_id: 8,
                    author: String::from("AtlantisPleb"),
                    edited: false,
                    created_at: String::from("2026-04-17T08:00:00Z"),
                    minimized: false,
                    body: String::from(
                        "Closing because the retained completed-worker proof lane now reaches an authoritative terminal result instead of stalling in post-execution publication.",
                    ),
                },
                IssueComment {
                    comment_id: 9,
                    author: String::from("AtlantisPleb"),
                    edited: false,
                    created_at: String::from("2026-04-17T09:00:00Z"),
                    minimized: false,
                    body: String::from(
                        "Reopening because the prior close was too narrow relative to the original issue contract.\nThe current audit shows that live homework execution is real, autonomous closeout exists in code/tests, and the post-execution transport stall is fixed, but a fresh live homework run still has not been proven to reach `rewarded` / `payout_eligible=true`, queue accepted-work payouts, dispatch real Lightning sends, and confirm payout receipts for contributing Pylons.",
                    ),
                },
            ],
        }
    }
}
