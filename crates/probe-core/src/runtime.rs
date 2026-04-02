use std::collections::BTreeMap;
use std::env;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use probe_protocol::backend::{BackendKind, BackendProfile};
use probe_protocol::session::{
    BackendTurnReceipt, CacheSignal, PendingToolApproval, SessionBackendTarget,
    SessionHarnessProfile, SessionId, SessionMetadata, SessionTurn, ToolApprovalResolution,
    ToolRiskClass, TranscriptEvent, TranscriptItem, TranscriptItemKind, TurnObservability,
};
use probe_provider_openai::{ChatCompletionChunk, ChatMessage, ChatToolCall};
use psionic_apple_fm::{
    APPLE_FM_TRANSCRIPT_TYPE, APPLE_FM_TRANSCRIPT_VERSION, AppleFmTextStreamEvent,
    AppleFmToolCallError, AppleFmTranscript, AppleFmTranscriptContent, AppleFmTranscriptEntry,
    AppleFmTranscriptPayload,
};
use serde_json::Value;

use crate::dataset_export::build_decision_summary;
use crate::provider::{
    OpenAiRequestContext, PlainTextMessage, ProviderError, ProviderUsage,
    apple_fm_tool_loop_response, apple_fm_tool_loop_response_with_callback,
    complete_apple_fm_plain_text_with_callback,
    complete_openai_plain_text_with_context_and_callback, complete_plain_text_with_context,
    normalize_openai_stream_display_text, observability_usage_measurement,
    openai_tool_loop_response_with_callback, openai_tool_loop_response_with_context,
};
use crate::session_store::{FilesystemSessionStore, NewItem, NewSession, SessionStoreError};
use crate::tools::{
    ExecutedToolCall, ToolExecutionContext, ToolExecutionSession, ToolLongContextContext,
    ToolLoopConfig, ToolOracleContext, stored_tool_result_model_text, tool_result_model_text,
};

const DEFAULT_PROBE_HOME_DIR: &str = ".probe";
const LIKELY_WARM_WALLCLOCK_RATIO_NUMERATOR: u64 = 80;
const LIKELY_WARM_WALLCLOCK_RATIO_DENOMINATOR: u64 = 100;

#[derive(Clone, Debug)]
enum AppleFmToolLoopInterruption {
    ApprovalPending {
        tool_name: String,
        call_id: String,
        reason: Option<String>,
    },
    CallbackBudgetExceeded {
        max_round_trips: usize,
    },
}

struct AppleFmToolLoopRecorder {
    session_id: SessionId,
    execution_session: ToolExecutionSession,
    records: Vec<ExecutedToolCall>,
    next_call_index: usize,
    max_callback_calls: usize,
    interruption: Option<AppleFmToolLoopInterruption>,
    event_sink: Option<Arc<dyn RuntimeEventSink>>,
}

#[derive(Clone, Debug)]
pub struct PlainTextExecRequest {
    pub profile: BackendProfile,
    pub prompt: String,
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub system_prompt: Option<String>,
    pub harness_profile: Option<SessionHarnessProfile>,
    pub tool_loop: Option<ToolLoopConfig>,
}

#[derive(Clone, Debug)]
pub struct PlainTextResumeRequest {
    pub session_id: SessionId,
    pub profile: BackendProfile,
    pub prompt: String,
    pub tool_loop: Option<ToolLoopConfig>,
}

#[derive(Clone, Debug)]
pub struct ResolvePendingToolApprovalRequest {
    pub session_id: SessionId,
    pub profile: BackendProfile,
    pub tool_loop: ToolLoopConfig,
    pub call_id: String,
    pub resolution: ToolApprovalResolution,
}

#[derive(Clone, Debug)]
pub struct PlainTextExecOutcome {
    pub session: SessionMetadata,
    pub turn: SessionTurn,
    pub assistant_text: String,
    pub response_id: String,
    pub response_model: String,
    pub usage: Option<ProviderUsage>,
    pub executed_tool_calls: usize,
    pub tool_results: Vec<ExecutedToolCall>,
    pub retained_session_summary: String,
    pub accepted_patch_summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamedToolCallDelta {
    pub tool_index: usize,
    pub call_id: Option<String>,
    pub tool_name: Option<String>,
    pub arguments_delta: Option<String>,
}

#[derive(Clone, Debug)]
pub enum ResolvePendingToolApprovalOutcome {
    StillPending {
        session: SessionMetadata,
        pending_approvals: Vec<PendingToolApproval>,
    },
    Resumed {
        outcome: PlainTextExecOutcome,
    },
}

#[derive(Clone, Debug)]
pub enum RuntimeEvent {
    TurnStarted {
        session_id: SessionId,
        profile_name: String,
        prompt: String,
        tool_loop_enabled: bool,
    },
    ModelRequestStarted {
        session_id: SessionId,
        round_trip: usize,
        backend_kind: BackendKind,
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
        deltas: Vec<StreamedToolCallDelta>,
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
        tool: ExecutedToolCall,
    },
    ToolRefused {
        session_id: SessionId,
        round_trip: usize,
        tool: ExecutedToolCall,
    },
    ToolPaused {
        session_id: SessionId,
        round_trip: usize,
        tool: ExecutedToolCall,
    },
    AssistantStreamFinished {
        session_id: SessionId,
        round_trip: usize,
        response_id: String,
        response_model: String,
        finish_reason: Option<String>,
    },
    ModelRequestFailed {
        session_id: SessionId,
        round_trip: usize,
        backend_kind: BackendKind,
    },
}