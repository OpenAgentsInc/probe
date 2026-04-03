use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::backend::{BackendControlPlaneKind, BackendProfile, PsionicMeshAttachInfo};

pub type TimestampMs = u64;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ItemId(String);

impl ItemId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TurnId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Active,
    Archived,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptItemKind {
    UserMessage,
    AssistantMessage,
    ToolCall,
    ToolResult,
    Note,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheSignal {
    Unknown,
    ColdStart,
    LikelyWarm,
    NoClearSignal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageTruth {
    Exact,
    Estimated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageMeasurement {
    pub value: u64,
    pub truth: UsageTruth,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnObservability {
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendFailureReceipt {
    pub family: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_suggestion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal_explanation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendAvailabilityReceipt {
    pub ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendTranscriptReceipt {
    pub format: String,
    pub payload: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendTurnReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<BackendFailureReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<BackendAvailabilityReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<BackendTranscriptReceipt>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRiskClass {
    ReadOnly,
    ShellReadOnly,
    Write,
    Network,
    Destructive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicyDecision {
    AutoAllow,
    Approved,
    Refused,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolApprovalState {
    NotRequired,
    Approved,
    Refused,
    Pending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolApprovalResolution {
    Approved,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolExecutionRecord {
    pub risk_class: ToolRiskClass,
    pub policy_decision: ToolPolicyDecision,
    pub approval_state: ToolApprovalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timed_out: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_returned: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_touched: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingToolApproval {
    pub session_id: SessionId,
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub risk_class: ToolRiskClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub tool_call_turn_index: u64,
    pub paused_result_turn_index: u64,
    pub requested_at_ms: TimestampMs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at_ms: Option<TimestampMs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<ToolApprovalResolution>,
}

impl PendingToolApproval {
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.resolution.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptItem {
    pub id: ItemId,
    pub turn_id: TurnId,
    pub sequence: u32,
    pub kind: TranscriptItemKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_execution: Option<ToolExecutionRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTurn {
    pub id: TurnId,
    pub index: u64,
    pub started_at_ms: TimestampMs,
    pub completed_at_ms: Option<TimestampMs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability: Option<TurnObservability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_receipt: Option<BackendTurnReceipt>,
    pub items: Vec<TranscriptItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBackendTarget {
    pub profile_name: String,
    pub base_url: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_plane: Option<BackendControlPlaneKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psionic_mesh: Option<PsionicMeshAttachInfo>,
}

impl SessionBackendTarget {
    #[must_use]
    pub fn from_profile(profile: &BackendProfile) -> Self {
        Self {
            profile_name: profile.name.clone(),
            base_url: profile.base_url.clone(),
            model: profile.model.clone(),
            control_plane: profile.control_plane,
            psionic_mesh: profile.psionic_mesh.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMeshCoordinationMode {
    Disabled,
    Local,
    BootstrapProxy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMeshCoordinationKind {
    Status,
    Finding,
    Question,
    Tip,
    Done,
    Note,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMeshCoordinationVisibility {
    Mesh,
    OperatorInternal,
    NodeLocal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMeshCoordinationProvenance {
    LocalPost,
    BootstrapProxyForwarded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeshCoordinationRedactionReceipt {
    pub reason: String,
    pub redacted_by: String,
    pub redacted_at_ms: TimestampMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeshCoordinationEntry {
    pub id: u64,
    pub kind: SessionMeshCoordinationKind,
    pub author: String,
    pub worker_id: String,
    pub visibility: SessionMeshCoordinationVisibility,
    pub provenance: SessionMeshCoordinationProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub created_at_ms: TimestampMs,
    pub expires_at_ms: TimestampMs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redaction: Option<SessionMeshCoordinationRedactionReceipt>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeshCoordinationStatus {
    pub status: String,
    pub mode: SessionMeshCoordinationMode,
    pub feed_path: String,
    pub search_path: String,
    pub post_path: String,
    pub redact_path: String,
    pub ttl_secs: u64,
    pub max_items: usize,
    pub max_body_bytes: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_kinds: Vec<SessionMeshCoordinationKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_visibilities: Vec<SessionMeshCoordinationVisibility>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_provenances: Vec<SessionMeshCoordinationProvenance>,
    pub redaction_mode: String,
    pub item_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_post_at_ms: Option<TimestampMs>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMountKind {
    KnowledgePack,
    EvalPack,
    #[serde(other)]
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMountProvenance {
    pub publisher: String,
    pub source_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMountRef {
    pub mount_id: String,
    pub kind: SessionMountKind,
    pub resource_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub provenance: SessionMountProvenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRuntimeOwnerKind {
    ForegroundChild,
    LocalDaemon,
    HostedControlPlane,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionAttachTransport {
    StdioJsonl,
    UnixSocketJsonl,
    TcpJsonl,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRuntimeOwner {
    pub kind: SessionRuntimeOwnerKind,
    pub owner_id: String,
    pub attach_transport: SessionAttachTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach_target: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionHostedAuthKind {
    ControlPlaneAssertion,
    OperatorToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHostedAuthReceipt {
    pub authority: String,
    pub subject: String,
    pub auth_kind: SessionHostedAuthKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub recorded_at_ms: TimestampMs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionHostedCheckoutKind {
    GitRepository,
    PlainWorkspace,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHostedCheckoutReceipt {
    pub kind: SessionHostedCheckoutKind,
    pub workspace_root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub recorded_at_ms: TimestampMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHostedWorkerReceipt {
    pub owner_kind: SessionRuntimeOwnerKind,
    pub owner_id: String,
    pub attach_transport: SessionAttachTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach_target: Option<String>,
    pub execution_host_kind: SessionExecutionHostKind,
    pub execution_host_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_host_label: Option<String>,
    pub recorded_at_ms: TimestampMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHostedCostReceipt {
    pub observed_turn_count: u64,
    pub wallclock_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub recorded_at_ms: TimestampMs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionHostedCleanupStatus {
    NotRequired,
    Pending,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHostedCleanupReceipt {
    pub status: SessionHostedCleanupStatus,
    pub workspace_root: PathBuf,
    pub strategy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub recorded_at_ms: TimestampMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionHostedLifecycleEvent {
    ControlPlaneRestartObserved {
        control_plane_started_at_ms: TimestampMs,
        session_owner_id: String,
        execution_host_id: String,
        summary: String,
        recorded_at_ms: TimestampMs,
    },
    RunningTurnFailedOnRestart {
        turn_id: String,
        session_owner_id: String,
        execution_host_id: String,
        summary: String,
        recorded_at_ms: TimestampMs,
    },
    ApprovalPausedTakeoverAvailable {
        turn_id: String,
        session_owner_id: String,
        execution_host_id: String,
        pending_approval_count: usize,
        summary: String,
        recorded_at_ms: TimestampMs,
    },
    CleanupStateChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_status: Option<SessionHostedCleanupStatus>,
        status: SessionHostedCleanupStatus,
        workspace_root: PathBuf,
        strategy: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_host_id: Option<String>,
        summary: String,
        recorded_at_ms: TimestampMs,
    },
    OrphanedManagedWorkspaceReaped {
        workspace_root: PathBuf,
        session_owner_id: String,
        execution_host_id: String,
        summary: String,
        recorded_at_ms: TimestampMs,
    },
    ControllerLeaseChanged {
        action: SessionControllerAction,
        actor_participant_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_participant_id: Option<String>,
        session_owner_id: String,
        execution_host_id: String,
        summary: String,
        recorded_at_ms: TimestampMs,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHostedReceipts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<SessionHostedAuthReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<SessionHostedCheckoutReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<SessionHostedWorkerReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<SessionHostedCostReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<SessionHostedCleanupReceipt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<SessionHostedLifecycleEvent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionWorkspaceBootMode {
    Fresh,
    PreparedBaseline,
    SnapshotRestore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPreparedBaselineStatus {
    Ready,
    Missing,
    Stale,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPreparedBaselineRef {
    pub baseline_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    pub status: SessionPreparedBaselineStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionWorkspaceSnapshotRef {
    pub snapshot_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_manifest_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_baseline_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionExecutionHostKind {
    LocalMachine,
    HostedWorker,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionExecutionHost {
    pub kind: SessionExecutionHostKind,
    pub host_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionWorkspaceState {
    pub boot_mode: SessionWorkspaceBootMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<SessionPreparedBaselineRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SessionWorkspaceSnapshotRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_host: Option<SessionExecutionHost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance_note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHarnessProfile {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionParentLink {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiator: Option<SessionInitiator>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionChildLink {
    pub session_id: SessionId,
    pub added_at_ms: TimestampMs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionChildStatus {
    Idle,
    Running,
    Queued,
    ApprovalPaused,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionChildSummary {
    pub session_id: SessionId,
    pub title: String,
    pub cwd: PathBuf,
    pub state: SessionState,
    pub status: SessionChildStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiator: Option<SessionInitiator>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_turn_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closure: Option<SessionChildClosureSummary>,
    pub created_at_ms: TimestampMs,
    pub updated_at_ms: TimestampMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInitiator {
    pub client_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participant_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionParticipant {
    pub participant_id: String,
    pub client_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub attached_at_ms: TimestampMs,
    pub last_seen_at_ms: TimestampMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionControllerLease {
    pub participant_id: String,
    pub acquired_at_ms: TimestampMs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionControllerAction {
    Claim,
    Release,
    Handoff,
    Takeover,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionChildClosureSummary {
    pub status: SessionChildStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_status: Option<SessionDeliveryStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare_ref: Option<String>,
    pub updated_at_ms: TimestampMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBranchState {
    pub repo_root: PathBuf,
    pub head_ref: String,
    pub head_commit: String,
    pub detached_head: bool,
    pub working_tree_dirty: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead_by: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind_by: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionDeliveryStatus {
    NeedsCommit,
    LocalOnly,
    NeedsPush,
    Synced,
    Diverged,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDeliveryArtifact {
    pub kind: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDeliveryState {
    pub status: SessionDeliveryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_tracking_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare_ref: Option<String>,
    pub updated_at_ms: TimestampMs,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<SessionDeliveryArtifact>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSummaryArtifactKind {
    RetainedSessionSummary,
    AcceptedPatchSummary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummaryArtifactRef {
    pub artifact_id: String,
    pub kind: SessionSummaryArtifactKind,
    pub path: PathBuf,
    pub stable_digest: String,
    pub updated_at_ms: TimestampMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummaryArtifactSource {
    pub session_id: SessionId,
    pub transcript_path: PathBuf,
    pub transcript_sha256: String,
    pub session_created_at_ms: TimestampMs,
    pub session_updated_at_ms: TimestampMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainedSessionSummaryArtifact {
    pub schema_version: String,
    pub artifact: SessionSummaryArtifactRef,
    pub session_id: SessionId,
    pub title: String,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_profile: Option<String>,
    pub turn_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_listed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_searched: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_read: Vec<String>,
    pub patch_attempts: usize,
    pub successful_patch_attempts: usize,
    pub failed_patch_attempts: usize,
    pub verification_step_count: usize,
    pub verification_caught_problem: bool,
    pub summary_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_assistant_text: Option<String>,
    pub source: SessionSummaryArtifactSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedPatchSummarySourceTurn {
    pub turn_index: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_touched: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedPatchSummaryArtifact {
    pub schema_version: String,
    pub artifact: SessionSummaryArtifactRef,
    pub session_id: SessionId,
    pub title: String,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patch_turns: Vec<AcceptedPatchSummarySourceTurn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_touched: Vec<String>,
    pub summary_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_assistant_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_state: Option<SessionBranchState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_state: Option<SessionDeliveryState>,
    pub source: SessionSummaryArtifactSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionSummaryArtifact {
    RetainedSessionSummary(RetainedSessionSummaryArtifact),
    AcceptedPatchSummary(AcceptedPatchSummaryArtifact),
}

impl SessionSummaryArtifact {
    #[must_use]
    pub fn artifact_ref(&self) -> &SessionSummaryArtifactRef {
        match self {
            Self::RetainedSessionSummary(artifact) => &artifact.artifact,
            Self::AcceptedPatchSummary(artifact) => &artifact.artifact,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub id: SessionId,
    pub title: String,
    pub cwd: PathBuf,
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_profile: Option<SessionHarnessProfile>,
    pub created_at_ms: TimestampMs,
    pub updated_at_ms: TimestampMs,
    pub state: SessionState,
    pub next_turn_index: u64,
    pub backend: Option<SessionBackendTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_owner: Option<SessionRuntimeOwner>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_state: Option<SessionWorkspaceState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosted_receipts: Option<SessionHostedReceipts>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounted_refs: Vec<SessionMountRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<SessionParticipant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_lease: Option<SessionControllerLease>,
    pub transcript_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_link: Option<SessionParentLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_links: Vec<SessionChildLink>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionIndex {
    pub sessions: Vec<SessionMetadata>,
}

impl Default for SessionIndex {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptEvent {
    pub session_id: SessionId,
    pub turn: SessionTurn,
}
