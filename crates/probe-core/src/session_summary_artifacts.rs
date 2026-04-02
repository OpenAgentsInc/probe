use std::fmt::{Display, Formatter};
use std::time::{SystemTime, UNIX_EPOCH};

use probe_protocol::session::{
    AcceptedPatchSummaryArtifact, AcceptedPatchSummarySourceTurn, SessionBranchState,
    SessionDeliveryState, SessionMetadata, SessionSummaryArtifact, SessionSummaryArtifactKind,
    SessionSummaryArtifactRef, SessionSummaryArtifactSource, ToolPolicyDecision, TranscriptEvent,
    TranscriptItem,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::dataset_export::{DecisionSessionSummary, build_decision_summary};
use crate::session_store::{FilesystemSessionStore, SessionStoreError};

pub const RETAINED_SESSION_SUMMARY_SCHEMA_VERSION: &str = "probe.retained_session_summary.v1";
pub const ACCEPTED_PATCH_SUMMARY_SCHEMA_VERSION: &str = "probe.accepted_patch_summary.v1";

const RETAINED_SESSION_SUMMARY_FILE: &str = "retained_session_summary_v1.json";
const ACCEPTED_PATCH_SUMMARY_FILE: &str = "accepted_patch_summary_v1.json";
const RETAINED_SESSION_SUMMARY_DIGEST_PREFIX: &[u8] = b"probe_retained_session_summary|";
const ACCEPTED_PATCH_SUMMARY_DIGEST_PREFIX: &[u8] = b"probe_accepted_patch_summary|";

#[derive(Debug)]
pub enum SessionSummaryArtifactError {
    SessionStore(SessionStoreError),
    Json(serde_json::Error),
}

impl Display for SessionSummaryArtifactError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionStore(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
        }
    }
}

impl std::error::Error for SessionSummaryArtifactError {}

impl From<SessionStoreError> for SessionSummaryArtifactError {
    fn from(value: SessionStoreError) -> Self {
        Self::SessionStore(value)
    }
}

impl From<serde_json::Error> for SessionSummaryArtifactError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub fn refresh_session_summary_artifacts(
    store: &FilesystemSessionStore,
    metadata: &SessionMetadata,
    transcript: &[TranscriptEvent],
    branch_state: Option<&SessionBranchState>,
    delivery_state: Option<&SessionDeliveryState>,
) -> Result<Vec<SessionSummaryArtifact>, SessionSummaryArtifactError> {
    let source = SessionSummaryArtifactSource {
        session_id: metadata.id.clone(),
        transcript_path: metadata.transcript_path.clone(),
        transcript_sha256: transcript_sha256(transcript)?,
        session_created_at_ms: metadata.created_at_ms,
        session_updated_at_ms: metadata.updated_at_ms,
    };
    let decision_summary = build_decision_summary(metadata, transcript);
    let retained =
        persist_retained_session_summary_artifact(store, metadata, &decision_summary, &source)?;

    let accepted_patch = persist_accepted_patch_summary_artifact(
        store,
        metadata,
        transcript,
        &decision_summary,
        &source,
        branch_state,
        delivery_state,
    )?;

    let mut artifacts = vec![SessionSummaryArtifact::RetainedSessionSummary(retained)];
    if let Some(artifact) = accepted_patch {
        artifacts.push(SessionSummaryArtifact::AcceptedPatchSummary(artifact));
    }
    Ok(artifacts)
}

