use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::io::{self, BufRead, BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use probe_core::runtime::{
    PlainTextResumeRequest, ProbeRuntime, ResolvePendingToolApprovalOutcome,
    ResolvePendingToolApprovalRequest, RuntimeError, RuntimeEvent, RuntimeEventSink,
    default_probe_home,
};
use probe_core::session_store::{NewSession, SessionStoreError};
use probe_core::tools::{
    ExecutedToolCall, ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction as CoreDeniedAction,
    ToolLongContextConfig, ToolLoopConfig, ToolOracleConfig,
};
use probe_protocol::runtime::{
    ClientMessage, EventDeliveryGuarantee, EventEnvelope, InitializeResponse,
    InterruptTurnResponse, ListPendingApprovalsRequest, ListPendingApprovalsResponse,
    ListSessionsResponse, RequestEnvelope, ResolvePendingApprovalResponse, ResponseBody,
    ResponseEnvelope, RuntimeCapabilities, RuntimeProgressEvent, RuntimeProtocolError,
    RuntimeRequest, RuntimeResponse, RuntimeToolCallDelta, RuntimeUsage, ServerEvent,
    ServerMessage, SessionLookupRequest, SessionSnapshot, ShutdownResponse, StartSessionRequest,
    ToolApprovalRecipe, ToolCallResult, ToolChoice, ToolDeniedAction, ToolLongContextRecipe,
    ToolLoopRecipe, ToolOracleRecipe, ToolSetKind, TransportKind, TurnCompleted, TurnPaused,
    TurnRequest, TurnResponse,
};
use probe_protocol::session::{SessionBackendTarget, SessionId, UsageMeasurement, UsageTruth};
use probe_protocol::{PROBE_PROTOCOL_VERSION, PROBE_RUNTIME_NAME};

#[derive(Debug)]
pub enum ServerError {
    Io(io::Error),
    Json(serde_json::Error),
}

impl Display for ServerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
        }
    }
}

impl std::error::Error for ServerError {}

impl From<io::Error> for ServerError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ServerError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Clone)]
struct SharedJsonlWriter {
    stdout: Arc<Mutex<BufWriter<io::Stdout>>>,
}

impl SharedJsonlWriter {
    fn new() -> Self {
        Self {
            stdout: Arc::new(Mutex::new(BufWriter::new(io::stdout()))),
        }
    }

    fn send_response_ok(
        &self,
        request_id: &str,
        response: RuntimeResponse,
    ) -> Result<(), ServerError> {
        self.send(ServerMessage::Response(ResponseEnvelope {
            request_id: String::from(request_id),
            body: ResponseBody::Ok { response },
        }))
    }

    fn send_response_error(
        &self,
        request_id: &str,
        error: RuntimeProtocolError,
    ) -> Result<(), ServerError> {
        self.send(ServerMessage::Response(ResponseEnvelope {
            request_id: String::from(request_id),
            body: ResponseBody::Error { error },
        }))
    }

    fn send_event(&self, request_id: &str, event: ServerEvent) -> Result<(), ServerError> {
        self.send(ServerMessage::Event(EventEnvelope {
            request_id: String::from(request_id),
            event,
        }))
    }

    fn send(&self, message: ServerMessage) -> Result<(), ServerError> {
        let mut stdout = self
            .stdout
            .lock()
            .expect("probe-server stdout mutex should not be poisoned");
        serde_json::to_writer(&mut *stdout, &message)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
        Ok(())
    }
}

pub fn run_stdio_server(probe_home: Option<PathBuf>) -> Result<(), ServerError> {
    let home = match probe_home {
        Some(home) => home,
        None => default_probe_home().map_err(|error| {
            io::Error::other(format!(
                "failed to resolve probe home for probe-server: {error}"
            ))
        })?,
    };
    let server = ProbeStdioServer::new(ProbeRuntime::new(home));
    let stdin = io::stdin();
    server.run(stdin.lock())
}

struct ProbeStdioServer {
    runtime: ProbeRuntime,
    writer: SharedJsonlWriter,
    active_turns: Arc<Mutex<HashSet<String>>>,
}

