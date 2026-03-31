use std::fmt::{Display, Formatter};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use probe_protocol::session::{
    CacheSignal, SessionId, SessionMetadata, ToolPolicyDecision, TranscriptEvent,
    TranscriptItemKind,
};
use serde::{Deserialize, Serialize};

use crate::session_store::{FilesystemSessionStore, SessionStoreError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatasetKind {
    Replay,
    Decision,
}

impl DatasetKind {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "replay" => Ok(Self::Replay),
            "decision" => Ok(Self::Decision),
            other => Err(format!(
                "unknown dataset kind `{other}`; expected `replay` or `decision`"
            )),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Replay => "replay",
            Self::Decision => "decision",
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
    turn_count: usize,
    transcript: Vec<TranscriptEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionSessionSummary {
    pub session_id: String,
    pub title: String,
    pub cwd: String,
    pub backend_profile: Option<String>,
    pub harness_profile: Option<String>,
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
    pub likely_warm_turns: usize,
    pub cache_reuse_improved_latency: bool,
    pub cache_reuse_improved_throughput: bool,
    pub final_assistant_text: Option<String>,
}

pub fn export_dataset(
    session_store: &FilesystemSessionStore,
    config: &DatasetExportConfig,
) -> Result<DatasetExportReport, DatasetExportError> {
    if let Some(parent) = config.output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::create(&config.output_path)?;
    let mut writer = BufWriter::new(file);

    let sessions = session_store.list_sessions()?;
    let mut sessions_exported = 0_usize;
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
                    turn_count: transcript.len(),
                    transcript,
                };
                serde_json::to_writer(&mut writer, &record)?;
            }
            DatasetKind::Decision => {
                let record = build_decision_summary(&metadata, transcript.as_slice());
                serde_json::to_writer(&mut writer, &record)?;
            }
        }
        writer.write_all(b"\n")?;
        sessions_exported += 1;
    }
    writer.flush()?;

    Ok(DatasetExportReport {
        kind: config.kind,
        output_path: config.output_path.clone(),
        sessions_exported,
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
        likely_warm_turns,
        cache_reuse_improved_latency: likely_warm_turns > 0,
        cache_reuse_improved_throughput,
        final_assistant_text,
    }
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

    use crate::session_store::{FilesystemSessionStore, NewItem, NewSession};

    use super::{DatasetExportConfig, DatasetKind, export_dataset};

    #[test]
    fn export_replay_dataset_writes_jsonl_for_coding_session() {
        let temp = tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let metadata = store
            .create_session_with(
                NewSession::new("coding replay", temp.path()).with_harness_profile(Some(
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
                    NewItem::new(TranscriptItemKind::UserMessage, "hello"),
                    NewItem::new(TranscriptItemKind::AssistantMessage, "world"),
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
    }
}