fn persist_retained_session_summary_artifact(
    store: &FilesystemSessionStore,
    metadata: &SessionMetadata,
    summary: &DecisionSessionSummary,
    source: &SessionSummaryArtifactSource,
) -> Result<probe_protocol::session::RetainedSessionSummaryArtifact, SessionSummaryArtifactError> {
    let path = store.session_artifact_path(&metadata.id, RETAINED_SESSION_SUMMARY_FILE);
    let existing = store
        .read_session_artifact_json::<probe_protocol::session::RetainedSessionSummaryArtifact>(
            &metadata.id,
            RETAINED_SESSION_SUMMARY_FILE,
        )?;
    let digest = retained_session_summary_digest(metadata, summary, source)?;
    if let Some(existing) = existing {
        if existing.artifact.stable_digest == digest {
            return Ok(existing);
        }
    }

    let persisted_at_ms = now_ms();
    let artifact = probe_protocol::session::RetainedSessionSummaryArtifact {
        schema_version: String::from(RETAINED_SESSION_SUMMARY_SCHEMA_VERSION),
        artifact: SessionSummaryArtifactRef {
            artifact_id: format!("probe.retained_session_summary:{}", metadata.id.as_str()),
            kind: SessionSummaryArtifactKind::RetainedSessionSummary,
            path,
            stable_digest: digest,
            updated_at_ms: persisted_at_ms,
        },
        session_id: metadata.id.clone(),
        title: metadata.title.clone(),
        cwd: metadata.cwd.clone(),
        backend_profile: summary.backend_profile.clone(),
        harness_profile: summary.harness_profile.clone(),
        turn_count: summary.turn_count,
        tool_names: summary.tool_names.clone(),
        files_listed: summary.files_listed.clone(),
        files_searched: summary.files_searched.clone(),
        files_read: summary.files_read.clone(),
        patch_attempts: summary.patch_attempts,
        successful_patch_attempts: summary.successful_patch_attempts,
        failed_patch_attempts: summary.failed_patch_attempts,
        verification_step_count: summary.verification_step_count,
        verification_caught_problem: summary.verification_caught_problem,
        summary_text: retained_summary_text(metadata, summary),
        final_assistant_text: summary.final_assistant_text.clone(),
        source: source.clone(),
    };
    store.write_session_artifact_json(&metadata.id, RETAINED_SESSION_SUMMARY_FILE, &artifact)?;
    Ok(artifact)
}

fn persist_accepted_patch_summary_artifact(
    store: &FilesystemSessionStore,
    metadata: &SessionMetadata,
    transcript: &[TranscriptEvent],
    summary: &DecisionSessionSummary,
    source: &SessionSummaryArtifactSource,
    branch_state: Option<&SessionBranchState>,
    delivery_state: Option<&SessionDeliveryState>,
) -> Result<Option<AcceptedPatchSummaryArtifact>, SessionSummaryArtifactError> {
    let patch_turns = accepted_patch_turns(transcript);
    if patch_turns.is_empty() {
        store.remove_session_artifact(&metadata.id, ACCEPTED_PATCH_SUMMARY_FILE)?;
        return Ok(None);
    }

    let path = store.session_artifact_path(&metadata.id, ACCEPTED_PATCH_SUMMARY_FILE);
    let existing = store.read_session_artifact_json::<AcceptedPatchSummaryArtifact>(
        &metadata.id,
        ACCEPTED_PATCH_SUMMARY_FILE,
    )?;
    let files_touched = unique_files_touched(patch_turns.as_slice());
    let digest = accepted_patch_summary_digest(
        metadata,
        patch_turns.as_slice(),
        files_touched.as_slice(),
        summary.final_assistant_text.as_deref(),
        branch_state,
        delivery_state,
        source,
    )?;
    if let Some(existing) = existing {
        if existing.artifact.stable_digest == digest {
            return Ok(Some(existing));
        }
    }

    let persisted_at_ms = now_ms();
    let artifact = AcceptedPatchSummaryArtifact {
        schema_version: String::from(ACCEPTED_PATCH_SUMMARY_SCHEMA_VERSION),
        artifact: SessionSummaryArtifactRef {
            artifact_id: format!("probe.accepted_patch_summary:{}", metadata.id.as_str()),
            kind: SessionSummaryArtifactKind::AcceptedPatchSummary,
            path,
            stable_digest: digest,
            updated_at_ms: persisted_at_ms,
        },
        session_id: metadata.id.clone(),
        title: metadata.title.clone(),
        cwd: metadata.cwd.clone(),
        patch_turns,
        files_touched: files_touched.clone(),
        summary_text: accepted_patch_summary_text(
            metadata,
            files_touched.as_slice(),
            summary.final_assistant_text.as_deref(),
            branch_state,
            delivery_state,
        ),
        final_assistant_text: summary.final_assistant_text.clone(),
        branch_state: branch_state.cloned(),
        delivery_state: delivery_state.cloned(),
        source: source.clone(),
    };
    store.write_session_artifact_json(&metadata.id, ACCEPTED_PATCH_SUMMARY_FILE, &artifact)?;
    Ok(Some(artifact))
}

