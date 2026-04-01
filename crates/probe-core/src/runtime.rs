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
    PlainTextMessage, ProviderError, ProviderUsage, apple_fm_tool_loop_response,
    apple_fm_tool_loop_response_with_callback, complete_apple_fm_plain_text_with_callback,
    complete_openai_plain_text_with_callback, complete_plain_text, observability_usage_measurement,
    openai_tool_loop_response, openai_tool_loop_response_with_callback,
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

#[derive(Clone, Debug, PartialEq, Eq)]
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
        error: String,
    },
    AssistantTurnCommitted {
        session_id: SessionId,
        response_id: String,
        response_model: String,
        assistant_text: String,
    },
}

pub trait RuntimeEventSink: Send + Sync {
    fn emit(&self, event: RuntimeEvent);
}

impl<F> RuntimeEventSink for F
where
    F: Fn(RuntimeEvent) + Send + Sync,
{
    fn emit(&self, event: RuntimeEvent) {
        self(event);
    }
}

#[derive(Debug)]
pub enum RuntimeError {
    ProbeHomeUnavailable,
    CurrentDir(std::io::Error),
    SessionStore(SessionStoreError),
    ProviderRequest {
        session_id: SessionId,
        source: ProviderError,
    },
    MissingAssistantMessage {
        session_id: SessionId,
        response_id: String,
    },
    UnsupportedBackendFeature {
        session_id: SessionId,
        backend: BackendKind,
        feature: &'static str,
    },
    ToolApprovalPending {
        session_id: SessionId,
        tool_name: String,
        call_id: String,
        reason: Option<String>,
    },
    PendingToolApprovalNotFound {
        session_id: SessionId,
        call_id: String,
    },
    PendingToolApprovalAlreadyResolved {
        session_id: SessionId,
        call_id: String,
        resolution: ToolApprovalResolution,
    },
    MaxToolRoundTrips {
        session_id: SessionId,
        max_round_trips: usize,
    },
    MalformedTranscript(String),
}

impl Display for RuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProbeHomeUnavailable => {
                write!(f, "failed to resolve probe home; set PROBE_HOME or HOME")
            }
            Self::CurrentDir(error) => write!(f, "failed to resolve current directory: {error}"),
            Self::SessionStore(error) => write!(f, "{error}"),
            Self::ProviderRequest { session_id, source } => {
                write!(
                    f,
                    "backend request failed for session {}: {source}",
                    session_id.as_str()
                )
            }
            Self::MissingAssistantMessage {
                session_id,
                response_id,
            } => write!(
                f,
                "backend response {response_id} for session {} did not include assistant text",
                session_id.as_str()
            ),
            Self::UnsupportedBackendFeature {
                session_id,
                backend,
                feature,
            } => write!(
                f,
                "session {} cannot use backend {:?} for {feature}",
                session_id.as_str(),
                backend
            ),
            Self::ToolApprovalPending {
                session_id,
                tool_name,
                call_id,
                reason,
            } => write!(
                f,
                "session {} paused for approval on tool `{tool_name}` ({call_id}){}",
                session_id.as_str(),
                reason
                    .as_deref()
                    .map(|value| format!(": {value}"))
                    .unwrap_or_default()
            ),
            Self::PendingToolApprovalNotFound {
                session_id,
                call_id,
            } => write!(
                f,
                "session {} has no pending approval for call `{call_id}`",
                session_id.as_str()
            ),
            Self::PendingToolApprovalAlreadyResolved {
                session_id,
                call_id,
                resolution,
            } => write!(
                f,
                "session {} already resolved approval for call `{call_id}` as {}",
                session_id.as_str(),
                render_tool_approval_resolution(*resolution)
            ),
            Self::MaxToolRoundTrips {
                session_id,
                max_round_trips,
            } => write!(
                f,
                "session {} exceeded the configured tool loop bound of {} controller round trips",
                session_id.as_str(),
                max_round_trips
            ),
            Self::MalformedTranscript(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl RuntimeError {
    #[must_use]
    fn backend_turn_receipt(&self) -> Option<BackendTurnReceipt> {
        match self {
            Self::ProviderRequest { source, .. } => source.backend_turn_receipt(),
            Self::ProbeHomeUnavailable
            | Self::CurrentDir(_)
            | Self::SessionStore(_)
            | Self::MissingAssistantMessage { .. }
            | Self::UnsupportedBackendFeature { .. }
            | Self::ToolApprovalPending { .. }
            | Self::PendingToolApprovalNotFound { .. }
            | Self::PendingToolApprovalAlreadyResolved { .. }
            | Self::MaxToolRoundTrips { .. }
            | Self::MalformedTranscript(_) => None,
        }
    }
}

impl From<SessionStoreError> for RuntimeError {
    fn from(value: SessionStoreError) -> Self {
        Self::SessionStore(value)
    }
}

#[derive(Clone, Debug)]
pub struct ProbeRuntime {
    session_store: FilesystemSessionStore,
}

impl ProbeRuntime {
    #[must_use]
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            session_store: FilesystemSessionStore::new(home),
        }
    }

    pub fn detect() -> Result<Self, RuntimeError> {
        Ok(Self::new(default_probe_home()?))
    }

    #[must_use]
    pub fn session_store(&self) -> &FilesystemSessionStore {
        &self.session_store
    }

    pub fn pending_tool_approvals(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<PendingToolApproval>, RuntimeError> {
        self.session_store
            .read_pending_tool_approvals(session_id)
            .map_err(RuntimeError::from)
    }

    pub fn exec_plain_text(
        &self,
        request: PlainTextExecRequest,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        self.exec_plain_text_internal(request, None)
    }

    pub fn exec_plain_text_with_events(
        &self,
        request: PlainTextExecRequest,
        event_sink: Arc<dyn RuntimeEventSink>,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        self.exec_plain_text_internal(request, Some(event_sink))
    }

    fn exec_plain_text_internal(
        &self,
        request: PlainTextExecRequest,
        event_sink: Option<Arc<dyn RuntimeEventSink>>,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        let session = self.session_store.create_session_with(
            NewSession::new(
                request
                    .title
                    .clone()
                    .unwrap_or_else(|| default_session_title(request.prompt.as_str())),
                request.cwd,
            )
            .with_system_prompt(request.system_prompt.clone())
            .with_harness_profile(request.harness_profile.clone())
            .with_backend(SessionBackendTarget {
                profile_name: request.profile.name.clone(),
                base_url: request.profile.base_url.clone(),
                model: request.profile.model.clone(),
            }),
        )?;

        self.run_plain_text_turn(
            session,
            request.profile,
            request.prompt,
            request.tool_loop,
            event_sink,
        )
    }

    pub fn continue_plain_text_session(
        &self,
        request: PlainTextResumeRequest,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        self.continue_plain_text_session_internal(request, None)
    }

    pub fn continue_plain_text_session_with_events(
        &self,
        request: PlainTextResumeRequest,
        event_sink: Arc<dyn RuntimeEventSink>,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        self.continue_plain_text_session_internal(request, Some(event_sink))
    }

    pub fn resolve_pending_tool_approval(
        &self,
        request: ResolvePendingToolApprovalRequest,
    ) -> Result<ResolvePendingToolApprovalOutcome, RuntimeError> {
        self.resolve_pending_tool_approval_internal(request, None)
    }

    pub fn resolve_pending_tool_approval_with_events(
        &self,
        request: ResolvePendingToolApprovalRequest,
        event_sink: Arc<dyn RuntimeEventSink>,
    ) -> Result<ResolvePendingToolApprovalOutcome, RuntimeError> {
        self.resolve_pending_tool_approval_internal(request, Some(event_sink))
    }

    fn continue_plain_text_session_internal(
        &self,
        request: PlainTextResumeRequest,
        event_sink: Option<Arc<dyn RuntimeEventSink>>,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        let session = self.session_store.read_metadata(&request.session_id)?;
        self.ensure_no_pending_approvals(&session.id)?;
        self.run_plain_text_turn(
            session,
            request.profile,
            request.prompt,
            request.tool_loop,
            event_sink,
        )
    }

    fn resolve_pending_tool_approval_internal(
        &self,
        request: ResolvePendingToolApprovalRequest,
        event_sink: Option<Arc<dyn RuntimeEventSink>>,
    ) -> Result<ResolvePendingToolApprovalOutcome, RuntimeError> {
        let session = self.session_store.read_metadata(&request.session_id)?;
        let pending_approvals = self.pending_tool_approvals(&session.id)?;
        let pending = pending_approvals
            .iter()
            .find(|approval| approval.tool_call_id == request.call_id)
            .cloned()
            .ok_or_else(|| {
                let resolved = self
                    .session_store
                    .read_tool_approvals(&session.id)
                    .ok()
                    .and_then(|approvals| {
                        approvals
                            .into_iter()
                            .find(|approval| approval.tool_call_id == request.call_id)
                    });
                match resolved.and_then(|approval| approval.resolution) {
                    Some(resolution) => RuntimeError::PendingToolApprovalAlreadyResolved {
                        session_id: session.id.clone(),
                        call_id: request.call_id.clone(),
                        resolution,
                    },
                    None => RuntimeError::PendingToolApprovalNotFound {
                        session_id: session.id.clone(),
                        call_id: request.call_id.clone(),
                    },
                }
            })?;
        let resolved_tool = self.execute_pending_tool_resolution(
            &session,
            &request.tool_loop,
            &pending,
            request.resolution,
            event_sink.as_ref(),
        )?;
        let _ = self.append_tool_result_turn(&session.id, &[resolved_tool])?;
        let _ = self.session_store.resolve_pending_tool_approval(
            &session.id,
            request.call_id.as_str(),
            request.resolution,
        )?;

        let remaining = self.pending_tool_approvals(&session.id)?;
        if !remaining.is_empty() {
            let session = self.session_store.read_metadata(&session.id)?;
            return Ok(ResolvePendingToolApprovalOutcome::StillPending {
                session,
                pending_approvals: remaining,
            });
        }

        let outcome = self.resume_tool_loop_after_approval(
            session,
            request.profile,
            request.tool_loop,
            event_sink,
        )?;
        Ok(ResolvePendingToolApprovalOutcome::Resumed { outcome })
    }
}

fn emit_runtime_event(event_sink: Option<&Arc<dyn RuntimeEventSink>>, event: RuntimeEvent) {
    if let Some(event_sink) = event_sink {
        event_sink.emit(event);
    }
}

fn parsed_openai_tool_arguments(tool_call: &ChatToolCall) -> Value {
    serde_json::from_str(tool_call.function.arguments.as_str())
        .unwrap_or_else(|_| Value::String(tool_call.function.arguments.clone()))
}

fn emit_tool_result_event(
    event_sink: Option<&Arc<dyn RuntimeEventSink>>,
    session_id: &SessionId,
    round_trip: usize,
    tool: &ExecutedToolCall,
) {
    let event = if tool.was_executed() {
        RuntimeEvent::ToolExecutionCompleted {
            session_id: session_id.clone(),
            round_trip,
            tool: tool.clone(),
        }
    } else if tool.was_paused() {
        RuntimeEvent::ToolPaused {
            session_id: session_id.clone(),
            round_trip,
            tool: tool.clone(),
        }
    } else {
        RuntimeEvent::ToolRefused {
            session_id: session_id.clone(),
            round_trip,
            tool: tool.clone(),
        }
    };
    emit_runtime_event(event_sink, event);
}

#[derive(Default)]
struct OpenAiStreamRuntimeState {
    stream_started: bool,
    ttft_recorded: bool,
    stream_finished: bool,
}

fn emit_openai_stream_events(
    event_sink: Option<&Arc<dyn RuntimeEventSink>>,
    session_id: &SessionId,
    round_trip: usize,
    request_started: Instant,
    state: &mut OpenAiStreamRuntimeState,
    chunk: &ChatCompletionChunk,
) {
    if !state.stream_started {
        emit_runtime_event(
            event_sink,
            RuntimeEvent::AssistantStreamStarted {
                session_id: session_id.clone(),
                round_trip,
                response_id: chunk.id.clone(),
                response_model: chunk.model.clone(),
            },
        );
        state.stream_started = true;
    }
    if !state.ttft_recorded {
        emit_runtime_event(
            event_sink,
            RuntimeEvent::TimeToFirstTokenObserved {
                session_id: session_id.clone(),
                round_trip,
                milliseconds: elapsed_ms(request_started),
            },
        );
        state.ttft_recorded = true;
    }

    for choice in &chunk.choices {
        if let Some(content) = choice.delta.content.as_ref()
            && !content.is_empty()
        {
            emit_runtime_event(
                event_sink,
                RuntimeEvent::AssistantDelta {
                    session_id: session_id.clone(),
                    round_trip,
                    delta: content.clone(),
                },
            );
        }
        if let Some(tool_calls) = choice.delta.tool_calls.as_ref()
            && !tool_calls.is_empty()
        {
            emit_runtime_event(
                event_sink,
                RuntimeEvent::ToolCallDelta {
                    session_id: session_id.clone(),
                    round_trip,
                    deltas: tool_calls
                        .iter()
                        .map(|tool_call| StreamedToolCallDelta {
                            tool_index: tool_call.index,
                            call_id: tool_call.id.clone(),
                            tool_name: tool_call
                                .function
                                .as_ref()
                                .and_then(|function| function.name.clone()),
                            arguments_delta: tool_call
                                .function
                                .as_ref()
                                .and_then(|function| function.arguments.clone()),
                        })
                        .collect(),
                },
            );
        }
        if !state.stream_finished && choice.finish_reason.is_some() {
            emit_runtime_event(
                event_sink,
                RuntimeEvent::AssistantStreamFinished {
                    session_id: session_id.clone(),
                    round_trip,
                    response_id: chunk.id.clone(),
                    response_model: chunk.model.clone(),
                    finish_reason: choice.finish_reason.clone(),
                },
            );
            state.stream_finished = true;
        }
    }
}

fn emit_model_request_failed(
    event_sink: Option<&Arc<dyn RuntimeEventSink>>,
    session_id: &SessionId,
    round_trip: usize,
    backend_kind: BackendKind,
    error: &RuntimeError,
) {
    emit_runtime_event(
        event_sink,
        RuntimeEvent::ModelRequestFailed {
            session_id: session_id.clone(),
            round_trip,
            backend_kind,
            error: error.to_string(),
        },
    );
}

#[derive(Default)]
struct AppleFmStreamRuntimeState {
    stream_started: bool,
    ttft_recorded: bool,
    stream_finished: bool,
}

fn emit_apple_fm_stream_events(
    event_sink: Option<&Arc<dyn RuntimeEventSink>>,
    session_id: &SessionId,
    round_trip: usize,
    response_id: &str,
    request_started: Instant,
    state: &mut AppleFmStreamRuntimeState,
    event: &AppleFmTextStreamEvent,
) {
    if !state.stream_started {
        emit_runtime_event(
            event_sink,
            RuntimeEvent::AssistantStreamStarted {
                session_id: session_id.clone(),
                round_trip,
                response_id: response_id.to_string(),
                response_model: event.model.clone(),
            },
        );
        state.stream_started = true;
    }
    if !state.ttft_recorded {
        emit_runtime_event(
            event_sink,
            RuntimeEvent::TimeToFirstTokenObserved {
                session_id: session_id.clone(),
                round_trip,
                milliseconds: elapsed_ms(request_started),
            },
        );
        state.ttft_recorded = true;
    }
    emit_runtime_event(
        event_sink,
        RuntimeEvent::AssistantSnapshot {
            session_id: session_id.clone(),
            round_trip,
            snapshot: event.output.clone(),
        },
    );
    if event.is_terminal() && !state.stream_finished {
        emit_runtime_event(
            event_sink,
            RuntimeEvent::AssistantStreamFinished {
                session_id: session_id.clone(),
                round_trip,
                response_id: response_id.to_string(),
                response_model: event.model.clone(),
                finish_reason: Some(String::from("snapshot_completed")),
            },
        );
        state.stream_finished = true;
    }
}

