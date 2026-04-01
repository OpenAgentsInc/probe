use std::env;
use std::fmt::{Display, Formatter};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;

use probe_core::provider::{ProviderUsage, ProviderUsageMeasurement, ProviderUsageTruth};
use probe_core::runtime::{
    PlainTextExecOutcome, PlainTextExecRequest, PlainTextResumeRequest,
    ResolvePendingToolApprovalOutcome, ResolvePendingToolApprovalRequest, RuntimeEvent,
    RuntimeEventSink, StreamedToolCallDelta,
};
use probe_core::tools::{ExecutedToolCall, ProbeToolChoice, ToolDeniedAction, ToolLoopConfig};
use probe_protocol::runtime::{
    CancelQueuedTurnRequest, CancelQueuedTurnResponse, ClientMessage, EventEnvelope,
    InitializeRequest, InspectSessionTurnsResponse, InterruptTurnResponse, ListSessionsResponse,
    QueueTurnResponse, RequestEnvelope, ResolvePendingApprovalResponse, ResponseBody,
    ResponseEnvelope, RuntimeProgressEvent, RuntimeProtocolError, RuntimeRequest, RuntimeResponse,
    RuntimeToolCallDelta, RuntimeUsage, ServerEvent, ServerMessage, SessionLookupRequest,
    SessionSnapshot, StartSessionRequest, ToolApprovalRecipe, ToolCallResult, ToolChoice,
    ToolDeniedAction as ProtocolDeniedAction, ToolLongContextRecipe, ToolLoopRecipe,
    ToolOracleRecipe, ToolSetKind, TurnAuthor, TurnCompleted, TurnPaused, TurnRequest,
    TurnResponse,
};
use probe_protocol::session::{
    PendingToolApproval, SessionId, SessionMetadata, UsageMeasurement, UsageTruth,
};

pub const INTERNAL_SERVER_SUBCOMMAND: &str = "__internal-probe-server";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeClientConfig {
    pub probe_home: PathBuf,
    pub client_name: String,
    pub client_version: Option<String>,
    pub server_binary: Option<PathBuf>,
}

impl ProbeClientConfig {
    #[must_use]
    pub fn new(probe_home: impl Into<PathBuf>, client_name: impl Into<String>) -> Self {
        Self {
            probe_home: probe_home.into(),
            client_name: client_name.into(),
            client_version: None,
            server_binary: None,
        }
    }
}

#[derive(Debug)]
pub enum ProbeClientError {
    CurrentExecutable(std::io::Error),
    Spawn(std::io::Error),
    MissingChildStdin,
    MissingChildStdout,
    Io(std::io::Error),
    Json(serde_json::Error),
    Protocol(RuntimeProtocolError),
    SessionScopedProtocol {
        session_id: SessionId,
        source: RuntimeProtocolError,
    },
    UnexpectedServerMessage(String),
    UnsupportedToolSet(String),
    ToolApprovalPending {
        session_id: SessionId,
        tool_name: String,
        call_id: String,
        reason: Option<String>,
    },
    ShutdownRejected {
        active_turns: usize,
    },
}

impl Display for ProbeClientError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CurrentExecutable(error) => {
                write!(
                    f,
                    "failed to resolve current executable for probe-server spawn: {error}"
                )
            }
            Self::Spawn(error) => write!(f, "failed to spawn probe-server: {error}"),
            Self::MissingChildStdin => write!(f, "probe-server child did not expose stdin"),
            Self::MissingChildStdout => write!(f, "probe-server child did not expose stdout"),
            Self::Io(error) => write!(f, "probe-server io error: {error}"),
            Self::Json(error) => write!(f, "probe-server protocol json error: {error}"),
            Self::Protocol(error) => write!(f, "{} ({})", error.message, error.code),
            Self::SessionScopedProtocol { source, .. } => {
                write!(f, "{} ({})", source.message, source.code)
            }
            Self::UnexpectedServerMessage(message) => write!(f, "{message}"),
            Self::UnsupportedToolSet(name) => {
                write!(
                    f,
                    "unsupported tool registry for probe-server transport: {name}"
                )
            }
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
            Self::ShutdownRejected { active_turns } => write!(
                f,
                "probe-server rejected shutdown because {active_turns} active turn(s) are still running"
            ),
        }
    }
}