fn retained_summary_text(metadata: &SessionMetadata, summary: &DecisionSessionSummary) -> String {
    let mut sentences = vec![format!(
        "Session {} recorded {} turn(s).",
        metadata.title, summary.turn_count
    )];
    if !summary.tool_names.is_empty() {
        sentences.push(format!(
            "Tools used: {}.",
            summarize_list(summary.tool_names.as_slice(), 6)
        ));
    }
    if !summary.files_read.is_empty() {
        sentences.push(format!(
            "Files read: {}.",
            summarize_list(summary.files_read.as_slice(), 6)
        ));
    }
    if summary.patch_attempts > 0 {
        sentences.push(format!(
            "Patch attempts: {} total, {} successful, {} failed.",
            summary.patch_attempts,
            summary.successful_patch_attempts,
            summary.failed_patch_attempts
        ));
    }
    if summary.verification_step_count > 0 {
        sentences.push(format!(
            "Verification steps after patching: {}.",
            summary.verification_step_count
        ));
    }
    if let Some(final_assistant_text) = summary.final_assistant_text.as_deref() {
        sentences.push(format!("Final assistant summary: {final_assistant_text}"));
    }
    sentences.join(" ")
}

fn accepted_patch_summary_text(
    metadata: &SessionMetadata,
    files_touched: &[String],
    final_assistant_text: Option<&str>,
    branch_state: Option<&SessionBranchState>,
    delivery_state: Option<&SessionDeliveryState>,
) -> String {
    let mut sentences = vec![format!(
        "Session {} produced a persisted patch summary touching {} file(s).",
        metadata.title,
        files_touched.len()
    )];
    if !files_touched.is_empty() {
        sentences.push(format!(
            "Files touched: {}.",
            summarize_list(files_touched, 8)
        ));
    }
    if let Some(delivery_state) = delivery_state {
        sentences.push(format!(
            "Delivery status: {}.",
            delivery_status_label(delivery_state.status)
        ));
        if let Some(compare_ref) = delivery_state.compare_ref.as_deref() {
            sentences.push(format!("Compare ref: {compare_ref}."));
        }
    } else if let Some(branch_state) = branch_state {
        sentences.push(format!(
            "Current head commit: {}.",
            branch_state.head_commit
        ));
    }
    if let Some(final_assistant_text) = final_assistant_text {
        sentences.push(format!("Final assistant summary: {final_assistant_text}"));
    }
    sentences.join(" ")
}

fn accepted_patch_turns(transcript: &[TranscriptEvent]) -> Vec<AcceptedPatchSummarySourceTurn> {
    transcript
        .iter()
        .flat_map(|event| {
            event.turn.items.iter().filter_map(move |item| {
                if item.name.as_deref() != Some("apply_patch") || !tool_result_succeeded(item) {
                    return None;
                }
                Some(AcceptedPatchSummarySourceTurn {
                    turn_index: event.turn.index,
                    tool_call_id: item.tool_call_id.clone(),
                    files_touched: item
                        .tool_execution
                        .as_ref()
                        .map(|execution| execution.files_touched.clone())
                        .unwrap_or_default(),
                })
            })
        })
        .collect()
}

fn unique_files_touched(patch_turns: &[AcceptedPatchSummarySourceTurn]) -> Vec<String> {
    let mut files = Vec::new();
    for patch in patch_turns {
        for path in &patch.files_touched {
            push_unique(&mut files, path.clone());
        }
    }
    files
}