impl ProbeStdioServer {
    fn new(runtime: ProbeRuntime) -> Self {
        Self {
            runtime,
            writer: SharedJsonlWriter::new(),
            active_turns: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    fn run(&self, reader: impl BufRead) -> Result<(), ServerError> {
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let message = match serde_json::from_str::<ClientMessage>(line.as_str()) {
                Ok(message) => message,
                Err(error) => {
                    eprintln!("probe-server discarded invalid request line: {error}");
                    continue;
                }
            };

            let ClientMessage::Request(envelope) = message;
            if !self.handle_request(envelope)? {
                break;
            }
        }
        Ok(())
    }

    fn handle_request(&self, envelope: RequestEnvelope) -> Result<bool, ServerError> {
        let request_id = envelope.request_id.clone();
        match envelope.request {
            RuntimeRequest::Initialize(request) => {
                if request.protocol_version != PROBE_PROTOCOL_VERSION {
                    self.writer.send_response_error(
                        request_id.as_str(),
                        protocol_error(
                            "protocol_version_mismatch",
                            format!(
                                "client requested protocol version {}, but probe-server speaks {}",
                                request.protocol_version, PROBE_PROTOCOL_VERSION
                            ),
                        ),
                    )?;
                } else {
                    self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::Initialize(InitializeResponse {
                            runtime_name: String::from(PROBE_RUNTIME_NAME),
                            protocol_version: PROBE_PROTOCOL_VERSION,
                            capabilities: RuntimeCapabilities {
                                transport: TransportKind::StdioJsonl,
                                supports_stdio_child_process: true,
                                supports_session_resume: true,
                                supports_session_inspect: true,
                                supports_pending_approval_resolution: true,
                                supports_interrupt_requests: true,
                                supports_queued_turns: false,
                            },
                        }),
                    )?;
                }
                Ok(true)
            }
            RuntimeRequest::StartSession(request) => {
                match self.start_session(request) {
                    Ok(snapshot) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::StartSession(snapshot),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(true)
            }
            RuntimeRequest::ResumeSession(SessionLookupRequest { session_id }) => {
                match self.session_snapshot(&session_id) {
                    Ok(snapshot) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::ResumeSession(snapshot),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(true)
            }
            RuntimeRequest::ListSessions => {
                match self.runtime.session_store().list_sessions() {
                    Ok(sessions) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::ListSessions(ListSessionsResponse { sessions }),
                    )?,
                    Err(error) => self.writer.send_response_error(
                        request_id.as_str(),
                        session_store_error_to_protocol(error),
                    )?,
                }
                Ok(true)
            }
            RuntimeRequest::InspectSession(SessionLookupRequest { session_id }) => {
                match self.session_snapshot(&session_id) {
                    Ok(snapshot) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::InspectSession(snapshot),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(true)
            }
            RuntimeRequest::StartTurn(request) => {
                self.spawn_turn_request(request_id, request, TurnMode::Start)?;
                Ok(true)
            }
            RuntimeRequest::ContinueTurn(request) => {
                self.spawn_turn_request(request_id, request, TurnMode::Continue)?;
                Ok(true)
            }
            RuntimeRequest::InterruptTurn(request) => {
                let response = self.interrupt_turn(request.session_id);
                self.writer.send_response_ok(
                    request_id.as_str(),
                    RuntimeResponse::InterruptTurn(response),
                )?;
                Ok(true)
            }
            RuntimeRequest::ListPendingApprovals(request) => {
                match self.list_pending_approvals(request) {
                    Ok(approvals) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::ListPendingApprovals(ListPendingApprovalsResponse {
                            approvals,
                        }),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(true)
            }
            RuntimeRequest::ResolvePendingApproval(request) => {
                self.spawn_resolve_pending_approval(request_id, request)?;
                Ok(true)
            }
            RuntimeRequest::Shutdown => {
                let active_turns = self.active_turn_count();
                let accepted = active_turns == 0;
                self.writer.send_response_ok(
                    request_id.as_str(),
                    RuntimeResponse::Shutdown(ShutdownResponse {
                        accepted,
                        active_turns,
                    }),
                )?;
                Ok(!accepted)
            }
        }
    }

    fn start_session(
        &self,
        request: StartSessionRequest,
    ) -> Result<SessionSnapshot, RuntimeProtocolError> {
        let session = self
            .runtime
            .session_store()
            .create_session_with(
                NewSession::new(normalize_session_title(request.title), request.cwd)
                    .with_system_prompt(request.system_prompt)
                    .with_harness_profile(request.harness_profile)
                    .with_backend(SessionBackendTarget {
                        profile_name: request.profile.name,
                        base_url: request.profile.base_url,
                        model: request.profile.model,
                    }),
            )
            .map_err(session_store_error_to_protocol)?;
        self.session_snapshot(&session.id)
    }

    fn session_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionSnapshot, RuntimeProtocolError> {
        let session = self
            .runtime
            .session_store()
            .read_metadata(session_id)
            .map_err(session_store_error_to_protocol)?;
        let transcript = self
            .runtime
            .session_store()
            .read_transcript(session_id)
            .map_err(session_store_error_to_protocol)?;
        let pending_approvals = self
            .runtime
            .pending_tool_approvals(session_id)
            .map_err(runtime_error_to_protocol)?;
        Ok(SessionSnapshot {
            session,
            transcript,
            pending_approvals,
        })
    }

    fn list_pending_approvals(
        &self,
        request: ListPendingApprovalsRequest,
    ) -> Result<Vec<probe_protocol::session::PendingToolApproval>, RuntimeProtocolError> {
        if let Some(session_id) = request.session_id {
            return self
                .runtime
                .pending_tool_approvals(&session_id)
                .map_err(runtime_error_to_protocol);
        }

        let mut approvals = Vec::new();
        let sessions = self
            .runtime
            .session_store()
            .list_sessions()
            .map_err(session_store_error_to_protocol)?;
        for session in sessions {
            approvals.extend(
                self.runtime
                    .pending_tool_approvals(&session.id)
                    .map_err(runtime_error_to_protocol)?,
            );
        }
        approvals.sort_by(|left, right| right.requested_at_ms.cmp(&left.requested_at_ms));
        Ok(approvals)
    }

    fn interrupt_turn(&self, session_id: SessionId) -> InterruptTurnResponse {
        let active = self
            .active_turns
            .lock()
            .expect("probe-server active turn mutex should not be poisoned")
            .contains(session_id.as_str());
        if active {
            InterruptTurnResponse {
                session_id,
                interrupted: false,
                reason_code: Some(String::from("unsupported")),
                message: String::from(
                    "probe-server knows the session is busy, but the current Probe runtime has no cooperative interrupt path yet",
                ),
            }
        } else {
            InterruptTurnResponse {
                session_id,
                interrupted: false,
                reason_code: Some(String::from("not_running")),
                message: String::from("session is not currently running a turn"),
            }
        }
    }

    fn spawn_turn_request(
        &self,
        request_id: String,
        request: TurnRequest,
        mode: TurnMode,
    ) -> Result<(), ServerError> {
        let session_key = String::from(request.session_id.as_str());
        if !self.mark_turn_active(session_key.as_str()) {
            self.writer.send_response_error(
                request_id.as_str(),
                protocol_error(
                    "session_busy",
                    format!(
                        "session {} already has an active turn; queued follow-ups are not available on this server cut yet",
                        request.session_id.as_str()
                    ),
                ),
            )?;
            return Ok(());
        }

        let runtime = self.runtime.clone();
        let writer = self.writer.clone();
        let active_turns = Arc::clone(&self.active_turns);
        thread::Builder::new()
            .name(format!("probe-server-turn-{}", request.session_id.as_str()))
            .spawn(move || {
                let response =
                    run_turn_request(&runtime, &writer, request_id.as_str(), request, mode);
                active_turns
                    .lock()
                    .expect("probe-server active turn mutex should not be poisoned")
                    .remove(session_key.as_str());
                match response {
                    Ok(response) => {
                        let _ = writer.send_response_ok(request_id.as_str(), response);
                    }
                    Err(error) => {
                        let _ = writer.send_response_error(request_id.as_str(), error);
                    }
                }
            })?;
        Ok(())
    }

    fn spawn_resolve_pending_approval(
        &self,
        request_id: String,
        request: probe_protocol::runtime::ResolvePendingApprovalRequest,
    ) -> Result<(), ServerError> {
        let session_key = String::from(request.session_id.as_str());
        if !self.mark_turn_active(session_key.as_str()) {
            self.writer.send_response_error(
                request_id.as_str(),
                protocol_error(
                    "session_busy",
                    format!(
                        "session {} already has an active turn; queued approval continuations are not available yet",
                        request.session_id.as_str()
                    ),
                ),
            )?;
            return Ok(());
        }

        let runtime = self.runtime.clone();
        let writer = self.writer.clone();
        let active_turns = Arc::clone(&self.active_turns);
        thread::Builder::new()
            .name(format!(
                "probe-server-approval-{}",
                request.session_id.as_str()
            ))
            .spawn(move || {
                let response = run_pending_approval_resolution(
                    &runtime,
                    &writer,
                    request_id.as_str(),
                    request,
                );
                active_turns
                    .lock()
                    .expect("probe-server active turn mutex should not be poisoned")
                    .remove(session_key.as_str());
                match response {
                    Ok(response) => {
                        let _ = writer.send_response_ok(request_id.as_str(), response);
                    }
                    Err(error) => {
                        let _ = writer.send_response_error(request_id.as_str(), error);
                    }
                }
            })?;
        Ok(())
    }

    fn mark_turn_active(&self, session_id: &str) -> bool {
        self.active_turns
            .lock()
            .expect("probe-server active turn mutex should not be poisoned")
            .insert(String::from(session_id))
    }

    fn active_turn_count(&self) -> usize {
        self.active_turns
            .lock()
            .expect("probe-server active turn mutex should not be poisoned")
            .len()
    }
}

#[derive(Clone, Copy)]
enum TurnMode {
    Start,
    Continue,
}

fn run_turn_request(
    runtime: &ProbeRuntime,
    writer: &SharedJsonlWriter,
    request_id: &str,
    request: TurnRequest,
    mode: TurnMode,
) -> Result<RuntimeResponse, RuntimeProtocolError> {
    let tool_loop = request.tool_loop.map(tool_loop_from_recipe).transpose()?;
    let writer_for_events = writer.clone();
    let request_id_for_events = String::from(request_id);
    let event_sink: Arc<dyn RuntimeEventSink> = Arc::new(move |event| {
        let delivery = delivery_for_runtime_event(&event);
        let encoded = encode_runtime_event(event);
        let _ = writer_for_events.send_event(
            request_id_for_events.as_str(),
            ServerEvent::RuntimeProgress {
                delivery,
                event: encoded,
            },
        );
    });

    let result = runtime.continue_plain_text_session_with_events(
        PlainTextResumeRequest {
            session_id: request.session_id.clone(),
            profile: request.profile,
            prompt: request.prompt,
            tool_loop,
        },
        event_sink,
    );

    let response = match result {
        Ok(outcome) => turn_response_to_runtime_response(
            TurnResponse::Completed(turn_completed(outcome)),
            mode,
        ),
        Err(RuntimeError::ToolApprovalPending {
            session_id,
            tool_name,
            call_id,
            reason,
        }) => {
            let pending_approvals = runtime
                .pending_tool_approvals(&session_id)
                .map_err(runtime_error_to_protocol)?;
            writer
                .send_event(
                    request_id,
                    ServerEvent::PendingApprovalsUpdated {
                        delivery: EventDeliveryGuarantee::Lossless,
                        session_id: session_id.clone(),
                        approvals: pending_approvals.clone(),
                    },
                )
                .map_err(|error| protocol_error("event_write_failed", error.to_string()))?;
            let session = runtime
                .session_store()
                .read_metadata(&session_id)
                .map_err(session_store_error_to_protocol)?;
            turn_response_to_runtime_response(
                TurnResponse::Paused(TurnPaused {
                    session,
                    call_id,
                    tool_name,
                    reason,
                    pending_approvals,
                }),
                mode,
            )
        }
        Err(error) => return Err(runtime_error_to_protocol(error)),
    };
    Ok(response)
}

fn run_pending_approval_resolution(
    runtime: &ProbeRuntime,
    writer: &SharedJsonlWriter,
    request_id: &str,
    request: probe_protocol::runtime::ResolvePendingApprovalRequest,
) -> Result<RuntimeResponse, RuntimeProtocolError> {
    let tool_loop = tool_loop_from_recipe(request.tool_loop)?;
    let writer_for_events = writer.clone();
    let request_id_for_events = String::from(request_id);
    let event_sink: Arc<dyn RuntimeEventSink> = Arc::new(move |event| {
        let delivery = delivery_for_runtime_event(&event);
        let encoded = encode_runtime_event(event);
        let _ = writer_for_events.send_event(
            request_id_for_events.as_str(),
            ServerEvent::RuntimeProgress {
                delivery,
                event: encoded,
            },
        );
    });

    let result = runtime.resolve_pending_tool_approval_with_events(
        ResolvePendingToolApprovalRequest {
            session_id: request.session_id.clone(),
            profile: request.profile,
            tool_loop,
            call_id: request.call_id,
            resolution: request.resolution,
        },
        event_sink,
    );

    match result.map_err(runtime_error_to_protocol)? {
        ResolvePendingToolApprovalOutcome::StillPending {
            session,
            pending_approvals,
        } => {
            writer
                .send_event(
                    request_id,
                    ServerEvent::PendingApprovalsUpdated {
                        delivery: EventDeliveryGuarantee::Lossless,
                        session_id: session.id.clone(),
                        approvals: pending_approvals.clone(),
                    },
                )
                .map_err(|error| protocol_error("event_write_failed", error.to_string()))?;
            Ok(RuntimeResponse::ResolvePendingApproval(
                ResolvePendingApprovalResponse::StillPending {
                    session,
                    pending_approvals,
                },
            ))
        }
        ResolvePendingToolApprovalOutcome::Resumed { outcome } => {
            Ok(RuntimeResponse::ResolvePendingApproval(
                ResolvePendingApprovalResponse::Resumed(turn_completed(outcome)),
            ))
        }
    }
}

fn turn_response_to_runtime_response(response: TurnResponse, mode: TurnMode) -> RuntimeResponse {
    match mode {
        TurnMode::Start => RuntimeResponse::StartTurn(response),
        TurnMode::Continue => RuntimeResponse::ContinueTurn(response),
    }
}

fn turn_completed(outcome: probe_core::runtime::PlainTextExecOutcome) -> TurnCompleted {
    TurnCompleted {
        session: outcome.session,
        turn: outcome.turn,
        assistant_text: outcome.assistant_text,
        response_id: outcome.response_id,
        response_model: outcome.response_model,
        usage: outcome.usage.map(runtime_usage),
        executed_tool_calls: outcome.executed_tool_calls,
        tool_results: outcome
            .tool_results
            .into_iter()
            .map(tool_call_result)
            .collect(),
    }
}

fn runtime_usage(usage: probe_core::provider::ProviderUsage) -> RuntimeUsage {
    RuntimeUsage {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
        prompt_tokens_detail: usage.prompt_tokens_detail.map(provider_usage_measurement),
        completion_tokens_detail: usage
            .completion_tokens_detail
            .map(provider_usage_measurement),
        total_tokens_detail: usage.total_tokens_detail.map(provider_usage_measurement),
    }
}

fn provider_usage_measurement(
    measurement: probe_core::provider::ProviderUsageMeasurement,
) -> UsageMeasurement {
    UsageMeasurement {
        value: measurement.value,
        truth: match measurement.truth {
            probe_core::provider::ProviderUsageTruth::Exact => UsageTruth::Exact,
            probe_core::provider::ProviderUsageTruth::Estimated => UsageTruth::Estimated,
        },
    }
}

fn tool_call_result(tool: ExecutedToolCall) -> ToolCallResult {
    ToolCallResult {
        call_id: tool.call_id,
        name: tool.name,
        arguments: tool.arguments,
        output: tool.output,
        tool_execution: tool.tool_execution,
    }
}

fn delivery_for_runtime_event(event: &RuntimeEvent) -> EventDeliveryGuarantee {
    match event {
        RuntimeEvent::AssistantDelta { .. }
        | RuntimeEvent::AssistantSnapshot { .. }
        | RuntimeEvent::ToolCallDelta { .. } => EventDeliveryGuarantee::BestEffort,
        RuntimeEvent::TurnStarted { .. }
        | RuntimeEvent::ModelRequestStarted { .. }
        | RuntimeEvent::AssistantStreamStarted { .. }
        | RuntimeEvent::TimeToFirstTokenObserved { .. }
        | RuntimeEvent::ToolCallRequested { .. }
        | RuntimeEvent::ToolExecutionStarted { .. }
        | RuntimeEvent::ToolExecutionCompleted { .. }
        | RuntimeEvent::ToolRefused { .. }
        | RuntimeEvent::ToolPaused { .. }
        | RuntimeEvent::AssistantStreamFinished { .. }
        | RuntimeEvent::ModelRequestFailed { .. }
        | RuntimeEvent::AssistantTurnCommitted { .. } => EventDeliveryGuarantee::Lossless,
    }
}

fn encode_runtime_event(event: RuntimeEvent) -> RuntimeProgressEvent {
    match event {
        RuntimeEvent::TurnStarted {
            session_id,
            profile_name,
            prompt,
            tool_loop_enabled,
        } => RuntimeProgressEvent::TurnStarted {
            session_id,
            profile_name,
            prompt,
            tool_loop_enabled,
        },
        RuntimeEvent::ModelRequestStarted {
            session_id,
            round_trip,
            backend_kind,
        } => RuntimeProgressEvent::ModelRequestStarted {
            session_id,
            round_trip,
            backend_kind,
        },
        RuntimeEvent::AssistantStreamStarted {
            session_id,
            round_trip,
            response_id,
            response_model,
        } => RuntimeProgressEvent::AssistantStreamStarted {
            session_id,
            round_trip,
            response_id,
            response_model,
        },
        RuntimeEvent::TimeToFirstTokenObserved {
            session_id,
            round_trip,
            milliseconds,
        } => RuntimeProgressEvent::TimeToFirstTokenObserved {
            session_id,
            round_trip,
            milliseconds,
        },
        RuntimeEvent::AssistantDelta {
            session_id,
            round_trip,
            delta,
        } => RuntimeProgressEvent::AssistantDelta {
            session_id,
            round_trip,
            delta,
        },
        RuntimeEvent::AssistantSnapshot {
            session_id,
            round_trip,
            snapshot,
        } => RuntimeProgressEvent::AssistantSnapshot {
            session_id,
            round_trip,
            snapshot,
        },
        RuntimeEvent::ToolCallDelta {
            session_id,
            round_trip,
            deltas,
        } => RuntimeProgressEvent::ToolCallDelta {
            session_id,
            round_trip,
            deltas: deltas
                .into_iter()
                .map(|delta| RuntimeToolCallDelta {
                    tool_index: delta.tool_index,
                    call_id: delta.call_id,
                    tool_name: delta.tool_name,
                    arguments_delta: delta.arguments_delta,
                })
                .collect(),
        },
        RuntimeEvent::ToolCallRequested {
            session_id,
            round_trip,
            call_id,
            tool_name,
            arguments,
        } => RuntimeProgressEvent::ToolCallRequested {
            session_id,
            round_trip,
            call_id,
            tool_name,
            arguments,
        },
        RuntimeEvent::ToolExecutionStarted {
            session_id,
            round_trip,
            call_id,
            tool_name,
            risk_class,
        } => RuntimeProgressEvent::ToolExecutionStarted {
            session_id,
            round_trip,
            call_id,
            tool_name,
            risk_class,
        },
        RuntimeEvent::ToolExecutionCompleted {
            session_id,
            round_trip,
            tool,
        } => RuntimeProgressEvent::ToolExecutionCompleted {
            session_id,
            round_trip,
            tool: tool_call_result(tool),
        },
        RuntimeEvent::ToolRefused {
            session_id,
            round_trip,
            tool,
        } => RuntimeProgressEvent::ToolRefused {
            session_id,
            round_trip,
            tool: tool_call_result(tool),
        },
        RuntimeEvent::ToolPaused {
            session_id,
            round_trip,
            tool,
        } => RuntimeProgressEvent::ToolPaused {
            session_id,
            round_trip,
            tool: tool_call_result(tool),
        },
        RuntimeEvent::AssistantStreamFinished {
            session_id,
            round_trip,
            response_id,
            response_model,
            finish_reason,
        } => RuntimeProgressEvent::AssistantStreamFinished {
            session_id,
            round_trip,
            response_id,
            response_model,
            finish_reason,
        },
        RuntimeEvent::ModelRequestFailed {
            session_id,
            round_trip,
            backend_kind,
            error,
        } => RuntimeProgressEvent::ModelRequestFailed {
            session_id,
            round_trip,
            backend_kind,
            error,
        },
        RuntimeEvent::AssistantTurnCommitted {
            session_id,
            response_id,
            response_model,
            assistant_text,
        } => RuntimeProgressEvent::AssistantTurnCommitted {
            session_id,
            response_id,
            response_model,
            assistant_text,
        },
    }
}

fn normalize_session_title(title: Option<String>) -> String {
    let Some(title) = title else {
        return String::from("Probe Session");
    };
    let trimmed = title.trim();
    if trimmed.is_empty() {
        String::from("Probe Session")
    } else {
        String::from(trimmed)
    }
}

fn tool_loop_from_recipe(recipe: ToolLoopRecipe) -> Result<ToolLoopConfig, RuntimeProtocolError> {
    if recipe.max_model_round_trips == 0 {
        return Err(protocol_error(
            "invalid_tool_loop",
            "max_model_round_trips must be at least 1",
        ));
    }

    let tool_choice = tool_choice_from_recipe(recipe.tool_choice)?;
    let mut config = match recipe.tool_set {
        ToolSetKind::CodingBootstrap => {
            ToolLoopConfig::coding_bootstrap(tool_choice, recipe.parallel_tool_calls)
        }
    };
    config.max_model_round_trips = recipe.max_model_round_trips;
    config.approval = approval_from_recipe(recipe.approval);
    if let Some(oracle) = recipe.oracle {
        config = config.with_oracle(oracle_from_recipe(oracle)?);
    }
    if let Some(long_context) = recipe.long_context {
        config = config.with_long_context(long_context_from_recipe(long_context)?);
    }
    Ok(config)
}

fn tool_choice_from_recipe(choice: ToolChoice) -> Result<ProbeToolChoice, RuntimeProtocolError> {
    Ok(match choice {
        ToolChoice::None => ProbeToolChoice::None,
        ToolChoice::Auto => ProbeToolChoice::Auto,
        ToolChoice::Required => ProbeToolChoice::Required,
        ToolChoice::Named { tool_name } => {
            if tool_name.trim().is_empty() {
                return Err(protocol_error(
                    "invalid_tool_choice",
                    "named tool choice requires a non-empty tool_name",
                ));
            }
            ProbeToolChoice::Named(tool_name)
        }
    })
}

fn approval_from_recipe(recipe: ToolApprovalRecipe) -> ToolApprovalConfig {
    ToolApprovalConfig {
        allow_write_tools: recipe.allow_write_tools,
        allow_network_shell: recipe.allow_network_shell,
        allow_destructive_shell: recipe.allow_destructive_shell,
        denied_action: match recipe.denied_action {
            ToolDeniedAction::Refuse => CoreDeniedAction::Refuse,
            ToolDeniedAction::Pause => CoreDeniedAction::Pause,
        },
    }
}

fn oracle_from_recipe(recipe: ToolOracleRecipe) -> Result<ToolOracleConfig, RuntimeProtocolError> {
    if recipe.max_calls == 0 {
        return Err(protocol_error(
            "invalid_oracle_config",
            "oracle max_calls must be at least 1",
        ));
    }
    Ok(ToolOracleConfig {
        profile: recipe.profile,
        max_calls: recipe.max_calls,
    })
}

fn long_context_from_recipe(
    recipe: ToolLongContextRecipe,
) -> Result<ToolLongContextConfig, RuntimeProtocolError> {
    if recipe.max_calls == 0 {
        return Err(protocol_error(
            "invalid_long_context_config",
            "long-context max_calls must be at least 1",
        ));
    }
    if recipe.max_evidence_files == 0 {
        return Err(protocol_error(
            "invalid_long_context_config",
            "long-context max_evidence_files must be at least 1",
        ));
    }
    if recipe.max_lines_per_file == 0 {
        return Err(protocol_error(
            "invalid_long_context_config",
            "long-context max_lines_per_file must be at least 1",
        ));
    }
    let mut config = ToolLongContextConfig::bounded(recipe.profile, recipe.max_calls);
    config.max_evidence_files = recipe.max_evidence_files;
    config.max_lines_per_file = recipe.max_lines_per_file;
    Ok(config)
}

fn protocol_error(code: impl Into<String>, message: impl Into<String>) -> RuntimeProtocolError {
    RuntimeProtocolError {
        code: code.into(),
        message: message.into(),
    }
}

fn session_store_error_to_protocol(error: SessionStoreError) -> RuntimeProtocolError {
    match error {
        SessionStoreError::NotFound(_) => protocol_error("session_not_found", error.to_string()),
        SessionStoreError::Conflict(_) => protocol_error("session_conflict", error.to_string()),
        SessionStoreError::Io(_) | SessionStoreError::Json(_) => {
            protocol_error("session_store_error", error.to_string())
        }
    }
}

fn runtime_error_to_protocol(error: RuntimeError) -> RuntimeProtocolError {
    match error {
        RuntimeError::SessionStore(source) => session_store_error_to_protocol(source),
        RuntimeError::ProviderRequest { .. } => {
            protocol_error("backend_request_failed", error.to_string())
        }
        RuntimeError::MissingAssistantMessage { .. } => {
            protocol_error("missing_assistant_message", error.to_string())
        }
        RuntimeError::UnsupportedBackendFeature { .. } => {
            protocol_error("unsupported_backend_feature", error.to_string())
        }
        RuntimeError::ToolApprovalPending { .. } => {
            protocol_error("tool_approval_pending", error.to_string())
        }
        RuntimeError::PendingToolApprovalNotFound { .. } => {
            protocol_error("approval_not_found", error.to_string())
        }
        RuntimeError::PendingToolApprovalAlreadyResolved { .. } => {
            protocol_error("approval_already_resolved", error.to_string())
        }
        RuntimeError::MaxToolRoundTrips { .. } => {
            protocol_error("max_tool_round_trips", error.to_string())
        }
        RuntimeError::ProbeHomeUnavailable => {
            protocol_error("probe_home_unavailable", error.to_string())
        }
        RuntimeError::CurrentDir(_) => protocol_error("cwd_unavailable", error.to_string()),
        RuntimeError::MalformedTranscript(_) => {
            protocol_error("malformed_transcript", error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use probe_protocol::backend::{BackendKind, PrefixCacheMode, ServerAttachMode};
    use probe_protocol::runtime::{ToolApprovalRecipe, ToolChoice, ToolDeniedAction};

    use super::{
        ProbeStdioServer, ProbeToolChoice, ToolLoopRecipe, ToolSetKind, approval_from_recipe,
        normalize_session_title, tool_choice_from_recipe, tool_loop_from_recipe,
    };

    #[test]
    fn session_titles_fall_back_when_blank() {
        assert_eq!(normalize_session_title(None), "Probe Session");
        assert_eq!(
            normalize_session_title(Some(String::from("   "))),
            "Probe Session"
        );
        assert_eq!(
            normalize_session_title(Some(String::from(" runtime test "))),
            "runtime test"
        );
    }

    #[test]
    fn tool_choice_recipe_maps_named_choice() {
        let choice = tool_choice_from_recipe(ToolChoice::Named {
            tool_name: String::from("read_file"),
        })
        .expect("named choice should map");
        assert!(matches!(choice, ProbeToolChoice::Named(name) if name == "read_file"));
    }

    #[test]
    fn tool_loop_recipe_builds_coding_bootstrap() {
        let config = tool_loop_from_recipe(ToolLoopRecipe {
            tool_set: ToolSetKind::CodingBootstrap,
            tool_choice: ToolChoice::Auto,
            parallel_tool_calls: false,
            max_model_round_trips: 4,
            approval: ToolApprovalRecipe {
                allow_write_tools: false,
                allow_network_shell: false,
                allow_destructive_shell: false,
                denied_action: ToolDeniedAction::Pause,
            },
            oracle: None,
            long_context: None,
        })
        .expect("tool loop recipe should convert");
        assert_eq!(config.max_model_round_trips, 4);
        assert!(matches!(config.tool_choice, ProbeToolChoice::Auto));
    }

    #[test]
    fn approval_recipe_maps_pause_policy() {
        let approval = approval_from_recipe(ToolApprovalRecipe {
            allow_write_tools: true,
            allow_network_shell: false,
            allow_destructive_shell: false,
            denied_action: ToolDeniedAction::Pause,
        });
        assert!(approval.allow_write_tools);
        assert!(matches!(
            approval.denied_action,
            probe_core::tools::ToolDeniedAction::Pause
        ));
    }

    #[test]
    fn server_constructs_with_detected_runtime() {
        let runtime = probe_core::runtime::ProbeRuntime::new("/tmp/probe-server-test");
        let _server = ProbeStdioServer::new(runtime);
    }

    #[test]
    fn named_choice_rejects_empty_tool_name() {
        let error = tool_choice_from_recipe(ToolChoice::Named {
            tool_name: String::new(),
        })
        .expect_err("empty tool name should fail");
        assert_eq!(error.code, "invalid_tool_choice");
    }

    #[test]
    fn backend_profile_can_seed_loop_recipes() {
        let profile = probe_protocol::backend::BackendProfile {
            name: String::from("test"),
            kind: BackendKind::OpenAiChatCompletions,
            base_url: String::from("http://127.0.0.1:11434/v1"),
            model: String::from("tiny"),
            api_key_env: String::from("PROBE_OPENAI_API_KEY"),
            timeout_secs: 30,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
        };
        let loop_recipe = ToolLoopRecipe {
            tool_set: ToolSetKind::CodingBootstrap,
            tool_choice: ToolChoice::Auto,
            parallel_tool_calls: false,
            max_model_round_trips: 8,
            approval: ToolApprovalRecipe {
                allow_write_tools: false,
                allow_network_shell: false,
                allow_destructive_shell: false,
                denied_action: ToolDeniedAction::Refuse,
            },
            oracle: Some(super::ToolOracleRecipe {
                profile,
                max_calls: 1,
            }),
            long_context: None,
        };
        let config = tool_loop_from_recipe(loop_recipe).expect("oracle loop recipe should map");
        assert!(config.oracle.is_some());
    }
}
