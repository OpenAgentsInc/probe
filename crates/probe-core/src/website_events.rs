use probe_protocol::session::{
    SessionChildStatus, SessionId, SessionSummaryArtifactKind, SessionSummaryArtifactRef,
    ToolApprovalResolution, TranscriptEvent,
};
use probe_protocol::website_events::{
    ProbeWebsiteArtifactKind, ProbeWebsiteArtifactRef, ProbeWebsiteEvent, ProbeWebsiteEventActor,
    ProbeWebsiteEventCorrelation, ProbeWebsiteEventSource, ProbeWebsiteEventType,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::runtime::RuntimeEvent;
use crate::tools::{ExecutedToolCall, tool_input_summary, tool_output_summary};

const DEFAULT_PREVIEW_CHARS: usize = 240;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebsiteEventExportContext {
    pub actor: ProbeWebsiteEventActor,
    pub source: ProbeWebsiteEventSource,
    pub correlation: ProbeWebsiteEventCorrelation,
    pub artifact_refs: Vec<ProbeWebsiteArtifactRef>,
}

impl WebsiteEventExportContext {
    #[must_use]
    pub fn probe_runtime(session_id: &SessionId) -> Self {
        Self {
            actor: ProbeWebsiteEventActor {
                kind: String::from("probe"),
                id: Some(session_id.as_str().to_string()),
                label: None,
            },
            source: ProbeWebsiteEventSource {
                kind: String::from("runtime"),
                id: None,
                label: None,
            },
            correlation: ProbeWebsiteEventCorrelation {
                probe_session_id: Some(session_id.as_str().to_string()),
                ..ProbeWebsiteEventCorrelation::default()
            },
            artifact_refs: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_correlation(mut self, correlation: ProbeWebsiteEventCorrelation) -> Self {
        self.correlation = correlation;
        self
    }

    #[must_use]
    pub fn with_artifact_refs(mut self, artifact_refs: Vec<ProbeWebsiteArtifactRef>) -> Self {
        self.artifact_refs = artifact_refs;
        self
    }
}

#[must_use]
pub fn runtime_event_to_website_event(
    event: &RuntimeEvent,
    sequence: u64,
    occurred_at_ms: u64,
    context: &WebsiteEventExportContext,
) -> ProbeWebsiteEvent {
    let (event_type, mut correlation, payload) = match event {
        RuntimeEvent::TurnStarted {
            session_id,
            profile_name,
            prompt,
            tool_loop_enabled,
        } => {
            let mut payload = Map::new();
            payload.insert(String::from("profileName"), json!(profile_name));
            payload.insert(String::from("promptPreview"), json!(safe_preview(prompt)));
            payload.insert(
                String::from("promptHash"),
                json!(stable_text_digest(prompt)),
            );
            payload.insert(String::from("toolLoopEnabled"), json!(tool_loop_enabled));
            (
                ProbeWebsiteEventType::RunStarted,
                correlation_for_session(context, session_id, None),
                payload,
            )
        }
        RuntimeEvent::AssistantDelta {
            session_id,
            round_trip,
            delta,
        } => {
            let mut payload = Map::new();
            payload.insert(String::from("roundTrip"), json!(round_trip));
            payload.insert(String::from("delta"), json!(redact_text(delta)));
            (
                ProbeWebsiteEventType::TextDelta,
                correlation_for_session(context, session_id, None),
                payload,
            )
        }
        RuntimeEvent::ToolExecutionStarted {
            session_id,
            round_trip,
            call_id,
            tool_name,
            risk_class,
        } => {
            let mut payload = Map::new();
            payload.insert(String::from("roundTrip"), json!(round_trip));
            payload.insert(String::from("callId"), json!(call_id));
            payload.insert(String::from("toolName"), json!(tool_name));
            payload.insert(String::from("riskClass"), json!(risk_class));
            (
                ProbeWebsiteEventType::ToolCallStarted,
                correlation_for_session(context, session_id, None),
                payload,
            )
        }
        RuntimeEvent::ToolExecutionCompleted {
            session_id,
            round_trip,
            tool,
        }
        | RuntimeEvent::ToolRefused {
            session_id,
            round_trip,
            tool,
        } => (
            ProbeWebsiteEventType::ToolCallCompleted,
            correlation_for_session(context, session_id, None),
            tool_payload(*round_trip, tool, "completed"),
        ),
        RuntimeEvent::ToolPaused {
            session_id,
            round_trip,
            tool,
        } => (
            ProbeWebsiteEventType::ApprovalRequested,
            correlation_for_session(context, session_id, None),
            approval_requested_payload(*round_trip, tool),
        ),
        RuntimeEvent::ModelRequestFailed {
            session_id,
            round_trip,
            backend_kind,
            error,
        } => {
            let mut payload = Map::new();
            payload.insert(String::from("roundTrip"), json!(round_trip));
            payload.insert(String::from("backendKind"), json!(backend_kind));
            payload.insert(String::from("errorPreview"), json!(safe_preview(error)));
            payload.insert(String::from("errorHash"), json!(stable_text_digest(error)));
            (
                ProbeWebsiteEventType::RunFailed,
                correlation_for_session(context, session_id, None),
                payload,
            )
        }
        RuntimeEvent::AssistantTurnCommitted {
            session_id,
            response_id,
            response_model,
            assistant_text,
        } => {
            let mut payload = Map::new();
            payload.insert(String::from("responseId"), json!(response_id));
            payload.insert(String::from("responseModel"), json!(response_model));
            payload.insert(
                String::from("assistantTextPreview"),
                json!(safe_preview(assistant_text)),
            );
            payload.insert(
                String::from("assistantTextHash"),
                json!(stable_text_digest(assistant_text)),
            );
            (
                ProbeWebsiteEventType::RunCompleted,
                correlation_for_session(context, session_id, None),
                payload,
            )
        }
        RuntimeEvent::ToolCallRequested {
            session_id,
            round_trip,
            call_id,
            tool_name,
            arguments,
        } => {
            let mut payload = Map::new();
            payload.insert(String::from("roundTrip"), json!(round_trip));
            payload.insert(String::from("callId"), json!(call_id));
            payload.insert(String::from("toolName"), json!(tool_name));
            payload.insert(
                String::from("argumentsHash"),
                json!(stable_json_digest(arguments)),
            );
            payload.insert(
                String::from("argumentsSummary"),
                tool_input_summary(tool_name, arguments),
            );
            (
                ProbeWebsiteEventType::ToolCallStarted,
                correlation_for_session(context, session_id, None),
                payload,
            )
        }
        other => fallback_runtime_payload(other, context),
    };

    if correlation.probe_session_id.is_none() {
        correlation.probe_session_id = context.correlation.probe_session_id.clone();
    }

    ProbeWebsiteEvent::new(
        sequence,
        occurred_at_ms,
        event_type,
        context.actor.clone(),
        context.source.clone(),
        correlation,
        payload,
    )
    .with_artifact_refs(context.artifact_refs.clone())
}

#[must_use]
pub fn approval_resolved_event(
    sequence: u64,
    occurred_at_ms: u64,
    context: &WebsiteEventExportContext,
    approval_id: impl Into<String>,
    resolution: ToolApprovalResolution,
) -> ProbeWebsiteEvent {
    let mut payload = Map::new();
    payload.insert(String::from("approvalId"), json!(approval_id.into()));
    payload.insert(String::from("resolution"), json!(resolution));
    ProbeWebsiteEvent::new(
        sequence,
        occurred_at_ms,
        ProbeWebsiteEventType::ApprovalResolved,
        context.actor.clone(),
        context.source.clone(),
        context.correlation.clone(),
        payload,
    )
}

#[must_use]
pub fn child_session_event(
    sequence: u64,
    occurred_at_ms: u64,
    context: &WebsiteEventExportContext,
    child_session_id: &SessionId,
    status: SessionChildStatus,
    started: bool,
) -> ProbeWebsiteEvent {
    let mut correlation = context.correlation.clone();
    correlation.child_probe_session_id = Some(child_session_id.as_str().to_string());

    let mut payload = Map::new();
    payload.insert(
        String::from("childProbeSessionId"),
        json!(child_session_id.as_str()),
    );
    payload.insert(String::from("status"), json!(status));

    ProbeWebsiteEvent::new(
        sequence,
        occurred_at_ms,
        if started {
            ProbeWebsiteEventType::ChildSessionStarted
        } else {
            ProbeWebsiteEventType::ChildSessionUpdated
        },
        context.actor.clone(),
        context.source.clone(),
        correlation,
        payload,
    )
}

#[must_use]
pub fn artifact_ref_event(
    sequence: u64,
    occurred_at_ms: u64,
    context: &WebsiteEventExportContext,
    artifact_ref: ProbeWebsiteArtifactRef,
) -> ProbeWebsiteEvent {
    ProbeWebsiteEvent::new(
        sequence,
        occurred_at_ms,
        ProbeWebsiteEventType::ArtifactRef,
        context.actor.clone(),
        context.source.clone(),
        context.correlation.clone(),
        artifact_payload(&artifact_ref),
    )
    .with_artifact_refs(vec![artifact_ref])
}

#[must_use]
pub fn run_cancelled_event(
    sequence: u64,
    occurred_at_ms: u64,
    context: &WebsiteEventExportContext,
    reason: impl AsRef<str>,
) -> ProbeWebsiteEvent {
    let mut payload = Map::new();
    payload.insert(
        String::from("reasonPreview"),
        json!(safe_preview(reason.as_ref())),
    );
    payload.insert(
        String::from("reasonHash"),
        json!(stable_text_digest(reason.as_ref())),
    );
    ProbeWebsiteEvent::new(
        sequence,
        occurred_at_ms,
        ProbeWebsiteEventType::RunCancelled,
        context.actor.clone(),
        context.source.clone(),
        context.correlation.clone(),
        payload,
    )
}

pub fn transcript_artifact_ref(
    session_id: &SessionId,
    transcript: &[TranscriptEvent],
) -> Result<ProbeWebsiteArtifactRef, serde_json::Error> {
    Ok(ProbeWebsiteArtifactRef {
        kind: ProbeWebsiteArtifactKind::Transcript,
        resource_ref: format!("probe://sessions/{}/transcript", session_id.as_str()),
        stable_digest: Some(stable_json_digest_result(transcript)?),
        label: Some(String::from("Probe transcript")),
        updated_at_ms: None,
    })
}

#[must_use]
pub fn summary_artifact_ref(reference: &SessionSummaryArtifactRef) -> ProbeWebsiteArtifactRef {
    ProbeWebsiteArtifactRef {
        kind: match &reference.kind {
            SessionSummaryArtifactKind::RetainedSessionSummary => {
                ProbeWebsiteArtifactKind::RetainedSessionSummary
            }
            SessionSummaryArtifactKind::AcceptedPatchSummary => {
                ProbeWebsiteArtifactKind::AcceptedPatchSummary
            }
        },
        resource_ref: format!("probe://artifacts/{}", reference.artifact_id),
        stable_digest: Some(reference.stable_digest.clone()),
        label: Some(reference.artifact_id.clone()),
        updated_at_ms: Some(reference.updated_at_ms),
    }
}

#[must_use]
pub fn verification_pack_artifact_ref(
    resource_ref: impl Into<String>,
    stable_digest: impl Into<String>,
) -> ProbeWebsiteArtifactRef {
    ProbeWebsiteArtifactRef {
        kind: ProbeWebsiteArtifactKind::VerificationPack,
        resource_ref: resource_ref.into(),
        stable_digest: Some(stable_digest.into()),
        label: Some(String::from("Probe worker verification pack")),
        updated_at_ms: None,
    }
}

fn fallback_runtime_payload(
    event: &RuntimeEvent,
    context: &WebsiteEventExportContext,
) -> (
    ProbeWebsiteEventType,
    ProbeWebsiteEventCorrelation,
    Map<String, Value>,
) {
    let mut payload = Map::new();
    payload.insert(
        String::from("runtimeEventKind"),
        json!(runtime_event_name(event)),
    );
    (
        ProbeWebsiteEventType::RuntimeProgress,
        context.correlation.clone(),
        payload,
    )
}

fn tool_payload(round_trip: usize, tool: &ExecutedToolCall, status: &str) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert(String::from("roundTrip"), json!(round_trip));
    payload.insert(String::from("callId"), json!(tool.call_id.as_str()));
    payload.insert(String::from("toolName"), json!(tool.name.as_str()));
    payload.insert(String::from("status"), json!(status));
    payload.insert(
        String::from("riskClass"),
        json!(tool.tool_execution.risk_class),
    );
    payload.insert(
        String::from("policyDecision"),
        json!(tool.tool_execution.policy_decision),
    );
    payload.insert(
        String::from("approvalState"),
        json!(tool.tool_execution.approval_state),
    );
    payload.insert(
        String::from("argumentsHash"),
        json!(stable_json_digest(&tool.arguments)),
    );
    payload.insert(
        String::from("argumentsSummary"),
        tool_input_summary(tool.name.as_str(), &tool.arguments),
    );
    payload.insert(
        String::from("outputHash"),
        json!(stable_json_digest(&tool.output)),
    );
    payload.insert(
        String::from("outputSummary"),
        tool_output_summary(tool.name.as_str(), &tool.output),
    );
    payload.insert(
        String::from("filesTouchedCount"),
        json!(tool.tool_execution.files_touched.len()),
    );
    payload
}

fn approval_requested_payload(round_trip: usize, tool: &ExecutedToolCall) -> Map<String, Value> {
    let mut payload = tool_payload(round_trip, tool, "approval_required");
    payload.insert(String::from("approvalId"), json!(tool.call_id.as_str()));
    payload.insert(
        String::from("summary"),
        json!(format!(
            "Approval required for `{}` ({})",
            tool.name, tool.call_id
        )),
    );
    payload
}

fn artifact_payload(artifact_ref: &ProbeWebsiteArtifactRef) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert(String::from("kind"), json!(&artifact_ref.kind));
    payload.insert(
        String::from("resourceRef"),
        json!(artifact_ref.resource_ref.as_str()),
    );
    if let Some(digest) = artifact_ref.stable_digest.as_deref() {
        payload.insert(String::from("stableDigest"), json!(digest));
    }
    if let Some(label) = artifact_ref.label.as_deref() {
        payload.insert(String::from("label"), json!(label));
    }
    payload
}

fn correlation_for_session(
    context: &WebsiteEventExportContext,
    session_id: &SessionId,
    turn_id: Option<String>,
) -> ProbeWebsiteEventCorrelation {
    let mut correlation = context.correlation.clone();
    correlation.probe_session_id = Some(session_id.as_str().to_string());
    if let Some(turn_id) = turn_id {
        correlation.probe_turn_id = Some(turn_id);
    }
    correlation
}

fn safe_preview(text: &str) -> String {
    let redacted = redact_text(text);
    let mut preview = redacted
        .chars()
        .take(DEFAULT_PREVIEW_CHARS)
        .collect::<String>();
    if redacted.chars().count() > DEFAULT_PREVIEW_CHARS {
        preview.push_str("...");
    }
    preview
}

fn redact_text(text: &str) -> String {
    text.split_whitespace()
        .map(redact_token)
        .collect::<Vec<_>>()
        .join(" ")
        .replace("/Users/", "[redacted-path]/")
        .replace("/private/var/", "[redacted-path]/")
}

fn redact_token(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    if token.starts_with("sk-")
        || token.starts_with("sess-")
        || lower.contains("refresh_token")
        || lower.contains("access_token")
        || lower.contains("authorization:")
        || lower.contains("bearer ")
    {
        String::from("[redacted]")
    } else {
        token.to_string()
    }
}

fn stable_text_digest(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"probe_website_text|");
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}