fn tool_result_succeeded(item: &TranscriptItem) -> bool {
    let Some(tool_execution) = item.tool_execution.as_ref() else {
        return false;
    };
    if !matches!(
        tool_execution.policy_decision,
        ToolPolicyDecision::AutoAllow | ToolPolicyDecision::Approved
    ) {
        return false;
    }
    !serde_json::from_str::<serde_json::Value>(&item.text)
        .ok()
        .and_then(|value| value.get("error").cloned())
        .is_some()
}

fn transcript_sha256(
    transcript: &[TranscriptEvent],
) -> Result<String, SessionSummaryArtifactError> {
    Ok(stable_digest(b"probe_transcript|", transcript)?)
}

fn retained_session_summary_digest(
    metadata: &SessionMetadata,
    summary: &DecisionSessionSummary,
    source: &SessionSummaryArtifactSource,
) -> Result<String, SessionSummaryArtifactError> {
    #[derive(Serialize)]
    struct RetainedDigestPayload<'a> {
        session_id: &'a str,
        title: &'a str,
        backend_profile: &'a Option<String>,
        harness_profile: &'a Option<String>,
        turn_count: usize,
        tool_names: &'a [String],
        files_listed: &'a [String],
        files_searched: &'a [String],
        files_read: &'a [String],
        patch_attempts: usize,
        successful_patch_attempts: usize,
        failed_patch_attempts: usize,
        verification_step_count: usize,
        verification_caught_problem: bool,
        final_assistant_text: &'a Option<String>,
        transcript_sha256: &'a str,
        session_created_at_ms: u64,
        session_updated_at_ms: u64,
    }

    stable_digest(
        RETAINED_SESSION_SUMMARY_DIGEST_PREFIX,
        &RetainedDigestPayload {
            session_id: metadata.id.as_str(),
            title: &metadata.title,
            backend_profile: &summary.backend_profile,
            harness_profile: &summary.harness_profile,
            turn_count: summary.turn_count,
            tool_names: &summary.tool_names,
            files_listed: &summary.files_listed,
            files_searched: &summary.files_searched,
            files_read: &summary.files_read,
            patch_attempts: summary.patch_attempts,
            successful_patch_attempts: summary.successful_patch_attempts,
            failed_patch_attempts: summary.failed_patch_attempts,
            verification_step_count: summary.verification_step_count,
            verification_caught_problem: summary.verification_caught_problem,
            final_assistant_text: &summary.final_assistant_text,
            transcript_sha256: &source.transcript_sha256,
            session_created_at_ms: source.session_created_at_ms,
            session_updated_at_ms: source.session_updated_at_ms,
        },
    )
}

fn accepted_patch_summary_digest(
    metadata: &SessionMetadata,
    patch_turns: &[AcceptedPatchSummarySourceTurn],
    files_touched: &[String],
    final_assistant_text: Option<&str>,
    branch_state: Option<&SessionBranchState>,
    delivery_state: Option<&SessionDeliveryState>,
    source: &SessionSummaryArtifactSource,
) -> Result<String, SessionSummaryArtifactError> {
    #[derive(Serialize)]
    struct AcceptedPatchDigestPayload<'a> {
        session_id: &'a str,
        title: &'a str,
        patch_turns: &'a [AcceptedPatchSummarySourceTurn],
        files_touched: &'a [String],
        final_assistant_text: Option<&'a str>,
        branch_state: Option<&'a SessionBranchState>,
        delivery_state: Option<NormalizedDeliveryState<'a>>,
        transcript_sha256: &'a str,
        session_created_at_ms: u64,
        session_updated_at_ms: u64,
    }

    stable_digest(
        ACCEPTED_PATCH_SUMMARY_DIGEST_PREFIX,
        &AcceptedPatchDigestPayload {
            session_id: metadata.id.as_str(),
            title: &metadata.title,
            patch_turns,
            files_touched,
            final_assistant_text,
            branch_state,
            delivery_state: delivery_state.map(NormalizedDeliveryState::from),
            transcript_sha256: &source.transcript_sha256,
            session_created_at_ms: source.session_created_at_ms,
            session_updated_at_ms: source.session_updated_at_ms,
        },
    )
}