fn render_tool_approval_resolution(value: ToolApprovalResolution) -> &'static str {
    match value {
        ToolApprovalResolution::Approved => "approved",
        ToolApprovalResolution::Rejected => "rejected",
    }
}

pub fn default_probe_home() -> Result<PathBuf, RuntimeError> {
    if let Ok(path) = env::var("PROBE_HOME") {
        return Ok(PathBuf::from(path));
    }

    if let Ok(home) = env::var("HOME") {
        return Ok(PathBuf::from(home).join(DEFAULT_PROBE_HOME_DIR));
    }

    Err(RuntimeError::ProbeHomeUnavailable)
}

pub fn current_working_dir() -> Result<PathBuf, RuntimeError> {
    env::current_dir().map_err(RuntimeError::CurrentDir)
}

fn default_session_title(prompt: &str) -> String {
    let trimmed = prompt.trim();
    let mut title = trimmed.chars().take(48).collect::<String>();
    if trimmed.chars().count() > 48 {
        title.push_str("...");
    }
    if title.is_empty() {
        String::from("Probe Session")
    } else {
        title
    }
}

fn latest_tool_result_positions(transcript: &[TranscriptEvent]) -> BTreeMap<String, (u64, u32)> {
    let mut positions = BTreeMap::new();
    for event in transcript {
        for item in &event.turn.items {
            if item.kind != TranscriptItemKind::ToolResult {
                continue;
            }
            if let Some(call_id) = item.tool_call_id.as_ref() {
                positions.insert(call_id.clone(), (event.turn.index, item.sequence));
            }
        }
    }
    positions
}

fn should_replay_tool_result(
    turn_index: u64,
    item: &TranscriptItem,
    latest_positions: &BTreeMap<String, (u64, u32)>,
) -> bool {
    let Some(call_id) = item.tool_call_id.as_ref() else {
        return true;
    };
    latest_positions
        .get(call_id)
        .is_none_or(|position| *position == (turn_index, item.sequence))
}

impl ProbeRuntime {
    fn run_plain_text_turn(
        &self,
        session: SessionMetadata,
        profile: BackendProfile,
        prompt: String,
        tool_loop: Option<ToolLoopConfig>,
        event_sink: Option<Arc<dyn RuntimeEventSink>>,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        let tool_loop = tool_loop.filter(|config| !config.registry.is_empty());
        emit_runtime_event(
            event_sink.as_ref(),
            RuntimeEvent::TurnStarted {
                session_id: session.id.clone(),
                profile_name: profile.name.clone(),
                prompt: prompt.clone(),
                tool_loop_enabled: tool_loop.is_some(),
            },
        );
        if tool_loop.is_none() {
            return self.run_plain_completion_turn(session, profile, prompt, event_sink);
        }
        match profile.kind {
            BackendKind::OpenAiChatCompletions => self.run_openai_tool_loop_turn(
                session,
                profile,
                Some(prompt),
                tool_loop.expect("filtered"),
                event_sink,
            ),
            BackendKind::AppleFmBridge => self.run_apple_fm_tool_loop_turn(
                session,
                profile,
                Some(prompt),
                tool_loop.expect("filtered"),
                event_sink,
            ),
        }
    }

    fn run_openai_tool_loop_turn(
        &self,
        session: SessionMetadata,
        profile: BackendProfile,
        prompt: Option<String>,
        tool_loop: ToolLoopConfig,
        event_sink: Option<Arc<dyn RuntimeEventSink>>,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        let mut messages = self.replay_messages(&session)?;
        let mut pending_user_prompt = prompt;
        let mut executed_tool_calls = 0_usize;
        let mut tool_results = Vec::new();
        let max_round_trips = tool_loop.max_model_round_trips;

        for round_trip in 1..=max_round_trips {
            let next_user_prompt = pending_user_prompt.as_ref().cloned();
            if let Some(user_prompt) = next_user_prompt.as_ref() {
                messages.push(ChatMessage::user(user_prompt));
            }

            emit_runtime_event(
                event_sink.as_ref(),
                RuntimeEvent::ModelRequestStarted {
                    session_id: session.id.clone(),
                    round_trip,
                    backend_kind: profile.kind,
                },
            );
            let request_started = Instant::now();
            let response = {
                let mut stream_state = OpenAiStreamRuntimeState::default();
                let mut callback = |chunk: &ChatCompletionChunk| {
                    emit_openai_stream_events(
                        event_sink.as_ref(),
                        &session.id,
                        round_trip,
                        request_started,
                        &mut stream_state,
                        chunk,
                    );
                };
                if event_sink.is_some() {
                    openai_tool_loop_response_with_callback(
                        &profile,
                        messages.clone(),
                        tool_loop.registry.declared_tools(),
                        tool_loop.tool_choice.to_provider_choice(),
                        Some(tool_loop.parallel_tool_calls),
                        Some(&mut callback),
                    )
                } else {
                    openai_tool_loop_response(
                        &profile,
                        messages.clone(),
                        tool_loop.registry.declared_tools(),
                        tool_loop.tool_choice.to_provider_choice(),
                        Some(tool_loop.parallel_tool_calls),
                    )
                }
            }
            .map_err(|source| RuntimeError::ProviderRequest {
                session_id: session.id.clone(),
                source,
            });
            let wallclock_ms = elapsed_ms(request_started);

            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    emit_model_request_failed(
                        event_sink.as_ref(),
                        &session.id,
                        round_trip,
                        profile.kind,
                        &error,
                    );
                    let mut items = Vec::new();
                    if let Some(user_prompt) = pending_user_prompt.take() {
                        items.push(NewItem::new(TranscriptItemKind::UserMessage, user_prompt));
                    }
                    items.push(NewItem::new(TranscriptItemKind::Note, error.to_string()));
                    let _ = self.session_store.append_turn_with_details(
                        &session.id,
                        &items,
                        None,
                        error.backend_turn_receipt(),
                    );
                    return Err(error);
                }
            };
            let observability =
                self.build_turn_observability(&session.id, wallclock_ms, response.usage.as_ref())?;

            if let Some(tool_calls) = response.tool_calls.as_ref()
                && !tool_calls.is_empty()
            {
                let tool_calls = tool_calls.clone();
                for tool_call in &tool_calls {
                    emit_runtime_event(
                        event_sink.as_ref(),
                        RuntimeEvent::ToolCallRequested {
                            session_id: session.id.clone(),
                            round_trip,
                            call_id: tool_call.id.clone(),
                            tool_name: tool_call.function.name.clone(),
                            arguments: parsed_openai_tool_arguments(tool_call),
                        },
                    );
                }
                let tool_call_turn = self.append_tool_call_turn(
                    &session.id,
                    pending_user_prompt.take(),
                    &tool_calls,
                    Some(observability),
                    response.backend_receipt.clone(),
                )?;

                let execution_context = self.build_tool_execution_context(
                    &session,
                    &tool_loop,
                    next_user_prompt.as_deref(),
                )?;
                let session_id = session.id.clone();
                let event_sink_for_tools = event_sink.clone();
                let executed = tool_loop.registry.execute_batch_with_observer(
                    &execution_context,
                    tool_calls.as_slice(),
                    &tool_loop.approval,
                    &mut |call_id, tool_name, _arguments, risk_class| {
                        emit_runtime_event(
                            event_sink_for_tools.as_ref(),
                            RuntimeEvent::ToolExecutionStarted {
                                session_id: session_id.clone(),
                                round_trip,
                                call_id: call_id.to_string(),
                                tool_name: tool_name.to_string(),
                                risk_class,
                            },
                        );
                    },
                );
                for tool in &executed {
                    emit_tool_result_event(event_sink.as_ref(), &session.id, round_trip, tool);
                }
                executed_tool_calls += executed.iter().filter(|tool| tool.was_executed()).count();
                tool_results.extend(executed.clone());
                let tool_result_turn = self.append_tool_result_turn(&session.id, &executed)?;
                self.persist_pending_tool_approvals(
                    &session.id,
                    tool_call_turn.index,
                    tool_result_turn.index,
                    executed.as_slice(),
                )?;
                if let Some(paused) = executed.iter().find(|tool| tool.was_paused()) {
                    return Err(RuntimeError::ToolApprovalPending {
                        session_id: session.id.clone(),
                        tool_name: paused.name.clone(),
                        call_id: paused.call_id.clone(),
                        reason: paused.tool_execution.reason.clone(),
                    });
                }

                messages.push(ChatMessage::assistant_tool_calls(tool_calls.clone()));
                for tool_result in executed {
                    messages.push(ChatMessage::tool(
                        tool_result.name.clone(),
                        tool_result.call_id,
                        tool_result_model_text(tool_result.name.as_str(), &tool_result.output),
                    ));
                }
                continue;
            }