fn stable_json_digest<T: Serialize + ?Sized>(value: &T) -> String {
    stable_json_digest_result(value).unwrap_or_else(|_| String::from("json_digest_unavailable"))
}

fn stable_json_digest_result<T: Serialize + ?Sized>(
    value: &T,
) -> Result<String, serde_json::Error> {
    let encoded = serde_json::to_vec(value)?;
    let mut hasher = Sha256::new();
    hasher.update(b"probe_website_json|");
    hasher.update(encoded);
    Ok(hex::encode(hasher.finalize()))
}

fn runtime_event_name(event: &RuntimeEvent) -> &'static str {
    match event {
        RuntimeEvent::TurnStarted { .. } => "turn_started",
        RuntimeEvent::ModelRequestStarted { .. } => "model_request_started",
        RuntimeEvent::AssistantStreamStarted { .. } => "assistant_stream_started",
        RuntimeEvent::TimeToFirstTokenObserved { .. } => "time_to_first_token_observed",
        RuntimeEvent::AssistantDelta { .. } => "assistant_delta",
        RuntimeEvent::AssistantSnapshot { .. } => "assistant_snapshot",
        RuntimeEvent::ToolCallDelta { .. } => "tool_call_delta",
        RuntimeEvent::ToolCallRequested { .. } => "tool_call_requested",
        RuntimeEvent::ToolExecutionStarted { .. } => "tool_execution_started",
        RuntimeEvent::ToolExecutionCompleted { .. } => "tool_execution_completed",
        RuntimeEvent::ToolRefused { .. } => "tool_refused",
        RuntimeEvent::ToolPaused { .. } => "tool_paused",
        RuntimeEvent::AssistantStreamFinished { .. } => "assistant_stream_finished",
        RuntimeEvent::ModelRequestFailed { .. } => "model_request_failed",
        RuntimeEvent::AssistantTurnCommitted { .. } => "assistant_turn_committed",
    }
}

