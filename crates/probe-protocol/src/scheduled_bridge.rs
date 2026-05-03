use crate::website_events::{ProbeWebsiteArtifactRef, ProbeWebsiteEventBatch};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const PROBE_SCHEDULED_AGENT_BRIDGE_SCHEMA_VERSION: &str = "probe.scheduled_agent_bridge.v1";
pub const PROBE_SCHEDULED_AGENT_BRIDGE_SIGNATURE_CONTEXT: &str = "probe-scheduled-agent-bridge-v1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeSignedRequest {
    pub auth: ScheduledAgentBridgeAuth,
    pub request: ScheduledAgentBridgeRequest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeAuth {
    pub key_id: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeRequest {
    pub schema_version: String,
    pub request_id: String,
    pub workspace: String,
    pub actor: ScheduledAgentBridgeActor,
    pub conversation: ScheduledAgentBridgeConversationRef,
    pub run: ScheduledAgentBridgeRunRef,
    pub schedule: ScheduledAgentBridgeScheduleRef,
    pub wake: ScheduledAgentBridgeWakeRef,
    pub orchestration_job: ScheduledAgentBridgeOrchestrationJobRef,
    pub goal: ScheduledAgentBridgeGoal,
    #[serde(default)]
    pub context: ScheduledAgentBridgeContext,
    pub backend: ScheduledAgentBridgeBackendSelection,
    pub tool_policy: ScheduledAgentBridgeToolPolicy,
    pub idempotency_key: String,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeActor {
    pub web_user_id: u64,
    pub email: String,
    pub role: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeConversationRef {
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeRunRef {
    pub run_id: String,
    pub scheduled_run_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeScheduleRef {
    pub schedule_id: String,
    pub name: String,
    pub regularity: ScheduledAgentBridgeRegularity,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeRegularity {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeWakeRef {
    pub wake_id: String,
    pub due_at_ms: u64,
    pub attempt: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeOrchestrationJobRef {
    pub orchestration_job_id: String,
    pub queue: String,
    pub attempt: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeGoal {
    pub master_goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_goal: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default)]
    pub issue_refs: Vec<String>,
    #[serde(default)]
    pub device_refs: Vec<String>,
    #[serde(default)]
    pub memory_refs: Vec<String>,
    #[serde(default)]
    pub state_snapshot_refs: Vec<ProbeWebsiteArtifactRef>,
    #[serde(default)]
    pub instructions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeBackendSelection {
    pub key: String,
    pub family: String,
    pub profile: String,
    pub model: String,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeToolPolicy {
    pub mode: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    pub approval_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledAgentBridgeRunStatus {
    Accepted,
    Running,
    ApprovalRequired,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeAcceptedResponse {
    pub schema_version: String,
    pub request_id: String,
    pub run_id: String,
    pub scheduled_run_id: String,
    pub probe_session_id: String,
    pub probe_turn_id: String,
    pub status: ScheduledAgentBridgeRunStatus,
    pub backend: ScheduledAgentBridgeBackendSelection,
    pub transcript_ref: String,
    pub correlation: ScheduledAgentBridgeCorrelation,
    #[serde(default)]
    pub diagnostics: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeCorrelation {
    pub request_id: String,
    pub workspace: String,
    pub conversation_id: String,
    pub run_id: String,
    pub schedule_id: String,
    pub wake_id: String,
    pub scheduled_run_id: String,
    pub orchestration_job_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledAgentBridgeApprovalStatus {
    Pending,
    Approved,
    Rejected,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeApproval {
    pub approval_id: String,
    pub status: ScheduledAgentBridgeApprovalStatus,
    pub action_ref: String,
    pub risk_class: String,
    pub tool_name: String,
    pub call_id: String,
    pub summary: String,
    pub requested_at_ms: u64,
    #[serde(default)]
    pub payload_summary: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeErrorResponse {
    pub schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub error: ScheduledAgentBridgeError,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeError {
    pub code: String,
    pub message: String,
    pub category: String,
    pub retryable: bool,
    #[serde(default)]
    pub diagnostics: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAgentBridgeContractFixture {
    pub name: String,
    pub scenario: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_request: Option<ScheduledAgentBridgeSignedRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_response: Option<ScheduledAgentBridgeAcceptedResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ScheduledAgentBridgeApproval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_batch: Option<ProbeWebsiteEventBatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_response: Option<ScheduledAgentBridgeErrorResponse>,
}