            let assistant_text = response.assistant_text.clone().ok_or_else(|| {
                RuntimeError::MissingAssistantMessage {
                    session_id: session.id.clone(),
                    response_id: response.response_id.clone(),
                }
            })?;
            let turn = self.append_assistant_turn(
                &session.id,
                pending_user_prompt.take(),
                assistant_text.clone(),
                Some(observability),
                response.backend_receipt,
            )?;
            emit_runtime_event(
                event_sink.as_ref(),
                RuntimeEvent::AssistantTurnCommitted {
                    session_id: session.id.clone(),
                    response_id: response.response_id.clone(),
                    response_model: response.response_model.clone(),
                    assistant_text: assistant_text.clone(),
                },
            );
            let session = self.session_store.read_metadata(&session.id)?;
            return Ok(PlainTextExecOutcome {
                session,
                turn,
                assistant_text,
                response_id: response.response_id,
                response_model: response.response_model,
                usage: response.usage,
                executed_tool_calls,
                tool_results,
            });
        }

        let _ = self.session_store.append_turn(
            &session.id,
            &[NewItem::new(
                TranscriptItemKind::Note,
                format!(
                    "session exceeded the configured tool loop bound of {} controller round trips",
                    max_round_trips
                ),
            )],
        );
        Err(RuntimeError::MaxToolRoundTrips {
            session_id: session.id,
            max_round_trips,
        })
    }

    fn run_apple_fm_tool_loop_turn(
        &self,
        session: SessionMetadata,
        profile: BackendProfile,
        prompt: Option<String>,
        tool_loop: ToolLoopConfig,
        event_sink: Option<Arc<dyn RuntimeEventSink>>,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        let prompt_text = prompt.clone().unwrap_or_default();
        let transcript = self.replay_apple_fm_transcript(&session)?;
        let execution_context =
            self.build_tool_execution_context(&session, &tool_loop, prompt.as_deref())?;
        let recorder = Arc::new(Mutex::new(AppleFmToolLoopRecorder::new(
            session.id.clone(),
            tool_loop
                .registry
                .execution_session(&execution_context, &tool_loop.approval),
            tool_loop.max_model_round_trips,
            event_sink.clone(),
        )));
        let callback_recorder = Arc::clone(&recorder);
        let tool_definitions = tool_loop
            .registry
            .declared_tools()
            .into_iter()
            .map(
                |tool| probe_provider_apple_fm::AppleFmProviderToolDefinition {
                    name: tool.function.name,
                    description: tool.function.description,
                    parameters: tool.function.parameters,
                },
            )
            .collect::<Vec<_>>();

        emit_runtime_event(
            event_sink.as_ref(),
            RuntimeEvent::ModelRequestStarted {
                session_id: session.id.clone(),
                round_trip: 1,
                backend_kind: profile.kind,
            },
        );
        let request_started = Instant::now();
        let response = {
            let mut stream_state = AppleFmStreamRuntimeState::default();
            let mut stream_callback = |event: &AppleFmTextStreamEvent| {
                emit_apple_fm_stream_events(
                    event_sink.as_ref(),
                    &session.id,
                    1,
                    session.id.as_str(),
                    request_started,
                    &mut stream_state,
                    event,
                );
            };
            if event_sink.is_some() {
                apple_fm_tool_loop_response_with_callback(
                    &profile,
                    session.system_prompt.as_deref(),
                    transcript,
                    prompt_text.as_str(),
                    tool_definitions,
                    Arc::new(move |tool_call| {
                        callback_recorder
                            .lock()
                            .expect("apple fm tool recorder lock")
                            .handle_call(tool_call)
                    }),
                    Some(&mut stream_callback),
                )
            } else {
                apple_fm_tool_loop_response(
                    &profile,
                    session.system_prompt.as_deref(),
                    transcript,
                    prompt_text.as_str(),
                    tool_definitions,
                    Arc::new(move |tool_call| {
                        callback_recorder
                            .lock()
                            .expect("apple fm tool recorder lock")
                            .handle_call(tool_call)
                    }),
                )
            }
        }
        .map_err(|source| RuntimeError::ProviderRequest {
            session_id: session.id.clone(),
            source,
        });
        let wallclock_ms = elapsed_ms(request_started);

        let (tool_results, interruption) = {
            let recorder = recorder.lock().expect("apple fm tool recorder lock");
            (recorder.records.clone(), recorder.interruption.clone())
        };
        let executed_tool_calls = tool_results
            .iter()
            .filter(|tool| tool.was_executed())
            .count();

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                if !tool_results.is_empty() {
                    let tool_call_turn = self.append_recorded_tool_call_turn(
                        &session.id,
                        prompt.clone(),
                        &tool_results,
                        None,
                        None,
                    );
                    let tool_result_turn = self.append_tool_result_turn(&session.id, &tool_results);
                    if let (Ok(tool_call_turn), Ok(tool_result_turn)) =
                        (&tool_call_turn, &tool_result_turn)
                    {
                        let _ = self.persist_pending_tool_approvals(
                            &session.id,
                            tool_call_turn.index,
                            tool_result_turn.index,
                            tool_results.as_slice(),
                        );
                    }
                } else if matches!(
                    interruption,
                    Some(AppleFmToolLoopInterruption::CallbackBudgetExceeded { .. })
                ) {
                    if let Some(prompt) = prompt.clone() {
                        let _ = self.session_store.append_turn(
                            &session.id,
                            &[NewItem::new(TranscriptItemKind::UserMessage, prompt)],
                        );
                    }
                }

                return match interruption {
                    Some(AppleFmToolLoopInterruption::ApprovalPending {
                        tool_name,
                        call_id,
                        reason,
                    }) => Err(RuntimeError::ToolApprovalPending {
                        session_id: session.id,
                        tool_name,
                        call_id,
                        reason,
                    }),
                    Some(AppleFmToolLoopInterruption::CallbackBudgetExceeded {
                        max_round_trips,
                    }) => {
                        let _ = self.session_store.append_turn(
                            &session.id,
                            &[NewItem::new(
                                TranscriptItemKind::Note,
                                format!(
                                    "session exceeded the configured tool loop bound of {} controller round trips",
                                    max_round_trips
                                ),
                            )],
                        );
                        Err(RuntimeError::MaxToolRoundTrips {
                            session_id: session.id,
                            max_round_trips,
                        })
                    }
                    None => {
                        emit_model_request_failed(
                            event_sink.as_ref(),
                            &session.id,
                            1,
                            profile.kind,
                            &error,
                        );
                        let mut items = Vec::new();
                        if tool_results.is_empty()
                            && let Some(prompt) = prompt.clone()
                        {
                            items.push(NewItem::new(TranscriptItemKind::UserMessage, prompt));
                        }
                        items.push(NewItem::new(TranscriptItemKind::Note, error.to_string()));
                        let _ = self.session_store.append_turn_with_details(
                            &session.id,
                            &items,
                            None,
                            error.backend_turn_receipt(),
                        );
                        Err(error)
                    }
                };
            }
        };
        let observability =
            self.build_turn_observability(&session.id, wallclock_ms, response.usage.as_ref())?;
        if !tool_results.is_empty() {
            let tool_call_turn = self.append_recorded_tool_call_turn(
                &session.id,
                prompt.clone(),
                &tool_results,
                None,
                None,
            )?;
            let tool_result_turn = self.append_tool_result_turn(&session.id, &tool_results)?;
            self.persist_pending_tool_approvals(
                &session.id,
                tool_call_turn.index,
                tool_result_turn.index,
                tool_results.as_slice(),
            )?;
            let turn = self.append_assistant_turn(
                &session.id,
                None,
                response.assistant_text.clone().ok_or_else(|| {
                    RuntimeError::MissingAssistantMessage {
                        session_id: session.id.clone(),
                        response_id: response.response_id.clone(),
                    }
                })?,
                Some(observability),
                response.backend_receipt,
            )?;
            emit_runtime_event(
                event_sink.as_ref(),
                RuntimeEvent::AssistantTurnCommitted {
                    session_id: session.id.clone(),
                    response_id: response.response_id.clone(),
                    response_model: response.response_model.clone(),
                    assistant_text: response.assistant_text.clone().unwrap_or_default(),
                },
            );
            let session = self.session_store.read_metadata(&session.id)?;
            return Ok(PlainTextExecOutcome {
                session,
                turn,
                assistant_text: response.assistant_text.unwrap_or_default(),
                response_id: response.response_id,
                response_model: response.response_model,
                usage: response.usage,
                executed_tool_calls,
                tool_results,
            });
        }

        let assistant_text = response.assistant_text.clone().ok_or_else(|| {
            RuntimeError::MissingAssistantMessage {
                session_id: session.id.clone(),
                response_id: response.response_id.clone(),
            }
        })?;
        let turn = self.append_assistant_turn(
            &session.id,
            prompt,
            assistant_text.clone(),
            Some(observability),
            response.backend_receipt,
        )?;
        emit_runtime_event(
            event_sink.as_ref(),
            RuntimeEvent::AssistantTurnCommitted {
                session_id: session.id.clone(),
                response_id: response.response_id.clone(),
                response_model: response.response_model.clone(),
                assistant_text: assistant_text.clone(),
            },
        );
        let session = self.session_store.read_metadata(&session.id)?;
        Ok(PlainTextExecOutcome {
            session,
            turn,
            assistant_text,
            response_id: response.response_id,
            response_model: response.response_model,
            usage: response.usage,
            executed_tool_calls,
            tool_results,
        })
    }

    fn run_plain_completion_turn(
        &self,
        session: SessionMetadata,
        profile: BackendProfile,
        prompt: String,
        event_sink: Option<Arc<dyn RuntimeEventSink>>,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        let mut messages = self.replay_plain_text_messages(&session)?;
        messages.push(PlainTextMessage::user(prompt.clone()));

        emit_runtime_event(
            event_sink.as_ref(),
            RuntimeEvent::ModelRequestStarted {
                session_id: session.id.clone(),
                round_trip: 1,
                backend_kind: profile.kind,
            },
        );
        let request_started = Instant::now();
        let response = match profile.kind {
            BackendKind::OpenAiChatCompletions if event_sink.is_some() => {
                let mut stream_state = OpenAiStreamRuntimeState::default();
                let mut callback = |chunk: &ChatCompletionChunk| {
                    emit_openai_stream_events(
                        event_sink.as_ref(),
                        &session.id,
                        1,
                        request_started,
                        &mut stream_state,
                        chunk,
                    );
                };
                complete_openai_plain_text_with_callback(&profile, messages, Some(&mut callback))
            }
            BackendKind::AppleFmBridge if event_sink.is_some() => {
                let mut stream_state = AppleFmStreamRuntimeState::default();
                let mut callback = |event: &AppleFmTextStreamEvent| {
                    emit_apple_fm_stream_events(
                        event_sink.as_ref(),
                        &session.id,
                        1,
                        session.id.as_str(),
                        request_started,
                        &mut stream_state,
                        event,
                    );
                };
                complete_apple_fm_plain_text_with_callback(&profile, messages, Some(&mut callback))
            }
            _ => complete_plain_text(&profile, messages),
        }
        .map_err(|source| RuntimeError::ProviderRequest {
            session_id: session.id.clone(),
            source,
        });
        let wallclock_ms = elapsed_ms(request_started);

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                emit_model_request_failed(
                    event_sink.as_ref(),
                    &session.id,
                    1,
                    profile.kind,
                    &error,
                );
                let _ = self.session_store.append_turn_with_details(
                    &session.id,
                    &[
                        NewItem::new(TranscriptItemKind::UserMessage, prompt),
                        NewItem::new(TranscriptItemKind::Note, error.to_string()),
                    ],
                    None,
                    error.backend_turn_receipt(),
                );
                return Err(error);
            }
        };
        let observability =
            self.build_turn_observability(&session.id, wallclock_ms, response.usage.as_ref())?;
        let assistant_text = response.assistant_text.clone().ok_or_else(|| {
            RuntimeError::MissingAssistantMessage {
                session_id: session.id.clone(),
                response_id: response.response_id.clone(),
            }
        })?;

        let turn = self.append_assistant_turn(
            &session.id,
            Some(prompt),
            assistant_text.clone(),
            Some(observability),
            response.backend_receipt,
        )?;
        emit_runtime_event(
            event_sink.as_ref(),
            RuntimeEvent::AssistantTurnCommitted {
                session_id: session.id.clone(),
                response_id: response.response_id.clone(),
                response_model: response.response_model.clone(),
                assistant_text: assistant_text.clone(),
            },
        );
        let session = self.session_store.read_metadata(&session.id)?;
        Ok(PlainTextExecOutcome {
            session,
            turn,
            assistant_text,
            response_id: response.response_id,
            response_model: response.response_model,
            usage: response.usage,
            executed_tool_calls: 0,
            tool_results: Vec::new(),
        })
    }

    fn ensure_no_pending_approvals(&self, session_id: &SessionId) -> Result<(), RuntimeError> {
        if let Some(pending) = self.pending_tool_approvals(session_id)?.into_iter().next() {
            return Err(RuntimeError::ToolApprovalPending {
                session_id: session_id.clone(),
                tool_name: pending.tool_name,
                call_id: pending.tool_call_id,
                reason: pending.reason,
            });
        }
        Ok(())
    }

    fn persist_pending_tool_approvals(
        &self,
        session_id: &SessionId,
        tool_call_turn_index: u64,
        paused_result_turn_index: u64,
        executed_tool_calls: &[ExecutedToolCall],
    ) -> Result<(), RuntimeError> {
        let approvals = executed_tool_calls
            .iter()
            .filter(|tool| tool.was_paused())
            .map(|tool| PendingToolApproval {
                session_id: session_id.clone(),
                tool_call_id: tool.call_id.clone(),
                tool_name: tool.name.clone(),
                arguments: tool.arguments.clone(),
                risk_class: tool.tool_execution.risk_class,
                reason: tool.tool_execution.reason.clone(),
                tool_call_turn_index,
                paused_result_turn_index,
                requested_at_ms: now_ms(),
                resolved_at_ms: None,
                resolution: None,
            })
            .collect::<Vec<_>>();
        self.session_store
            .append_pending_tool_approvals(session_id, approvals.as_slice())
            .map_err(RuntimeError::from)
    }

    fn execute_pending_tool_resolution(
        &self,
        session: &SessionMetadata,
        tool_loop: &ToolLoopConfig,
        pending: &PendingToolApproval,
        resolution: ToolApprovalResolution,
        event_sink: Option<&Arc<dyn RuntimeEventSink>>,
    ) -> Result<ExecutedToolCall, RuntimeError> {
        let execution_context = self.build_tool_execution_context(session, tool_loop, None)?;
        let mut execution_session = tool_loop
            .registry
            .execution_session(&execution_context, &tool_loop.approval);
        let mut observer = |call_id: &str, tool_name: &str, _arguments: &Value, risk_class| {
            emit_runtime_event(
                event_sink,
                RuntimeEvent::ToolExecutionStarted {
                    session_id: session.id.clone(),
                    round_trip: 1,
                    call_id: call_id.to_string(),
                    tool_name: tool_name.to_string(),
                    risk_class,
                },
            );
        };
        let tool = execution_session.execute_named_call_with_resolution(
            pending.tool_call_id.clone(),
            pending.tool_name.clone(),
            pending.arguments.clone(),
            pending.risk_class,
            resolution,
            &mut observer,
        );
        emit_tool_result_event(event_sink, &session.id, 1, &tool);
        Ok(tool)
    }

    fn resume_tool_loop_after_approval(
        &self,
        session: SessionMetadata,
        profile: BackendProfile,
        tool_loop: ToolLoopConfig,
        event_sink: Option<Arc<dyn RuntimeEventSink>>,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        emit_runtime_event(
            event_sink.as_ref(),
            RuntimeEvent::TurnStarted {
                session_id: session.id.clone(),
                profile_name: profile.name.clone(),
                prompt: String::from("[resume after approval]"),
                tool_loop_enabled: true,
            },
        );
        match profile.kind {
            BackendKind::OpenAiChatCompletions => {
                self.run_openai_tool_loop_turn(session, profile, None, tool_loop, event_sink)
            }
            BackendKind::AppleFmBridge => {
                self.run_apple_fm_tool_loop_turn(session, profile, None, tool_loop, event_sink)
            }
        }
    }

    fn build_tool_execution_context(
        &self,
        session: &SessionMetadata,
        tool_loop: &ToolLoopConfig,
        prompt: Option<&str>,
    ) -> Result<ToolExecutionContext, RuntimeError> {
        let transcript = self.session_store.read_transcript(&session.id)?;
        let session_summary = build_decision_summary(session, transcript.as_slice());
        let mut execution_context = ToolExecutionContext::new(session.cwd.clone());
        if let Some(oracle) = tool_loop.oracle.as_ref() {
            execution_context = execution_context.with_oracle(ToolOracleContext::new(
                oracle.profile.clone(),
                oracle.max_calls,
                session_summary.oracle_calls,
            ));
        }
        if let Some(long_context) = tool_loop.long_context.as_ref() {
            execution_context = execution_context.with_long_context(ToolLongContextContext::new(
                long_context.profile.clone(),
                long_context.max_calls,
                session_summary.long_context_calls,
                long_context.max_evidence_files,
                long_context.max_lines_per_file,
                prompt.map_or(0, |value| value.chars().count()),
                session_summary.files_listed.len(),
                session_summary.files_searched.len(),
                session_summary.files_read.len(),
                session_summary.too_many_turns,
                session_summary.oracle_calls,
            ));
        }
        Ok(execution_context)
    }

    fn replay_apple_fm_transcript(
        &self,
        session: &SessionMetadata,
    ) -> Result<AppleFmTranscript, RuntimeError> {
        let mut entries = Vec::new();
        let transcript = self.session_store.read_transcript(&session.id)?;
        let latest_tool_results = latest_tool_result_positions(transcript.as_slice());

        for event in transcript {
            let mut pending_tool_calls = Vec::new();
            let mut pending_tool_entry_id: Option<String> = None;
            for item in event.turn.items {
                match item.kind {
                    TranscriptItemKind::UserMessage => {
                        flush_apple_pending_tool_calls(
                            &mut entries,
                            &mut pending_tool_calls,
                            &mut pending_tool_entry_id,
                        );
                        entries.push(apple_fm_text_entry(
                            format!("turn-{}-user-{}", event.turn.index, item.sequence),
                            "user",
                            item.text,
                            BTreeMap::new(),
                        ));
                    }
                    TranscriptItemKind::AssistantMessage => {
                        flush_apple_pending_tool_calls(
                            &mut entries,
                            &mut pending_tool_calls,
                            &mut pending_tool_entry_id,
                        );
                        entries.push(apple_fm_text_entry(
                            format!("turn-{}-assistant-{}", event.turn.index, item.sequence),
                            "assistant",
                            item.text,
                            BTreeMap::new(),
                        ));
                    }
                    TranscriptItemKind::ToolCall => {
                        let name = item.name.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool call transcript items require a tool name",
                            ))
                        })?;
                        let arguments = item.arguments.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool call transcript items require structured arguments",
                            ))
                        })?;
                        if pending_tool_entry_id.is_none() {
                            pending_tool_entry_id =
                                Some(format!("turn-{}-assistant-tools", event.turn.index));
                        }
                        pending_tool_calls.push(serde_json::json!({
                            "name": name,
                            "arguments": arguments,
                        }));
                    }
                    TranscriptItemKind::ToolResult => {
                        if !should_replay_tool_result(event.turn.index, &item, &latest_tool_results)
                        {
                            continue;
                        }
                        flush_apple_pending_tool_calls(
                            &mut entries,
                            &mut pending_tool_calls,
                            &mut pending_tool_entry_id,
                        );
                        let name = item.name.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool result transcript items require a tool name",
                            ))
                        })?;
                        let mut extra = BTreeMap::from([(
                            String::from("toolName"),
                            serde_json::Value::String(name.clone()),
                        )]);
                        if let Some(tool_call_id) = item.tool_call_id {
                            extra.insert(
                                String::from("toolCallId"),
                                serde_json::Value::String(tool_call_id),
                            );
                        }
                        entries.push(apple_fm_text_entry(
                            format!("turn-{}-tool-{}", event.turn.index, item.sequence),
                            "tool",
                            stored_tool_result_model_text(name.as_str(), item.text.as_str()),
                            extra,
                        ));
                    }
                    TranscriptItemKind::Note => {}
                }
            }
            flush_apple_pending_tool_calls(
                &mut entries,
                &mut pending_tool_calls,
                &mut pending_tool_entry_id,
            );
        }

        Ok(AppleFmTranscript {
            version: APPLE_FM_TRANSCRIPT_VERSION,
            transcript_type: APPLE_FM_TRANSCRIPT_TYPE.to_string(),
            transcript: AppleFmTranscriptPayload { entries },
        })
    }

    fn replay_messages(&self, session: &SessionMetadata) -> Result<Vec<ChatMessage>, RuntimeError> {
        let mut messages = Vec::new();
        if let Some(system_prompt) = &session.system_prompt {
            messages.push(ChatMessage::system(system_prompt.clone()));
        }

        let transcript = self.session_store.read_transcript(&session.id)?;
        let latest_tool_results = latest_tool_result_positions(transcript.as_slice());
        for event in transcript {
            let mut pending_tool_calls = Vec::new();
            for item in event.turn.items {
                match item.kind {
                    TranscriptItemKind::UserMessage => {
                        if !pending_tool_calls.is_empty() {
                            messages.push(ChatMessage::assistant_tool_calls(std::mem::take(
                                &mut pending_tool_calls,
                            )));
                        }
                        messages.push(ChatMessage::user(item.text));
                    }
                    TranscriptItemKind::AssistantMessage => {
                        if !pending_tool_calls.is_empty() {
                            messages.push(ChatMessage::assistant_tool_calls(std::mem::take(
                                &mut pending_tool_calls,
                            )));
                        }
                        messages.push(ChatMessage::assistant(item.text))
                    }
                    TranscriptItemKind::ToolCall => {
                        let name = item.name.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool call transcript items require a tool name",
                            ))
                        })?;
                        let tool_call_id = item.tool_call_id.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool call transcript items require a tool_call_id",
                            ))
                        })?;
                        let arguments = item.arguments.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool call transcript items require structured arguments",
                            ))
                        })?;
                        pending_tool_calls.push(ChatToolCall {
                            id: tool_call_id,
                            kind: String::from("function"),
                            function: probe_provider_openai::ChatToolCallFunction {
                                name,
                                arguments: serde_json::to_string(&arguments).map_err(|error| {
                                    RuntimeError::MalformedTranscript(format!(
                                        "failed to encode stored tool arguments: {error}"
                                    ))
                                })?,
                            },
                        });
                    }
                    TranscriptItemKind::ToolResult => {
                        if !should_replay_tool_result(event.turn.index, &item, &latest_tool_results)
                        {
                            continue;
                        }
                        if !pending_tool_calls.is_empty() {
                            messages.push(ChatMessage::assistant_tool_calls(std::mem::take(
                                &mut pending_tool_calls,
                            )));
                        }
                        let name = item.name.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool result transcript items require a tool name",
                            ))
                        })?;
                        let tool_call_id = item.tool_call_id.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool result transcript items require a tool_call_id",
                            ))
                        })?;
                        let tool_result_text =
                            stored_tool_result_model_text(name.as_str(), item.text.as_str());
                        messages.push(ChatMessage::tool(name, tool_call_id, tool_result_text));
                    }
                    TranscriptItemKind::Note => {
                        if !pending_tool_calls.is_empty() {
                            messages.push(ChatMessage::assistant_tool_calls(std::mem::take(
                                &mut pending_tool_calls,
                            )));
                        }
                    }
                }
            }
            if !pending_tool_calls.is_empty() {
                messages.push(ChatMessage::assistant_tool_calls(pending_tool_calls));
            }
        }

        Ok(messages)
    }

    fn replay_plain_text_messages(
        &self,
        session: &SessionMetadata,
    ) -> Result<Vec<PlainTextMessage>, RuntimeError> {
        let mut messages = Vec::new();
        if let Some(system_prompt) = &session.system_prompt {
            messages.push(PlainTextMessage::system(system_prompt.clone()));
        }

        for event in self.session_store.read_transcript(&session.id)? {
            for item in event.turn.items {
                match item.kind {
                    TranscriptItemKind::UserMessage => {
                        messages.push(PlainTextMessage::user(item.text));
                    }
                    TranscriptItemKind::AssistantMessage => {
                        messages.push(PlainTextMessage::assistant(item.text));
                    }
                    TranscriptItemKind::Note => {}
                    TranscriptItemKind::ToolCall | TranscriptItemKind::ToolResult => {
                        return Err(RuntimeError::MalformedTranscript(String::from(
                            "plain-text backend replay does not support stored tool items",
                        )));
                    }
                }
            }
        }

        Ok(messages)
    }

    fn append_tool_call_turn(
        &self,
        session_id: &SessionId,
        user_prompt: Option<String>,
        tool_calls: &[ChatToolCall],
        observability: Option<TurnObservability>,
        backend_receipt: Option<BackendTurnReceipt>,
    ) -> Result<SessionTurn, RuntimeError> {
        let mut items = Vec::new();
        if let Some(user_prompt) = user_prompt {
            items.push(NewItem::new(TranscriptItemKind::UserMessage, user_prompt));
        }
        for tool_call in tool_calls {
            let arguments =
                serde_json::from_str::<serde_json::Value>(tool_call.function.arguments.as_str())
                    .map_err(|error| {
                        RuntimeError::MalformedTranscript(format!(
                            "tool call `{}` returned non-JSON arguments: {error}",
                            tool_call.function.name
                        ))
                    })?;
            items.push(NewItem::tool_call(
                tool_call.function.name.clone(),
                tool_call.id.clone(),
                arguments,
            ));
        }
        self.session_store
            .append_turn_with_details(session_id, &items, observability, backend_receipt)
            .map_err(RuntimeError::from)
    }

    fn append_tool_result_turn(
        &self,
        session_id: &SessionId,
        executed_tool_calls: &[crate::tools::ExecutedToolCall],
    ) -> Result<SessionTurn, RuntimeError> {
        let items = executed_tool_calls
            .iter()
            .map(|tool_call| {
                NewItem::tool_result(
                    tool_call.name.clone(),
                    tool_call.call_id.clone(),
                    serde_json::to_string(&tool_call.output).unwrap_or_else(|_| {
                        String::from("{\"error\":\"tool output encode failed\"}")
                    }),
                    tool_call.tool_execution.clone(),
                )
            })
            .collect::<Vec<_>>();
        self.session_store
            .append_turn(session_id, &items)
            .map_err(RuntimeError::from)
    }

    fn append_recorded_tool_call_turn(
        &self,
        session_id: &SessionId,
        user_prompt: Option<String>,
        executed_tool_calls: &[ExecutedToolCall],
        observability: Option<TurnObservability>,
        backend_receipt: Option<BackendTurnReceipt>,
    ) -> Result<SessionTurn, RuntimeError> {
        let mut items = Vec::new();
        if let Some(user_prompt) = user_prompt {
            items.push(NewItem::new(TranscriptItemKind::UserMessage, user_prompt));
        }
        for tool_call in executed_tool_calls {
            items.push(NewItem::tool_call(
                tool_call.name.clone(),
                tool_call.call_id.clone(),
                tool_call.arguments.clone(),
            ));
        }
        self.session_store
            .append_turn_with_details(session_id, &items, observability, backend_receipt)
            .map_err(RuntimeError::from)
    }

    fn append_assistant_turn(
        &self,
        session_id: &SessionId,
        user_prompt: Option<String>,
        assistant_text: String,
        observability: Option<TurnObservability>,
        backend_receipt: Option<BackendTurnReceipt>,
    ) -> Result<SessionTurn, RuntimeError> {
        let mut items = Vec::new();
        if let Some(user_prompt) = user_prompt {
            items.push(NewItem::new(TranscriptItemKind::UserMessage, user_prompt));
        }
        items.push(NewItem::new(
            TranscriptItemKind::AssistantMessage,
            assistant_text,
        ));
        self.session_store
            .append_turn_with_details(session_id, &items, observability, backend_receipt)
            .map_err(RuntimeError::from)
    }

    fn build_turn_observability(
        &self,
        session_id: &SessionId,
        wallclock_ms: u64,
        usage: Option<&ProviderUsage>,
    ) -> Result<TurnObservability, RuntimeError> {
        let prompt_tokens = usage.and_then(|usage| usage.prompt_tokens);
        let previous = if prompt_tokens.is_some() {
            self.last_prompt_bearing_observability(session_id)?
        } else {
            self.last_turn_observability(session_id)?
        };

        Ok(TurnObservability {
            wallclock_ms,
            model_output_ms: Some(wallclock_ms),
            prompt_tokens,
            prompt_tokens_detail: usage.and_then(|usage| {
                observability_usage_measurement(usage.prompt_tokens_detail.as_ref())
            }),
            completion_tokens: usage.and_then(|usage| usage.completion_tokens),
            completion_tokens_detail: usage.and_then(|usage| {
                observability_usage_measurement(usage.completion_tokens_detail.as_ref())
            }),
            total_tokens: usage.and_then(|usage| usage.total_tokens),
            total_tokens_detail: usage.and_then(|usage| {
                observability_usage_measurement(usage.total_tokens_detail.as_ref())
            }),
            completion_tokens_per_second_x1000: usage.and_then(|usage| {
                usage.completion_tokens.and_then(|completion_tokens| {
                    completion_tokens_per_second_x1000(completion_tokens, wallclock_ms)
                })
            }),
            cache_signal: infer_cache_signal(previous.as_ref(), prompt_tokens, wallclock_ms),
        })
    }

    fn last_turn_observability(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<TurnObservability>, RuntimeError> {
        let transcript = self.session_store.read_transcript(session_id)?;
        Ok(transcript
            .into_iter()
            .rev()
            .find_map(|event| event.turn.observability))
    }

    fn last_prompt_bearing_observability(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<TurnObservability>, RuntimeError> {
        let transcript = self.session_store.read_transcript(session_id)?;
        Ok(transcript.into_iter().rev().find_map(|event| {
            let observability = event.turn.observability?;
            observability.prompt_tokens.map(|_| observability)
        }))
    }
}

impl AppleFmToolLoopRecorder {
    fn new(
        session_id: SessionId,
        execution_session: ToolExecutionSession,
        max_callback_calls: usize,
        event_sink: Option<Arc<dyn RuntimeEventSink>>,
    ) -> Self {
        Self {
            session_id,
            execution_session,
            records: Vec::new(),
            next_call_index: 0,
            max_callback_calls,
            interruption: None,
            event_sink,
        }
    }

    fn handle_call(
        &mut self,
        tool_call: probe_provider_apple_fm::AppleFmProviderToolCall,
    ) -> Result<String, AppleFmToolCallError> {
        if self.next_call_index >= self.max_callback_calls {
            self.interruption = Some(AppleFmToolLoopInterruption::CallbackBudgetExceeded {
                max_round_trips: self.max_callback_calls,
            });
            return Err(AppleFmToolCallError::new(
                tool_call.name,
                format!(
                    "Probe controller-side Apple FM callback budget of {} round trips was exhausted",
                    self.max_callback_calls
                ),
            ));
        }

        self.next_call_index += 1;
        let call_id = format!("apple_fm_call_{}", self.next_call_index);
        emit_runtime_event(
            self.event_sink.as_ref(),
            RuntimeEvent::ToolCallRequested {
                session_id: self.session_id.clone(),
                round_trip: 1,
                call_id: call_id.clone(),
                tool_name: tool_call.name.clone(),
                arguments: tool_call.arguments.clone(),
            },
        );
        let session_id = self.session_id.clone();
        let event_sink = self.event_sink.clone();
        let executed = self.execution_session.execute_named_call_with_observer(
            call_id.clone(),
            tool_call.name.clone(),
            tool_call.arguments,
            &mut |call_id, tool_name, _arguments, risk_class| {
                emit_runtime_event(
                    event_sink.as_ref(),
                    RuntimeEvent::ToolExecutionStarted {
                        session_id: session_id.clone(),
                        round_trip: 1,
                        call_id: call_id.to_string(),
                        tool_name: tool_name.to_string(),
                        risk_class,
                    },
                );
            },
        );
        emit_tool_result_event(self.event_sink.as_ref(), &self.session_id, 1, &executed);
        let output = tool_result_model_text(executed.name.as_str(), &executed.output);
        if executed.was_paused() {
            self.interruption = Some(AppleFmToolLoopInterruption::ApprovalPending {
                tool_name: executed.name.clone(),
                call_id,
                reason: executed.tool_execution.reason.clone(),
            });
            self.records.push(executed.clone());
            return Err(AppleFmToolCallError::new(
                executed.name,
                executed
                    .tool_execution
                    .reason
                    .clone()
                    .unwrap_or_else(|| String::from("tool execution paused for approval")),
            ));
        }
        self.records.push(executed);
        Ok(output)
    }
}

fn apple_fm_text_entry(
    id: String,
    role: &str,
    text: String,
    extra: BTreeMap<String, serde_json::Value>,
) -> AppleFmTranscriptEntry {
    AppleFmTranscriptEntry {
        id: Some(id.clone()),
        role: role.to_string(),
        contents: vec![AppleFmTranscriptContent {
            content_type: String::from("text"),
            id: Some(format!("{id}-content")),
            extra: BTreeMap::from([(String::from("text"), serde_json::Value::String(text))]),
        }],
        extra,
    }
}

fn flush_apple_pending_tool_calls(
    entries: &mut Vec<AppleFmTranscriptEntry>,
    pending_tool_calls: &mut Vec<serde_json::Value>,
    pending_tool_entry_id: &mut Option<String>,
) {
    if pending_tool_calls.is_empty() {
        return;
    }
    let entry_id = pending_tool_entry_id
        .take()
        .unwrap_or_else(|| String::from("assistant-tools"));
    let mut extra = BTreeMap::new();
    extra.insert(
        String::from("toolCalls"),
        serde_json::Value::Array(std::mem::take(pending_tool_calls)),
    );
    entries.push(apple_fm_text_entry(
        entry_id,
        "assistant",
        String::new(),
        extra,
    ));
}

fn infer_cache_signal(
    previous: Option<&TurnObservability>,
    current_prompt_tokens: Option<u64>,
    current_wallclock_ms: u64,
) -> CacheSignal {
    let Some(previous) = previous else {
        return CacheSignal::ColdStart;
    };
    let Some(current_prompt_tokens) = current_prompt_tokens else {
        return CacheSignal::Unknown;
    };
    let Some(previous_prompt_tokens) = previous.prompt_tokens else {
        return CacheSignal::Unknown;
    };
    if previous.wallclock_ms == 0 || current_prompt_tokens < previous_prompt_tokens {
        return CacheSignal::NoClearSignal;
    }
    if current_wallclock_ms.saturating_mul(LIKELY_WARM_WALLCLOCK_RATIO_DENOMINATOR)
        <= previous
            .wallclock_ms
            .saturating_mul(LIKELY_WARM_WALLCLOCK_RATIO_NUMERATOR)
    {
        CacheSignal::LikelyWarm
    } else {
        CacheSignal::NoClearSignal
    }
}

fn completion_tokens_per_second_x1000(completion_tokens: u64, model_output_ms: u64) -> Option<u64> {
    if completion_tokens == 0 || model_output_ms == 0 {
        return None;
    }
    Some(completion_tokens.saturating_mul(1_000_000) / model_output_ms)
}

fn elapsed_ms(started: Instant) -> u64 {
    let elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    elapsed_ms.max(1)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use crate::backend_profiles::{psionic_apple_fm_bridge, psionic_qwen35_2b_q8_registry};
    use crate::tools::{ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction, ToolLoopConfig};
    use probe_protocol::session::{
        CacheSignal, SessionHarnessProfile, ToolApprovalResolution, ToolApprovalState,
        ToolPolicyDecision, TranscriptItemKind, UsageTruth,
    };
    use probe_test_support::{
        FakeAppleFmServer, FakeHttpRequest, FakeHttpResponse, FakeOpenAiServer,
        ProbeTestEnvironment,
    };
    use serde_json::json;

    use super::{
        PlainTextExecRequest, PlainTextResumeRequest, ProbeRuntime,
        ResolvePendingToolApprovalOutcome, ResolvePendingToolApprovalRequest, RuntimeError,
        RuntimeEvent, RuntimeEventSink, default_session_title,
    };

    #[derive(Default)]
    struct AppleFmSessionBridgeState {
        callback_url: String,
        session_token: String,
        create_requests: Vec<serde_json::Value>,
        response_requests: Vec<serde_json::Value>,
    }

    struct ToolCallbackResponse {
        status_code: u16,
        body: String,
    }

    #[derive(Default)]
    struct TestRuntimeEventCollector {
        events: Mutex<Vec<RuntimeEvent>>,
    }

    impl RuntimeEventSink for TestRuntimeEventCollector {
        fn emit(&self, event: RuntimeEvent) {
            self.events
                .lock()
                .expect("runtime event collector lock")
                .push(event);
        }
    }

    impl TestRuntimeEventCollector {
        fn snapshot(&self) -> Vec<RuntimeEvent> {
            self.events
                .lock()
                .expect("runtime event collector lock")
                .clone()
        }
    }

    fn record_apple_session_create(
        state: &Arc<Mutex<AppleFmSessionBridgeState>>,
        request: &FakeHttpRequest,
        session_id: &str,
    ) -> FakeHttpResponse {
        let request_json: serde_json::Value =
            serde_json::from_str(request.body.as_str()).expect("session create json");
        let callback_url = request_json["tool_callback"]["url"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let session_token = request_json["tool_callback"]["session_token"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let mut guard = state.lock().expect("apple fm session bridge lock");
        guard.callback_url = callback_url;
        guard.session_token = session_token;
        guard.create_requests.push(request_json.clone());
        FakeHttpResponse::json_ok(json!({
            "session": {
                "id": session_id,
                "instructions": request_json["instructions"],
                "model": {
                    "id": "apple-foundation-model",
                    "use_case": "general",
                    "guardrails": "default"
                },
                "tools": request_json["tools"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|tool| json!({
                        "name": tool["name"],
                        "description": tool["description"]
                    }))
                    .collect::<Vec<_>>(),
                "is_responding": false,
                "transcript_json": serde_json::to_string(&request_json["transcript"])
                    .unwrap_or_else(|_| String::from("{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"))
            }
        }))
    }

    fn invoke_apple_tool_callback(
        state: &Arc<Mutex<AppleFmSessionBridgeState>>,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> ToolCallbackResponse {
        let (callback_url, session_token) = {
            let guard = state.lock().expect("apple fm session bridge lock");
            (guard.callback_url.clone(), guard.session_token.clone())
        };
        let url = callback_url
            .strip_prefix("http://")
            .expect("callback url should be http");
        let (authority, path) = url
            .split_once('/')
            .expect("callback url should include path");
        let body = json!({
            "session_token": session_token,
            "tool_name": tool_name,
            "arguments": {
                "content": arguments,
                "is_complete": true
            }
        })
        .to_string();
        let mut stream = TcpStream::connect(authority).expect("connect tool callback");
        let request = format!(
            "POST /{} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            path,
            authority,
            body.len(),
            body
        );
        stream
            .write_all(request.as_bytes())
            .expect("write tool callback request");
        stream.flush().expect("flush tool callback request");
        stream
            .shutdown(Shutdown::Write)
            .expect("close tool callback request writer");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read tool callback response");
        let (head, body) = response
            .split_once("\r\n\r\n")
            .expect("tool callback response should include body");
        let status_code = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u16>().ok())
            .expect("tool callback status code");
        ToolCallbackResponse {
            status_code,
            body: body.to_string(),
        }
    }

    fn record_apple_response_request(
        state: &Arc<Mutex<AppleFmSessionBridgeState>>,
        request: &FakeHttpRequest,
    ) {
        let request_json: serde_json::Value =
            serde_json::from_str(request.body.as_str()).expect("session respond json");
        state
            .lock()
            .expect("apple fm session bridge lock")
            .response_requests
            .push(request_json);
    }

    #[test]
    fn default_title_is_trimmed_for_exec_prompts() {
        assert_eq!(default_session_title("  hello world  "), "hello world");
        assert_eq!(default_session_title(""), "Probe Session");
    }

    #[test]
    fn exec_plain_text_persists_session_and_transcript() {
        let server = FakeOpenAiServer::from_json_responses(vec![serde_json::json!({
            "id": "chatcmpl_exec_test",
            "model": "qwen3.5-2b-q8_0-registry.gguf",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "hello from probe exec"},
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 6,
                "completion_tokens": 5,
                "total_tokens": 11
            }
        })]);

        let environment = ProbeTestEnvironment::new();
        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = String::from(server.base_url());

        let outcome = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("say hello"),
                title: Some(String::from("Exec Test")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: None,
                harness_profile: Some(SessionHarnessProfile {
                    name: String::from("coding_bootstrap_default"),
                    version: String::from("v1"),
                }),
                tool_loop: None,
            })
            .expect("exec should succeed");

        assert_eq!(outcome.assistant_text, "hello from probe exec");
        assert_eq!(outcome.response_id, "chatcmpl_exec_test");
        assert_eq!(outcome.response_model, "qwen3.5-2b-q8_0-registry.gguf");
        assert_eq!(outcome.turn.items.len(), 2);
        assert_eq!(outcome.session.title, "Exec Test");
        let observability = outcome
            .turn
            .observability
            .as_ref()
            .expect("observability should be recorded");
        assert!(observability.wallclock_ms > 0);
        assert_eq!(
            observability.model_output_ms,
            Some(observability.wallclock_ms)
        );
        assert_eq!(observability.prompt_tokens, Some(6));
        assert_eq!(
            observability
                .prompt_tokens_detail
                .as_ref()
                .expect("prompt token detail should exist")
                .truth,
            UsageTruth::Exact
        );
        assert_eq!(observability.completion_tokens, Some(5));
        assert_eq!(observability.total_tokens, Some(11));
        assert!(
            observability
                .completion_tokens_per_second_x1000
                .expect("throughput should be computed")
                > 0
        );
        assert!(matches!(observability.cache_signal, CacheSignal::ColdStart));
        assert_eq!(
            outcome
                .session
                .backend
                .as_ref()
                .expect("backend metadata should exist")
                .profile_name,
            "psionic-qwen35-2b-q8-registry"
        );
        assert_eq!(
            outcome
                .session
                .harness_profile
                .as_ref()
                .expect("harness profile should persist")
                .name,
            "coding_bootstrap_default"
        );

        let transcript = runtime
            .session_store()
            .read_transcript(&outcome.session.id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].turn.items[0].text, "say hello");
        assert_eq!(transcript[0].turn.items[1].text, "hello from probe exec");
        assert_eq!(
            transcript[0]
                .turn
                .observability
                .as_ref()
                .expect("observability should persist")
                .prompt_tokens,
            Some(6)
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("say hello"));
    }

    #[test]
    fn apple_fm_exec_plain_text_persists_session_and_transcript() {
        let server = FakeAppleFmServer::from_json_responses(vec![serde_json::json!({
            "id": "apple_fm_exec_test",
            "model": "apple-foundation-model",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "hello from apple fm"},
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens_detail": {"value": 9, "truth": "estimated"},
                "completion_tokens_detail": {"value": 4, "truth": "estimated"},
                "total_tokens_detail": {"value": 13, "truth": "estimated"}
            }
        })]);

        let environment = ProbeTestEnvironment::new();
        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = String::from(server.base_url());

        let outcome = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("say hello"),
                title: Some(String::from("Apple FM Exec Test")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: Some(String::from("You are helpful")),
                harness_profile: None,
                tool_loop: None,
            })
            .expect("apple fm exec should succeed");

        assert_eq!(outcome.assistant_text, "hello from apple fm");
        assert_eq!(outcome.response_id, "apple_fm_exec_test");
        assert_eq!(outcome.response_model, "apple-foundation-model");
        assert_eq!(outcome.turn.items.len(), 2);
        assert_eq!(
            outcome
                .session
                .backend
                .as_ref()
                .expect("backend metadata")
                .profile_name,
            "psionic-apple-fm-bridge"
        );
        let observability = outcome
            .turn
            .observability
            .as_ref()
            .expect("observability should exist");
        assert_eq!(observability.prompt_tokens, Some(9));
        assert_eq!(
            observability
                .prompt_tokens_detail
                .as_ref()
                .expect("prompt token detail should exist")
                .truth,
            UsageTruth::Estimated
        );
        assert_eq!(observability.completion_tokens, Some(4));
        assert_eq!(observability.total_tokens, Some(13));

        let transcript = runtime
            .session_store()
            .read_transcript(&outcome.session.id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].turn.items[1].text, "hello from apple fm");

        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("POST /v1/chat/completions HTTP/1.1"));
        assert!(requests[0].contains("\"system\""));
        assert!(requests[0].contains("say hello"));
    }

    #[test]
    fn eventful_apple_fm_plain_turn_emits_snapshot_events_when_backend_supports_streaming() {
        let environment = ProbeTestEnvironment::new();
        let server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => FakeHttpResponse::json_ok(json!({
                    "session": {
                        "id": "sess-apple-stream-plain-1",
                        "model": {
                            "id": "apple-foundation-model",
                            "use_case": "general",
                            "guardrails": "default"
                        },
                        "tools": [],
                        "is_responding": false,
                        "transcript_json": "{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"
                    }
                })),
                ("POST", "/v1/sessions/sess-apple-stream-plain-1/responses/stream") => {
                    FakeHttpResponse::text_event_stream(
                        200,
                        concat!(
                            "event: snapshot\n",
                            "data: {\"kind\":\"snapshot\",\"model\":\"apple-foundation-model\",\"output\":\"hello\"}\n\n",
                            "event: completed\n",
                            "data: {\"kind\":\"completed\",\"model\":\"apple-foundation-model\",\"output\":\"hello world\",\"session\":{\"id\":\"sess-apple-stream-plain-1\",\"model\":{\"id\":\"apple-foundation-model\",\"use_case\":\"general\",\"guardrails\":\"default\"},\"tools\":[],\"is_responding\":false,\"transcript_json\":\"{\\\"version\\\":1,\\\"type\\\":\\\"FoundationModels.Transcript\\\",\\\"transcript\\\":{\\\"entries\\\":[]}}\"},\"usage\":{\"total_tokens_detail\":{\"value\":13,\"truth\":\"estimated\"}}}\n\n",
                        ),
                    )
                }
                ("DELETE", "/v1/sessions/sess-apple-stream-plain-1") => {
                    FakeHttpResponse::json_ok(json!({}))
                }
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();
        let collector = Arc::new(TestRuntimeEventCollector::default());

        let outcome = runtime
            .exec_plain_text_with_events(
                PlainTextExecRequest {
                    profile,
                    prompt: String::from("say hello"),
                    title: Some(String::from("Apple FM Streamed Plain Turn")),
                    cwd: environment.workspace().to_path_buf(),
                    system_prompt: Some(String::from("You are helpful")),
                    harness_profile: None,
                    tool_loop: None,
                },
                collector.clone(),
            )
            .expect("apple fm streamed plain turn should succeed");

        assert_eq!(outcome.assistant_text, "hello world");
        let events = collector.snapshot();
        let session_id = outcome.session.id.clone();
        assert_eq!(events.len(), 8);
        assert!(
            matches!(&events[0], RuntimeEvent::TurnStarted { session_id: event_session, .. } if event_session == &session_id)
        );
        assert!(
            matches!(&events[1], RuntimeEvent::ModelRequestStarted { session_id: event_session, round_trip: 1, backend_kind: probe_protocol::backend::BackendKind::AppleFmBridge } if event_session == &session_id)
        );
        assert!(
            matches!(&events[2], RuntimeEvent::AssistantStreamStarted { session_id: event_session, round_trip: 1, response_id, .. } if event_session == &session_id && response_id == session_id.as_str())
        );
        assert!(
            matches!(&events[3], RuntimeEvent::TimeToFirstTokenObserved { session_id: event_session, round_trip: 1, .. } if event_session == &session_id)
        );
        assert!(
            matches!(&events[4], RuntimeEvent::AssistantSnapshot { session_id: event_session, round_trip: 1, snapshot } if event_session == &session_id && snapshot == "hello")
        );
        assert!(
            matches!(&events[5], RuntimeEvent::AssistantSnapshot { session_id: event_session, round_trip: 1, snapshot } if event_session == &session_id && snapshot == "hello world")
        );
        assert!(
            matches!(&events[6], RuntimeEvent::AssistantStreamFinished { session_id: event_session, round_trip: 1, finish_reason, .. } if event_session == &session_id && finish_reason.as_deref() == Some("snapshot_completed"))
        );
        assert!(
            matches!(&events[7], RuntimeEvent::AssistantTurnCommitted { session_id: event_session, assistant_text, .. } if event_session == &session_id && assistant_text == "hello world")
        );
    }

    #[test]
    fn continue_plain_text_session_replays_prior_context() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            for expected_response in ["first answer", "second answer"] {
                let (mut stream, _) = listener.accept().expect("accept connection");
                let mut buffer = [0_u8; 8192];
                let bytes = stream.read(&mut buffer).expect("read request");
                let request_body = String::from_utf8_lossy(&buffer[..bytes]);
                if expected_response == "second answer" {
                    assert!(request_body.contains("first prompt"));
                    assert!(request_body.contains("first answer"));
                    assert!(request_body.contains("second prompt"));
                    thread::sleep(Duration::from_millis(5));
                } else {
                    thread::sleep(Duration::from_millis(60));
                }
                let body = serde_json::json!({
                    "id": format!("chatcmpl_{expected_response}"),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": expected_response},
                            "finish_reason": "stop"
                        }
                    ],
                    "usage": {
                        "prompt_tokens": 10,
                        "completion_tokens": 4,
                        "total_tokens": 14
                    }
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });

        let temp = tempfile::tempdir().expect("temp dir");
        let runtime = ProbeRuntime::new(temp.path().join(".probe"));
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = format!("http://{address}/v1");

        let first = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile: profile.clone(),
                prompt: String::from("first prompt"),
                title: Some(String::from("Interactive Test")),
                cwd: temp.path().to_path_buf(),
                system_prompt: Some(String::from("You are helpful")),
                harness_profile: None,
                tool_loop: None,
            })
            .expect("first turn should succeed");

        let second = runtime
            .continue_plain_text_session(PlainTextResumeRequest {
                session_id: first.session.id.clone(),
                profile,
                prompt: String::from("second prompt"),
                tool_loop: None,
            })
            .expect("second turn should succeed");

        assert_eq!(second.assistant_text, "second answer");
        assert_eq!(second.turn.index, 1);
        assert_eq!(
            second
                .session
                .system_prompt
                .as_deref()
                .expect("system prompt should be persisted"),
            "You are helpful"
        );

        let transcript = runtime
            .session_store()
            .read_transcript(&first.session.id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[1].turn.items[0].text, "second prompt");
        assert!(matches!(
            transcript[0]
                .turn
                .observability
                .as_ref()
                .expect("first turn observability should exist")
                .cache_signal,
            CacheSignal::ColdStart
        ));
        assert!(matches!(
            transcript[1]
                .turn
                .observability
                .as_ref()
                .expect("second turn observability should exist")
                .cache_signal,
            CacheSignal::LikelyWarm
        ));

        handle.join().expect("server thread should exit cleanly");
    }

    #[test]
    fn apple_fm_resume_replays_prior_context() {
        let server = FakeAppleFmServer::from_json_responses(vec![
            serde_json::json!({
                "id": "apple_fm_first",
                "model": "apple-foundation-model",
                "choices": [
                    {
                        "index": 0,
                        "message": {"role": "assistant", "content": "first apple answer"},
                        "finish_reason": "stop"
                    }
                ]
            }),
            serde_json::json!({
                "id": "apple_fm_second",
                "model": "apple-foundation-model",
                "choices": [
                    {
                        "index": 0,
                        "message": {"role": "assistant", "content": "second apple answer"},
                        "finish_reason": "stop"
                    }
                ]
            }),
        ]);

        let temp = tempfile::tempdir().expect("temp dir");
        let runtime = ProbeRuntime::new(temp.path().join(".probe"));
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();

        let first = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile: profile.clone(),
                prompt: String::from("first prompt"),
                title: Some(String::from("Apple FM Chat")),
                cwd: temp.path().to_path_buf(),
                system_prompt: Some(String::from("You are helpful")),
                harness_profile: None,
                tool_loop: None,
            })
            .expect("first turn should succeed");

        let second = runtime
            .continue_plain_text_session(PlainTextResumeRequest {
                session_id: first.session.id.clone(),
                profile,
                prompt: String::from("second prompt"),
                tool_loop: None,
            })
            .expect("second turn should succeed");

        assert_eq!(second.assistant_text, "second apple answer");
        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].contains("first prompt"));
        assert!(requests[1].contains("first apple answer"));
        assert!(requests[1].contains("second prompt"));
    }

    #[test]
    fn apple_fm_tool_loop_executes_probe_tools_through_session_callbacks() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let bridge_state = Arc::new(Mutex::new(AppleFmSessionBridgeState::default()));
        let captured_state = Arc::clone(&bridge_state);
        let server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => {
                    record_apple_session_create(&captured_state, &request, "sess-apple-tool-1")
                }
                ("POST", "/v1/sessions/sess-apple-tool-1/responses") => {
                    record_apple_response_request(&captured_state, &request);
                    let callback_response = invoke_apple_tool_callback(
                        &captured_state,
                        "read_file",
                        json!({
                            "path": "hello.txt",
                            "start_line": 1,
                            "max_lines": 10
                        }),
                    );
                    assert_eq!(callback_response.status_code, 200);
                    let callback_json: serde_json::Value =
                        serde_json::from_str(callback_response.body.as_str())
                            .expect("callback json");
                    let callback_output = callback_json["output"]
                        .as_str()
                        .expect("callback output text");
                    assert!(callback_output.contains("path: hello.txt"));
                    assert!(callback_output.contains("content:"));
                    assert!(callback_output.contains("hello world"));
                    assert!(!callback_output.contains("\"content\""));
                    FakeHttpResponse::json_ok(json!({
                        "session": {
                            "id": "sess-apple-tool-1",
                            "instructions": "You are helpful",
                            "model": {
                                "id": "apple-foundation-model",
                                "use_case": "general",
                                "guardrails": "default"
                            },
                            "tools": [{"name": "read_file"}],
                            "is_responding": false,
                            "transcript_json": "{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"
                        },
                        "model": "apple-foundation-model",
                        "output": format!(
                            "tool-backed answer: {}",
                            callback_json["output"].as_str().unwrap_or_default()
                        ),
                        "usage": {
                            "total_tokens_detail": {"value": 21, "truth": "estimated"}
                        }
                    }))
                }
                ("DELETE", "/v1/sessions/sess-apple-tool-1") => {
                    FakeHttpResponse::json_ok(json!({}))
                }
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();

        let outcome = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("Use read_file on hello.txt and tell me the contents."),
                title: Some(String::from("Apple FM Tool Success")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: Some(String::from("You are helpful")),
                harness_profile: None,
                tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                    ProbeToolChoice::Required,
                    false,
                )),
            })
            .expect("apple fm tool loop should succeed");

        assert!(outcome.assistant_text.contains("hello world"));
        assert_eq!(outcome.executed_tool_calls, 1);
        assert_eq!(outcome.tool_results.len(), 1);
        assert_eq!(
            outcome.tool_results[0].tool_execution.policy_decision,
            ToolPolicyDecision::AutoAllow
        );
        let transcript = runtime
            .session_store()
            .read_transcript(&outcome.session.id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 3);
        assert!(matches!(
            transcript[0].turn.items[1].kind,
            TranscriptItemKind::ToolCall
        ));
        assert!(matches!(
            transcript[1].turn.items[0].kind,
            TranscriptItemKind::ToolResult
        ));
        assert!(matches!(
            transcript[2].turn.items[0].kind,
            TranscriptItemKind::AssistantMessage
        ));
        assert_eq!(
            transcript[2]
                .turn
                .observability
                .as_ref()
                .and_then(|observability| observability.total_tokens_detail.as_ref())
                .map(|detail| detail.truth),
            Some(UsageTruth::Estimated)
        );
        assert_eq!(
            transcript[2]
                .turn
                .backend_receipt
                .as_ref()
                .and_then(|receipt| receipt.transcript.as_ref())
                .map(|transcript| transcript.format.as_str()),
            Some("foundation_models.transcript.v1")
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
    }

    #[test]
    fn apple_fm_tool_loop_refusal_persists_probe_receipts() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let bridge_state = Arc::new(Mutex::new(AppleFmSessionBridgeState::default()));
        let captured_state = Arc::clone(&bridge_state);
        let server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => {
                    record_apple_session_create(&captured_state, &request, "sess-apple-refuse-1")
                }
                ("POST", "/v1/sessions/sess-apple-refuse-1/responses") => {
                    record_apple_response_request(&captured_state, &request);
                    let callback_response = invoke_apple_tool_callback(
                        &captured_state,
                        "apply_patch",
                        json!({
                            "path": "hello.txt",
                            "old_text": "world",
                            "new_text": "probe"
                        }),
                    );
                    assert_eq!(callback_response.status_code, 200);
                    let callback_json: serde_json::Value =
                        serde_json::from_str(callback_response.body.as_str())
                            .expect("callback json");
                    FakeHttpResponse::json_ok(json!({
                        "session": {
                            "id": "sess-apple-refuse-1",
                            "instructions": "You are helpful",
                            "model": {
                                "id": "apple-foundation-model",
                                "use_case": "general",
                                "guardrails": "default"
                            },
                            "tools": [{"name": "apply_patch"}],
                            "is_responding": false,
                            "transcript_json": "{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"
                        },
                        "model": "apple-foundation-model",
                        "output": format!(
                            "refused output seen: {}",
                            callback_json["output"].as_str().unwrap_or_default()
                        )
                    }))
                }
                ("DELETE", "/v1/sessions/sess-apple-refuse-1") => {
                    FakeHttpResponse::json_ok(json!({}))
                }
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();

        let outcome = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("Patch hello.txt."),
                title: Some(String::from("Apple FM Tool Refusal")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: Some(String::from("You are helpful")),
                harness_profile: None,
                tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                    ProbeToolChoice::Required,
                    false,
                )),
            })
            .expect("apple fm refusal should complete");

        assert_eq!(outcome.executed_tool_calls, 0);
        assert_eq!(outcome.tool_results.len(), 1);
        assert_eq!(
            outcome.tool_results[0].tool_execution.policy_decision,
            ToolPolicyDecision::Refused
        );
        let transcript = runtime
            .session_store()
            .read_transcript(&outcome.session.id)
            .expect("read transcript");
        assert_eq!(
            transcript[1].turn.items[0]
                .tool_execution
                .as_ref()
                .expect("tool execution should persist")
                .policy_decision,
            ToolPolicyDecision::Refused
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
    }

    #[test]
    fn apple_fm_tool_loop_can_pause_for_approval() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let bridge_state = Arc::new(Mutex::new(AppleFmSessionBridgeState::default()));
        let captured_state = Arc::clone(&bridge_state);
        let server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => {
                    record_apple_session_create(&captured_state, &request, "sess-apple-pause-1")
                }
                ("POST", "/v1/sessions/sess-apple-pause-1/responses") => {
                    record_apple_response_request(&captured_state, &request);
                    let callback_response = invoke_apple_tool_callback(
                        &captured_state,
                        "apply_patch",
                        json!({
                            "path": "hello.txt",
                            "old_text": "world",
                            "new_text": "probe"
                        }),
                    );
                    assert_eq!(callback_response.status_code, 422);
                    let callback_error: serde_json::Value =
                        serde_json::from_str(callback_response.body.as_str())
                            .expect("callback error json");
                    FakeHttpResponse::json_status(
                        422,
                        json!({
                            "error": {
                                "message": format!(
                                    "tool '{}' failed: {}",
                                    callback_error["tool_name"].as_str().unwrap_or_default(),
                                    callback_error["underlying_error"].as_str().unwrap_or_default()
                                ),
                                "type": "tool_call_failed",
                                "code": "tool_call_failed",
                                "tool_name": callback_error["tool_name"],
                                "underlying_error": callback_error["underlying_error"]
                            }
                        }),
                    )
                }
                ("DELETE", "/v1/sessions/sess-apple-pause-1") => {
                    FakeHttpResponse::json_ok(json!({}))
                }
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();
        let mut tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Required, false);
        tool_loop.approval = ToolApprovalConfig {
            allow_write_tools: false,
            allow_network_shell: false,
            allow_destructive_shell: false,
            denied_action: ToolDeniedAction::Pause,
        };

        let error = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("Patch hello.txt."),
                title: Some(String::from("Apple FM Tool Pause")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: Some(String::from("You are helpful")),
                harness_profile: None,
                tool_loop: Some(tool_loop),
            })
            .expect_err("apple fm pause should surface pending approval");

        assert!(matches!(
            error,
            super::RuntimeError::ToolApprovalPending { .. }
        ));
        let sessions = runtime
            .session_store()
            .list_sessions()
            .expect("list sessions");
        let transcript = runtime
            .session_store()
            .read_transcript(&sessions[0].id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 2);
        assert_eq!(
            transcript[1].turn.items[0]
                .tool_execution
                .as_ref()
                .expect("tool execution should persist")
                .policy_decision,
            ToolPolicyDecision::Paused
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
    }

    #[test]
    fn apple_fm_tool_loop_resume_reconstructs_session_transcript() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let bridge_state = Arc::new(Mutex::new(AppleFmSessionBridgeState::default()));
        let captured_state = Arc::clone(&bridge_state);
        let response_count = Arc::new(Mutex::new(0_usize));
        let captured_responses = Arc::clone(&response_count);
        let server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => {
                    let index = captured_state
                        .lock()
                        .expect("apple fm session bridge lock")
                        .create_requests
                        .len()
                        + 1;
                    record_apple_session_create(
                        &captured_state,
                        &request,
                        format!("sess-apple-resume-{index}").as_str(),
                    )
                }
                ("POST", path) if path.starts_with("/v1/sessions/sess-apple-resume-") => {
                    record_apple_response_request(&captured_state, &request);
                    let mut response_index = captured_responses
                        .lock()
                        .expect("apple fm response count lock");
                    *response_index += 1;
                    if *response_index == 1 {
                        let callback_response = invoke_apple_tool_callback(
                            &captured_state,
                            "read_file",
                            json!({
                                "path": "hello.txt",
                                "start_line": 1,
                                "max_lines": 10
                            }),
                        );
                        assert_eq!(callback_response.status_code, 200);
                        FakeHttpResponse::json_ok(json!({
                            "session": {
                                "id": "sess-apple-resume-1",
                                "instructions": "You are helpful",
                                "model": {
                                    "id": "apple-foundation-model",
                                    "use_case": "general",
                                    "guardrails": "default"
                                },
                                "tools": [{"name": "read_file"}],
                                "is_responding": false,
                                "transcript_json": "{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"
                            },
                            "model": "apple-foundation-model",
                            "output": "first tool-backed answer"
                        }))
                    } else {
                        FakeHttpResponse::json_ok(json!({
                            "session": {
                                "id": "sess-apple-resume-2",
                                "instructions": "You are helpful",
                                "model": {
                                    "id": "apple-foundation-model",
                                    "use_case": "general",
                                    "guardrails": "default"
                                },
                                "tools": [{"name": "read_file"}],
                                "is_responding": false,
                                "transcript_json": "{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"
                            },
                            "model": "apple-foundation-model",
                            "output": "second tool-backed answer"
                        }))
                    }
                }
                ("DELETE", path) if path.starts_with("/v1/sessions/sess-apple-resume-") => {
                    FakeHttpResponse::json_ok(json!({}))
                }
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();
        let first = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile: profile.clone(),
                prompt: String::from("Read hello.txt"),
                title: Some(String::from("Apple FM Tool Resume")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: Some(String::from("You are helpful")),
                harness_profile: None,
                tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                    ProbeToolChoice::Required,
                    false,
                )),
            })
            .expect("first apple fm tool turn should succeed");

        let second = runtime
            .continue_plain_text_session(PlainTextResumeRequest {
                session_id: first.session.id.clone(),
                profile,
                prompt: String::from("Summarize the previous read."),
                tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                    ProbeToolChoice::Required,
                    false,
                )),
            })
            .expect("second apple fm tool turn should succeed");

        assert_eq!(second.assistant_text, "second tool-backed answer");
        let bridge_state = bridge_state.lock().expect("apple fm session bridge lock");
        assert_eq!(bridge_state.create_requests.len(), 2);
        let restored_entries =
            bridge_state.create_requests[1]["transcript"]["transcript"]["entries"]
                .as_array()
                .expect("restored entries");
        assert!(restored_entries.iter().any(|entry| {
            entry["toolCalls"]
                .as_array()
                .is_some_and(|calls| calls.iter().any(|call| call["name"] == "read_file"))
        }));
        assert!(restored_entries.iter().any(|entry| {
            entry["role"] == "tool"
                && entry["toolName"] == "read_file"
                && entry["contents"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("path: hello.txt")
        }));
        assert!(restored_entries.iter().any(|entry| {
            entry["role"] == "tool"
                && entry["toolName"] == "read_file"
                && entry["contents"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("content:")
        }));
        assert!(restored_entries.iter().any(|entry| {
            entry["role"] == "tool"
                && entry["toolName"] == "read_file"
                && entry["contents"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("hello world")
        }));
        drop(bridge_state);
        let requests = server.finish();
        assert_eq!(requests.len(), 6);
    }

    #[test]
    fn apple_fm_plain_text_provider_failure_persists_typed_backend_receipt() {
        let environment = ProbeTestEnvironment::new();
        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let server = FakeAppleFmServer::from_responses(vec![FakeHttpResponse::json_status(
            503,
            json!({
                "error": {
                    "message": "Apple Intelligence is not enabled",
                    "type": "assets_unavailable",
                    "code": "assets_unavailable",
                    "failure_reason": "Apple Intelligence is disabled",
                    "recovery_suggestion": "Enable Apple Intelligence and retry"
                }
            }),
        )]);
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();

        let error = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("say hello"),
                title: Some(String::from("Apple FM Failure Receipt")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: None,
                harness_profile: None,
                tool_loop: None,
            })
            .expect_err("apple fm request should fail");

        assert!(matches!(error, super::RuntimeError::ProviderRequest { .. }));
        let sessions = runtime
            .session_store()
            .list_sessions()
            .expect("list sessions");
        let transcript = runtime
            .session_store()
            .read_transcript(&sessions[0].id)
            .expect("read transcript");
        let receipt = transcript[0]
            .turn
            .backend_receipt
            .as_ref()
            .expect("backend receipt should persist");
        assert_eq!(
            receipt
                .failure
                .as_ref()
                .expect("failure receipt should exist")
                .code
                .as_deref(),
            Some("assets_unavailable")
        );
        assert_eq!(
            receipt
                .availability
                .as_ref()
                .expect("availability receipt should exist")
                .ready,
            false
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
    }

    #[test]
    fn tool_loop_executes_required_single_tool_and_replays_result() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            for step in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept connection");
                let mut buffer = [0_u8; 8192];
                let bytes = stream.read(&mut buffer).expect("read request");
                let request_body = String::from_utf8_lossy(&buffer[..bytes]);
                if step == 0 {
                    assert!(request_body.contains("\"tools\""));
                    assert!(request_body.contains("\"tool_choice\":\"required\""));
                    assert!(request_body.contains("\"parallel_tool_calls\":false"));
                } else {
                    assert!(request_body.contains("\"tool_calls\""));
                    assert!(request_body.contains("\"call_readme_1\""));
                    assert!(request_body.contains("\"read_file\""));
                    assert!(request_body.contains("path: README.md"));
                    assert!(request_body.contains("content:"));
                }
                let body = if step == 0 {
                    serde_json::json!({
                        "id": "chatcmpl_tool_required",
                        "model": "qwen3.5-2b-q8_0-registry.gguf",
                        "choices": [
                            {
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "tool_calls": [
                                        {
                                            "id": "call_readme_1",
                                            "type": "function",
                                            "function": {
                                                "name": "read_file",
                                                "arguments": "{\"path\":\"README.md\",\"start_line\":1,\"max_lines\":8}"
                                            }
                                        }
                                    ]
                                },
                                "finish_reason": "tool_calls"
                            }
                        ]
                    })
                } else {
                    serde_json::json!({
                        "id": "chatcmpl_tool_final",
                        "model": "qwen3.5-2b-q8_0-registry.gguf",
                        "choices": [
                            {
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "content": "README inspected."
                                },
                                "finish_reason": "stop"
                            }
                        ],
                        "usage": {
                            "prompt_tokens": 20,
                            "completion_tokens": 6,
                            "total_tokens": 26
                        }
                    })
                }
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });

        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            temp.path().join("README.md"),
            "# Probe\n\nA coding-agent runtime.\n",
        )
        .expect("write readme");
        let runtime = ProbeRuntime::new(temp.path().join(".probe"));
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = format!("http://{address}/v1");

        let outcome = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("Read README.md and summarize it."),
                title: Some(String::from("Tool Test")),
                cwd: temp.path().to_path_buf(),
                system_prompt: None,
                harness_profile: None,
                tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                    ProbeToolChoice::Required,
                    false,
                )),
            })
            .expect("tool loop should succeed");

        assert_eq!(outcome.assistant_text, "README inspected.");
        assert_eq!(outcome.executed_tool_calls, 1);
        assert_eq!(outcome.tool_results.len(), 1);
        assert_eq!(outcome.tool_results[0].name, "read_file");
        assert_eq!(outcome.tool_results[0].output["path"], "README.md");
        assert_eq!(
            outcome.tool_results[0].tool_execution.policy_decision,
            ToolPolicyDecision::AutoAllow
        );

        let transcript = runtime
            .session_store()
            .read_transcript(&outcome.session.id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 3);
        assert!(transcript[0].turn.observability.is_some());
        assert!(transcript[1].turn.observability.is_none());
        assert!(transcript[2].turn.observability.is_some());
        assert!(matches!(
            transcript[0].turn.items[1].kind,
            TranscriptItemKind::ToolCall
        ));
        assert!(matches!(
            transcript[1].turn.items[0].kind,
            TranscriptItemKind::ToolResult
        ));
        assert_eq!(
            transcript[1].turn.items[0]
                .tool_execution
                .as_ref()
                .expect("tool execution should persist")
                .approval_state,
            ToolApprovalState::NotRequired
        );
        assert_eq!(transcript[2].turn.items[0].text, "README inspected.");

        handle.join().expect("server thread should exit cleanly");
    }

    #[test]
    fn eventful_tool_loop_emits_ordered_events_for_successful_turn() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let server = FakeOpenAiServer::from_json_responses(vec![
            json!({
                "id": "chatcmpl_events_tool_1",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "call_readme_1",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"README.md\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
            json!({
                "id": "chatcmpl_events_tool_final",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "README inspected."
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 18,
                    "completion_tokens": 4,
                    "total_tokens": 22
                }
            }),
        ]);
        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = server.base_url().to_string();
        let collector = Arc::new(TestRuntimeEventCollector::default());

        let outcome = runtime
            .exec_plain_text_with_events(
                PlainTextExecRequest {
                    profile,
                    prompt: String::from("Inspect README.md"),
                    title: Some(String::from("Eventful Success")),
                    cwd: environment.workspace().to_path_buf(),
                    system_prompt: None,
                    harness_profile: None,
                    tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                        ProbeToolChoice::Required,
                        false,
                    )),
                },
                collector.clone(),
            )
            .expect("eventful tool loop should succeed");

        assert_eq!(outcome.assistant_text, "README inspected.");
        let events = collector.snapshot();
        let session_id = outcome.session.id.clone();
        assert_eq!(
            events,
            vec![
                RuntimeEvent::TurnStarted {
                    session_id: session_id.clone(),
                    profile_name: String::from("psionic-qwen35-2b-q8-registry"),
                    prompt: String::from("Inspect README.md"),
                    tool_loop_enabled: true,
                },
                RuntimeEvent::ModelRequestStarted {
                    session_id: session_id.clone(),
                    round_trip: 1,
                    backend_kind: probe_protocol::backend::BackendKind::OpenAiChatCompletions,
                },
                RuntimeEvent::ToolCallRequested {
                    session_id: session_id.clone(),
                    round_trip: 1,
                    call_id: String::from("call_readme_1"),
                    tool_name: String::from("read_file"),
                    arguments: json!({"path":"README.md"}),
                },
                RuntimeEvent::ToolExecutionStarted {
                    session_id: session_id.clone(),
                    round_trip: 1,
                    call_id: String::from("call_readme_1"),
                    tool_name: String::from("read_file"),
                    risk_class: probe_protocol::session::ToolRiskClass::ReadOnly,
                },
                RuntimeEvent::ToolExecutionCompleted {
                    session_id: session_id.clone(),
                    round_trip: 1,
                    tool: outcome.tool_results[0].clone(),
                },
                RuntimeEvent::ModelRequestStarted {
                    session_id: session_id.clone(),
                    round_trip: 2,
                    backend_kind: probe_protocol::backend::BackendKind::OpenAiChatCompletions,
                },
                RuntimeEvent::AssistantTurnCommitted {
                    session_id,
                    response_id: String::from("chatcmpl_events_tool_final"),
                    response_model: String::from("qwen3.5-2b-q8_0-registry.gguf"),
                    assistant_text: String::from("README inspected."),
                },
            ]
        );
    }

    #[test]
    fn eventful_plain_openai_turn_emits_stream_deltas_when_backend_supports_streaming() {
        let environment = ProbeTestEnvironment::new();
        let server = FakeOpenAiServer::from_responses(vec![FakeHttpResponse::text_event_stream(
            200,
            concat!(
                "data: {\"id\":\"chatcmpl_stream_plain\",\"model\":\"qwen3.5-2b-q8_0-registry.gguf\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hello\"}}]}\n\n",
                "data: {\"id\":\"chatcmpl_stream_plain\",\"model\":\"qwen3.5-2b-q8_0-registry.gguf\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"}}]}\n\n",
                "data: {\"id\":\"chatcmpl_stream_plain\",\"model\":\"qwen3.5-2b-q8_0-registry.gguf\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
                "data: [DONE]\n\n",
            ),
        )]);
        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = server.base_url().to_string();
        let collector = Arc::new(TestRuntimeEventCollector::default());

        let outcome = runtime
            .exec_plain_text_with_events(
                PlainTextExecRequest {
                    profile,
                    prompt: String::from("say hello"),
                    title: Some(String::from("Streamed Plain Turn")),
                    cwd: environment.workspace().to_path_buf(),
                    system_prompt: None,
                    harness_profile: None,
                    tool_loop: None,
                },
                collector.clone(),
            )
            .expect("streamed plain turn should succeed");

        assert_eq!(outcome.assistant_text, "hello world");
        assert_eq!(
            outcome.usage.as_ref().and_then(|usage| usage.total_tokens),
            Some(5)
        );
        let events = collector.snapshot();
        let session_id = outcome.session.id.clone();
        assert_eq!(events.len(), 8);
        assert!(matches!(
            &events[0],
            RuntimeEvent::TurnStarted {
                session_id: event_session,
                prompt,
                tool_loop_enabled: false,
                ..
            } if event_session == &session_id && prompt == "say hello"
        ));
        assert!(matches!(
            &events[1],
            RuntimeEvent::ModelRequestStarted {
                session_id: event_session,
                round_trip: 1,
                backend_kind: probe_protocol::backend::BackendKind::OpenAiChatCompletions,
            } if event_session == &session_id
        ));
        assert!(matches!(
            &events[2],
            RuntimeEvent::AssistantStreamStarted {
                session_id: event_session,
                round_trip: 1,
                response_id,
                response_model,
            } if event_session == &session_id
                && response_id == "chatcmpl_stream_plain"
                && response_model == "qwen3.5-2b-q8_0-registry.gguf"
        ));
        assert!(matches!(
            &events[3],
            RuntimeEvent::TimeToFirstTokenObserved {
                session_id: event_session,
                round_trip: 1,
                ..
            } if event_session == &session_id
        ));
        assert!(matches!(
            &events[4],
            RuntimeEvent::AssistantDelta {
                session_id: event_session,
                round_trip: 1,
                delta,
            } if event_session == &session_id && delta == "hello"
        ));
        assert!(matches!(
            &events[5],
            RuntimeEvent::AssistantDelta {
                session_id: event_session,
                round_trip: 1,
                delta,
            } if event_session == &session_id && delta == " world"
        ));
        assert!(matches!(
            &events[6],
            RuntimeEvent::AssistantStreamFinished {
                session_id: event_session,
                round_trip: 1,
                response_id,
                finish_reason,
                ..
            } if event_session == &session_id
                && response_id == "chatcmpl_stream_plain"
                && finish_reason.as_deref() == Some("stop")
        ));
        assert!(matches!(
            &events[7],
            RuntimeEvent::AssistantTurnCommitted {
                session_id: event_session,
                assistant_text,
                ..
            } if event_session == &session_id && assistant_text == "hello world"
        ));
    }

    #[test]
    fn eventful_plain_openai_turn_unwraps_message_envelope_text() {
        let environment = ProbeTestEnvironment::new();
        let server = FakeOpenAiServer::from_responses(vec![FakeHttpResponse::text_event_stream(
            200,
            concat!(
                "data: {\"id\":\"chatcmpl_stream_envelope\",\"model\":\"qwen3.5-2b-q8_0-registry.gguf\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"{\\\"kind\\\":\\\"message\\\",\"}}]}\n\n",
                "data: {\"id\":\"chatcmpl_stream_envelope\",\"model\":\"qwen3.5-2b-q8_0-registry.gguf\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\\\"content\\\":\\\"hello world\\\"}\"}}]}\n\n",
                "data: {\"id\":\"chatcmpl_stream_envelope\",\"model\":\"qwen3.5-2b-q8_0-registry.gguf\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2,\"total_tokens\":6}}\n\n",
                "data: [DONE]\n\n",
            ),
        )]);
        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = server.base_url().to_string();
        let collector = Arc::new(TestRuntimeEventCollector::default());

        let outcome = runtime
            .exec_plain_text_with_events(
                PlainTextExecRequest {
                    profile,
                    prompt: String::from("say hello"),
                    title: Some(String::from("Envelope Plain Turn")),
                    cwd: environment.workspace().to_path_buf(),
                    system_prompt: None,
                    harness_profile: None,
                    tool_loop: None,
                },
                collector.clone(),
            )
            .expect("streamed envelope turn should succeed");

        assert_eq!(outcome.assistant_text, "hello world");
        let events = collector.snapshot();
        let session_id = outcome.session.id.clone();
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeEvent::AssistantTurnCommitted {
                session_id: event_session,
                assistant_text,
                ..
            } if event_session == &session_id && assistant_text == "hello world"
        )));
    }

    #[test]
    fn eventful_openai_tool_loop_emits_streamed_tool_call_deltas() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let server = FakeOpenAiServer::from_responses(vec![
            FakeHttpResponse::text_event_stream(
                200,
                concat!(
                    "data: {\"id\":\"chatcmpl_stream_tool_1\",\"model\":\"qwen3.5-2b-q8_0-registry.gguf\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"call_readme_1\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"README.md\\\"}\"}}]}}]}\n\n",
                    "data: {\"id\":\"chatcmpl_stream_tool_1\",\"model\":\"qwen3.5-2b-q8_0-registry.gguf\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
                    "data: [DONE]\n\n",
                ),
            ),
            FakeHttpResponse::text_event_stream(
                200,
                concat!(
                    "data: {\"id\":\"chatcmpl_stream_tool_2\",\"model\":\"qwen3.5-2b-q8_0-registry.gguf\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"README inspected.\"}}]}\n\n",
                    "data: {\"id\":\"chatcmpl_stream_tool_2\",\"model\":\"qwen3.5-2b-q8_0-registry.gguf\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":18,\"completion_tokens\":4,\"total_tokens\":22}}\n\n",
                    "data: [DONE]\n\n",
                ),
            ),
        ]);
        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = server.base_url().to_string();
        let collector = Arc::new(TestRuntimeEventCollector::default());

        let outcome = runtime
            .exec_plain_text_with_events(
                PlainTextExecRequest {
                    profile,
                    prompt: String::from("Inspect README.md"),
                    title: Some(String::from("Streamed Tool Loop")),
                    cwd: environment.workspace().to_path_buf(),
                    system_prompt: None,
                    harness_profile: None,
                    tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                        ProbeToolChoice::Required,
                        false,
                    )),
                },
                collector.clone(),
            )
            .expect("streamed tool loop should succeed");

        assert_eq!(outcome.assistant_text, "README inspected.");
        assert_eq!(outcome.tool_results.len(), 1);
        let events = collector.snapshot();
        let session_id = outcome.session.id.clone();

        assert!(
            matches!(&events[0], RuntimeEvent::TurnStarted { session_id: event_session, .. } if event_session == &session_id)
        );
        assert!(
            matches!(&events[1], RuntimeEvent::ModelRequestStarted { session_id: event_session, round_trip: 1, .. } if event_session == &session_id)
        );
        assert!(
            matches!(&events[2], RuntimeEvent::AssistantStreamStarted { session_id: event_session, round_trip: 1, response_id, .. } if event_session == &session_id && response_id == "chatcmpl_stream_tool_1")
        );
        assert!(
            matches!(&events[3], RuntimeEvent::TimeToFirstTokenObserved { session_id: event_session, round_trip: 1, .. } if event_session == &session_id)
        );
        assert!(matches!(
            &events[4],
            RuntimeEvent::ToolCallDelta {
                session_id: event_session,
                round_trip: 1,
                deltas,
            } if event_session == &session_id
                && deltas == &vec![super::StreamedToolCallDelta {
                    tool_index: 0,
                    call_id: Some(String::from("call_readme_1")),
                    tool_name: Some(String::from("read_file")),
                    arguments_delta: Some(String::from("{\"path\":\"README.md\"}")),
                }]
        ));
        assert!(matches!(
            &events[5],
            RuntimeEvent::AssistantStreamFinished {
                session_id: event_session,
                round_trip: 1,
                response_id,
                finish_reason,
                ..
            } if event_session == &session_id
                && response_id == "chatcmpl_stream_tool_1"
                && finish_reason.as_deref() == Some("tool_calls")
        ));
        assert!(
            matches!(&events[6], RuntimeEvent::ToolCallRequested { session_id: event_session, round_trip: 1, call_id, tool_name, .. } if event_session == &session_id && call_id == "call_readme_1" && tool_name == "read_file")
        );
        assert!(
            matches!(&events[7], RuntimeEvent::ToolExecutionStarted { session_id: event_session, round_trip: 1, .. } if event_session == &session_id)
        );
        assert!(
            matches!(&events[8], RuntimeEvent::ToolExecutionCompleted { session_id: event_session, round_trip: 1, .. } if event_session == &session_id)
        );
        assert!(
            matches!(&events[9], RuntimeEvent::ModelRequestStarted { session_id: event_session, round_trip: 2, .. } if event_session == &session_id)
        );
        assert!(
            matches!(&events[10], RuntimeEvent::AssistantStreamStarted { session_id: event_session, round_trip: 2, response_id, .. } if event_session == &session_id && response_id == "chatcmpl_stream_tool_2")
        );
        assert!(
            matches!(&events[11], RuntimeEvent::TimeToFirstTokenObserved { session_id: event_session, round_trip: 2, .. } if event_session == &session_id)
        );
        assert!(
            matches!(&events[12], RuntimeEvent::AssistantDelta { session_id: event_session, round_trip: 2, delta } if event_session == &session_id && delta == "README inspected.")
        );
        assert!(
            matches!(&events[13], RuntimeEvent::AssistantStreamFinished { session_id: event_session, round_trip: 2, finish_reason, .. } if event_session == &session_id && finish_reason.as_deref() == Some("stop"))
        );
        assert!(
            matches!(&events[14], RuntimeEvent::AssistantTurnCommitted { session_id: event_session, assistant_text, .. } if event_session == &session_id && assistant_text == "README inspected.")
        );
    }

    #[test]
    fn eventful_apple_fm_tool_loop_emits_snapshot_events_and_local_tool_lifecycle() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let bridge_state = Arc::new(Mutex::new(AppleFmSessionBridgeState::default()));
        let captured_state = Arc::clone(&bridge_state);
        let server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => record_apple_session_create(
                    &captured_state,
                    &request,
                    "sess-apple-stream-tool-1",
                ),
                ("POST", "/v1/sessions/sess-apple-stream-tool-1/responses/stream") => {
                    let callback_response = invoke_apple_tool_callback(
                        &captured_state,
                        "read_file",
                        json!({
                            "path": "README.md",
                            "start_line": 1,
                            "max_lines": 8
                        }),
                    );
                    assert_eq!(callback_response.status_code, 200);
                    FakeHttpResponse::text_event_stream(
                        200,
                        concat!(
                            "event: snapshot\n",
                            "data: {\"kind\":\"snapshot\",\"model\":\"apple-foundation-model\",\"output\":\"reading README.md\"}\n\n",
                            "event: completed\n",
                            "data: {\"kind\":\"completed\",\"model\":\"apple-foundation-model\",\"output\":\"README inspected.\",\"session\":{\"id\":\"sess-apple-stream-tool-1\",\"instructions\":\"You are helpful\",\"model\":{\"id\":\"apple-foundation-model\",\"use_case\":\"general\",\"guardrails\":\"default\"},\"tools\":[{\"name\":\"read_file\"}],\"is_responding\":false,\"transcript_json\":\"{\\\"version\\\":1,\\\"type\\\":\\\"FoundationModels.Transcript\\\",\\\"transcript\\\":{\\\"entries\\\":[]}}\"},\"usage\":{\"total_tokens_detail\":{\"value\":21,\"truth\":\"estimated\"}}}\n\n",
                        ),
                    )
                }
                ("DELETE", "/v1/sessions/sess-apple-stream-tool-1") => {
                    FakeHttpResponse::json_ok(json!({}))
                }
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();
        let collector = Arc::new(TestRuntimeEventCollector::default());

        let outcome = runtime
            .exec_plain_text_with_events(
                PlainTextExecRequest {
                    profile,
                    prompt: String::from("Inspect README.md"),
                    title: Some(String::from("Apple FM Streamed Tool Loop")),
                    cwd: environment.workspace().to_path_buf(),
                    system_prompt: Some(String::from("You are helpful")),
                    harness_profile: None,
                    tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                        ProbeToolChoice::Required,
                        false,
                    )),
                },
                collector.clone(),
            )
            .expect("apple fm streamed tool loop should succeed");

        assert_eq!(outcome.assistant_text, "README inspected.");
        assert_eq!(outcome.tool_results.len(), 1);
        let events = collector.snapshot();
        let session_id = outcome.session.id.clone();
        assert!(
            matches!(&events[0], RuntimeEvent::TurnStarted { session_id: event_session, .. } if event_session == &session_id)
        );
        assert!(
            matches!(&events[1], RuntimeEvent::ModelRequestStarted { session_id: event_session, round_trip: 1, backend_kind: probe_protocol::backend::BackendKind::AppleFmBridge } if event_session == &session_id)
        );
        assert!(
            matches!(&events[2], RuntimeEvent::ToolCallRequested { session_id: event_session, round_trip: 1, call_id, tool_name, .. } if event_session == &session_id && call_id == "apple_fm_call_1" && tool_name == "read_file")
        );
        assert!(
            matches!(&events[3], RuntimeEvent::ToolExecutionStarted { session_id: event_session, round_trip: 1, call_id, tool_name, .. } if event_session == &session_id && call_id == "apple_fm_call_1" && tool_name == "read_file")
        );
        assert!(
            matches!(&events[4], RuntimeEvent::ToolExecutionCompleted { session_id: event_session, round_trip: 1, tool } if event_session == &session_id && tool.name == "read_file")
        );
        assert!(
            matches!(&events[5], RuntimeEvent::AssistantStreamStarted { session_id: event_session, round_trip: 1, response_id, .. } if event_session == &session_id && response_id == session_id.as_str())
        );
        assert!(
            matches!(&events[6], RuntimeEvent::TimeToFirstTokenObserved { session_id: event_session, round_trip: 1, .. } if event_session == &session_id)
        );
        assert!(
            matches!(&events[7], RuntimeEvent::AssistantSnapshot { session_id: event_session, round_trip: 1, snapshot } if event_session == &session_id && snapshot == "reading README.md")
        );
        assert!(
            matches!(&events[8], RuntimeEvent::AssistantSnapshot { session_id: event_session, round_trip: 1, snapshot } if event_session == &session_id && snapshot == "README inspected.")
        );
        assert!(
            matches!(&events[9], RuntimeEvent::AssistantStreamFinished { session_id: event_session, round_trip: 1, finish_reason, .. } if event_session == &session_id && finish_reason.as_deref() == Some("snapshot_completed"))
        );
        assert!(
            matches!(&events[10], RuntimeEvent::AssistantTurnCommitted { session_id: event_session, assistant_text, .. } if event_session == &session_id && assistant_text == "README inspected.")
        );
    }

    #[test]
    fn tool_loop_can_pause_for_approval() {
        let server = FakeOpenAiServer::from_json_responses(vec![serde_json::json!({
            "id": "chatcmpl_pause_test",
            "model": "qwen3.5-2b-q8_0-registry.gguf",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "tool_calls": [
                            {
                                "id": "call_patch_1",
                                "type": "function",
                                "function": {
                                    "name": "apply_patch",
                                    "arguments": "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        })]);

        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = String::from(server.base_url());
        let mut tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Required, false);
        tool_loop.approval = ToolApprovalConfig {
            allow_write_tools: false,
            allow_network_shell: false,
            allow_destructive_shell: false,
            denied_action: ToolDeniedAction::Pause,
        };

        let error = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("patch hello.txt"),
                title: Some(String::from("Pause Test")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: None,
                harness_profile: None,
                tool_loop: Some(tool_loop),
            })
            .expect_err("tool loop should pause");

        assert!(matches!(
            error,
            super::RuntimeError::ToolApprovalPending { .. }
        ));

        let sessions = runtime
            .session_store()
            .list_sessions()
            .expect("list sessions");
        let session_id = sessions[0].id.clone();
        let transcript = runtime
            .session_store()
            .read_transcript(&session_id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 2);
        assert_eq!(
            transcript[1].turn.items[0]
                .tool_execution
                .as_ref()
                .expect("tool execution should persist")
                .policy_decision,
            ToolPolicyDecision::Paused
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("\"tool_choice\":\"required\""));
    }

    #[test]
    fn resolving_pending_tool_approval_can_approve_and_resume_openai_turn() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let server = FakeOpenAiServer::from_json_responses(vec![
            serde_json::json!({
                "id": "chatcmpl_pause_then_approve_1",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "call_patch_1",
                            "type": "function",
                            "function": {
                                "name": "apply_patch",
                                "arguments": "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
            serde_json::json!({
                "id": "chatcmpl_pause_then_approve_2",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Patched hello.txt after approval."
                    },
                    "finish_reason": "stop"
                }]
            }),
        ]);

        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = String::from(server.base_url());
        let mut tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Required, false);
        tool_loop.approval = ToolApprovalConfig {
            allow_write_tools: false,
            allow_network_shell: false,
            allow_destructive_shell: false,
            denied_action: ToolDeniedAction::Pause,
        };

        let error = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile: profile.clone(),
                prompt: String::from("patch hello.txt"),
                title: Some(String::from("Pause Then Approve")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: None,
                harness_profile: None,
                tool_loop: Some(tool_loop.clone()),
            })
            .expect_err("tool loop should pause");
        let RuntimeError::ToolApprovalPending { session_id, .. } = error else {
            panic!("expected tool approval pending error");
        };

        let pending = runtime
            .pending_tool_approvals(&session_id)
            .expect("read pending approvals");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].tool_call_id, "call_patch_1");

        let outcome = runtime
            .resolve_pending_tool_approval(ResolvePendingToolApprovalRequest {
                session_id: session_id.clone(),
                profile,
                tool_loop,
                call_id: String::from("call_patch_1"),
                resolution: ToolApprovalResolution::Approved,
            })
            .expect("approval resolution should succeed");

        let ResolvePendingToolApprovalOutcome::Resumed { outcome } = outcome else {
            panic!("expected resumed outcome");
        };
        assert_eq!(outcome.assistant_text, "Patched hello.txt after approval.");
        assert_eq!(
            std::fs::read_to_string(environment.workspace().join("hello.txt"))
                .expect("read patched file"),
            "hello probe\n"
        );
        assert!(
            runtime
                .pending_tool_approvals(&session_id)
                .expect("pending approvals should clear")
                .is_empty()
        );
        let transcript = runtime
            .session_store()
            .read_transcript(&session_id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 4);
        assert_eq!(
            transcript[2].turn.items[0]
                .tool_execution
                .as_ref()
                .expect("resolved tool execution should persist")
                .policy_decision,
            ToolPolicyDecision::Approved
        );
    }

    #[test]
    fn resolving_pending_tool_approval_can_reject_and_resume_openai_turn() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let server = FakeOpenAiServer::from_json_responses(vec![
            serde_json::json!({
                "id": "chatcmpl_pause_then_reject_1",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "call_patch_1",
                            "type": "function",
                            "function": {
                                "name": "apply_patch",
                                "arguments": "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
            serde_json::json!({
                "id": "chatcmpl_pause_then_reject_2",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "The patch stayed blocked after rejection."
                    },
                    "finish_reason": "stop"
                }]
            }),
        ]);

        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = String::from(server.base_url());
        let mut tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Required, false);
        tool_loop.approval = ToolApprovalConfig {
            allow_write_tools: false,
            allow_network_shell: false,
            allow_destructive_shell: false,
            denied_action: ToolDeniedAction::Pause,
        };

        let error = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile: profile.clone(),
                prompt: String::from("patch hello.txt"),
                title: Some(String::from("Pause Then Reject")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: None,
                harness_profile: None,
                tool_loop: Some(tool_loop.clone()),
            })
            .expect_err("tool loop should pause");
        let RuntimeError::ToolApprovalPending { session_id, .. } = error else {
            panic!("expected tool approval pending error");
        };

        let outcome = runtime
            .resolve_pending_tool_approval(ResolvePendingToolApprovalRequest {
                session_id: session_id.clone(),
                profile,
                tool_loop,
                call_id: String::from("call_patch_1"),
                resolution: ToolApprovalResolution::Rejected,
            })
            .expect("rejection should succeed");

        let ResolvePendingToolApprovalOutcome::Resumed { outcome } = outcome else {
            panic!("expected resumed outcome");
        };
        assert_eq!(
            outcome.assistant_text,
            "The patch stayed blocked after rejection."
        );
        assert_eq!(
            std::fs::read_to_string(environment.workspace().join("hello.txt"))
                .expect("read unpatched file"),
            "hello world\n"
        );
        let transcript = runtime
            .session_store()
            .read_transcript(&session_id)
            .expect("read transcript");
        assert_eq!(
            transcript[2].turn.items[0]
                .tool_execution
                .as_ref()
                .expect("rejected tool execution should persist")
                .policy_decision,
            ToolPolicyDecision::Refused
        );
        assert!(
            runtime
                .pending_tool_approvals(&session_id)
                .expect("pending approvals should clear")
                .is_empty()
        );
    }

    #[test]
    fn eventful_tool_loop_emits_pause_event_before_returning_error() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let server = FakeOpenAiServer::from_json_responses(vec![json!({
            "id": "chatcmpl_pause_events",
            "model": "qwen3.5-2b-q8_0-registry.gguf",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_patch_1",
                        "type": "function",
                        "function": {
                            "name": "apply_patch",
                            "arguments": "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })]);
        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = server.base_url().to_string();
        let mut tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Required, false);
        tool_loop.approval = ToolApprovalConfig {
            allow_write_tools: false,
            allow_network_shell: false,
            allow_destructive_shell: false,
            denied_action: ToolDeniedAction::Pause,
        };
        let collector = Arc::new(TestRuntimeEventCollector::default());

        let error = runtime
            .exec_plain_text_with_events(
                PlainTextExecRequest {
                    profile,
                    prompt: String::from("patch hello.txt"),
                    title: Some(String::from("Eventful Pause")),
                    cwd: environment.workspace().to_path_buf(),
                    system_prompt: None,
                    harness_profile: None,
                    tool_loop: Some(tool_loop),
                },
                collector.clone(),
            )
            .expect_err("tool loop should pause");

        let RuntimeError::ToolApprovalPending { session_id, .. } = error else {
            panic!("expected tool approval pending error");
        };
        let events = collector.snapshot();
        assert_eq!(events.len(), 4);
        assert!(matches!(
            &events[0],
            RuntimeEvent::TurnStarted {
                session_id: event_session,
                profile_name,
                prompt,
                tool_loop_enabled: true,
            } if event_session == &session_id
                && profile_name == "psionic-qwen35-2b-q8-registry"
                && prompt == "patch hello.txt"
        ));
        assert!(matches!(
            &events[1],
            RuntimeEvent::ModelRequestStarted {
                session_id: event_session,
                round_trip: 1,
                backend_kind: probe_protocol::backend::BackendKind::OpenAiChatCompletions,
            } if event_session == &session_id
        ));
        assert!(matches!(
            &events[2],
            RuntimeEvent::ToolCallRequested {
                session_id: event_session,
                round_trip: 1,
                call_id,
                tool_name,
                arguments,
            } if event_session == &session_id
                && call_id == "call_patch_1"
                && tool_name == "apply_patch"
                && arguments == &json!({
                    "path":"hello.txt",
                    "old_text":"world",
                    "new_text":"probe"
                })
        ));
        match &events[3] {
            RuntimeEvent::ToolPaused {
                session_id: event_session,
                round_trip: 1,
                tool,
            } => {
                assert_eq!(event_session, &session_id);
                assert_eq!(tool.call_id, "call_patch_1");
                assert_eq!(tool.name, "apply_patch");
                assert_eq!(
                    tool.arguments,
                    json!({
                        "path":"hello.txt",
                        "old_text":"world",
                        "new_text":"probe"
                    })
                );
                assert_eq!(
                    tool.tool_execution.risk_class,
                    probe_protocol::session::ToolRiskClass::Write
                );
                assert_eq!(
                    tool.tool_execution.policy_decision,
                    ToolPolicyDecision::Paused
                );
                assert_eq!(
                    tool.tool_execution.approval_state,
                    ToolApprovalState::Pending
                );
                assert_eq!(
                    tool.tool_execution.reason.as_deref(),
                    Some("tool `apply_patch` requires write approval")
                );
                assert_eq!(
                    tool.output["error"],
                    "tool execution blocked by local approval policy"
                );
            }
            other => panic!("expected ToolPaused event, got {other:?}"),
        }
    }

    #[test]
    fn tool_loop_executes_parallel_tool_batches() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            for step in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept connection");
                let mut buffer = [0_u8; 8192];
                let bytes = stream.read(&mut buffer).expect("read request");
                let request_body = String::from_utf8_lossy(&buffer[..bytes]);
                if step == 0 {
                    assert!(request_body.contains("\"parallel_tool_calls\":true"));
                } else {
                    assert!(request_body.contains("\"call_list_root\""));
                    assert!(request_body.contains("\"call_readme_1\""));
                }
                let body = if step == 0 {
                    serde_json::json!({
                        "id": "chatcmpl_parallel_tools",
                        "model": "qwen3.5-2b-q8_0-registry.gguf",
                        "choices": [
                            {
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "tool_calls": [
                                        {
                                            "id": "call_list_root",
                                            "type": "function",
                                            "function": {
                                                "name": "list_files",
                                                "arguments": "{\"path\":\".\",\"max_depth\":2,\"max_entries\":10}"
                                            }
                                        },
                                        {
                                            "id": "call_readme_1",
                                            "type": "function",
                                            "function": {
                                                "name": "read_file",
                                                "arguments": "{\"path\":\"README.md\",\"start_line\":1,\"max_lines\":8}"
                                            }
                                        }
                                    ]
                                },
                                "finish_reason": "tool_calls"
                            }
                        ]
                    })
                } else {
                    serde_json::json!({
                        "id": "chatcmpl_parallel_final",
                        "model": "qwen3.5-2b-q8_0-registry.gguf",
                        "choices": [
                            {
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "content": "Root files listed and README inspected."
                                },
                                "finish_reason": "stop"
                            }
                        ]
                    })
                }
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });

        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir_all(temp.path().join("src")).expect("create src");
        std::fs::write(
            temp.path().join("README.md"),
            "# Probe\n\nA coding-agent runtime.\n",
        )
        .expect("write readme");
        std::fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write main");
        let runtime = ProbeRuntime::new(temp.path().join(".probe"));
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = format!("http://{address}/v1");

        let outcome = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("List the repo and inspect README.md"),
                title: Some(String::from("Parallel Tool Test")),
                cwd: temp.path().to_path_buf(),
                system_prompt: None,
                harness_profile: None,
                tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                    ProbeToolChoice::Required,
                    true,
                )),
            })
            .expect("parallel tool loop should succeed");

        assert_eq!(outcome.executed_tool_calls, 2);
        assert_eq!(
            outcome.assistant_text,
            "Root files listed and README inspected."
        );

        let transcript = runtime
            .session_store()
            .read_transcript(&outcome.session.id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 3);
        assert_eq!(transcript[0].turn.items.len(), 3);
        assert_eq!(transcript[1].turn.items.len(), 2);
        assert!(transcript[0].turn.observability.is_some());
        assert!(transcript[1].turn.observability.is_none());
        assert!(transcript[2].turn.observability.is_some());

        handle.join().expect("server thread should exit cleanly");
    }
}