impl std::error::Error for ProbeClientError {}

impl From<std::io::Error> for ProbeClientError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ProbeClientError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub struct ProbeClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_request_id: u64,
    client_name: String,
    client_version: Option<String>,
}

impl ProbeClient {
    pub fn spawn(config: ProbeClientConfig) -> Result<Self, ProbeClientError> {
        let mut command = build_server_command(&config)?;
        let mut child = command.spawn().map_err(ProbeClientError::Spawn)?;
        let stdin = child
            .stdin
            .take()
            .ok_or(ProbeClientError::MissingChildStdin)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(ProbeClientError::MissingChildStdout)?;
        let mut client = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_request_id: 0,
            client_name: config.client_name.clone(),
            client_version: config.client_version.clone(),
        };
        let _ = client.send_request(
            RuntimeRequest::Initialize(InitializeRequest {
                client_name: config.client_name,
                client_version: config.client_version,
                protocol_version: probe_protocol::PROBE_PROTOCOL_VERSION,
            }),
            None,
        )?;
        Ok(client)
    }

    pub fn exec_plain_text(
        &mut self,
        request: PlainTextExecRequest,
    ) -> Result<PlainTextExecOutcome, ProbeClientError> {
        self.exec_plain_text_internal(request, None)
    }

    pub fn exec_plain_text_with_events(
        &mut self,
        request: PlainTextExecRequest,
        event_sink: Arc<dyn RuntimeEventSink>,
    ) -> Result<PlainTextExecOutcome, ProbeClientError> {
        self.exec_plain_text_internal(request, Some(event_sink))
    }

    fn exec_plain_text_internal(
        &mut self,
        request: PlainTextExecRequest,
        event_sink: Option<Arc<dyn RuntimeEventSink>>,
    ) -> Result<PlainTextExecOutcome, ProbeClientError> {
        let session = self.start_session(StartSessionRequest {
            title: request
                .title
                .clone()
                .or_else(|| Some(default_session_title(request.prompt.as_str()))),
            cwd: request.cwd,
            profile: request.profile.clone(),
            system_prompt: request.system_prompt,
            harness_profile: request.harness_profile,
        })?;
        let session_id = session.session.id.clone();
        let turn = self
            .send_request(
                RuntimeRequest::StartTurn(TurnRequest {
                    session_id: session_id.clone(),
                    profile: request.profile,
                    prompt: request.prompt,
                    author: Some(self.turn_author()),
                    tool_loop: request
                        .tool_loop
                        .as_ref()
                        .map(tool_loop_recipe_from_config)
                        .transpose()?,
                }),
                event_sink,
            )
            .map_err(|error| session_scoped_error(session_id, error))?;
        match turn {
            RuntimeResponse::StartTurn(TurnResponse::Completed(completed)) => {
                Ok(turn_completed(completed))
            }
            RuntimeResponse::StartTurn(TurnResponse::Paused(paused)) => {
                Err(tool_approval_pending(paused))
            }
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected start_turn response, got {other:?}"
            ))),
        }
    }

    pub fn continue_plain_text_session(
        &mut self,
        request: PlainTextResumeRequest,
    ) -> Result<PlainTextExecOutcome, ProbeClientError> {
        self.continue_plain_text_session_internal(request, None)
    }

    pub fn continue_plain_text_session_with_events(
        &mut self,
        request: PlainTextResumeRequest,
        event_sink: Arc<dyn RuntimeEventSink>,
    ) -> Result<PlainTextExecOutcome, ProbeClientError> {
        self.continue_plain_text_session_internal(request, Some(event_sink))
    }

    fn continue_plain_text_session_internal(
        &mut self,
        request: PlainTextResumeRequest,
        event_sink: Option<Arc<dyn RuntimeEventSink>>,
    ) -> Result<PlainTextExecOutcome, ProbeClientError> {
        let session_id = request.session_id.clone();
        let turn = self
            .send_request(
                RuntimeRequest::ContinueTurn(TurnRequest {
                    session_id: session_id.clone(),
                    profile: request.profile,
                    prompt: request.prompt,
                    author: Some(self.turn_author()),
                    tool_loop: request
                        .tool_loop
                        .as_ref()
                        .map(tool_loop_recipe_from_config)
                        .transpose()?,
                }),
                event_sink,
            )
            .map_err(|error| session_scoped_error(session_id, error))?;
        match turn {
            RuntimeResponse::ContinueTurn(TurnResponse::Completed(completed)) => {
                Ok(turn_completed(completed))
            }
            RuntimeResponse::ContinueTurn(TurnResponse::Paused(paused)) => {
                Err(tool_approval_pending(paused))
            }
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected continue_turn response, got {other:?}"
            ))),
        }
    }

    pub fn queue_plain_text_session_turn(
        &mut self,
        request: PlainTextResumeRequest,
    ) -> Result<QueueTurnResponse, ProbeClientError> {
        let session_id = request.session_id.clone();
        match self
            .send_request(
                RuntimeRequest::QueueTurn(TurnRequest {
                    session_id: session_id.clone(),
                    profile: request.profile,
                    prompt: request.prompt,
                    author: Some(self.turn_author()),
                    tool_loop: request
                        .tool_loop
                        .as_ref()
                        .map(tool_loop_recipe_from_config)
                        .transpose()?,
                }),
                None,
            )
            .map_err(|error| session_scoped_error(session_id, error))?
        {
            RuntimeResponse::QueueTurn(response) => Ok(response),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected queue_turn response, got {other:?}"
            ))),
        }
    }

    pub fn resolve_pending_tool_approval(
        &mut self,
        request: ResolvePendingToolApprovalRequest,
    ) -> Result<ResolvePendingToolApprovalOutcome, ProbeClientError> {
        self.resolve_pending_tool_approval_internal(request, None)
    }

    pub fn resolve_pending_tool_approval_with_events(
        &mut self,
        request: ResolvePendingToolApprovalRequest,
        event_sink: Arc<dyn RuntimeEventSink>,
    ) -> Result<ResolvePendingToolApprovalOutcome, ProbeClientError> {
        self.resolve_pending_tool_approval_internal(request, Some(event_sink))
    }

    fn resolve_pending_tool_approval_internal(
        &mut self,
        request: ResolvePendingToolApprovalRequest,
        event_sink: Option<Arc<dyn RuntimeEventSink>>,
    ) -> Result<ResolvePendingToolApprovalOutcome, ProbeClientError> {
        let session_id = request.session_id.clone();
        let response = self
            .send_request(
                RuntimeRequest::ResolvePendingApproval(
                    probe_protocol::runtime::ResolvePendingApprovalRequest {
                        session_id: session_id.clone(),
                        profile: request.profile,
                        tool_loop: tool_loop_recipe_from_config(&request.tool_loop)?,
                        call_id: request.call_id,
                        resolution: request.resolution,
                    },
                ),
                event_sink,
            )
            .map_err(|error| session_scoped_error(session_id, error))?;
        match response {
            RuntimeResponse::ResolvePendingApproval(
                ResolvePendingApprovalResponse::StillPending {
                    session,
                    pending_approvals,
                },
            ) => Ok(ResolvePendingToolApprovalOutcome::StillPending {
                session,
                pending_approvals,
            }),
            RuntimeResponse::ResolvePendingApproval(ResolvePendingApprovalResponse::Resumed(
                completed,
            )) => Ok(ResolvePendingToolApprovalOutcome::Resumed {
                outcome: turn_completed(completed),
            }),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected resolve_pending_approval response, got {other:?}"
            ))),
        }
    }

    pub fn list_sessions(&mut self) -> Result<Vec<SessionMetadata>, ProbeClientError> {
        match self.send_request(RuntimeRequest::ListSessions, None)? {
            RuntimeResponse::ListSessions(ListSessionsResponse { sessions }) => Ok(sessions),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected list_sessions response, got {other:?}"
            ))),
        }
    }

    pub fn inspect_session(
        &mut self,
        session_id: &SessionId,
    ) -> Result<SessionSnapshot, ProbeClientError> {
        match self.send_request(
            RuntimeRequest::InspectSession(SessionLookupRequest {
                session_id: session_id.clone(),
            }),
            None,
        )? {
            RuntimeResponse::InspectSession(snapshot) => Ok(snapshot),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected inspect_session response, got {other:?}"
            ))),
        }
    }

    pub fn resume_session(
        &mut self,
        session_id: &SessionId,
    ) -> Result<SessionSnapshot, ProbeClientError> {
        match self.send_request(
            RuntimeRequest::ResumeSession(SessionLookupRequest {
                session_id: session_id.clone(),
            }),
            None,
        )? {
            RuntimeResponse::ResumeSession(snapshot) => Ok(snapshot),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected resume_session response, got {other:?}"
            ))),
        }
    }

    pub fn inspect_session_turns(
        &mut self,
        session_id: &SessionId,
    ) -> Result<InspectSessionTurnsResponse, ProbeClientError> {
        match self.send_request(
            RuntimeRequest::InspectSessionTurns(SessionLookupRequest {
                session_id: session_id.clone(),
            }),
            None,
        )? {
            RuntimeResponse::InspectSessionTurns(response) => Ok(response),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected inspect_session_turns response, got {other:?}"
            ))),
        }
    }

    pub fn read_metadata(
        &mut self,
        session_id: &SessionId,
    ) -> Result<SessionMetadata, ProbeClientError> {
        self.inspect_session(session_id)
            .map(|snapshot| snapshot.session)
    }

    pub fn read_transcript(
        &mut self,
        session_id: &SessionId,
    ) -> Result<Vec<probe_protocol::session::TranscriptEvent>, ProbeClientError> {
        self.inspect_session(session_id)
            .map(|snapshot| snapshot.transcript)
    }

    pub fn pending_tool_approvals(
        &mut self,
        session_id: &SessionId,
    ) -> Result<Vec<PendingToolApproval>, ProbeClientError> {
        match self.send_request(
            RuntimeRequest::ListPendingApprovals(
                probe_protocol::runtime::ListPendingApprovalsRequest {
                    session_id: Some(session_id.clone()),
                },
            ),
            None,
        )? {
            RuntimeResponse::ListPendingApprovals(response) => Ok(response.approvals),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected list_pending_approvals response, got {other:?}"
            ))),
        }
    }

    pub fn interrupt_turn(
        &mut self,
        session_id: &SessionId,
    ) -> Result<InterruptTurnResponse, ProbeClientError> {
        match self.send_request(
            RuntimeRequest::InterruptTurn(probe_protocol::runtime::InterruptTurnRequest {
                session_id: session_id.clone(),
            }),
            None,
        )? {
            RuntimeResponse::InterruptTurn(response) => Ok(response),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected interrupt_turn response, got {other:?}"
            ))),
        }
    }

    pub fn cancel_queued_turn(
        &mut self,
        session_id: &SessionId,
        turn_id: impl Into<String>,
    ) -> Result<CancelQueuedTurnResponse, ProbeClientError> {
        match self.send_request(
            RuntimeRequest::CancelQueuedTurn(CancelQueuedTurnRequest {
                session_id: session_id.clone(),
                turn_id: turn_id.into(),
            }),
            None,
        )? {
            RuntimeResponse::CancelQueuedTurn(response) => Ok(response),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected cancel_queued_turn response, got {other:?}"
            ))),
        }
    }

    pub fn shutdown(&mut self) -> Result<(), ProbeClientError> {
        match self.send_request(RuntimeRequest::Shutdown, None)? {
            RuntimeResponse::Shutdown(response) if response.accepted => {
                let _ = self.child.wait();
                Ok(())
            }
            RuntimeResponse::Shutdown(response) => Err(ProbeClientError::ShutdownRejected {
                active_turns: response.active_turns,
            }),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected shutdown response, got {other:?}"
            ))),
        }
    }

    fn turn_author(&self) -> TurnAuthor {
        TurnAuthor {
            client_name: self.client_name.clone(),
            client_version: self.client_version.clone(),
            display_name: None,
        }
    }

    fn start_session(
        &mut self,
        request: StartSessionRequest,
    ) -> Result<SessionSnapshot, ProbeClientError> {
        match self.send_request(RuntimeRequest::StartSession(request), None)? {
            RuntimeResponse::StartSession(snapshot) => Ok(snapshot),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected start_session response, got {other:?}"
            ))),
        }
    }

    fn send_request(
        &mut self,
        request: RuntimeRequest,
        event_sink: Option<Arc<dyn RuntimeEventSink>>,
    ) -> Result<RuntimeResponse, ProbeClientError> {
        let request_id = self.next_request_id();
        self.write_request(request_id.as_str(), request)?;
        loop {
            match self.read_message()? {
                ServerMessage::Response(ResponseEnvelope {
                    request_id: response_id,
                    body,
                }) if response_id == request_id => {
                    return match body {
                        ResponseBody::Ok { response } => Ok(response),
                        ResponseBody::Error { error } => Err(ProbeClientError::Protocol(error)),
                    };
                }
                ServerMessage::Event(EventEnvelope {
                    request_id: event_id,
                    event,
                }) if event_id == request_id => {
                    forward_server_event(event_sink.as_ref(), event)?;
                }
                message => {
                    return Err(ProbeClientError::UnexpectedServerMessage(format!(
                        "received unexpected server message while waiting for request {request_id}: {message:?}"
                    )));
                }
            }
        }
    }

    fn next_request_id(&mut self) -> String {
        self.next_request_id += 1;
        format!("req-{}", self.next_request_id)
    }

    fn write_request(
        &mut self,
        request_id: &str,
        request: RuntimeRequest,
    ) -> Result<(), ProbeClientError> {
        let message = ClientMessage::Request(RequestEnvelope {
            request_id: String::from(request_id),
            request,
        });
        serde_json::to_writer(&mut self.stdin, &message)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn read_message(&mut self) -> Result<ServerMessage, ProbeClientError> {
        let mut line = String::new();
        let bytes = self.stdout.read_line(&mut line)?;
        if bytes == 0 {
            return Err(ProbeClientError::UnexpectedServerMessage(String::from(
                "probe-server exited before sending a response",
            )));
        }
        Ok(serde_json::from_str(line.trim_end())?)
    }
}

