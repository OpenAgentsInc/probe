use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::backend::BackendProfile;
use crate::session::{
    PendingToolApproval, SessionHarnessProfile, SessionId, SessionMetadata, SessionTurn,
    TimestampMs, ToolApprovalResolution, ToolExecutionRecord, ToolRiskClass, TranscriptEvent,
    UsageMeasurement,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    StdioJsonl,
    UnixSocketJsonl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventDeliveryGuarantee {
    Lossless,
    BestEffort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSetKind {
    CodingBootstrap,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolChoice {
    None,
    Auto,
    Required,
    Named { tool_name: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDeniedAction {
    Refuse,
    Pause,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolApprovalRecipe {
    pub allow_write_tools: bool,
    pub allow_network_shell: bool,
    pub allow_destructive_shell: bool,
    pub denied_action: ToolDeniedAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOracleRecipe {
    pub profile: BackendProfile,
    pub max_calls: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolLongContextRecipe {
    pub profile: BackendProfile,
    pub max_calls: usize,
    pub max_evidence_files: usize,
    pub max_lines_per_file: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolLoopRecipe {
    pub tool_set: ToolSetKind,
    pub tool_choice: ToolChoice,
    pub parallel_tool_calls: bool,
    pub max_model_round_trips: usize,
    pub approval: ToolApprovalRecipe,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oracle: Option<ToolOracleRecipe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_context: Option<ToolLongContextRecipe>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitializeRequest {
    pub client_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
    pub protocol_version: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCapabilities {
    pub transport: TransportKind,
    pub supports_stdio_child_process: bool,
    pub supports_local_daemon_socket: bool,
    pub supports_session_resume: bool,
    pub supports_session_inspect: bool,
    pub supports_pending_approval_resolution: bool,
    pub supports_interrupt_requests: bool,
    pub supports_queued_turns: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitializeResponse {
    pub runtime_name: String,
    pub protocol_version: u32,
    pub capabilities: RuntimeCapabilities,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartSessionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub profile: BackendProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_profile: Option<SessionHarnessProfile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLookupRequest {
    pub session_id: SessionId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnRequest {
    pub session_id: SessionId,
    pub profile: BackendProfile,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<TurnAuthor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_loop: Option<ToolLoopRecipe>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnAuthor {
    pub client_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnSubmissionKind {
    Start,
    Continue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueuedTurnStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTurnControlRecord {
    pub turn_id: String,
    pub session_id: SessionId,
    pub submission_kind: TurnSubmissionKind,
    pub status: QueuedTurnStatus,
    pub prompt: String,
    pub author: TurnAuthor,
    pub requested_at_ms: TimestampMs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<TimestampMs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<TimestampMs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_position: Option<usize>,
    #[serde(default)]
    pub awaiting_approval: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellation_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueTurnResponse {
    pub turn: SessionTurnControlRecord,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectSessionTurnsResponse {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn: Option<SessionTurnControlRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queued_turns: Vec<SessionTurnControlRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_turns: Vec<SessionTurnControlRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelQueuedTurnRequest {
    pub session_id: SessionId,
    pub turn_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelQueuedTurnResponse {
    pub session_id: SessionId,
    pub turn_id: String,
    pub cancelled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvePendingApprovalRequest {
    pub session_id: SessionId,
    pub profile: BackendProfile,
    pub tool_loop: ToolLoopRecipe,
    pub call_id: String,
    pub resolution: ToolApprovalResolution,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListPendingApprovalsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptTurnRequest {
    pub session_id: SessionId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub session: SessionMetadata,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transcript: Vec<TranscriptEvent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_approvals: Vec<PendingToolApproval>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_detail: Option<UsageMeasurement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens_detail: Option<UsageMeasurement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens_detail: Option<UsageMeasurement>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
    pub output: Value,
    pub tool_execution: ToolExecutionRecord,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnCompleted {
    pub session: SessionMetadata,
    pub turn: SessionTurn,
    pub assistant_text: String,
    pub response_id: String,
    pub response_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<RuntimeUsage>,
    pub executed_tool_calls: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<ToolCallResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnPaused {
    pub session: SessionMetadata,
    pub call_id: String,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_approvals: Vec<PendingToolApproval>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TurnResponse {
    Completed(TurnCompleted),
    Paused(TurnPaused),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptTurnResponse {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub interrupted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownResponse {
    pub accepted: bool,
    pub active_turns: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionMetadata>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListPendingApprovalsResponse {
    pub approvals: Vec<PendingToolApproval>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResolvePendingApprovalResponse {
    StillPending {
        session: SessionMetadata,
        pending_approvals: Vec<PendingToolApproval>,
    },
    Resumed(TurnCompleted),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeToolCallDelta {
    pub tool_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments_delta: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeProgressEvent {
    TurnStarted {
        session_id: SessionId,
        profile_name: String,
        prompt: String,
        tool_loop_enabled: bool,
    },
    ModelRequestStarted {
        session_id: SessionId,
        round_trip: usize,
        backend_kind: crate::backend::BackendKind,
    },
    AssistantStreamStarted {
        session_id: SessionId,
        round_trip: usize,
        response_id: String,
        response_model: String,
    },
    TimeToFirstTokenObserved {
        session_id: SessionId,
        round_trip: usize,
        milliseconds: u64,
    },
    AssistantDelta {
        session_id: SessionId,
        round_trip: usize,
        delta: String,
    },
    AssistantSnapshot {
        session_id: SessionId,
        round_trip: usize,
        snapshot: String,
    },
    ToolCallDelta {
        session_id: SessionId,
        round_trip: usize,
        deltas: Vec<RuntimeToolCallDelta>,
    },
    ToolCallRequested {
        session_id: SessionId,
        round_trip: usize,
        call_id: String,
        tool_name: String,
        arguments: Value,
    },
    ToolExecutionStarted {
        session_id: SessionId,
        round_trip: usize,
        call_id: String,
        tool_name: String,
        risk_class: ToolRiskClass,
    },
    ToolExecutionCompleted {
        session_id: SessionId,
        round_trip: usize,
        tool: ToolCallResult,
    },
    ToolRefused {
        session_id: SessionId,
        round_trip: usize,
        tool: ToolCallResult,
    },
    ToolPaused {
        session_id: SessionId,
        round_trip: usize,
        tool: ToolCallResult,
    },
    AssistantStreamFinished {
        session_id: SessionId,
        round_trip: usize,
        response_id: String,
        response_model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        finish_reason: Option<String>,
    },
    ModelRequestFailed {
        session_id: SessionId,
        round_trip: usize,
        backend_kind: crate::backend::BackendKind,
        error: String,
    },
    AssistantTurnCommitted {
        session_id: SessionId,
        response_id: String,
        response_model: String,
        assistant_text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerEvent {
    RuntimeProgress {
        delivery: EventDeliveryGuarantee,
        event: RuntimeProgressEvent,
    },
    PendingApprovalsUpdated {
        delivery: EventDeliveryGuarantee,
        session_id: SessionId,
        approvals: Vec<PendingToolApproval>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProtocolError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RuntimeRequest {
    Initialize(InitializeRequest),
    StartSession(StartSessionRequest),
    ResumeSession(SessionLookupRequest),
    ListSessions,
    InspectSession(SessionLookupRequest),
    StartTurn(TurnRequest),
    ContinueTurn(TurnRequest),
    QueueTurn(TurnRequest),
    InspectSessionTurns(SessionLookupRequest),
    InterruptTurn(InterruptTurnRequest),
    CancelQueuedTurn(CancelQueuedTurnRequest),
    ListPendingApprovals(ListPendingApprovalsRequest),
    ResolvePendingApproval(ResolvePendingApprovalRequest),
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RuntimeResponse {
    Initialize(InitializeResponse),
    StartSession(SessionSnapshot),
    ResumeSession(SessionSnapshot),
    ListSessions(ListSessionsResponse),
    InspectSession(SessionSnapshot),
    StartTurn(TurnResponse),
    ContinueTurn(TurnResponse),
    QueueTurn(QueueTurnResponse),
    InspectSessionTurns(InspectSessionTurnsResponse),
    InterruptTurn(InterruptTurnResponse),
    CancelQueuedTurn(CancelQueuedTurnResponse),
    ListPendingApprovals(ListPendingApprovalsResponse),
    ResolvePendingApproval(ResolvePendingApprovalResponse),
    Shutdown(ShutdownResponse),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub request_id: String,
    pub request: RuntimeRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResponseBody {
    Ok { response: RuntimeResponse },
    Error { error: RuntimeProtocolError },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub request_id: String,
    #[serde(flatten)]
    pub body: ResponseBody,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub request_id: String,
    pub event: ServerEvent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "message_type", rename_all = "snake_case")]
pub enum ClientMessage {
    Request(RequestEnvelope),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "message_type", rename_all = "snake_case")]
pub enum ServerMessage {
    Response(ResponseEnvelope),
    Event(EventEnvelope),
}

#[cfg(test)]
mod tests {
    use super::{
        ClientMessage, EventDeliveryGuarantee, RequestEnvelope, ResponseBody, ResponseEnvelope,
        RuntimeCapabilities, RuntimeProgressEvent, RuntimeRequest, RuntimeResponse, ServerEvent,
        ServerMessage, ShutdownResponse, ToolApprovalRecipe, ToolChoice, ToolDeniedAction,
        ToolLoopRecipe, ToolSetKind, TransportKind, TurnAuthor, TurnRequest,
    };

    #[test]
    fn request_envelope_round_trips_through_json() {
        let message = ClientMessage::Request(RequestEnvelope {
            request_id: String::from("req-1"),
            request: RuntimeRequest::QueueTurn(TurnRequest {
                session_id: crate::session::SessionId::new("sess-1"),
                profile: crate::backend::BackendProfile {
                    name: String::from("test"),
                    kind: crate::backend::BackendKind::OpenAiChatCompletions,
                    base_url: String::from("http://127.0.0.1:11434/v1"),
                    model: String::from("tiny"),
                    reasoning_level: None,
                    api_key_env: String::from("PROBE_OPENAI_API_KEY"),
                    timeout_secs: 30,
                    attach_mode: crate::backend::ServerAttachMode::AttachToExisting,
                    prefix_cache_mode: crate::backend::PrefixCacheMode::BackendDefault,
                },
                prompt: String::from("hello"),
                author: Some(TurnAuthor {
                    client_name: String::from("probe-cli"),
                    client_version: Some(String::from("0.1.0")),
                    display_name: Some(String::from("operator")),
                }),
                tool_loop: None,
            }),
        });

        let encoded = serde_json::to_string(&message).expect("request should encode");
        let decoded: ClientMessage =
            serde_json::from_str(encoded.as_str()).expect("request should decode");
        assert_eq!(decoded, message);
    }

    #[test]
    fn tool_loop_recipe_is_constructible() {
        let recipe = ToolLoopRecipe {
            tool_set: ToolSetKind::CodingBootstrap,
            tool_choice: ToolChoice::Auto,
            parallel_tool_calls: false,
            max_model_round_trips: 8,
            approval: ToolApprovalRecipe {
                allow_write_tools: false,
                allow_network_shell: false,
                allow_destructive_shell: false,
                denied_action: ToolDeniedAction::Pause,
            },
            oracle: None,
            long_context: None,
        };
        assert_eq!(recipe.max_model_round_trips, 8);
        assert!(matches!(recipe.tool_choice, ToolChoice::Auto));
    }

    #[test]
    fn server_messages_encode_response_and_event_shapes() {
        let response = ServerMessage::Response(ResponseEnvelope {
            request_id: String::from("req-2"),
            body: ResponseBody::Ok {
                response: RuntimeResponse::Shutdown(ShutdownResponse {
                    accepted: true,
                    active_turns: 0,
                }),
            },
        });
        let event = ServerMessage::Event(super::EventEnvelope {
            request_id: String::from("req-3"),
            event: ServerEvent::RuntimeProgress {
                delivery: EventDeliveryGuarantee::BestEffort,
                event: RuntimeProgressEvent::AssistantDelta {
                    session_id: crate::session::SessionId::new("sess-1"),
                    round_trip: 1,
                    delta: String::from("hello"),
                },
            },
        });
        let capabilities = RuntimeCapabilities {
            transport: TransportKind::StdioJsonl,
            supports_stdio_child_process: true,
            supports_local_daemon_socket: false,
            supports_session_resume: true,
            supports_session_inspect: true,
            supports_pending_approval_resolution: true,
            supports_interrupt_requests: true,
            supports_queued_turns: true,
        };

        let response_json = serde_json::to_value(&response).expect("response should encode");
        let event_json = serde_json::to_value(&event).expect("event should encode");
        let caps_json = serde_json::to_value(&capabilities).expect("caps should encode");

        assert_eq!(response_json["message_type"], "response");
        assert_eq!(event_json["message_type"], "event");
        assert_eq!(caps_json["transport"], "stdio_jsonl");
    }
}