#[derive(Serialize)]
struct NormalizedDeliveryState<'a> {
    status: &'a probe_protocol::session::SessionDeliveryStatus,
    branch_name: &'a Option<String>,
    remote_tracking_ref: &'a Option<String>,
    compare_ref: &'a Option<String>,
    artifacts: &'a [probe_protocol::session::SessionDeliveryArtifact],
}

impl<'a> From<&'a SessionDeliveryState> for NormalizedDeliveryState<'a> {
    fn from(value: &'a SessionDeliveryState) -> Self {
        Self {
            status: &value.status,
            branch_name: &value.branch_name,
            remote_tracking_ref: &value.remote_tracking_ref,
            compare_ref: &value.compare_ref,
            artifacts: &value.artifacts,
        }
    }
}

fn stable_digest<T: Serialize + ?Sized>(
    prefix: &[u8],
    value: &T,
) -> Result<String, SessionSummaryArtifactError> {
    let payload = serde_json::to_vec(value)?;
    let mut hasher = Sha256::new();
    hasher.update(prefix);
    hasher.update(payload);
    Ok(hex::encode(hasher.finalize()))
}

fn summarize_list(values: &[String], limit: usize) -> String {
    let mut preview = values.iter().take(limit).cloned().collect::<Vec<_>>();
    if values.len() > limit {
        preview.push(format!("and {} more", values.len() - limit));
    }
    preview.join(", ")
}

fn push_unique(target: &mut Vec<String>, value: String) {
    if !target.iter().any(|existing| existing == &value) {
        target.push(value);
    }
}

