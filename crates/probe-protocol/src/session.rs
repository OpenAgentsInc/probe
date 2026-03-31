use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHarnessProfile {
    pub name: String,
    pub version: String,
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
    pub transcript_path: PathBuf,
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