#[cfg(test)]
mod tests {
    use probe_protocol::backend::BackendKind;
    use probe_protocol::session::{
        ItemId, SessionChildStatus, SessionId, ToolApprovalState, ToolExecutionRecord,
        ToolPolicyDecision, ToolRiskClass, TranscriptEvent, TranscriptItem, TranscriptItemKind,
        TurnId,
    };
    use probe_protocol::website_events::{ProbeWebsiteEventCorrelation, ProbeWebsiteEventType};
    use serde_json::json;

    use super::{
        WebsiteEventExportContext, artifact_ref_event, child_session_event,
        runtime_event_to_website_event, transcript_artifact_ref,
    };
    use crate::runtime::RuntimeEvent;
    use crate::tools::ExecutedToolCall;

    #[test]
    fn runtime_events_map_to_stable_website_events_without_secret_payloads() {
        let session_id = SessionId::new("sess-visible");
        let context = WebsiteEventExportContext::probe_runtime(&session_id).with_correlation(
            ProbeWebsiteEventCorrelation {
                request_id: Some(String::from("request-1")),
                run_id: Some(String::from("run-1")),
                ..ProbeWebsiteEventCorrelation::default()
            },
        );

        let event = RuntimeEvent::TurnStarted {
            session_id: session_id.clone(),
            profile_name: String::from("openai-codex-subscription"),
            prompt: String::from("Use sk-should-not-leak from /Users/example/secret.txt"),
            tool_loop_enabled: true,
        };

        let website_event = runtime_event_to_website_event(&event, 1, 1_000, &context);
        let encoded = serde_json::to_string(&website_event).expect("serialize event");

        assert_eq!(website_event.event_type, ProbeWebsiteEventType::RunStarted);
        assert_eq!(website_event.sequence, 1);
        assert!(encoded.contains("promptHash"));
        assert!(!encoded.contains("sk-should-not-leak"));
        assert!(!encoded.contains("/Users/example"));
    }

