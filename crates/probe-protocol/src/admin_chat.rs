use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminChatBridgeRequest {
    pub request_id: String,
    pub workspace: String,
    pub web_user_id: u64,
    pub web_user_email: String,
    pub conversation_id: String,
    pub run_id: String,
    pub prompt: String,
    #[serde(default)]
    pub messages: Vec<AdminChatBridgeMessage>,
    pub provider: AdminChatProviderSelection,
    pub tool_policy: AdminChatToolPolicy,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminChatBridgeSignedRequest {
    pub auth: AdminChatBridgeAuth,
    pub request: AdminChatBridgeRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminChatBridgeAuth {
    pub key_id: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminChatBridgeMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminChatProviderSelection {
    pub key: String,
    pub mode: String,
    #[serde(default)]
    pub account_ref: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminChatToolPolicy {
    pub mode: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    pub approval_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminChatProviderMetadata {
    pub key: String,
    pub mode: String,
    #[serde(default)]
    pub account_ref: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    pub backend_family: String,
    pub backend_profile: String,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminChatUsageSnapshot {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub raw: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminChatRedactedDiagnostics {
    pub probe_session_id: String,
    #[serde(default)]
    pub probe_turn_id: Option<String>,
    pub transcript_ref: String,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub response_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminChatBridgeCorrelationMetadata {
    pub request_id: String,
    pub workspace: String,
    pub web_user_id: u64,
    pub conversation_id: String,
    pub run_id: String,
    #[serde(default)]
    pub schedule_id: Option<String>,
    #[serde(default)]
    pub wake_id: Option<String>,
    #[serde(default)]
    pub scheduled_run_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminChatBridgeAcceptedResponse {
    pub request_id: String,
    pub run_id: String,
    pub probe_session_id: String,
    pub probe_turn_id: String,
    pub provider: AdminChatProviderMetadata,
    pub transcript_ref: String,
    pub correlation: AdminChatBridgeCorrelationMetadata,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdminChatBridgeEvent {
    RunStarted {
        request_id: String,
        run_id: String,
        probe_session_id: String,
        provider: AdminChatProviderMetadata,
        tool_policy: AdminChatToolPolicy,
    },
    ModelStreamStarted {
        run_id: String,
        provider: AdminChatProviderMetadata,
    },
    TextDelta {
        run_id: String,
        id: String,
        delta: String,
    },
    ToolCallStarted {
        run_id: String,
        call_id: String,
        tool_name: String,
    },
    ToolCallResult {
        run_id: String,
        call_id: String,
        status: String,
        summary: String,
    },
    ApprovalRequested {
        run_id: String,
        approval_id: String,
        risk_class: String,
        summary: String,
    },
    UsageLimitsSnapshot {
        run_id: String,
        provider: AdminChatProviderMetadata,
        #[serde(default)]
        usage: Option<AdminChatUsageSnapshot>,
        #[serde(default)]
        limits: Option<Value>,
    },
    RunCompleted {
        run_id: String,
        status: String,
        provider: AdminChatProviderMetadata,
        #[serde(default)]
        response_id: Option<String>,
        #[serde(default)]
        usage: Option<AdminChatUsageSnapshot>,
        diagnostics: AdminChatRedactedDiagnostics,
    },
    RunFailed {
        run_id: String,
        error_code: String,
        message: String,
        diagnostics: AdminChatRedactedDiagnostics,
    },
}

impl AdminChatBridgeRequest {
    #[must_use]
    pub fn fake(
        request_id: impl Into<String>,
        web_user_id: u64,
        web_user_email: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            workspace: String::from("openagents.com"),
            web_user_id,
            web_user_email: web_user_email.into(),
            conversation_id: String::from("conversation.fake"),
            run_id: String::from("run.fake"),
            prompt: prompt.into(),
            messages: Vec::new(),
            provider: AdminChatProviderSelection {
                key: String::from("openai"),
                mode: String::from("service_api_key"),
                account_ref: None,
                label: Some(String::from("fake bridge")),
            },
            tool_policy: AdminChatToolPolicy {
                mode: String::from("admin_chat"),
                allowed_tools: Vec::new(),
                approval_required: true,
            },
            metadata: Map::new(),
        }
    }
}