impl Drop for ProbeClient {
    fn drop(&mut self) {
        let _ = self.shutdown();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn build_server_command(config: &ProbeClientConfig) -> Result<Command, ProbeClientError> {
    let mut command = match explicit_server_binary(config) {
        Some(path) => Command::new(path),
        None => {
            let current_exe = env::current_exe().map_err(ProbeClientError::CurrentExecutable)?;
            let sibling_server = sibling_probe_server_path(current_exe.as_path());
            if sibling_server.exists() {
                Command::new(sibling_server)
            } else {
                let mut command = Command::new(current_exe);
                command.arg(INTERNAL_SERVER_SUBCOMMAND);
                command
            }
        }
    };
    command
        .arg("--probe-home")
        .arg(config.probe_home.as_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    Ok(command)
}

fn explicit_server_binary(config: &ProbeClientConfig) -> Option<PathBuf> {
    config
        .server_binary
        .clone()
        .or_else(|| env::var_os("PROBE_SERVER_BIN").map(PathBuf::from))
}

fn sibling_probe_server_path(current_exe: &Path) -> PathBuf {
    let base_dir = current_exe
        .parent()
        .and_then(|parent| {
            if parent.file_name().is_some_and(|name| name == "deps") {
                parent.parent()
            } else {
                Some(parent)
            }
        })
        .unwrap_or_else(|| Path::new("."));
    base_dir.join(format!("probe-server{}", env::consts::EXE_SUFFIX))
}

fn tool_loop_recipe_from_config(
    config: &ToolLoopConfig,
) -> Result<ToolLoopRecipe, ProbeClientError> {
    let tool_set = match config.registry.name() {
        "coding_bootstrap" => ToolSetKind::CodingBootstrap,
        other => return Err(ProbeClientError::UnsupportedToolSet(String::from(other))),
    };
    Ok(ToolLoopRecipe {
        tool_set,
        tool_choice: tool_choice_from_config(&config.tool_choice),
        parallel_tool_calls: config.parallel_tool_calls,
        max_model_round_trips: config.max_model_round_trips,
        approval: ToolApprovalRecipe {
            allow_write_tools: config.approval.allow_write_tools,
            allow_network_shell: config.approval.allow_network_shell,
            allow_destructive_shell: config.approval.allow_destructive_shell,
            denied_action: match config.approval.denied_action {
                ToolDeniedAction::Refuse => ProtocolDeniedAction::Refuse,
                ToolDeniedAction::Pause => ProtocolDeniedAction::Pause,
            },
        },
        oracle: config.oracle.as_ref().map(|oracle| ToolOracleRecipe {
            profile: oracle.profile.clone(),
            max_calls: oracle.max_calls,
        }),
        long_context: config
            .long_context
            .as_ref()
            .map(|long_context| ToolLongContextRecipe {
                profile: long_context.profile.clone(),
                max_calls: long_context.max_calls,
                max_evidence_files: long_context.max_evidence_files,
                max_lines_per_file: long_context.max_lines_per_file,
            }),
    })
}

fn tool_choice_from_config(choice: &ProbeToolChoice) -> ToolChoice {
    match choice {
        ProbeToolChoice::None => ToolChoice::None,
        ProbeToolChoice::Auto => ToolChoice::Auto,
        ProbeToolChoice::Required => ToolChoice::Required,
        ProbeToolChoice::Named(name) => ToolChoice::Named {
            tool_name: name.clone(),
        },
    }
}

fn turn_completed(completed: TurnCompleted) -> PlainTextExecOutcome {
    PlainTextExecOutcome {
        session: completed.session,
        turn: completed.turn,
        assistant_text: completed.assistant_text,
        response_id: completed.response_id,
        response_model: completed.response_model,
        usage: completed.usage.map(provider_usage_from_runtime),
        executed_tool_calls: completed.executed_tool_calls,
        tool_results: completed
            .tool_results
            .into_iter()
            .map(executed_tool_call)
            .collect(),
    }
}

fn tool_approval_pending(paused: TurnPaused) -> ProbeClientError {
    ProbeClientError::ToolApprovalPending {
        session_id: paused.session.id,
        tool_name: paused.tool_name,
        call_id: paused.call_id,
        reason: paused.reason,
    }
}

fn session_scoped_error(session_id: SessionId, error: ProbeClientError) -> ProbeClientError {
    match error {
        ProbeClientError::Protocol(source) => {
            ProbeClientError::SessionScopedProtocol { session_id, source }
        }
        other => other,
    }
}

fn provider_usage_from_runtime(usage: RuntimeUsage) -> ProviderUsage {
    ProviderUsage {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
        prompt_tokens_detail: usage
            .prompt_tokens_detail
            .map(provider_usage_measurement_from_runtime),
        completion_tokens_detail: usage
            .completion_tokens_detail
            .map(provider_usage_measurement_from_runtime),
        total_tokens_detail: usage
            .total_tokens_detail
            .map(provider_usage_measurement_from_runtime),
    }
}

fn provider_usage_measurement_from_runtime(
    measurement: UsageMeasurement,
) -> ProviderUsageMeasurement {
    ProviderUsageMeasurement {
        value: measurement.value,
        truth: match measurement.truth {
            UsageTruth::Exact => ProviderUsageTruth::Exact,
            UsageTruth::Estimated => ProviderUsageTruth::Estimated,
        },
    }
}

fn executed_tool_call(tool: ToolCallResult) -> ExecutedToolCall {
    ExecutedToolCall {
        call_id: tool.call_id,
        name: tool.name,
        arguments: tool.arguments,
        output: tool.output,
        tool_execution: tool.tool_execution,
    }
}

fn forward_server_event(
    event_sink: Option<&Arc<dyn RuntimeEventSink>>,
    event: ServerEvent,
) -> Result<(), ProbeClientError> {
    if let Some(event_sink) = event_sink
        && let ServerEvent::RuntimeProgress { event, .. } = event
    {
        event_sink.emit(runtime_event_from_progress(event));
    }
    Ok(())
}

fn runtime_event_from_progress(event: RuntimeProgressEvent) -> RuntimeEvent {
    match event {
        RuntimeProgressEvent::TurnStarted {
            session_id,
            profile_name,
            prompt,
            tool_loop_enabled,
        } => RuntimeEvent::TurnStarted {
            session_id,
            profile_name,
            prompt,
            tool_loop_enabled,
        },
        RuntimeProgressEvent::ModelRequestStarted {
            session_id,
            round_trip,
            backend_kind,
        } => RuntimeEvent::ModelRequestStarted {
            session_id,
            round_trip,
            backend_kind,
        },
        RuntimeProgressEvent::AssistantStreamStarted {
            session_id,
            round_trip,
            response_id,
            response_model,
        } => RuntimeEvent::AssistantStreamStarted {
            session_id,
            round_trip,
            response_id,
            response_model,
        },
        RuntimeProgressEvent::TimeToFirstTokenObserved {
            session_id,
            round_trip,
            milliseconds,
        } => RuntimeEvent::TimeToFirstTokenObserved {
            session_id,
            round_trip,
            milliseconds,
        },
        RuntimeProgressEvent::AssistantDelta {
            session_id,
            round_trip,
            delta,
        } => RuntimeEvent::AssistantDelta {
            session_id,
            round_trip,
            delta,
        },
        RuntimeProgressEvent::AssistantSnapshot {
            session_id,
            round_trip,
            snapshot,
        } => RuntimeEvent::AssistantSnapshot {
            session_id,
            round_trip,
            snapshot,
        },
        RuntimeProgressEvent::ToolCallDelta {
            session_id,
            round_trip,
            deltas,
        } => RuntimeEvent::ToolCallDelta {
            session_id,
            round_trip,
            deltas: deltas.into_iter().map(streamed_tool_call_delta).collect(),
        },
        RuntimeProgressEvent::ToolCallRequested {
            session_id,
            round_trip,
            call_id,
            tool_name,
            arguments,
        } => RuntimeEvent::ToolCallRequested {
            session_id,
            round_trip,
            call_id,
            tool_name,
            arguments,
        },
        RuntimeProgressEvent::ToolExecutionStarted {
            session_id,
            round_trip,
            call_id,
            tool_name,
            risk_class,
        } => RuntimeEvent::ToolExecutionStarted {
            session_id,
            round_trip,
            call_id,
            tool_name,
            risk_class,
        },
        RuntimeProgressEvent::ToolExecutionCompleted {
            session_id,
            round_trip,
            tool,
        } => RuntimeEvent::ToolExecutionCompleted {
            session_id,
            round_trip,
            tool: executed_tool_call(tool),
        },
        RuntimeProgressEvent::ToolRefused {
            session_id,
            round_trip,
            tool,
        } => RuntimeEvent::ToolRefused {
            session_id,
            round_trip,
            tool: executed_tool_call(tool),
        },
        RuntimeProgressEvent::ToolPaused {
            session_id,
            round_trip,
            tool,
        } => RuntimeEvent::ToolPaused {
            session_id,
            round_trip,
            tool: executed_tool_call(tool),
        },
        RuntimeProgressEvent::AssistantStreamFinished {
            session_id,
            round_trip,
            response_id,
            response_model,
            finish_reason,
        } => RuntimeEvent::AssistantStreamFinished {
            session_id,
            round_trip,
            response_id,
            response_model,
            finish_reason,
        },
        RuntimeProgressEvent::ModelRequestFailed {
            session_id,
            round_trip,
            backend_kind,
            error,
        } => RuntimeEvent::ModelRequestFailed {
            session_id,
            round_trip,
            backend_kind,
            error,
        },
        RuntimeProgressEvent::AssistantTurnCommitted {
            session_id,
            response_id,
            response_model,
            assistant_text,
        } => RuntimeEvent::AssistantTurnCommitted {
            session_id,
            response_id,
            response_model,
            assistant_text,
        },
    }
}

fn streamed_tool_call_delta(delta: RuntimeToolCallDelta) -> StreamedToolCallDelta {
    StreamedToolCallDelta {
        tool_index: delta.tool_index,
        call_id: delta.call_id,
        tool_name: delta.tool_name,
        arguments_delta: delta.arguments_delta,
    }
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use assert_cmd::cargo::cargo_bin;
    use probe_core::runtime::{PlainTextExecRequest, RuntimeEvent};
    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use probe_test_support::{FakeHttpResponse, FakeOpenAiServer, ProbeTestEnvironment};

    use super::{ProbeClient, ProbeClientConfig};

    const TEST_MODEL: &str = "tiny-qwen35";

    #[test]
    fn client_can_execute_a_turn_and_forward_streamed_events() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let fake_backend = FakeOpenAiServer::from_responses(vec![
            FakeHttpResponse::text_event_stream(
                200,
                concat!(
                    "data: {\"id\":\"chatcmpl_client_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hello\"}}]}\n\n",
                    "data: {\"id\":\"chatcmpl_client_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" from client\"}}]}\n\n",
                    "data: {\"id\":\"chatcmpl_client_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":3,\"total_tokens\":6}}\n\n"
                ),
            ),
        ]);
        let mut config = ProbeClientConfig::new(environment.probe_home(), "probe-client-test");
        config.server_binary = Some(cargo_bin("probe-server"));
        let mut client = ProbeClient::spawn(config).expect("client should spawn");
        let events = Arc::new(Mutex::new(Vec::<RuntimeEvent>::new()));
        let captured = Arc::clone(&events);
        let sink: Arc<dyn probe_core::runtime::RuntimeEventSink> = Arc::new(move |event| {
            captured.lock().expect("event capture mutex").push(event);
        });

        let outcome = client
            .exec_plain_text_with_events(
                PlainTextExecRequest {
                    profile: test_profile(fake_backend.base_url()),
                    prompt: String::from("hello"),
                    title: Some(String::from("client session")),
                    cwd: environment.workspace().to_path_buf(),
                    system_prompt: None,
                    harness_profile: None,
                    tool_loop: None,
                },
                sink,
            )
            .expect("turn should succeed");

        assert_eq!(outcome.assistant_text, "hello from client");
        assert!(events
            .lock()
            .expect("event capture mutex")
            .iter()
            .any(|event| matches!(event, RuntimeEvent::AssistantDelta { delta, .. } if delta.contains("hello"))));

        let requests = fake_backend.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("hello"));
    }

    fn test_profile(base_url: &str) -> BackendProfile {
        BackendProfile {
            name: String::from("client-test"),
            kind: BackendKind::OpenAiChatCompletions,
            base_url: String::from(base_url),
            model: String::from(TEST_MODEL),
            api_key_env: String::from("PROBE_OPENAI_API_KEY"),
            timeout_secs: 30,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
        }
    }
}