    #[test]
    fn approval_and_child_session_events_are_explicit() {
        let session_id = SessionId::new("sess-parent");
        let context = WebsiteEventExportContext::probe_runtime(&session_id);
        let tool = ExecutedToolCall {
            call_id: String::from("call-approval-1"),
            name: String::from("apply_patch"),
            arguments: json!({"path": "/Users/example/private.rs", "token": "sk-secret"}),
            output: json!({"status": "paused"}),
            tool_execution: ToolExecutionRecord {
                risk_class: ToolRiskClass::Write,
                policy_decision: ToolPolicyDecision::Paused,
                approval_state: ToolApprovalState::Pending,
                command: None,
                exit_code: None,
                timed_out: None,
                truncated: None,
                bytes_returned: None,
                files_touched: vec![String::from("/Users/example/private.rs")],
                reason: Some(String::from("write approval required")),
            },
        };
        let paused = RuntimeEvent::ToolPaused {
            session_id,
            round_trip: 1,
            tool,
        };

        let approval = runtime_event_to_website_event(&paused, 2, 2_000, &context);
        let child = child_session_event(
            3,
            3_000,
            &context,
            &SessionId::new("sess-child"),
            SessionChildStatus::Running,
            true,
        );
        let encoded = serde_json::to_string(&(&approval, &child)).expect("serialize events");

        assert_eq!(
            approval.event_type,
            ProbeWebsiteEventType::ApprovalRequested
        );
        assert_eq!(child.event_type, ProbeWebsiteEventType::ChildSessionStarted);
        assert!(encoded.contains("approvalId"));
        assert!(encoded.contains("childProbeSessionId"));
        assert!(!encoded.contains("sk-secret"));
        assert!(!encoded.contains("/Users/example"));
    }

