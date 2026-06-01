use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use probe_protocol::codex_workroom::redact_codex_workroom_text;
use probe_protocol::session::{
    CacheSignal, SessionId, SessionMetadata, ToolPolicyDecision, TranscriptEvent,
    TranscriptItemKind,
};
use probe_protocol::signature_context::{
    SessionSignatureContext, SignatureAdoptionState, SignatureRef, SignatureSelectionScore,
    SignatureSelectorMode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::session_store::{FilesystemSessionStore, SessionStoreError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatasetKind {
    Replay,
    Decision,
    DecisionCases,
    SignatureCases,
}

impl DatasetKind {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "replay" => Ok(Self::Replay),
            "decision" => Ok(Self::Decision),
            "decision-cases" | "decision_cases" => Ok(Self::DecisionCases),
            "signature-cases" | "signature_cases" => Ok(Self::SignatureCases),
            other => Err(format!(
                "unknown dataset kind `{other}`; expected `replay`, `decision`, `decision-cases`, or `signature-cases`"
            )),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Replay => "replay",
            Self::Decision => "decision",
            Self::DecisionCases => "decision-cases",
            Self::SignatureCases => "signature-cases",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DatasetExportConfig {
    pub kind: DatasetKind,
    pub output_path: PathBuf,
    pub session_ids: Vec<SessionId>,
    pub include_all_sessions: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatasetExportReport {
    pub kind: DatasetKind,
    pub output_path: PathBuf,
    pub sessions_exported: usize,
    pub cases_exported: usize,
}

#[derive(Debug)]
pub enum DatasetExportError {
    SessionStore(SessionStoreError),
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl Display for DatasetExportError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionStore(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
        }
    }
}

impl std::error::Error for DatasetExportError {}

impl From<SessionStoreError> for DatasetExportError {
    fn from(value: SessionStoreError) -> Self {
        Self::SessionStore(value)
    }
}

impl From<std::io::Error> for DatasetExportError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for DatasetExportError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Clone, Debug, Serialize)]
struct ReplayDatasetRecord {
    session_id: String,
    title: String,
    cwd: String,
    backend_profile: Option<String>,
    backend_model: Option<String>,
    harness_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signature_selection: Option<SignatureSelectionExportRefs>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    managed_usage_events: Vec<ManagedUsageEventExportRef>,
    turn_count: usize,
    transcript: Vec<TranscriptEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureSelectionExportRefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_id: Option<String>,
    pub selected_signature_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_envelope_digest: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedUsageEventExportRef {
    pub event_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count_source: Option<String>,
    pub turn_index: u64,
    pub item_sequence: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionSessionSummary {
    pub session_id: String,
    pub title: String,
    pub cwd: String,
    pub backend_profile: Option<String>,
    pub harness_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_selection: Option<SignatureSelectionExportRefs>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub managed_usage_events: Vec<ManagedUsageEventExportRef>,
    pub turn_count: usize,
    pub first_tool_name: Option<String>,
    pub tool_names: Vec<String>,
    pub files_listed: Vec<String>,
    pub files_searched: Vec<String>,
    pub files_read: Vec<String>,
    pub patch_attempts: usize,
    pub successful_patch_attempts: usize,
    pub failed_patch_attempts: usize,
    pub verification_step_count: usize,
    pub verification_caught_problem: bool,
    pub too_many_turns: bool,
    pub auto_allowed_tool_calls: usize,
    pub approved_tool_calls: usize,
    pub refused_tool_calls: usize,
    pub paused_tool_calls: usize,
    pub oracle_calls: usize,
    pub long_context_calls: usize,
    pub repo_analysis_files: Vec<String>,
    pub likely_warm_turns: usize,
    pub cache_reuse_improved_latency: bool,
    pub cache_reuse_improved_throughput: bool,
    pub final_assistant_text: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionCaseFamily {
    ToolRoute,
    PatchReadiness,
    LongContextEscalation,
}

impl DecisionCaseFamily {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolRoute => "tool_route",
            Self::PatchReadiness => "patch_readiness",
            Self::LongContextEscalation => "long_context_escalation",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionCaseSplit {
    Train,
    Validation,
}

impl DecisionCaseSplit {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Train => "train",
            Self::Validation => "validation",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionCaseTranscriptRef {
    pub turn_index: u64,
    pub item_sequence: u32,
    pub item_kind: TranscriptItemKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRouteDecisionCaseContext {
    pub files_listed: usize,
    pub files_searched: usize,
    pub files_read: usize,
    pub patch_attempts: usize,
    pub verification_step_count: usize,
    pub refused_or_paused_tool_calls: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchReadinessDecisionCaseContext {
    pub files_listed: usize,
    pub files_searched: usize,
    pub files_read: usize,
    pub patch_attempts: usize,
    pub verification_step_count: usize,
    pub refused_or_paused_tool_calls: usize,
    pub too_many_turns: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LongContextDecisionCaseContext {
    pub prompt_char_count: usize,
    pub files_listed: usize,
    pub files_searched: usize,
    pub files_read: usize,
    pub too_many_turns: bool,
    pub oracle_calls: usize,
    pub long_context_calls: usize,
    pub requested_task_kind: String,
    pub requested_evidence_files: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "family", rename_all = "snake_case")]
pub enum DecisionCaseContext {
    ToolRoute(ToolRouteDecisionCaseContext),
    PatchReadiness(PatchReadinessDecisionCaseContext),
    LongContextEscalation(LongContextDecisionCaseContext),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRouteObservedLabel {
    pub selected_tool: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchReadinessObservedLabel {
    pub should_patch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_tool: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LongContextObservedLabel {
    pub should_escalate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_tool: Option<String>,
    pub requested_task_kind: String,
    pub requested_evidence_files: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "family", rename_all = "snake_case")]
pub enum DecisionCaseObservedLabel {
    ToolRoute(ToolRouteObservedLabel),
    PatchReadiness(PatchReadinessObservedLabel),
    LongContextEscalation(LongContextObservedLabel),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionCaseRecord {
    pub case_id: String,
    pub stable_digest: String,
    pub family: DecisionCaseFamily,
    pub split: DecisionCaseSplit,
    pub session_id: String,
    pub title: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_selection: Option<SignatureSelectionExportRefs>,
    pub source_transcript_path: String,
    pub turn_index: u64,
    pub context: DecisionCaseContext,
    pub observed_label: DecisionCaseObservedLabel,
    pub transcript_refs: Vec<DecisionCaseTranscriptRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionCaseFamilySplitCounts {
    pub family: DecisionCaseFamily,
    pub total_cases: usize,
    pub train_cases: usize,
    pub validation_cases: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionCaseSplitManifest {
    pub schema_version: u16,
    pub report_id: String,
    pub output_dir: String,
    pub total_sessions: usize,
    pub total_cases: usize,
    pub train_cases: usize,
    pub validation_cases: usize,
    pub families: Vec<DecisionCaseFamilySplitCounts>,
    pub all_cases_path: String,
    pub train_cases_path: String,
    pub validation_cases_path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureCaseResultStatus {
    Completed,
    Failed,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureVerifierOutcome {
    NotObserved,
    Passed,
    Failed,
    Error,
    Timeout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureOutcomeLabel {
    Unknown,
    Helped,
    Hurt,
    Irrelevant,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureCaseSelection {
    pub signature_id: String,
    pub signature_version: String,
    pub adoption_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_bps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureToolPolicySnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_tool_set: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_tool_choice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_tool_choice: Option<String>,
    pub forbidden_tools: Vec<String>,
    pub auto_allowed_tool_calls: usize,
    pub approved_tool_calls: usize,
    pub refused_tool_calls: usize,
    pub paused_tool_calls: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureCaseResult {
    pub status: SignatureCaseResultStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_type: Option<String>,
    pub verifier_outcome: SignatureVerifierOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_assistant_text_hash: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureSelectionCaseRecord {
    pub schema_version: u16,
    pub case_id: String,
    pub stable_digest: String,
    pub split: DecisionCaseSplit,
    pub session_id: String,
    pub title: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_profile: Option<String>,
    pub source_transcript_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_envelope_digest: Option<String>,
    pub signature: SignatureCaseSelection,
    pub selected_signature_ids: Vec<String>,
    pub runner_up_signatures: Vec<SignatureCaseSelection>,
    pub tool_policy: SignatureToolPolicySnapshot,
    pub result: SignatureCaseResult,
    pub outcome_label: SignatureOutcomeLabel,
    pub transcript_refs: Vec<DecisionCaseTranscriptRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureCaseManifest {
    pub schema_version: u16,
    pub report_id: String,
    pub output_dir: String,
    pub total_sessions: usize,
    pub total_cases: usize,
    pub train_cases: usize,
    pub validation_cases: usize,
    pub all_cases_path: String,
    pub validation_cases_path: String,
    pub training_policy: String,
}

pub fn export_dataset(
    session_store: &FilesystemSessionStore,
    config: &DatasetExportConfig,
) -> Result<DatasetExportReport, DatasetExportError> {
    let sessions = session_store.list_sessions()?;
    let mut sessions_exported = 0_usize;
    let mut decision_cases = Vec::new();
    let mut signature_cases = Vec::new();

    if !matches!(
        config.kind,
        DatasetKind::DecisionCases | DatasetKind::SignatureCases
    ) {
        if let Some(parent) = config.output_path.parent() {
            fs::create_dir_all(parent)?;
        }
    }
    let mut writer = if matches!(
        config.kind,
        DatasetKind::DecisionCases | DatasetKind::SignatureCases
    ) {
        None
    } else {
        let file = File::create(&config.output_path)?;
        Some(BufWriter::new(file))
    };

    for metadata in sessions {
        let transcript = session_store.read_transcript(&metadata.id)?;
        if !should_export_session(&metadata, transcript.as_slice(), config) {
            continue;
        }
        match config.kind {
            DatasetKind::Replay => {
                let record = ReplayDatasetRecord {
                    session_id: metadata.id.as_str().to_string(),
                    title: metadata.title.clone(),
                    cwd: metadata.cwd.display().to_string(),
                    backend_profile: metadata
                        .backend
                        .as_ref()
                        .map(|backend| backend.profile_name.clone()),
                    backend_model: metadata
                        .backend
                        .as_ref()
                        .map(|backend| backend.model.clone()),
                    harness_profile: metadata
                        .harness_profile
                        .as_ref()
                        .map(|profile| format!("{}@{}", profile.name, profile.version)),
                    signature_selection: signature_selection_refs(&metadata),
                    managed_usage_events: managed_usage_event_refs(transcript.as_slice()),
                    turn_count: transcript.len(),
                    transcript,
                };
                serde_json::to_writer(writer.as_mut().expect("writer must exist"), &record)?;
            }
            DatasetKind::Decision => {
                let record = build_decision_summary(&metadata, transcript.as_slice());
                serde_json::to_writer(writer.as_mut().expect("writer must exist"), &record)?;
            }
            DatasetKind::DecisionCases => {
                decision_cases.extend(build_decision_cases(&metadata, transcript.as_slice()));
            }
            DatasetKind::SignatureCases => {
                signature_cases.extend(build_signature_selection_cases(
                    &metadata,
                    transcript.as_slice(),
                ));
            }
        }
        if let Some(writer) = writer.as_mut() {
            writer.write_all(b"\n")?;
        }
        sessions_exported += 1;
    }
    if let Some(writer) = writer.as_mut() {
        writer.flush()?;
    }
    if config.kind == DatasetKind::DecisionCases {
        write_decision_case_bundle(config.output_path.as_path(), decision_cases.as_slice())?;
    }
    if config.kind == DatasetKind::SignatureCases {
        write_signature_case_bundle(config.output_path.as_path(), signature_cases.as_slice())?;
    }

    Ok(DatasetExportReport {
        kind: config.kind,
        output_path: config.output_path.clone(),
        sessions_exported,
        cases_exported: match config.kind {
            DatasetKind::DecisionCases => decision_cases.len(),
            DatasetKind::SignatureCases => signature_cases.len(),
            DatasetKind::Replay | DatasetKind::Decision => 0,
        },
    })
}

fn should_export_session(
    metadata: &SessionMetadata,
    transcript: &[TranscriptEvent],
    config: &DatasetExportConfig,
) -> bool {
    if !config.session_ids.is_empty() {
        return config
            .session_ids
            .iter()
            .any(|session_id| session_id == &metadata.id);
    }
    if config.include_all_sessions {
        return true;
    }

    metadata
        .signature_context
        .as_ref()
        .is_some_and(|context| !context.signature_pack.entries.is_empty())
        || metadata
            .harness_profile
            .as_ref()
            .map(|profile| profile.name.starts_with("coding_bootstrap"))
            .unwrap_or(false)
        || transcript
            .iter()
            .flat_map(|event| event.turn.items.iter())
            .any(|item| {
                matches!(
                    item.name.as_deref(),
                    Some("read_file" | "list_files" | "code_search" | "shell" | "apply_patch")
                )
            })
}

pub fn build_decision_summary(
    metadata: &SessionMetadata,
    transcript: &[TranscriptEvent],
) -> DecisionSessionSummary {
    let mut first_tool_name = None;
    let mut tool_names = Vec::new();
    let mut files_listed = Vec::new();
    let mut files_searched = Vec::new();
    let mut files_read = Vec::new();
    let mut patch_attempts = 0_usize;
    let mut successful_patch_attempts = 0_usize;
    let mut failed_patch_attempts = 0_usize;
    let mut verification_step_count = 0_usize;
    let mut verification_caught_problem = false;
    let mut too_many_turns = false;
    let mut auto_allowed_tool_calls = 0_usize;
    let mut approved_tool_calls = 0_usize;
    let mut refused_tool_calls = 0_usize;
    let mut paused_tool_calls = 0_usize;
    let mut oracle_calls = 0_usize;
    let mut long_context_calls = 0_usize;
    let mut repo_analysis_files = Vec::new();
    let mut likely_warm_turns = 0_usize;
    let mut cache_reuse_improved_throughput = false;
    let mut previous_tps = None;
    let mut final_assistant_text = None;
    let mut patch_needs_verification = false;
    let mut saw_verification_after_patch = false;

    for event in transcript {
        if let Some(observability) = event.turn.observability.as_ref() {
            if observability.cache_signal == CacheSignal::LikelyWarm {
                likely_warm_turns += 1;
                if let (Some(previous), Some(current)) = (
                    previous_tps,
                    observability.completion_tokens_per_second_x1000,
                ) && current > previous
                {
                    cache_reuse_improved_throughput = true;
                }
            }
            if let Some(current_tps) = observability.completion_tokens_per_second_x1000 {
                previous_tps = Some(current_tps);
            }
        }

        for item in &event.turn.items {
            match item.kind {
                TranscriptItemKind::ToolCall => {
                    if first_tool_name.is_none() {
                        first_tool_name = item.name.clone();
                    }
                }
                TranscriptItemKind::AssistantMessage => {
                    final_assistant_text = Some(item.text.clone());
                }
                TranscriptItemKind::Note => {
                    if item
                        .text
                        .contains("exceeded the configured tool loop bound")
                    {
                        too_many_turns = true;
                    }
                }
                TranscriptItemKind::ToolResult => {
                    let Some(tool_name) = item.name.as_deref() else {
                        continue;
                    };
                    push_unique(&mut tool_names, tool_name.to_string());
                    if let Some(tool_execution) = item.tool_execution.as_ref() {
                        match tool_execution.policy_decision {
                            ToolPolicyDecision::AutoAllow => auto_allowed_tool_calls += 1,
                            ToolPolicyDecision::Approved => approved_tool_calls += 1,
                            ToolPolicyDecision::Refused => refused_tool_calls += 1,
                            ToolPolicyDecision::Paused => paused_tool_calls += 1,
                        }
                    }
                    match tool_name {
                        "list_files" => {
                            if let Some(path) = tool_result_path(item) {
                                push_unique(&mut files_listed, path);
                            }
                        }
                        "code_search" => {
                            if let Some(tool_execution) = item.tool_execution.as_ref() {
                                for path in &tool_execution.files_touched {
                                    push_unique(&mut files_searched, path.clone());
                                }
                            }
                        }
                        "read_file" => {
                            if let Some(tool_execution) = item.tool_execution.as_ref() {
                                for path in &tool_execution.files_touched {
                                    push_unique(&mut files_read, path.clone());
                                }
                            }
                            if patch_needs_verification {
                                verification_step_count += 1;
                                saw_verification_after_patch = true;
                                if tool_result_has_error(item) {
                                    verification_caught_problem = true;
                                }
                            }
                        }
                        "shell" => {
                            if patch_needs_verification {
                                verification_step_count += 1;
                                saw_verification_after_patch = true;
                                if tool_result_has_error(item) {
                                    verification_caught_problem = true;
                                }
                            }
                        }
                        "apply_patch" => {
                            if patch_needs_verification && saw_verification_after_patch {
                                verification_caught_problem = true;
                            }
                            patch_attempts += 1;
                            if tool_result_succeeded(item) {
                                successful_patch_attempts += 1;
                            } else {
                                failed_patch_attempts += 1;
                            }
                            patch_needs_verification = true;
                            saw_verification_after_patch = false;
                        }
                        "consult_oracle" => {
                            oracle_calls += 1;
                        }
                        "analyze_repository" => {
                            long_context_calls += 1;
                            if let Some(tool_execution) = item.tool_execution.as_ref() {
                                for path in &tool_execution.files_touched {
                                    push_unique(&mut repo_analysis_files, path.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                TranscriptItemKind::UserMessage => {}
            }
        }
    }

    DecisionSessionSummary {
        session_id: metadata.id.as_str().to_string(),
        title: metadata.title.clone(),
        cwd: metadata.cwd.display().to_string(),
        backend_profile: metadata
            .backend
            .as_ref()
            .map(|backend| backend.profile_name.clone()),
        harness_profile: metadata
            .harness_profile
            .as_ref()
            .map(|profile| format!("{}@{}", profile.name, profile.version)),
        signature_selection: signature_selection_refs(metadata),
        managed_usage_events: managed_usage_event_refs(transcript),
        turn_count: transcript.len(),
        first_tool_name,
        tool_names,
        files_listed,
        files_searched,
        files_read,
        patch_attempts,
        successful_patch_attempts,
        failed_patch_attempts,
        verification_step_count,
        verification_caught_problem,
        too_many_turns,
        auto_allowed_tool_calls,
        approved_tool_calls,
        refused_tool_calls,
        paused_tool_calls,
        oracle_calls,
        long_context_calls,
        repo_analysis_files,
        likely_warm_turns,
        cache_reuse_improved_latency: likely_warm_turns > 0,
        cache_reuse_improved_throughput,
        final_assistant_text,
    }
}

pub fn build_decision_cases(
    metadata: &SessionMetadata,
    transcript: &[TranscriptEvent],
) -> Vec<DecisionCaseRecord> {
    let mut cases = Vec::new();

    for (turn_position, event) in transcript.iter().enumerate() {
        let Some(tool_call) = first_named_tool_call(event) else {
            continue;
        };
        let pre_turn_summary = build_decision_summary(metadata, &transcript[..turn_position]);
        let transcript_refs = event
            .turn
            .items
            .iter()
            .map(|item| DecisionCaseTranscriptRef {
                turn_index: event.turn.index,
                item_sequence: item.sequence,
                item_kind: item.kind,
                item_name: item.name.clone(),
                tool_call_id: item.tool_call_id.clone(),
            })
            .collect::<Vec<_>>();
        let selected_tool = tool_call
            .name
            .clone()
            .expect("named tool call must carry a tool name");

        cases.push(build_decision_case_record(
            DecisionCaseFamily::ToolRoute,
            metadata,
            event.turn.index,
            DecisionCaseContext::ToolRoute(ToolRouteDecisionCaseContext {
                files_listed: pre_turn_summary.files_listed.len(),
                files_searched: pre_turn_summary.files_searched.len(),
                files_read: pre_turn_summary.files_read.len(),
                patch_attempts: pre_turn_summary.patch_attempts,
                verification_step_count: pre_turn_summary.verification_step_count,
                refused_or_paused_tool_calls: pre_turn_summary.refused_tool_calls
                    + pre_turn_summary.paused_tool_calls,
            }),
            DecisionCaseObservedLabel::ToolRoute(ToolRouteObservedLabel {
                selected_tool: selected_tool.clone(),
            }),
            transcript_refs.clone(),
            tool_call.tool_call_id.as_deref(),
        ));

        cases.push(build_decision_case_record(
            DecisionCaseFamily::PatchReadiness,
            metadata,
            event.turn.index,
            DecisionCaseContext::PatchReadiness(PatchReadinessDecisionCaseContext {
                files_listed: pre_turn_summary.files_listed.len(),
                files_searched: pre_turn_summary.files_searched.len(),
                files_read: pre_turn_summary.files_read.len(),
                patch_attempts: pre_turn_summary.patch_attempts,
                verification_step_count: pre_turn_summary.verification_step_count,
                refused_or_paused_tool_calls: pre_turn_summary.refused_tool_calls
                    + pre_turn_summary.paused_tool_calls,
                too_many_turns: pre_turn_summary.too_many_turns,
            }),
            DecisionCaseObservedLabel::PatchReadiness(PatchReadinessObservedLabel {
                should_patch: selected_tool == "apply_patch",
                selected_tool: Some(selected_tool.clone()),
            }),
            transcript_refs.clone(),
            tool_call.tool_call_id.as_deref(),
        ));

        let requested_task_kind = requested_task_kind_from_tool_call(tool_call)
            .unwrap_or_else(|| String::from("change_impact"));
        let requested_evidence_files = requested_evidence_files_from_tool_call(tool_call)
            .unwrap_or_else(|| {
                pre_turn_summary
                    .repo_analysis_files
                    .len()
                    .max(pre_turn_summary.files_read.len())
                    .max(pre_turn_summary.files_searched.len())
            });
        cases.push(build_decision_case_record(
            DecisionCaseFamily::LongContextEscalation,
            metadata,
            event.turn.index,
            DecisionCaseContext::LongContextEscalation(LongContextDecisionCaseContext {
                prompt_char_count: sum_user_message_chars(event),
                files_listed: pre_turn_summary.files_listed.len(),
                files_searched: pre_turn_summary.files_searched.len(),
                files_read: pre_turn_summary.files_read.len(),
                too_many_turns: pre_turn_summary.too_many_turns,
                oracle_calls: pre_turn_summary.oracle_calls,
                long_context_calls: pre_turn_summary.long_context_calls,
                requested_task_kind: requested_task_kind.clone(),
                requested_evidence_files,
            }),
            DecisionCaseObservedLabel::LongContextEscalation(LongContextObservedLabel {
                should_escalate: selected_tool == "analyze_repository",
                selected_tool: Some(selected_tool),
                requested_task_kind,
                requested_evidence_files,
            }),
            transcript_refs,
            tool_call.tool_call_id.as_deref(),
        ));
    }

    cases
}

pub fn build_signature_selection_cases(
    metadata: &SessionMetadata,
    transcript: &[TranscriptEvent],
) -> Vec<SignatureSelectionCaseRecord> {
    let Some(signature_context) = metadata.signature_context.as_ref() else {
        return Vec::new();
    };
    if signature_context.signature_pack.entries.is_empty() {
        return Vec::new();
    }

    let summary = build_decision_summary(metadata, transcript);
    let result = signature_case_result(&summary, transcript);
    let decision = signature_context.selection_decision.as_ref();
    let selected_scores = signature_score_index(
        decision
            .map(|decision| decision.selected_signatures.as_slice())
            .unwrap_or(&[]),
    );
    let selected_signature_ids = signature_context
        .signature_pack
        .entries
        .iter()
        .map(|entry| entry.signature.id.clone())
        .collect::<Vec<_>>();
    let runner_up_signatures = decision
        .map(|decision| {
            decision
                .runner_up_signatures
                .iter()
                .map(|score| signature_selection_from_ref(&score.signature, Some(score)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let transcript_refs = signature_case_transcript_refs(transcript);
    let tool_policy = signature_tool_policy_snapshot(signature_context, &summary, transcript);

    signature_context
        .signature_pack
        .entries
        .iter()
        .map(|entry| {
            let score = selected_scores.get(&signature_score_key(
                entry.signature.id.as_str(),
                entry.signature.version.as_str(),
            ));
            let decision_id = decision.map(|decision| decision.decision_id.clone());
            let case_id = format!(
                "signature_selection:{}:{}:{}:{}",
                metadata.id.as_str(),
                decision_id.as_deref().unwrap_or("none"),
                entry.signature.id,
                entry.signature.version
            );
            let mut record = SignatureSelectionCaseRecord {
                schema_version: 1,
                case_id,
                stable_digest: String::new(),
                // Signature outcomes are validation-only until an explicit
                // promotion job admits them into a training set.
                split: DecisionCaseSplit::Validation,
                session_id: metadata.id.as_str().to_string(),
                title: metadata.title.clone(),
                cwd: metadata.cwd.display().to_string(),
                backend_profile: metadata
                    .backend
                    .as_ref()
                    .map(|backend| backend.profile_name.clone()),
                harness_profile: metadata
                    .harness_profile
                    .as_ref()
                    .map(|profile| format!("{}@{}", profile.name, profile.version)),
                source_transcript_path: metadata.transcript_path.display().to_string(),
                pack_id: signature_context.signature_pack.pack_id.clone(),
                decision_id,
                selector_mode: decision.map(|decision| {
                    signature_selector_mode_label(decision.selector_mode).to_string()
                }),
                task_envelope_digest: decision
                    .and_then(|decision| decision.task_envelope_digest.clone()),
                signature: signature_selection_from_ref(&entry.signature, score.copied()),
                selected_signature_ids: selected_signature_ids.clone(),
                runner_up_signatures: runner_up_signatures.clone(),
                tool_policy: tool_policy.clone(),
                result: result.clone(),
                outcome_label: SignatureOutcomeLabel::Unknown,
                transcript_refs: transcript_refs.clone(),
            };
            record.stable_digest = signature_case_digest(&record);
            record
        })
        .collect()
}

fn write_decision_case_bundle(
    output_dir: &std::path::Path,
    cases: &[DecisionCaseRecord],
) -> Result<(), DatasetExportError> {
    fs::create_dir_all(output_dir)?;
    let all_cases_path = output_dir.join("decision_cases_all.jsonl");
    let train_cases_path = output_dir.join("decision_cases_train.jsonl");
    let validation_cases_path = output_dir.join("decision_cases_val.jsonl");
    let split_manifest_path = output_dir.join("decision_case_split_manifest.json");

    write_jsonl_records(all_cases_path.as_path(), cases)?;
    write_jsonl_records(
        train_cases_path.as_path(),
        &cases
            .iter()
            .filter(|case| case.split == DecisionCaseSplit::Train)
            .collect::<Vec<_>>(),
    )?;
    write_jsonl_records(
        validation_cases_path.as_path(),
        &cases
            .iter()
            .filter(|case| case.split == DecisionCaseSplit::Validation)
            .collect::<Vec<_>>(),
    )?;

    let families = [
        DecisionCaseFamily::ToolRoute,
        DecisionCaseFamily::PatchReadiness,
        DecisionCaseFamily::LongContextEscalation,
    ]
    .into_iter()
    .map(|family| DecisionCaseFamilySplitCounts {
        family,
        total_cases: cases.iter().filter(|case| case.family == family).count(),
        train_cases: cases
            .iter()
            .filter(|case| case.family == family && case.split == DecisionCaseSplit::Train)
            .count(),
        validation_cases: cases
            .iter()
            .filter(|case| case.family == family && case.split == DecisionCaseSplit::Validation)
            .count(),
    })
    .collect::<Vec<_>>();

    let manifest = DecisionCaseSplitManifest {
        schema_version: 1,
        report_id: String::from("probe.decision_case_split_manifest.v1"),
        output_dir: output_dir.display().to_string(),
        total_sessions: cases
            .iter()
            .map(|case| case.session_id.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        total_cases: cases.len(),
        train_cases: cases
            .iter()
            .filter(|case| case.split == DecisionCaseSplit::Train)
            .count(),
        validation_cases: cases
            .iter()
            .filter(|case| case.split == DecisionCaseSplit::Validation)
            .count(),
        families,
        all_cases_path: all_cases_path.display().to_string(),
        train_cases_path: train_cases_path.display().to_string(),
        validation_cases_path: validation_cases_path.display().to_string(),
    };
    fs::write(
        split_manifest_path,
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;

    Ok(())
}

fn write_signature_case_bundle(
    output_dir: &std::path::Path,
    cases: &[SignatureSelectionCaseRecord],
) -> Result<(), DatasetExportError> {
    fs::create_dir_all(output_dir)?;
    let all_cases_path = output_dir.join("signature_cases_all.jsonl");
    let validation_cases_path = output_dir.join("signature_cases_val.jsonl");
    let manifest_path = output_dir.join("signature_case_manifest.json");

    write_jsonl_records(all_cases_path.as_path(), cases)?;
    write_jsonl_records(
        validation_cases_path.as_path(),
        &cases
            .iter()
            .filter(|case| case.split == DecisionCaseSplit::Validation)
            .collect::<Vec<_>>(),
    )?;

    let manifest = SignatureCaseManifest {
        schema_version: 1,
        report_id: String::from("probe.signature_case_manifest.v1"),
        output_dir: output_dir.display().to_string(),
        total_sessions: cases
            .iter()
            .map(|case| case.session_id.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        total_cases: cases.len(),
        train_cases: cases
            .iter()
            .filter(|case| case.split == DecisionCaseSplit::Train)
            .count(),
        validation_cases: cases
            .iter()
            .filter(|case| case.split == DecisionCaseSplit::Validation)
            .count(),
        all_cases_path: all_cases_path.display().to_string(),
        validation_cases_path: validation_cases_path.display().to_string(),
        training_policy: String::from(
            "validation_only_until_explicit_signature_promotion_admits_training_use",
        ),
    };
    fs::write(
        manifest_path,
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;

    Ok(())
}

fn write_jsonl_records<T: Serialize>(
    output_path: &std::path::Path,
    records: &[T],
) -> Result<(), DatasetExportError> {
    let file = File::create(output_path)?;
    let mut writer = BufWriter::new(file);
    for record in records {
        serde_json::to_writer(&mut writer, record)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(())
}

fn signature_score_index<'a>(
    scores: &'a [SignatureSelectionScore],
) -> BTreeMap<String, &'a SignatureSelectionScore> {
    scores
        .iter()
        .map(|score| {
            (
                signature_score_key(
                    score.signature.id.as_str(),
                    score.signature.version.as_str(),
                ),
                score,
            )
        })
        .collect()
}

fn signature_score_key(id: &str, version: &str) -> String {
    format!("{id}@{version}")
}

fn signature_selection_from_ref(
    signature: &SignatureRef,
    score: Option<&SignatureSelectionScore>,
) -> SignatureCaseSelection {
    SignatureCaseSelection {
        signature_id: signature.id.clone(),
        signature_version: signature.version.clone(),
        adoption_state: signature_adoption_state_label(signature.adoption_state).to_string(),
        source_ref: signature.source_ref.clone(),
        rank: score.map(|score| score.rank as u32),
        score_bps: score.map(|score| u32::from(score.score_bps)),
        reason_code: score.and_then(|score| score.reason_code.clone()),
    }
}

fn signature_tool_policy_snapshot(
    signature_context: &SessionSignatureContext,
    summary: &DecisionSessionSummary,
    transcript: &[TranscriptEvent],
) -> SignatureToolPolicySnapshot {
    let decision = signature_context.selection_decision.as_ref();
    SignatureToolPolicySnapshot {
        recommended_tool_set: decision.and_then(|decision| decision.recommended_tool_set.clone()),
        recommended_tool_choice: decision
            .and_then(|decision| decision.recommended_tool_choice.clone()),
        actual_tool_choice: first_note_field(transcript, "actual_tool_choice"),
        forbidden_tools: decision
            .map(|decision| decision.forbidden_tools.clone())
            .unwrap_or_else(|| {
                signature_context
                    .signature_pack
                    .entries
                    .iter()
                    .flat_map(|entry| entry.forbidden_tools.iter().cloned())
                    .fold(Vec::new(), |mut acc, tool| {
                        push_unique(&mut acc, tool);
                        acc
                    })
            }),
        auto_allowed_tool_calls: summary.auto_allowed_tool_calls,
        approved_tool_calls: summary.approved_tool_calls,
        refused_tool_calls: summary.refused_tool_calls,
        paused_tool_calls: summary.paused_tool_calls,
    }
}

fn signature_case_result(
    summary: &DecisionSessionSummary,
    transcript: &[TranscriptEvent],
) -> SignatureCaseResult {
    let failure_type = signature_failure_type(summary, transcript);
    let status = if failure_type.is_some() {
        SignatureCaseResultStatus::Failed
    } else if summary.final_assistant_text.is_some() {
        SignatureCaseResultStatus::Completed
    } else {
        SignatureCaseResultStatus::Unknown
    };
    SignatureCaseResult {
        status,
        failure_type,
        verifier_outcome: signature_verifier_outcome(transcript),
        final_assistant_text_hash: summary
            .final_assistant_text
            .as_deref()
            .map(stable_text_hash),
    }
}

fn signature_failure_type(
    summary: &DecisionSessionSummary,
    transcript: &[TranscriptEvent],
) -> Option<String> {
    if summary.too_many_turns {
        return Some(String::from("tool_round_trip_bound"));
    }
    if summary.refused_tool_calls > 0 {
        return Some(String::from("tool_refused"));
    }
    if summary.paused_tool_calls > 0 {
        return Some(String::from("tool_approval_paused"));
    }
    if summary.failed_patch_attempts > 0 && summary.successful_patch_attempts == 0 {
        return Some(String::from("patch_failed"));
    }
    if transcript
        .iter()
        .flat_map(|event| event.turn.items.iter())
        .any(|item| {
            item.kind == TranscriptItemKind::Note
                && item.text.to_ascii_lowercase().contains("provider request")
        })
    {
        return Some(String::from("provider_request_failed"));
    }
    if summary.final_assistant_text.is_none()
        && transcript
            .iter()
            .flat_map(|event| event.turn.items.iter())
            .any(|item| item.kind == TranscriptItemKind::Note)
    {
        return Some(String::from("runtime_note"));
    }
    None
}

fn signature_verifier_outcome(transcript: &[TranscriptEvent]) -> SignatureVerifierOutcome {
    match first_note_field(transcript, "verifier_outcome").as_deref() {
        Some("passed") => SignatureVerifierOutcome::Passed,
        Some("failed") => SignatureVerifierOutcome::Failed,
        Some("error") => SignatureVerifierOutcome::Error,
        Some("timeout") => SignatureVerifierOutcome::Timeout,
        _ => SignatureVerifierOutcome::NotObserved,
    }
}

fn signature_case_transcript_refs(
    transcript: &[TranscriptEvent],
) -> Vec<DecisionCaseTranscriptRef> {
    transcript
        .iter()
        .flat_map(|event| {
            event.turn.items.iter().filter_map(|item| {
                if matches!(
                    item.kind,
                    TranscriptItemKind::Note
                        | TranscriptItemKind::ToolResult
                        | TranscriptItemKind::AssistantMessage
                ) {
                    Some(DecisionCaseTranscriptRef {
                        turn_index: event.turn.index,
                        item_sequence: item.sequence,
                        item_kind: item.kind,
                        item_name: item.name.clone(),
                        tool_call_id: item.tool_call_id.clone(),
                    })
                } else {
                    None
                }
            })
        })
        .take(16)
        .collect()
}

fn first_note_field(transcript: &[TranscriptEvent], field: &str) -> Option<String> {
    let prefix = format!("{field}=");
    transcript
        .iter()
        .flat_map(|event| event.turn.items.iter())
        .filter(|item| item.kind == TranscriptItemKind::Note)
        .flat_map(|item| item.text.split_whitespace())
        .find_map(|part| part.strip_prefix(prefix.as_str()).map(String::from))
}

fn managed_usage_event_refs(transcript: &[TranscriptEvent]) -> Vec<ManagedUsageEventExportRef> {
    transcript
        .iter()
        .flat_map(|event| {
            event
                .turn
                .items
                .iter()
                .filter(|item| item.kind == TranscriptItemKind::Note)
                .filter_map(|item| {
                    let fields = note_fields(item.text.as_str());
                    let event_type = first_field(
                        &fields,
                        &["managed_event_type", "usage_event_type", "event_type"],
                    )?;
                    let event_type = normalize_managed_event_type(event_type);
                    if !is_managed_usage_event_type(event_type.as_str()) {
                        return None;
                    }
                    Some(ManagedUsageEventExportRef {
                        event_type,
                        event_ref: first_field(
                            &fields,
                            &["managed_event_ref", "usage_event_ref", "event_ref"],
                        )
                        .map(redact_codex_workroom_text),
                        receipt_digest: first_field(
                            &fields,
                            &["receipt_digest", "receipt_ref", "usage_receipt_digest"],
                        )
                        .map(redact_codex_workroom_text),
                        mode: first_field(&fields, &["mode", "usage_mode"])
                            .map(redact_codex_workroom_text),
                        count_source: first_field(&fields, &["count_source"])
                            .map(redact_codex_workroom_text),
                        turn_index: event.turn.index,
                        item_sequence: item.sequence,
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn note_fields(text: &str) -> BTreeMap<String, String> {
    text.split_whitespace()
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            Some((key.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect()
}

fn first_field<'a>(fields: &'a BTreeMap<String, String>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| fields.get(*key).map(String::as_str))
}

fn normalize_managed_event_type(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['-', '.'], "_")
}

fn is_managed_usage_event_type(value: &str) -> bool {
    matches!(
        value,
        "model_usage_reported" | "usage_unavailable" | "resource_usage_captured"
    )
}

fn stable_text_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"probe_signature_case_text|");
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

fn build_decision_case_record(
    family: DecisionCaseFamily,
    metadata: &SessionMetadata,
    turn_index: u64,
    context: DecisionCaseContext,
    observed_label: DecisionCaseObservedLabel,
    transcript_refs: Vec<DecisionCaseTranscriptRef>,
    tool_call_id: Option<&str>,
) -> DecisionCaseRecord {
    let case_id = format!(
        "{}:{}:{}:{}",
        family.as_str(),
        metadata.id.as_str(),
        turn_index,
        tool_call_id.unwrap_or("uncalled")
    );
    let digest = decision_case_digest(
        family,
        metadata,
        turn_index,
        &context,
        &observed_label,
        transcript_refs.as_slice(),
        tool_call_id,
    );
    let split = decision_case_split_from_digest(digest.as_str());

    DecisionCaseRecord {
        case_id,
        stable_digest: digest,
        family,
        split,
        session_id: metadata.id.as_str().to_string(),
        title: metadata.title.clone(),
        cwd: metadata.cwd.display().to_string(),
        backend_profile: metadata
            .backend
            .as_ref()
            .map(|backend| backend.profile_name.clone()),
        harness_profile: metadata
            .harness_profile
            .as_ref()
            .map(|profile| format!("{}@{}", profile.name, profile.version)),
        signature_selection: signature_selection_refs(metadata),
        source_transcript_path: metadata.transcript_path.display().to_string(),
        turn_index,
        context,
        observed_label,
        transcript_refs,
    }
}

fn signature_selection_refs(metadata: &SessionMetadata) -> Option<SignatureSelectionExportRefs> {
    let context = metadata.signature_context.as_ref()?;
    if context.signature_pack.entries.is_empty() && context.selection_decision.is_none() {
        return None;
    }
    Some(SignatureSelectionExportRefs {
        pack_id: context.signature_pack.pack_id.clone(),
        selected_signature_ids: context
            .signature_pack
            .entries
            .iter()
            .map(|entry| entry.signature.id.clone())
            .collect(),
        decision_id: context
            .selection_decision
            .as_ref()
            .map(|decision| decision.decision_id.clone()),
        selector_mode: context
            .selection_decision
            .as_ref()
            .map(|decision| signature_selector_mode_label(decision.selector_mode).to_string()),
        task_envelope_digest: context
            .selection_decision
            .as_ref()
            .and_then(|decision| decision.task_envelope_digest.clone()),
    })
}

fn signature_selector_mode_label(mode: SignatureSelectorMode) -> &'static str {
    match mode {
        SignatureSelectorMode::ExactRef => "exact_ref",
        SignatureSelectorMode::SemanticEmbedding => "semantic_embedding",
        SignatureSelectorMode::StructuredQuery => "structured_query",
        SignatureSelectorMode::Hybrid => "hybrid",
        SignatureSelectorMode::Manual => "manual",
        SignatureSelectorMode::NoMatch => "no_match",
    }
}

fn signature_adoption_state_label(state: SignatureAdoptionState) -> &'static str {
    match state {
        SignatureAdoptionState::Candidate => "candidate",
        SignatureAdoptionState::Shadow => "shadow",
        SignatureAdoptionState::Promoted => "promoted",
        SignatureAdoptionState::Deprecated => "deprecated",
    }
}

fn signature_case_digest(record: &SignatureSelectionCaseRecord) -> String {
    let payload = serde_json::json!({
        "schema_version": record.schema_version,
        "case_id": record.case_id,
        "split": record.split,
        "session_id": record.session_id,
        "pack_id": record.pack_id,
        "decision_id": record.decision_id,
        "selector_mode": record.selector_mode,
        "task_envelope_digest": record.task_envelope_digest,
        "signature": record.signature,
        "selected_signature_ids": record.selected_signature_ids,
        "runner_up_signatures": record.runner_up_signatures,
        "tool_policy": record.tool_policy,
        "result": record.result,
        "outcome_label": record.outcome_label,
        "transcript_refs": record.transcript_refs,
    });
    let mut hasher = Sha256::new();
    hasher.update(b"probe_signature_selection_case|");
    hasher.update(payload.to_string().as_bytes());
    hex::encode(hasher.finalize())
}

fn decision_case_digest(
    family: DecisionCaseFamily,
    metadata: &SessionMetadata,
    turn_index: u64,
    context: &DecisionCaseContext,
    observed_label: &DecisionCaseObservedLabel,
    transcript_refs: &[DecisionCaseTranscriptRef],
    tool_call_id: Option<&str>,
) -> String {
    let payload = serde_json::json!({
        "family": family,
        "session_id": metadata.id.as_str(),
        "turn_index": turn_index,
        "tool_call_id": tool_call_id,
        "context": context,
        "observed_label": observed_label,
        "transcript_refs": transcript_refs,
    });
    let mut hasher = Sha256::new();
    hasher.update(b"probe_decision_case|");
    hasher.update(payload.to_string().as_bytes());
    hex::encode(hasher.finalize())
}

fn decision_case_split_from_digest(digest: &str) -> DecisionCaseSplit {
    let suffix = digest
        .get(digest.len().saturating_sub(2)..)
        .and_then(|value| u8::from_str_radix(value, 16).ok())
        .unwrap_or(0);
    if suffix < 205 {
        DecisionCaseSplit::Train
    } else {
        DecisionCaseSplit::Validation
    }
}

fn first_named_tool_call(
    event: &TranscriptEvent,
) -> Option<&probe_protocol::session::TranscriptItem> {
    event
        .turn
        .items
        .iter()
        .find(|item| item.kind == TranscriptItemKind::ToolCall && item.name.as_ref().is_some())
}

fn sum_user_message_chars(event: &TranscriptEvent) -> usize {
    let from_user_messages = event
        .turn
        .items
        .iter()
        .filter(|item| item.kind == TranscriptItemKind::UserMessage)
        .map(|item| item.text.chars().count())
        .sum::<usize>();
    if from_user_messages > 0 {
        return from_user_messages;
    }

    first_named_tool_call(event)
        .and_then(|item| item.arguments.as_ref())
        .and_then(|arguments| arguments.get("question"))
        .and_then(serde_json::Value::as_str)
        .map_or(0, |value| value.chars().count())
}

fn requested_task_kind_from_tool_call(
    tool_call: &probe_protocol::session::TranscriptItem,
) -> Option<String> {
    tool_call
        .arguments
        .as_ref()
        .and_then(|arguments| arguments.get("task_kind"))
        .and_then(serde_json::Value::as_str)
        .map(String::from)
}

fn requested_evidence_files_from_tool_call(
    tool_call: &probe_protocol::session::TranscriptItem,
) -> Option<usize> {
    tool_call
        .arguments
        .as_ref()
        .and_then(|arguments| arguments.get("evidence_paths"))
        .and_then(serde_json::Value::as_array)
        .map(std::vec::Vec::len)
}

fn tool_result_path(item: &probe_protocol::session::TranscriptItem) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(&item.text).ok()?;
    value
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(String::from)
}

fn tool_result_succeeded(item: &probe_protocol::session::TranscriptItem) -> bool {
    let Some(tool_execution) = item.tool_execution.as_ref() else {
        return false;
    };
    if !matches!(
        tool_execution.policy_decision,
        ToolPolicyDecision::AutoAllow | ToolPolicyDecision::Approved
    ) {
        return false;
    }
    !tool_result_has_error(item)
}

fn tool_result_has_error(item: &probe_protocol::session::TranscriptItem) -> bool {
    serde_json::from_str::<serde_json::Value>(&item.text)
        .ok()
        .and_then(|value| value.get("error").cloned())
        .is_some()
}

fn push_unique(target: &mut Vec<String>, value: String) {
    if !target.iter().any(|existing| existing == &value) {
        target.push(value);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use probe_protocol::session::{
        SessionHarnessProfile, ToolApprovalState, ToolExecutionRecord, ToolPolicyDecision,
        ToolRiskClass, TranscriptItemKind,
    };
    use probe_protocol::signature_context::{
        PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION, SessionSignatureContext, SignatureAdoptionState,
        SignaturePack, SignaturePackEntry, SignatureRef, SignatureSelectionDecision,
        SignatureSelectionScore, SignatureSelectorMode,
    };

    use crate::session_store::{FilesystemSessionStore, NewItem, NewSession};

    use super::{DatasetExportConfig, DatasetKind, export_dataset};

    #[test]
    fn dataset_kind_parses_signature_cases_aliases() {
        assert_eq!(
            DatasetKind::parse("signature-cases").expect("signature cases kind"),
            DatasetKind::SignatureCases
        );
        assert_eq!(
            DatasetKind::parse("signature_cases").expect("signature cases alias"),
            DatasetKind::SignatureCases
        );
        assert_eq!(DatasetKind::SignatureCases.as_str(), "signature-cases");
    }

    #[test]
    fn export_replay_dataset_writes_jsonl_for_coding_session() {
        let temp = tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let metadata = store
            .create_session_with(
                NewSession::new("coding replay", temp.path())
                    .with_harness_profile(Some(SessionHarnessProfile {
                        name: String::from("coding_bootstrap_default"),
                        version: String::from("v1"),
                    }))
                    .with_signature_context(Some(signature_context_fixture())),
            )
            .expect("create session");
        store
            .append_turn(
                &metadata.id,
                &[
                    NewItem::new(TranscriptItemKind::UserMessage, "hello"),
                    NewItem::new(TranscriptItemKind::AssistantMessage, "world"),
                    NewItem::new(
                        TranscriptItemKind::Note,
                        "managed_event_type=model.usage.reported managed_event_ref=probe://events/run-1/17 mode=signature_selector count_source=provider_reported",
                    ),
                ],
            )
            .expect("append turn");

        let output_path = temp.path().join("replay.jsonl");
        let report = export_dataset(
            &store,
            &DatasetExportConfig {
                kind: DatasetKind::Replay,
                output_path: output_path.clone(),
                session_ids: Vec::new(),
                include_all_sessions: false,
            },
        )
        .expect("export replay dataset");

        let body = fs::read_to_string(output_path).expect("read replay export");
        assert_eq!(report.sessions_exported, 1);
        assert!(body.contains("coding replay"));
        assert!(body.contains("coding_bootstrap_default"));
        assert!(body.contains("coding.service_readiness"));
        assert!(body.contains("sigsel-dataset-test"));
        assert!(body.contains("managed_usage_events"));
        assert!(body.contains("model_usage_reported"));
        assert!(body.contains("signature_selector"));
    }

    #[test]
    fn export_decision_dataset_derives_tool_and_policy_fields() {
        let temp = tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let metadata = store
            .create_session_with(
                NewSession::new("coding decision", temp.path()).with_harness_profile(Some(
                    SessionHarnessProfile {
                        name: String::from("coding_bootstrap_default"),
                        version: String::from("v1"),
                    },
                )),
            )
            .expect("create session");

        store
            .append_turn(
                &metadata.id,
                &[NewItem::tool_call(
                    "list_files",
                    "call_list",
                    serde_json::json!({ "path": "src" }),
                )],
            )
            .expect("append tool call");
        store
            .append_turn(
                &metadata.id,
                &[NewItem::tool_result(
                    "list_files",
                    "call_list",
                    "{\"path\":\"src\",\"entries\":[{\"path\":\"src/lib.rs\"}]}",
                    ToolExecutionRecord {
                        risk_class: ToolRiskClass::ReadOnly,
                        policy_decision: ToolPolicyDecision::AutoAllow,
                        approval_state: ToolApprovalState::NotRequired,
                        command: None,
                        exit_code: None,
                        timed_out: None,
                        truncated: Some(false),
                        bytes_returned: Some(32),
                        files_touched: vec![String::from("src")],
                        reason: None,
                    },
                )],
            )
            .expect("append tool result");
        store
            .append_turn(
                &metadata.id,
                &[NewItem::tool_call(
                    "read_file",
                    "call_read",
                    serde_json::json!({ "path": "src/lib.rs" }),
                )],
            )
            .expect("append tool call");
        store
            .append_turn(
                &metadata.id,
                &[NewItem::tool_result(
                    "read_file",
                    "call_read",
                    "{\"path\":\"src/lib.rs\",\"content\":\"beta\"}",
                    ToolExecutionRecord {
                        risk_class: ToolRiskClass::ReadOnly,
                        policy_decision: ToolPolicyDecision::AutoAllow,
                        approval_state: ToolApprovalState::NotRequired,
                        command: None,
                        exit_code: None,
                        timed_out: None,
                        truncated: Some(false),
                        bytes_returned: Some(24),
                        files_touched: vec![String::from("src/lib.rs")],
                        reason: None,
                    },
                )],
            )
            .expect("append tool result");
        store
            .append_turn(
                &metadata.id,
                &[NewItem::tool_call(
                    "apply_patch",
                    "call_patch",
                    serde_json::json!({ "path": "hello.txt" }),
                )],
            )
            .expect("append tool call");
        store
            .append_turn(
                &metadata.id,
                &[NewItem::tool_result(
                    "apply_patch",
                    "call_patch",
                    "{\"path\":\"hello.txt\",\"created\":false}",
                    ToolExecutionRecord {
                        risk_class: ToolRiskClass::Write,
                        policy_decision: ToolPolicyDecision::Approved,
                        approval_state: ToolApprovalState::Approved,
                        command: None,
                        exit_code: None,
                        timed_out: None,
                        truncated: Some(false),
                        bytes_returned: None,
                        files_touched: vec![String::from("hello.txt")],
                        reason: Some(String::from("approved")),
                    },
                )],
            )
            .expect("append tool result");
        store
            .append_turn(
                &metadata.id,
                &[NewItem::tool_result(
                    "read_file",
                    "call_verify",
                    "{\"path\":\"hello.txt\",\"content\":\"hello probe\"}",
                    ToolExecutionRecord {
                        risk_class: ToolRiskClass::ReadOnly,
                        policy_decision: ToolPolicyDecision::AutoAllow,
                        approval_state: ToolApprovalState::NotRequired,
                        command: None,
                        exit_code: None,
                        timed_out: None,
                        truncated: Some(false),
                        bytes_returned: Some(11),
                        files_touched: vec![String::from("hello.txt")],
                        reason: None,
                    },
                )],
            )
            .expect("append verification result");
        store
            .append_turn(
                &metadata.id,
                &[NewItem::new(
                    TranscriptItemKind::Note,
                    "session exceeded the configured tool loop bound of 4 model round trips",
                )],
            )
            .expect("append note");
        store
            .append_turn(
                &metadata.id,
                &[NewItem::new(
                    TranscriptItemKind::Note,
                    "managed_event_type=resource.usage.captured managed_event_ref=probe://events/run-1/16 receipt_digest=sha256:resource-usage mode=codex_backend",
                )],
            )
            .expect("append managed usage note");

        let output_path = temp.path().join("decision.jsonl");
        export_dataset(
            &store,
            &DatasetExportConfig {
                kind: DatasetKind::Decision,
                output_path: output_path.clone(),
                session_ids: Vec::new(),
                include_all_sessions: false,
            },
        )
        .expect("export decision dataset");

        let line = fs::read_to_string(output_path).expect("read decision export");
        let value: serde_json::Value = serde_json::from_str(line.lines().next().unwrap_or("{}"))
            .expect("parse decision export");
        assert_eq!(value["first_tool_name"], "list_files");
        assert_eq!(value["patch_attempts"], 1);
        assert_eq!(value["successful_patch_attempts"], 1);
        assert_eq!(value["verification_step_count"], 1);
        assert_eq!(value["too_many_turns"], true);
        assert_eq!(value["approved_tool_calls"], 1);
        assert_eq!(value["auto_allowed_tool_calls"], 3);
        assert_eq!(value["oracle_calls"], 0);
        assert_eq!(value["long_context_calls"], 0);
        assert_eq!(value["repo_analysis_files"], serde_json::json!([]));
        assert_eq!(
            value["managed_usage_events"][0]["event_type"],
            "resource_usage_captured"
        );
        assert_eq!(
            value["managed_usage_events"][0]["receipt_digest"],
            "sha256:resource-usage"
        );
    }

    #[test]
    fn export_decision_dataset_preserves_signature_selection_refs() {
        let temp = tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let metadata = store
            .create_session_with(
                NewSession::new("signature decision", temp.path())
                    .with_harness_profile(Some(SessionHarnessProfile {
                        name: String::from("coding_bootstrap_codex"),
                        version: String::from("v1"),
                    }))
                    .with_signature_context(Some(signature_context_fixture())),
            )
            .expect("create session");

        store
            .append_turn(
                &metadata.id,
                &[NewItem::tool_call(
                    "read_file",
                    "call_read",
                    serde_json::json!({ "path": "README.md" }),
                )],
            )
            .expect("append tool call");

        let output_path = temp.path().join("signature-decision.jsonl");
        export_dataset(
            &store,
            &DatasetExportConfig {
                kind: DatasetKind::Decision,
                output_path: output_path.clone(),
                session_ids: Vec::new(),
                include_all_sessions: false,
            },
        )
        .expect("export decision dataset");

        let line = fs::read_to_string(output_path).expect("read decision export");
        let value: serde_json::Value = serde_json::from_str(line.lines().next().unwrap_or("{}"))
            .expect("parse decision export");
        assert_eq!(
            value["signature_selection"]["pack_id"],
            "probe.seed_failure_signatures.v1"
        );
        assert_eq!(
            value["signature_selection"]["selected_signature_ids"],
            serde_json::json!(["coding.service_readiness"])
        );
        assert_eq!(
            value["signature_selection"]["decision_id"],
            "sigsel-dataset-test"
        );
        assert_eq!(value["signature_selection"]["selector_mode"], "hybrid");
        assert_eq!(
            value["signature_selection"]["task_envelope_digest"],
            "sha256:dataset"
        );
    }

    #[test]
    fn export_decision_case_bundle_writes_train_and_validation_artifacts() {
        let temp = tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let metadata = store
            .create_session_with(
                NewSession::new("coding cases", temp.path()).with_harness_profile(Some(
                    SessionHarnessProfile {
                        name: String::from("coding_bootstrap_default"),
                        version: String::from("v1"),
                    },
                )),
            )
            .expect("create session");
        store
            .append_turn(
                &metadata.id,
                &[
                    NewItem::new(TranscriptItemKind::UserMessage, "inspect the src tree"),
                    NewItem::tool_call(
                        "list_files",
                        "call_list",
                        serde_json::json!({ "path": "src" }),
                    ),
                ],
            )
            .expect("append turn");

        let output_path = temp.path().join("decision_cases");
        let report = export_dataset(
            &store,
            &DatasetExportConfig {
                kind: DatasetKind::DecisionCases,
                output_path: output_path.clone(),
                session_ids: Vec::new(),
                include_all_sessions: false,
            },
        )
        .expect("export decision case bundle");

        assert_eq!(report.sessions_exported, 1);
        assert_eq!(report.cases_exported, 3);

        let all_cases = fs::read_to_string(output_path.join("decision_cases_all.jsonl"))
            .expect("read decision cases");
        assert_eq!(all_cases.lines().count(), 3);
        let first_case: serde_json::Value =
            serde_json::from_str(all_cases.lines().next().expect("first case line"))
                .expect("parse case");
        assert_eq!(first_case["session_id"], metadata.id.as_str());
        assert_eq!(
            first_case["source_transcript_path"],
            serde_json::json!(metadata.transcript_path)
        );
        assert!(first_case["stable_digest"].as_str().is_some());
        assert!(
            matches!(first_case["split"].as_str(), Some("train" | "validation")),
            "case split should be train or validation"
        );

        let split_manifest =
            fs::read_to_string(output_path.join("decision_case_split_manifest.json"))
                .expect("read split manifest");
        let manifest: serde_json::Value =
            serde_json::from_str(&split_manifest).expect("parse split manifest");
        assert_eq!(manifest["total_sessions"], 1);
        assert_eq!(manifest["total_cases"], 3);
        assert_eq!(
            manifest["train_cases"].as_u64().unwrap_or(0)
                + manifest["validation_cases"].as_u64().unwrap_or(0),
            3
        );
    }

    #[test]
    fn export_signature_case_bundle_writes_validation_only_failure_cases() {
        let temp = tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let metadata = store
            .create_session_with(
                NewSession::new("signature failure", temp.path())
                    .with_harness_profile(Some(SessionHarnessProfile {
                        name: String::from("coding_bootstrap_codex"),
                        version: String::from("v1"),
                    }))
                    .with_signature_context(Some(signature_context_fixture())),
            )
            .expect("create session");
        store
            .append_turn(
                &metadata.id,
                &[NewItem::new(
                    TranscriptItemKind::Note,
                    "signature_context decision=sigsel-dataset-test selected=[coding.service_readiness@candidate] recommended_tool_choice=auto actual_tool_choice=auto tool_policy=probe_enforced verifier_outcome=failed",
                )],
            )
            .expect("append signature note");
        store
            .append_turn(
                &metadata.id,
                &[NewItem::tool_result(
                    "shell",
                    "call_shell",
                    "{\"error\":\"network shell refused\"}",
                    ToolExecutionRecord {
                        risk_class: ToolRiskClass::Network,
                        policy_decision: ToolPolicyDecision::Refused,
                        approval_state: ToolApprovalState::Refused,
                        command: Some(String::from("curl http://127.0.0.1:8080/health")),
                        exit_code: None,
                        timed_out: Some(false),
                        truncated: Some(false),
                        bytes_returned: Some(34),
                        files_touched: Vec::new(),
                        reason: Some(String::from("network shell disabled")),
                    },
                )],
            )
            .expect("append refused tool");

        let output_path = temp.path().join("signature_cases");
        let report = export_dataset(
            &store,
            &DatasetExportConfig {
                kind: DatasetKind::SignatureCases,
                output_path: output_path.clone(),
                session_ids: Vec::new(),
                include_all_sessions: false,
            },
        )
        .expect("export signature cases");

        assert_eq!(report.sessions_exported, 1);
        assert_eq!(report.cases_exported, 1);

        let all_cases = fs::read_to_string(output_path.join("signature_cases_all.jsonl"))
            .expect("read signature cases");
        let first_case: serde_json::Value =
            serde_json::from_str(all_cases.lines().next().expect("first case line"))
                .expect("parse signature case");
        assert_eq!(first_case["split"], "validation");
        assert_eq!(
            first_case["signature"]["signature_id"],
            "coding.service_readiness"
        );
        assert_eq!(first_case["signature"]["score_bps"], 9000);
        assert_eq!(first_case["result"]["status"], "failed");
        assert_eq!(first_case["result"]["failure_type"], "tool_refused");
        assert_eq!(first_case["result"]["verifier_outcome"], "failed");
        assert_eq!(first_case["outcome_label"], "unknown");
        assert_eq!(first_case["tool_policy"]["actual_tool_choice"], "auto");
        assert_eq!(first_case["tool_policy"]["refused_tool_calls"], 1);
        assert_eq!(
            first_case["runner_up_signatures"][0]["signature_id"],
            "coding.python_package_index"
        );
        assert!(first_case["stable_digest"].as_str().is_some());
        assert!(!all_cases.contains("Prove service readiness before declaring success"));

        let val_cases = fs::read_to_string(output_path.join("signature_cases_val.jsonl"))
            .expect("read validation signature cases");
        assert_eq!(val_cases.lines().count(), 1);
        let manifest = fs::read_to_string(output_path.join("signature_case_manifest.json"))
            .expect("read signature case manifest");
        let manifest: serde_json::Value =
            serde_json::from_str(&manifest).expect("parse signature manifest");
        assert_eq!(manifest["total_cases"], 1);
        assert_eq!(manifest["train_cases"], 0);
        assert_eq!(manifest["validation_cases"], 1);
        assert_eq!(
            manifest["training_policy"],
            "validation_only_until_explicit_signature_promotion_admits_training_use"
        );
    }

    fn signature_context_fixture() -> SessionSignatureContext {
        let signature = SignatureRef {
            id: String::from("coding.service_readiness"),
            version: String::from("candidate"),
            adoption_state: SignatureAdoptionState::Candidate,
            source_ref: Some(String::from("vortex://signatures/coding.service_readiness")),
        };
        SessionSignatureContext::new(SignaturePack {
            schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
            pack_id: Some(String::from("probe.seed_failure_signatures.v1")),
            selected_by: Some(String::from("probe_signature_selector")),
            selected_at_ms: None,
            max_signature_count: Some(4),
            entries: vec![SignaturePackEntry {
                signature: signature.clone(),
                task_classes: vec![String::from("service_orchestration")],
                benchmark_families: vec![String::from("terminal-bench")],
                required_evidence: Vec::new(),
                recommended_tools: Vec::new(),
                forbidden_tools: vec![String::from("destructive_shell")],
                closeout_artifacts: vec![String::from("service-readiness.json")],
                failure_fingerprints: vec![String::from("port_not_ready")],
                fixture_refs: vec![String::from("terminal-bench:2.0/pypi-server")],
                rendered_description: Some(String::from(
                    "Prove service readiness before declaring success.",
                )),
            }],
        })
        .with_selection_decision(SignatureSelectionDecision {
            schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
            decision_id: String::from("sigsel-dataset-test"),
            selector_mode: SignatureSelectorMode::Hybrid,
            task_envelope_digest: Some(String::from("sha256:dataset")),
            selected_signature_budget: Some(1),
            budget_mode: Some(String::from("adaptive_threshold")),
            selected_signatures: vec![SignatureSelectionScore {
                signature,
                rank: 1,
                score_bps: 9_000,
                reason_code: Some(String::from("structured_match")),
            }],
            runner_up_signatures: vec![SignatureSelectionScore {
                signature: SignatureRef {
                    id: String::from("coding.python_package_index"),
                    version: String::from("candidate"),
                    adoption_state: SignatureAdoptionState::Candidate,
                    source_ref: Some(String::from(
                        "vortex://signatures/coding.python_package_index",
                    )),
                },
                rank: 2,
                score_bps: 4_800,
                reason_code: Some(String::from("semantic_runner_up")),
            }],
            rejected_high_score_signatures: Vec::new(),
            rendered_context: Some(String::from(
                "Use for: service readiness. Required evidence: service-readiness.json.",
            )),
            recommended_harness_profile: Some(String::from("coding_bootstrap_codex@v1")),
            recommended_tool_set: Some(String::from("coding_bootstrap")),
            recommended_tool_choice: Some(String::from("auto")),
            forbidden_tools: vec![String::from("destructive_shell")],
            fallback_reason_code: None,
        })
    }
}