fn delivery_status_label(status: probe_protocol::session::SessionDeliveryStatus) -> &'static str {
    match status {
        probe_protocol::session::SessionDeliveryStatus::NeedsCommit => "needs_commit",
        probe_protocol::session::SessionDeliveryStatus::LocalOnly => "local_only",
        probe_protocol::session::SessionDeliveryStatus::NeedsPush => "needs_push",
        probe_protocol::session::SessionDeliveryStatus::Synced => "synced",
        probe_protocol::session::SessionDeliveryStatus::Diverged => "diverged",
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
    use tempfile::tempdir;

    use probe_protocol::session::{
        SessionBackendTarget, SessionBranchState, SessionDeliveryArtifact, SessionDeliveryState,
        SessionDeliveryStatus, SessionHarnessProfile, SessionSummaryArtifact, ToolApprovalState,
        ToolExecutionRecord, ToolPolicyDecision, ToolRiskClass, TranscriptItemKind,
    };

    use crate::session_store::{FilesystemSessionStore, NewItem, NewSession};

    use super::{
        ACCEPTED_PATCH_SUMMARY_SCHEMA_VERSION, RETAINED_SESSION_SUMMARY_SCHEMA_VERSION,
        refresh_session_summary_artifacts,
    };

    #[test]
    fn refresh_session_summary_artifacts_writes_retained_and_accepted_patch_artifacts() {
        let temp = tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let session = store
            .create_session_with(
                NewSession::new("artifact session", temp.path())
                    .with_backend(SessionBackendTarget {
                        profile_name: String::from("test-profile"),
                        base_url: String::from("http://127.0.0.1:9/v1"),
                        model: String::from("tiny"),
                        control_plane: None,
                        psionic_mesh: None,
                    })
                    .with_harness_profile(Some(SessionHarnessProfile {
                        name: String::from("coding_bootstrap_default"),
                        version: String::from("v1"),
                    })),
            )
            .expect("create session");

        store
            .append_turn(
                &session.id,
                &[NewItem::new(TranscriptItemKind::UserMessage, "fix the bug")],
            )
            .expect("append user turn");
        store
            .append_turn(
                &session.id,
                &[NewItem::tool_result(
                    "apply_patch",
                    "call-1",
                    r#"{"ok":true}"#,
                    ToolExecutionRecord {
                        risk_class: ToolRiskClass::Write,
                        policy_decision: ToolPolicyDecision::Approved,
                        approval_state: ToolApprovalState::Approved,
                        command: None,
                        exit_code: Some(0),
                        timed_out: None,
                        truncated: None,
                        bytes_returned: None,
                        files_touched: vec![String::from("src/lib.rs")],
                        reason: None,
                    },
                )],
            )
            .expect("append patch turn");
        store
            .append_turn(
                &session.id,
                &[NewItem::new(
                    TranscriptItemKind::AssistantMessage,
                    "Applied the fix and verified the patch.",
                )],
            )
            .expect("append assistant turn");

        let metadata = store.read_metadata(&session.id).expect("read metadata");
        let transcript = store.read_transcript(&session.id).expect("read transcript");
        let branch_state = SessionBranchState {
            repo_root: temp.path().to_path_buf(),
            head_ref: String::from("main"),
            head_commit: String::from("abc123"),
            detached_head: false,
            working_tree_dirty: false,
            upstream_ref: Some(String::from("origin/main")),
            ahead_by: Some(1),
            behind_by: Some(0),
        };
        let delivery_state = SessionDeliveryState {
            status: SessionDeliveryStatus::NeedsPush,
            branch_name: Some(String::from("main")),
            remote_tracking_ref: Some(String::from("origin/main")),
            compare_ref: Some(String::from("origin/main...main")),
            updated_at_ms: 1234,
            artifacts: vec![SessionDeliveryArtifact {
                kind: String::from("head_commit"),
                value: String::from("abc123"),
                label: Some(String::from("Head commit")),
            }],
        };

        let artifacts = refresh_session_summary_artifacts(
            &store,
            &metadata,
            transcript.as_slice(),
            Some(&branch_state),
            Some(&delivery_state),
        )
        .expect("refresh artifacts");
        assert_eq!(artifacts.len(), 2);

        let retained = artifacts
            .iter()
            .find_map(|artifact| match artifact {
                SessionSummaryArtifact::RetainedSessionSummary(artifact) => Some(artifact),
                SessionSummaryArtifact::AcceptedPatchSummary(_) => None,
            })
            .expect("retained session summary should exist");
        assert_eq!(
            retained.schema_version,
            RETAINED_SESSION_SUMMARY_SCHEMA_VERSION
        );
        assert_eq!(retained.session_id, metadata.id);
        assert!(retained.summary_text.contains("artifact session"));

        let accepted_patch = artifacts
            .iter()
            .find_map(|artifact| match artifact {
                SessionSummaryArtifact::RetainedSessionSummary(_) => None,
                SessionSummaryArtifact::AcceptedPatchSummary(artifact) => Some(artifact),
            })
            .expect("accepted patch summary should exist");
        assert_eq!(
            accepted_patch.schema_version,
            ACCEPTED_PATCH_SUMMARY_SCHEMA_VERSION
        );
        assert_eq!(
            accepted_patch
                .delivery_state
                .as_ref()
                .map(|state| state.status),
            Some(SessionDeliveryStatus::NeedsPush)
        );
        assert_eq!(
            accepted_patch.files_touched,
            vec![String::from("src/lib.rs")]
        );
    }

    #[test]
    fn refresh_session_summary_artifacts_reuses_existing_materialization_for_identical_content() {
        let temp = tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let session = store
            .create_session("stable summary", temp.path())
            .expect("create session");
        store
            .append_turn(
                &session.id,
                &[NewItem::new(
                    TranscriptItemKind::AssistantMessage,
                    "No code changes were needed.",
                )],
            )
            .expect("append assistant turn");

        let metadata = store.read_metadata(&session.id).expect("read metadata");
        let transcript = store.read_transcript(&session.id).expect("read transcript");
        let first =
            refresh_session_summary_artifacts(&store, &metadata, transcript.as_slice(), None, None)
                .expect("first refresh");
        let second =
            refresh_session_summary_artifacts(&store, &metadata, transcript.as_slice(), None, None)
                .expect("second refresh");

        let first_ref = first[0].artifact_ref();
        let second_ref = second[0].artifact_ref();
        assert_eq!(first_ref.stable_digest, second_ref.stable_digest);
        assert_eq!(first_ref.updated_at_ms, second_ref.updated_at_ms);
    }
}