    #[test]
    fn transcript_and_artifact_refs_are_digestible() {
        let session_id = SessionId::new("sess-artifact");
        let transcript = vec![TranscriptEvent {
            session_id: session_id.clone(),
            turn: probe_protocol::session::SessionTurn {
                id: TurnId(0),
                index: 0,
                started_at_ms: 10,
                completed_at_ms: Some(11),
                observability: None,
                backend_receipt: None,
                items: vec![TranscriptItem {
                    id: ItemId::new("item-0"),
                    turn_id: TurnId(0),
                    sequence: 0,
                    kind: TranscriptItemKind::AssistantMessage,
                    text: String::from("done"),
                    name: None,
                    tool_call_id: None,
                    arguments: None,
                    tool_execution: None,
                }],
            },
        }];
        let reference =
            transcript_artifact_ref(&session_id, transcript.as_slice()).expect("transcript ref");
        let context = WebsiteEventExportContext::probe_runtime(&session_id);
        let event = artifact_ref_event(4, 4_000, &context, reference.clone());

        assert_eq!(event.event_type, ProbeWebsiteEventType::ArtifactRef);
        assert!(reference.resource_ref.starts_with("probe://sessions/"));
        assert!(reference.stable_digest.is_some());
    }

    #[test]
    fn failed_runtime_event_uses_safe_error_preview_and_hash() {
        let session_id = SessionId::new("sess-failed");
        let context = WebsiteEventExportContext::probe_runtime(&session_id);
        let event = RuntimeEvent::ModelRequestFailed {
            session_id,
            round_trip: 1,
            backend_kind: BackendKind::OpenAiCodexSubscription,
            error: String::from("provider failed with bearer token sk-nope"),
        };

        let website_event = runtime_event_to_website_event(&event, 5, 5_000, &context);
        let encoded = serde_json::to_string(&website_event).expect("serialize event");

        assert_eq!(website_event.event_type, ProbeWebsiteEventType::RunFailed);
        assert!(encoded.contains("errorHash"));
        assert!(!encoded.contains("sk-nope"));
    }
}
