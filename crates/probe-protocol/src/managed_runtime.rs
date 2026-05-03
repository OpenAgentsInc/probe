use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::backend::BackendProfile;
use crate::managed_environment::{
    ManagedEnvironmentConstraints, ManagedEnvironmentWorkerAdvertisement,
};
use crate::runtime::{ToolLoopRecipe, TurnAuthor};
use crate::session::{
    PendingToolApproval, SessionHarnessProfile, SessionId, SessionMountRef, SessionWorkspaceState,
    TimestampMs, ToolApprovalResolution, ToolExecutionRecord, ToolRiskClass,
};

pub const PROBE_MANAGED_RUNTIME_SCHEMA_VERSION: &str = "probe.managed_runtime.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedRuntimeOperationKind {
    CreateSession,
    StartSession,
    ResumeSession,
    InterruptSession,
    CancelSession,
    ResolveApproval,
    ReplayEvents,
    Heartbeat,
    SpawnChildSession,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedRuntimeSessionStatus {
    Created,
    Starting,
    Running,
    Queued,
    ApprovalPaused,
    Interrupting,
    Interrupted,
    Cancelling,
    Cancelled,
    Completed,
    Failed,
    TimedOut,
}

impl ManagedRuntimeSessionStatus {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Interrupted | Self::Cancelled | Self::Completed | Self::Failed | Self::TimedOut
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedRuntimeEventType {
    SessionCreated,
    SessionStarted,
    TurnQueued,
    TurnStarted,
    TextDelta,
    ToolCallRequested,
    ToolCallCompleted,
    CustomToolResult,
    ApprovalRequested,
    ApprovalResolved,
    TranscriptRef,
    ArtifactRef,
    ChildSessionSpawned,
    Heartbeat,
    StatusChanged,
    SessionInterrupted,
    SessionCancelled,
    SessionCompleted,
    SessionFailed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntimeActor {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntimeSource {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntimeCorrelation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_agent_version_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_environment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_order_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_probe_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_probe_session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSessionRef {
    pub probe_session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_probe_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_probe_session_id: Option<String>,
}

impl ManagedSessionRef {
    #[must_use]
    pub fn new(probe_session_id: SessionId) -> Self {
        Self {
            probe_session_id,
            managed_session_id: None,
            parent_probe_session_id: None,
            child_probe_session_id: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedRuntimeArtifactKind {
    Transcript,
    RetainedSessionSummary,
    AcceptedPatchSummary,
    VerificationPack,
    WorkspaceSnapshot,
    ToolOutput,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntimeArtifactRef {
    pub kind: ManagedRuntimeArtifactKind,
    pub resource_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<TimestampMs>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntimeToolCall {
    pub call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_class: Option<ToolRiskClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntimeToolResult {
    pub call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub output: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ToolExecutionRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntimeApproval {
    pub approval_id: String,
    pub call_id: String,
    pub tool_name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_class: Option<ToolRiskClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<ToolApprovalResolution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_tool_approval: Option<PendingToolApproval>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntimeErrorPayload {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(default)]
    pub details: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntimeSessionStatusProjection {
    pub session_ref: ManagedSessionRef,
    pub status: ManagedRuntimeSessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_at_ms: Option<TimestampMs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_probe_turn_id: Option<String>,
    #[serde(default)]
    pub pending_approval_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManagedRuntimeEventPayload {
    SessionLifecycle {
        title: String,
        cwd: PathBuf,
        backend_profile: String,
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        environment_constraints: Option<ManagedEnvironmentConstraints>,
    },
    TurnLifecycle {
        probe_turn_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_sha256: Option<String>,
    },
    TextDelta {
        delta: String,
    },
    ToolCall {
        tool: ManagedRuntimeToolCall,
    },
    ToolResult {
        result: ManagedRuntimeToolResult,
    },
    CustomToolResult {
        call_id: String,
        tool_name: String,
        #[serde(default)]
        result: Value,
    },
    Approval {
        approval: ManagedRuntimeApproval,
    },
    TranscriptRef {
        transcript: ManagedRuntimeArtifactRef,
    },
    ArtifactRef {
        artifact: ManagedRuntimeArtifactRef,
    },
    Error {
        error: ManagedRuntimeErrorPayload,
    },
    Terminal {
        status: ManagedRuntimeSessionStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Heartbeat {
        projection: ManagedRuntimeSessionStatusProjection,
    },
    ChildSession {
        child: ManagedSessionRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        purpose: Option<String>,
        status: ManagedRuntimeSessionStatus,
    },
    Status {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntimeEvent {
    pub schema_version: String,
    pub sequence: u64,
    pub occurred_at_ms: TimestampMs,
    pub event_type: ManagedRuntimeEventType,
    pub status: ManagedRuntimeSessionStatus,
    pub actor: ManagedRuntimeActor,
    pub source: ManagedRuntimeSource,
    pub session: ManagedSessionRef,
    pub correlation: ManagedRuntimeCorrelation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<ManagedRuntimeArtifactRef>,
    pub payload: ManagedRuntimeEventPayload,
}

impl ManagedRuntimeEvent {
    #[must_use]
    pub fn new(
        sequence: u64,
        occurred_at_ms: TimestampMs,
        event_type: ManagedRuntimeEventType,
        status: ManagedRuntimeSessionStatus,
        actor: ManagedRuntimeActor,
        source: ManagedRuntimeSource,
        session: ManagedSessionRef,
        correlation: ManagedRuntimeCorrelation,
        payload: ManagedRuntimeEventPayload,
    ) -> Self {
        Self {
            schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
            sequence,
            occurred_at_ms,
            event_type,
            status,
            actor,
            source,
            session,
            correlation,
            artifact_refs: Vec::new(),
            payload,
        }
    }

    #[must_use]
    pub fn with_artifact_refs(mut self, artifact_refs: Vec<ManagedRuntimeArtifactRef>) -> Self {
        self.artifact_refs = artifact_refs;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSessionStartRequest {
    pub schema_version: String,
    pub request_id: String,
    pub idempotency_key: String,
    pub actor: ManagedRuntimeActor,
    #[serde(default)]
    pub correlation: ManagedRuntimeCorrelation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub profile: BackendProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_profile: Option<SessionHarnessProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_state: Option<SessionWorkspaceState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounted_refs: Vec<SessionMountRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_loop: Option<ToolLoopRecipe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_constraints: Option<ManagedEnvironmentConstraints>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSessionStartResponse {
    pub schema_version: String,
    pub request_id: String,
    pub session_ref: ManagedSessionRef,
    pub status: ManagedRuntimeSessionStatus,
    pub transcript_ref: ManagedRuntimeArtifactRef,
    pub replay_after_sequence: u64,
    pub next_sequence: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<ManagedRuntimeEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSessionResumeRequest {
    pub schema_version: String,
    pub request_id: String,
    pub actor: ManagedRuntimeActor,
    pub session_ref: ManagedSessionRef,
    #[serde(default)]
    pub correlation: ManagedRuntimeCorrelation,
    #[serde(default)]
    pub after_sequence: u64,
    #[serde(default)]
    pub include_snapshot: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSessionResumeResponse {
    pub schema_version: String,
    pub request_id: String,
    pub projection: ManagedRuntimeSessionStatusProjection,
    pub transcript_ref: ManagedRuntimeArtifactRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replayed_events: Vec<ManagedRuntimeEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_ref: Option<ManagedRuntimeArtifactRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSessionReplayRequest {
    pub schema_version: String,
    pub request_id: String,
    pub session_ref: ManagedSessionRef,
    #[serde(default)]
    pub after_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSessionReplayResponse {
    pub schema_version: String,
    pub request_id: String,
    pub session_ref: ManagedSessionRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<ManagedRuntimeEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest_sequence: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSessionControlRequest {
    pub schema_version: String,
    pub request_id: String,
    pub idempotency_key: String,
    pub actor: ManagedRuntimeActor,
    pub session_ref: ManagedSessionRef,
    #[serde(default)]
    pub correlation: ManagedRuntimeCorrelation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub cancel_queued_turns: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSessionControlResponse {
    pub schema_version: String,
    pub request_id: String,
    pub projection: ManagedRuntimeSessionStatusProjection,
    pub event: ManagedRuntimeEvent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedApprovalResolutionRequest {
    pub schema_version: String,
    pub request_id: String,
    pub idempotency_key: String,
    pub actor: ManagedRuntimeActor,
    pub session_ref: ManagedSessionRef,
    #[serde(default)]
    pub correlation: ManagedRuntimeCorrelation,
    pub approval_id: String,
    pub call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub resolution: ToolApprovalResolution,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<TurnAuthor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedApprovalResolutionResponse {
    pub schema_version: String,
    pub request_id: String,
    pub projection: ManagedRuntimeSessionStatusProjection,
    pub event: ManagedRuntimeEvent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntimeHeartbeatRequest {
    pub schema_version: String,
    pub request_id: String,
    pub worker_id: String,
    pub heartbeat_at_ms: TimestampMs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<ManagedEnvironmentWorkerAdvertisement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<ManagedRuntimeSessionStatusProjection>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntimeHeartbeatResponse {
    pub schema_version: String,
    pub request_id: String,
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<ManagedRuntimeEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedChildSessionHookRequest {
    pub schema_version: String,
    pub request_id: String,
    pub actor: ManagedRuntimeActor,
    pub parent: ManagedSessionRef,
    pub child: ManagedSessionRef,
    #[serde(default)]
    pub correlation: ManagedRuntimeCorrelation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    pub status: ManagedRuntimeSessionStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedChildSessionHookResponse {
    pub schema_version: String,
    pub request_id: String,
    pub projection: ManagedRuntimeSessionStatusProjection,
    pub event: ManagedRuntimeEvent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ManagedRuntimeRequest {
    StartSession(ManagedSessionStartRequest),
    ResumeSession(ManagedSessionResumeRequest),
    InterruptSession(ManagedSessionControlRequest),
    CancelSession(ManagedSessionControlRequest),
    ResolveApproval(ManagedApprovalResolutionRequest),
    ReplayEvents(ManagedSessionReplayRequest),
    Heartbeat(ManagedRuntimeHeartbeatRequest),
    RecordChildSession(ManagedChildSessionHookRequest),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntimeRequestEnvelope {
    pub request: ManagedRuntimeRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ManagedRuntimeResponse {
    StartSession(ManagedSessionStartResponse),
    ResumeSession(ManagedSessionResumeResponse),
    InterruptSession(ManagedSessionControlResponse),
    CancelSession(ManagedSessionControlResponse),
    ResolveApproval(ManagedApprovalResolutionResponse),
    ReplayEvents(ManagedSessionReplayResponse),
    Heartbeat(ManagedRuntimeHeartbeatResponse),
    RecordChildSession(ManagedChildSessionHookResponse),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntimeResponseEnvelope {
    pub response: ManagedRuntimeResponse,
}

#[must_use]
pub fn managed_runtime_transcript_ref(session_id: &SessionId) -> ManagedRuntimeArtifactRef {
    ManagedRuntimeArtifactRef {
        kind: ManagedRuntimeArtifactKind::Transcript,
        resource_ref: format!("probe://sessions/{}/transcript", session_id.as_str()),
        stable_digest: None,
        label: Some(String::from("Probe transcript")),
        updated_at_ms: None,
    }
}
