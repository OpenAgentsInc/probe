//! ACP v1 wire shapes, exactly as the sarah-computer-controller's SDK client
//! (`@agentclientprotocol/sdk`) speaks them: camelCase fields, session
//! updates discriminated on `sessionUpdate`, permission outcomes on
//! `outcome`. Protocol version is literally 1; a mismatch makes the
//! controller mark the delegation unavailable.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;

pub mod methods {
    pub const INITIALIZE: &str = "initialize";
    pub const SESSION_NEW: &str = "session/new";
    pub const SESSION_LOAD: &str = "session/load";
    pub const SESSION_PROMPT: &str = "session/prompt";
    pub const SESSION_CANCEL: &str = "session/cancel";
    pub const SESSION_UPDATE: &str = "session/update";
    pub const SESSION_REQUEST_PERMISSION: &str = "session/request_permission";
    pub const AUTHENTICATE: &str = "authenticate";
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsCapabilities {
    #[serde(default)]
    pub read_text_file: bool,
    #[serde(default)]
    pub write_text_file: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    #[serde(default)]
    pub fs: FsCapabilities,
    #[serde(default)]
    pub terminal: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: u16,
    #[serde(default)]
    pub client_capabilities: ClientCapabilities,
    #[serde(default)]
    pub client_info: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    pub load_session: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: u16,
    pub agent_capabilities: AgentCapabilities,
    /// Empty when a grant is present: probe never asks the controller to
    /// authenticate; a missing grant is a typed failure, not an auth dance.
    pub auth_methods: Vec<AuthMethod>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNewParams {
    pub cwd: String,
    #[serde(default)]
    pub mcp_servers: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNewResult {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLoadParams {
    pub session_id: String,
    pub cwd: String,
    #[serde(default)]
    pub mcp_servers: Vec<serde_json::Value>,
}

/// Prompt content blocks. Only text is meaningful to probe today; unknown
/// block types are preserved for forward compatibility, never misread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    #[serde(untagged)]
    Other(serde_json::Value),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptParams {
    pub session_id: String,
    pub prompt: Vec<ContentBlock>,
}

/// ACP stop reasons, in the vocabulary the controller maps to its terminal
/// statuses: `refusal` -> refused, `cancelled` -> cancelled/timeout,
/// anything else -> completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResult {
    pub stop_reason: StopReason,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelParams {
    pub session_id: String,
}

/// Tool-call content as the controller's `extractContentText` reads it:
/// entries whose `content.text` (or `text`) holds displayable text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolCallContent {
    Content { content: ContentBlock },
}

impl ToolCallContent {
    pub fn text(text: impl Into<String>) -> ToolCallContent {
        ToolCallContent::Content { content: ContentBlock::Text { text: text.into() } }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntry {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// `session/update` payloads, discriminated on `sessionUpdate` exactly as
/// the controller's `renderUpdate` switches on them. Fields it renders:
/// `toolCallId`, `kind`, `title`, `rawInput` (`command` preferred),
/// terminal `status`, and text content blocks. `agent_thought_chunk` is
/// dropped client-side — user-relevant information never rides only there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum SessionUpdate {
    AgentMessageChunk {
        content: ContentBlock,
    },
    AgentThoughtChunk {
        content: ContentBlock,
    },
    ToolCall {
        tool_call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_input: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ToolCallStatus>,
    },
    ToolCallUpdate {
        tool_call_id: String,
        status: ToolCallStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        content: Vec<ToolCallContent>,
    },
    Plan {
        entries: Vec<PlanEntry>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotification {
    pub session_id: String,
    pub update: SessionUpdate,
}

/// Permission option kinds. The controller prefers the conservative
/// one-shot (`allow_once`) and REFUSES any option whose id, name, or kind
/// matches /bypass/i — probe never offers one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: PermissionOptionKind,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionToolCall {
    pub tool_call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPermissionParams {
    pub session_id: String,
    pub tool_call: PermissionToolCall,
    pub options: Vec<PermissionOption>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum PermissionOutcome {
    Selected { option_id: String },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RequestPermissionResponse {
    pub outcome: PermissionOutcome,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_update_discriminates_on_session_update_field() {
        let update = SessionUpdate::AgentMessageChunk { content: ContentBlock::Text { text: "hi".into() } };
        let encoded = serde_json::to_value(&update).unwrap();
        assert_eq!(encoded["sessionUpdate"], "agent_message_chunk");
        assert_eq!(encoded["content"]["type"], "text");
    }

    #[test]
    fn tool_call_update_carries_camel_case_fields() {
        let update = SessionUpdate::ToolCallUpdate {
            tool_call_id: "t1".into(),
            status: ToolCallStatus::Completed,
            kind: Some("execute".into()),
            title: Some("ls".into()),
            content: vec![ToolCallContent::text("out")],
        };
        let encoded = serde_json::to_value(&update).unwrap();
        assert_eq!(encoded["toolCallId"], "t1");
        assert_eq!(encoded["status"], "completed");
        assert_eq!(encoded["content"][0]["content"]["text"], "out");
    }

    #[test]
    fn stop_reasons_use_the_controller_vocabulary() {
        assert_eq!(serde_json::to_value(StopReason::EndTurn).unwrap(), "end_turn");
        assert_eq!(serde_json::to_value(StopReason::Refusal).unwrap(), "refusal");
        assert_eq!(serde_json::to_value(StopReason::MaxTurnRequests).unwrap(), "max_turn_requests");
    }

    #[test]
    fn permission_outcomes_parse_both_shapes() {
        let selected: RequestPermissionResponse =
            serde_json::from_value(serde_json::json!({ "outcome": { "outcome": "selected", "optionId": "allow" } }))
                .unwrap();
        assert_eq!(selected.outcome, PermissionOutcome::Selected { option_id: "allow".into() });
        let cancelled: RequestPermissionResponse =
            serde_json::from_value(serde_json::json!({ "outcome": { "outcome": "cancelled" } })).unwrap();
        assert_eq!(cancelled.outcome, PermissionOutcome::Cancelled);
    }
}
