use std::env;
use std::fmt::{Display, Formatter};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use probe_core::provider::{ProviderUsage, ProviderUsageMeasurement, ProviderUsageTruth};
use probe_core::runtime::{
    PlainTextExecOutcome, PlainTextExecRequest, PlainTextResumeRequest,
    ResolvePendingToolApprovalOutcome, ResolvePendingToolApprovalRequest, RuntimeEvent,
    RuntimeEventSink, StreamedToolCallDelta,
};
use probe_core::tools::{ExecutedToolCall, ProbeToolChoice, ToolDeniedAction, ToolLoopConfig};
use probe_protocol::runtime::{
    AttachSessionParticipantRequest, AttachSessionParticipantResponse, CancelQueuedTurnRequest,
    CancelQueuedTurnResponse, ClientMessage, DetachedSessionEventRecord, DetachedSessionSummary,
    EventEnvelope, InitializeRequest, InspectDetachedSessionResponse,
    InspectSessionMeshCoordinationRequest, InspectSessionMeshCoordinationResponse,
    InspectSessionMeshPluginOffersRequest, InspectSessionMeshPluginOffersResponse,
    InspectSessionTurnsResponse, InterruptTurnResponse, ListDetachedSessionsResponse,
    ListSessionsResponse, PostSessionMeshCoordinationRequest, PostSessionMeshCoordinationResponse,
    PublishSessionMeshPluginOfferRequest, PublishSessionMeshPluginOfferResponse, QueueTurnResponse,
    ReadDetachedSessionLogRequest, ReadDetachedSessionLogResponse, RequestEnvelope,
    ResolvePendingApprovalResponse, ResponseBody, ResponseEnvelope, RuntimeProgressEvent,
    RuntimeProtocolError, RuntimeRequest, RuntimeResponse, RuntimeToolCallDelta, RuntimeUsage,
    ServerEvent, ServerMessage, SessionLookupRequest, SessionSnapshot, SpawnChildSessionRequest,
    SpawnChildSessionResponse, StartSessionRequest, ToolApprovalRecipe, ToolCallResult, ToolChoice,
    ToolDeniedAction as ProtocolDeniedAction, ToolLongContextRecipe, ToolLoopRecipe,
    ToolOracleRecipe, ToolSetKind, TurnAuthor, TurnCompleted, TurnPaused, TurnRequest,
    TurnResponse, UpdateSessionControllerRequest, UpdateSessionControllerResponse,
    WatchDetachedSessionRequest, WatchDetachedSessionResponse,
};
use probe_protocol::session::{
    PendingToolApproval, SessionControllerAction, SessionId, SessionMetadata, UsageMeasurement,
    UsageTruth,
};
use probe_protocol::{PROBE_PROTOCOL_VERSION, default_local_daemon_socket_path};

#[cfg(unix)]
use std::os::unix::net::UnixStream;

pub const INTERNAL_SERVER_SUBCOMMAND: &str = "__internal-probe-server";
pub const INTERNAL_DAEMON_SUBCOMMAND: &str = "__internal-probe-daemon";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeClientConfig {
    pub probe_home: PathBuf,
    pub client_name: String,
    pub client_version: Option<String>,
    pub display_name: Option<String>,
    pub participant_id: Option<String>,
    pub server_binary: Option<PathBuf>,
    pub transport: ProbeClientTransportConfig,
}

impl ProbeClientConfig {
    #[must_use]
    pub fn new(probe_home: impl Into<PathBuf>, client_name: impl Into<String>) -> Self {
        Self {
            probe_home: probe_home.into(),
            client_name: client_name.into(),
            client_version: None,
            display_name: None,
            participant_id: None,
            server_binary: None,
            transport: ProbeClientTransportConfig::SpawnStdio,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedGcpIapTransportConfig {
    pub project: String,
    pub zone: String,
    pub instance: String,
    pub remote_port: u16,
    pub local_host: String,
    pub local_port: Option<u16>,
    pub gcloud_binary: Option<PathBuf>,
}

impl HostedGcpIapTransportConfig {
    #[must_use]
    pub fn new(
        project: impl Into<String>,
        zone: impl Into<String>,
        instance: impl Into<String>,
    ) -> Self {
        Self {
            project: project.into(),
            zone: zone.into(),
            instance: instance.into(),
            remote_port: 7777,
            local_host: String::from("127.0.0.1"),
            local_port: None,
            gcloud_binary: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeClientTransportConfig {
    SpawnStdio,
    LocalDaemon { socket_path: Option<PathBuf> },
    HostedTcp { address: String },
    HostedGcpIap(HostedGcpIapTransportConfig),
}

#[derive(Debug)]
pub enum ProbeClientError {
    CurrentExecutable(std::io::Error),
    Spawn(std::io::Error),
    ConnectDaemon(std::io::Error),
    ConnectHosted(std::io::Error),
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
                    "failed to resolve current executable for Probe helper spawn: {error}"
                )
            }
            Self::Spawn(error) => write!(f, "failed to spawn Probe helper process: {error}"),
            Self::ConnectDaemon(error) => {
                write!(f, "failed to connect to probe-daemon: {error}")
            }
            Self::ConnectHosted(error) => {
                write!(f, "failed to connect to hosted Probe transport: {error}")
            }
            Self::MissingChildStdin => write!(f, "probe-server child did not expose stdin"),
            Self::MissingChildStdout => write!(f, "probe-server child did not expose stdout"),
            Self::Io(error) => write!(f, "Probe runtime io error: {error}"),
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

struct ClientTransport {
    child: Option<Child>,
    writer: Box<dyn Write + Send>,
    reader: Box<dyn BufRead + Send>,
    shutdown_on_drop: bool,
}

pub struct ProbeClient {
    transport: ClientTransport,
    next_request_id: u64,
    client_name: String,
    client_version: Option<String>,
    display_name: Option<String>,
    participant_id: Option<String>,
}

impl ProbeClient {
    pub fn spawn(config: ProbeClientConfig) -> Result<Self, ProbeClientError> {
        Self::connect(config)
    }

    pub fn connect(config: ProbeClientConfig) -> Result<Self, ProbeClientError> {
        let transport = build_transport(&config)?;
        let mut client = Self {
            transport,
            next_request_id: 0,
            client_name: config.client_name.clone(),
            client_version: config.client_version.clone(),
            display_name: config.display_name.clone(),
            participant_id: config.participant_id.clone(),
        };
        match client.send_request(
            RuntimeRequest::Initialize(InitializeRequest {
                client_name: config.client_name,
                client_version: config.client_version,
                protocol_version: PROBE_PROTOCOL_VERSION,
            }),
            None,
        )? {
            RuntimeResponse::Initialize(_) => {}
            other => {
                return Err(ProbeClientError::UnexpectedServerMessage(format!(
                    "expected initialize response, got {other:?}"
                )));
            }
        }
        Ok(client)
    }

    pub fn connect_or_autostart_local_daemon(
        config: ProbeClientConfig,
        wait_timeout: Duration,
    ) -> Result<Self, ProbeClientError> {
        match Self::connect(config.clone()) {
            Ok(client) => Ok(client),
            Err(error) if is_missing_local_daemon_error(&error) => {
                spawn_local_daemon(config.probe_home.as_path())?;
                wait_for_local_daemon(&config, wait_timeout)?;
                Self::connect(config)
            }
            Err(error) => Err(error),
        }
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
            workspace_state: None,
            mounted_refs: Vec::new(),
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
                        author: Some(self.turn_author()),
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

    pub fn list_detached_sessions(
        &mut self,
    ) -> Result<Vec<DetachedSessionSummary>, ProbeClientError> {
        match self.send_request(RuntimeRequest::ListDetachedSessions, None)? {
            RuntimeResponse::ListDetachedSessions(ListDetachedSessionsResponse { sessions }) => {
                Ok(sessions)
            }
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected list_detached_sessions response, got {other:?}"
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

    pub fn inspect_detached_session(
        &mut self,
        session_id: &SessionId,
    ) -> Result<InspectDetachedSessionResponse, ProbeClientError> {
        match self.send_request(
            RuntimeRequest::InspectDetachedSession(SessionLookupRequest {
                session_id: session_id.clone(),
            }),
            None,
        )? {
            RuntimeResponse::InspectDetachedSession(response) => Ok(response),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected inspect_detached_session response, got {other:?}"
            ))),
        }
    }

    pub fn inspect_session_mesh_coordination(
        &mut self,
        request: InspectSessionMeshCoordinationRequest,
    ) -> Result<InspectSessionMeshCoordinationResponse, ProbeClientError> {
        match self.send_request(
            RuntimeRequest::InspectSessionMeshCoordination(request),
            None,
        )? {
            RuntimeResponse::InspectSessionMeshCoordination(response) => Ok(response),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected inspect_session_mesh_coordination response, got {other:?}"
            ))),
        }
    }

    pub fn post_session_mesh_coordination(
        &mut self,
        mut request: PostSessionMeshCoordinationRequest,
    ) -> Result<PostSessionMeshCoordinationResponse, ProbeClientError> {
        if request.author.is_none() {
            request.author = Some(self.coordination_author());
        }
        match self.send_request(RuntimeRequest::PostSessionMeshCoordination(request), None)? {
            RuntimeResponse::PostSessionMeshCoordination(response) => Ok(response),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected post_session_mesh_coordination response, got {other:?}"
            ))),
        }
    }

    pub fn inspect_session_mesh_plugin_offers(
        &mut self,
        request: InspectSessionMeshPluginOffersRequest,
    ) -> Result<InspectSessionMeshPluginOffersResponse, ProbeClientError> {
        match self.send_request(
            RuntimeRequest::InspectSessionMeshPluginOffers(request),
            None,
        )? {
            RuntimeResponse::InspectSessionMeshPluginOffers(response) => Ok(response),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected inspect_session_mesh_plugin_offers response, got {other:?}"
            ))),
        }
    }

    pub fn publish_session_mesh_plugin_offer(
        &mut self,
        mut request: PublishSessionMeshPluginOfferRequest,
    ) -> Result<PublishSessionMeshPluginOfferResponse, ProbeClientError> {
        if request.author.is_none() {
            request.author = Some(self.coordination_author());
        }
        match self.send_request(RuntimeRequest::PublishSessionMeshPluginOffer(request), None)? {
            RuntimeResponse::PublishSessionMeshPluginOffer(response) => Ok(response),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected publish_session_mesh_plugin_offer response, got {other:?}"
            ))),
        }
    }

    pub fn read_detached_session_log(
        &mut self,
        session_id: &SessionId,
        after_cursor: Option<u64>,
        limit: usize,
    ) -> Result<ReadDetachedSessionLogResponse, ProbeClientError> {
        match self.send_request(
            RuntimeRequest::ReadDetachedSessionLog(ReadDetachedSessionLogRequest {
                session_id: session_id.clone(),
                after_cursor,
                limit,
            }),
            None,
        )? {
            RuntimeResponse::ReadDetachedSessionLog(response) => Ok(response),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected read_detached_session_log response, got {other:?}"
            ))),
        }
    }

    pub fn watch_detached_session<F>(
        mut self,
        request: WatchDetachedSessionRequest,
        mut on_event: F,
    ) -> Result<Option<WatchDetachedSessionResponse>, ProbeClientError>
    where
        F: FnMut(DetachedSessionEventRecord) -> bool,
    {
        let request_id = self.next_request_id();
        self.write_request(
            request_id.as_str(),
            RuntimeRequest::WatchDetachedSession(request),
        )?;
        loop {
            match self.read_message()? {
                ServerMessage::Event(EventEnvelope {
                    request_id: event_id,
                    event: ServerEvent::DetachedSessionStream { record },
                }) if event_id == request_id => {
                    if !on_event(record) {
                        return Ok(None);
                    }
                }
                ServerMessage::Response(ResponseEnvelope {
                    request_id: response_id,
                    body,
                }) if response_id == request_id => {
                    return match body {
                        ResponseBody::Ok {
                            response: RuntimeResponse::WatchDetachedSession(response),
                        } => Ok(Some(response)),
                        ResponseBody::Error { error } => Err(ProbeClientError::Protocol(error)),
                        ResponseBody::Ok { response } => {
                            Err(ProbeClientError::UnexpectedServerMessage(format!(
                                "expected watch_detached_session response, got {response:?}"
                            )))
                        }
                    };
                }
                message => {
                    return Err(ProbeClientError::UnexpectedServerMessage(format!(
                        "received unexpected server message while watching detached session {request_id}: {message:?}"
                    )));
                }
            }
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
                author: Some(self.turn_author()),
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
                author: Some(self.turn_author()),
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
                if let Some(child) = self.transport.child.as_mut() {
                    let _ = child.wait();
                }
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
            display_name: self.display_name.clone(),
            participant_id: self.participant_id.clone(),
        }
    }

    fn coordination_author(&self) -> String {
        self.display_name
            .clone()
            .or_else(|| self.participant_id.clone())
            .unwrap_or_else(|| self.client_name.clone())
    }

    pub fn attach_session_participant(
        &mut self,
        session_id: &SessionId,
        claim_controller: bool,
    ) -> Result<AttachSessionParticipantResponse, ProbeClientError> {
        match self.send_request(
            RuntimeRequest::AttachSessionParticipant(AttachSessionParticipantRequest {
                session_id: session_id.clone(),
                participant: self.turn_author(),
                claim_controller,
            }),
            None,
        )? {
            RuntimeResponse::AttachSessionParticipant(response) => Ok(response),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected attach_session_participant response, got {other:?}"
            ))),
        }
    }

    pub fn update_session_controller(
        &mut self,
        session_id: &SessionId,
        action: SessionControllerAction,
        target_participant_id: Option<String>,
    ) -> Result<UpdateSessionControllerResponse, ProbeClientError> {
        match self.send_request(
            RuntimeRequest::UpdateSessionController(UpdateSessionControllerRequest {
                session_id: session_id.clone(),
                actor: self.turn_author(),
                action,
                target_participant_id,
            }),
            None,
        )? {
            RuntimeResponse::UpdateSessionController(response) => Ok(response),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected update_session_controller response, got {other:?}"
            ))),
        }
    }

    pub fn start_session(
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

    pub fn spawn_child_session(
        &mut self,
        request: SpawnChildSessionRequest,
    ) -> Result<SpawnChildSessionResponse, ProbeClientError> {
        match self.send_request(RuntimeRequest::SpawnChildSession(request), None)? {
            RuntimeResponse::SpawnChildSession(response) => Ok(response),
            other => Err(ProbeClientError::UnexpectedServerMessage(format!(
                "expected spawn_child_session response, got {other:?}"
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
        serde_json::to_writer(&mut self.transport.writer, &message)?;
        self.transport.writer.write_all(b"\n")?;
        self.transport.writer.flush()?;
        Ok(())
    }

    fn read_message(&mut self) -> Result<ServerMessage, ProbeClientError> {
        let mut line = String::new();
        let bytes = self.transport.reader.read_line(&mut line)?;
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
        if self.transport.shutdown_on_drop {
            let _ = self.shutdown();
        }
        if let Some(child) = self.transport.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn build_transport(config: &ProbeClientConfig) -> Result<ClientTransport, ProbeClientError> {
    match &config.transport {
        ProbeClientTransportConfig::SpawnStdio => spawn_server_transport(config),
        ProbeClientTransportConfig::LocalDaemon { socket_path } => {
            connect_local_daemon_transport(config, socket_path.as_ref())
        }
        ProbeClientTransportConfig::HostedTcp { address } => {
            connect_hosted_tcp_transport(address.as_str())
        }
        ProbeClientTransportConfig::HostedGcpIap(iap) => connect_hosted_gcp_iap_transport(iap),
    }
}

pub fn is_missing_local_daemon_error(error: &ProbeClientError) -> bool {
    match error {
        ProbeClientError::ConnectDaemon(source) => matches!(
            source.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
        ),
        _ => false,
    }
}

fn spawn_server_transport(config: &ProbeClientConfig) -> Result<ClientTransport, ProbeClientError> {
    let mut command = build_server_command(config)?;
    let mut child = command.spawn().map_err(ProbeClientError::Spawn)?;
    let stdin = child
        .stdin
        .take()
        .ok_or(ProbeClientError::MissingChildStdin)?;
    let stdout = child
        .stdout
        .take()
        .ok_or(ProbeClientError::MissingChildStdout)?;
    Ok(ClientTransport {
        child: Some(child),
        writer: Box::new(stdin),
        reader: Box::new(BufReader::new(stdout)),
        shutdown_on_drop: true,
    })
}

#[cfg(unix)]
fn connect_local_daemon_transport(
    config: &ProbeClientConfig,
    socket_path: Option<&PathBuf>,
) -> Result<ClientTransport, ProbeClientError> {
    let socket_path = socket_path
        .cloned()
        .unwrap_or_else(|| default_local_daemon_socket_path(config.probe_home.as_path()));
    let stream = UnixStream::connect(&socket_path).map_err(ProbeClientError::ConnectDaemon)?;
    let writer = stream
        .try_clone()
        .map_err(ProbeClientError::ConnectDaemon)?;
    Ok(ClientTransport {
        child: None,
        writer: Box::new(writer),
        reader: Box::new(BufReader::new(stream)),
        shutdown_on_drop: false,
    })
}

#[cfg(not(unix))]
fn connect_local_daemon_transport(
    _config: &ProbeClientConfig,
    _socket_path: Option<&PathBuf>,
) -> Result<ClientTransport, ProbeClientError> {
    Err(ProbeClientError::ConnectDaemon(std::io::Error::other(
        "local daemon transport is only implemented on unix platforms",
    )))
}

fn connect_hosted_tcp_transport(address: &str) -> Result<ClientTransport, ProbeClientError> {
    let stream = TcpStream::connect(address).map_err(ProbeClientError::ConnectHosted)?;
    let writer = stream
        .try_clone()
        .map_err(ProbeClientError::ConnectHosted)?;
    Ok(ClientTransport {
        child: None,
        writer: Box::new(writer),
        reader: Box::new(BufReader::new(stream)),
        shutdown_on_drop: false,
    })
}

fn connect_hosted_gcp_iap_transport(
    config: &HostedGcpIapTransportConfig,
) -> Result<ClientTransport, ProbeClientError> {
    let local_port = reserve_local_tcp_port(config.local_host.as_str(), config.local_port)?;
    let local_address = format!("{}:{local_port}", config.local_host);
    let gcloud_binary = config
        .gcloud_binary
        .clone()
        .unwrap_or_else(|| PathBuf::from("gcloud"));
    let mut command = Command::new(gcloud_binary);
    command
        .arg("compute")
        .arg("start-iap-tunnel")
        .arg(config.instance.as_str())
        .arg(config.remote_port.to_string())
        .arg(format!("--local-host-port={local_address}"))
        .arg(format!("--project={}", config.project))
        .arg(format!("--zone={}", config.zone))
        .arg("--verbosity=error")
        .arg("--quiet")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(ProbeClientError::Spawn)?;
    let stream = wait_for_hosted_tunnel(local_address.as_str(), &mut child)?;
    let writer = stream
        .try_clone()
        .map_err(ProbeClientError::ConnectHosted)?;
    Ok(ClientTransport {
        child: Some(child),
        writer: Box::new(writer),
        reader: Box::new(BufReader::new(stream)),
        shutdown_on_drop: false,
    })
}

fn reserve_local_tcp_port(host: &str, requested: Option<u16>) -> Result<u16, ProbeClientError> {
    if let Some(port) = requested {
        return Ok(port);
    }
    let listener =
        TcpListener::bind(format!("{host}:0")).map_err(ProbeClientError::ConnectHosted)?;
    let port = listener
        .local_addr()
        .map_err(ProbeClientError::ConnectHosted)?
        .port();
    drop(listener);
    Ok(port)
}

fn wait_for_hosted_tunnel(address: &str, child: &mut Child) -> Result<TcpStream, ProbeClientError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_error = None;
    while Instant::now() < deadline {
        match TcpStream::connect(address) {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                last_error = Some(error);
                if child.try_wait().map_err(ProbeClientError::Spawn)?.is_some() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(ProbeClientError::ConnectHosted(std::io::Error::other(
        format!(
            "timed out waiting for hosted GCP IAP tunnel to expose {address}: {}",
            last_error
                .map(|error| error.to_string())
                .unwrap_or_else(|| String::from("no connection details"))
        ),
    )))
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

fn spawn_local_daemon(probe_home: &Path) -> Result<(), ProbeClientError> {
    let mut command = build_daemon_command()?;
    command
        .arg("--probe-home")
        .arg(probe_home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.spawn().map_err(ProbeClientError::Spawn)?;
    Ok(())
}

fn build_daemon_command() -> Result<Command, ProbeClientError> {
    if let Some(path) = env::var_os("PROBE_DAEMON_BIN").map(PathBuf::from) {
        let mut command = Command::new(path);
        command.arg("run");
        return Ok(command);
    }

    let current_exe = env::current_exe().map_err(ProbeClientError::CurrentExecutable)?;
    let sibling_daemon = sibling_named_binary_path(current_exe.as_path(), "probe-daemon");
    if sibling_daemon.exists() {
        let mut command = Command::new(sibling_daemon);
        command.arg("run");
        return Ok(command);
    }

    let mut command = Command::new(current_exe);
    command.arg(INTERNAL_DAEMON_SUBCOMMAND);
    Ok(command)
}

fn wait_for_local_daemon(
    config: &ProbeClientConfig,
    wait_timeout: Duration,
) -> Result<(), ProbeClientError> {
    let deadline = Instant::now() + wait_timeout;
    loop {
        match ProbeClient::connect(config.clone()) {
            Ok(client) => {
                drop(client);
                return Ok(());
            }
            Err(error) if is_missing_local_daemon_error(&error) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error),
        }
    }
}

fn explicit_server_binary(config: &ProbeClientConfig) -> Option<PathBuf> {
    config
        .server_binary
        .clone()
        .or_else(|| env::var_os("PROBE_SERVER_BIN").map(PathBuf::from))
}

fn sibling_probe_server_path(current_exe: &Path) -> PathBuf {
    sibling_named_binary_path(current_exe, "probe-server")
}

fn sibling_named_binary_path(current_exe: &Path, binary_name: &str) -> PathBuf {
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
    base_dir.join(format!("{binary_name}{}", env::consts::EXE_SUFFIX))
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
    use std::fs;
    use std::net::TcpListener;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use probe_core::runtime::{PlainTextExecRequest, PlainTextResumeRequest, RuntimeEvent};
    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use probe_protocol::runtime::{
        DetachedSessionRecoveryState, DetachedSessionStatus, StartSessionRequest,
    };
    use probe_protocol::session::{
        SessionAttachTransport, SessionControllerAction, SessionExecutionHostKind,
        SessionHostedAuthKind, SessionHostedCheckoutKind, SessionHostedCleanupStatus,
        SessionHostedLifecycleEvent, SessionPreparedBaselineRef, SessionPreparedBaselineStatus,
        SessionRuntimeOwnerKind, SessionWorkspaceBootMode, SessionWorkspaceSnapshotRef,
        SessionWorkspaceState,
    };
    use probe_test_support::{FakeHttpResponse, FakeOpenAiServer, ProbeTestEnvironment};

    use super::{
        HostedGcpIapTransportConfig, ProbeClient, ProbeClientConfig, ProbeClientTransportConfig,
    };

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
        config.server_binary = Some(workspace_binary("probe-server"));
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

    #[test]
    fn client_can_connect_to_hosted_tcp_transport_and_inspect_runtime_owner() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let baseline_dir = environment.probe_home().join("hosted").join("baselines");
        fs::create_dir_all(&baseline_dir).expect("create hosted baseline dir");
        fs::write(
            baseline_dir.join("repo-main.json"),
            r#"{
  "baseline_id": "repo-main",
  "repo_identity": "github.com/OpenAgentsInc/probe",
  "base_ref": "main",
  "stale": false
}"#,
        )
        .expect("write hosted baseline manifest");
        let address = reserve_loopback_addr();
        let attach_target = format!("tcp://{address}");
        let mut server = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );

        let mut config = ProbeClientConfig::new(environment.probe_home(), "probe-client-test");
        config.transport = ProbeClientTransportConfig::HostedTcp {
            address: address.clone(),
        };
        let mut client = wait_for_hosted_client(config);
        let sessions = client
            .list_sessions()
            .expect("hosted client should list sessions");
        assert!(
            sessions.is_empty(),
            "fresh hosted transport should start empty"
        );

        let snapshot = client
            .start_session(StartSessionRequest {
                title: Some(String::from("hosted tcp session")),
                cwd: environment.workspace().to_path_buf(),
                profile: test_profile("http://127.0.0.1:9/v1"),
                system_prompt: None,
                harness_profile: None,
                workspace_state: Some(SessionWorkspaceState {
                    boot_mode: SessionWorkspaceBootMode::PreparedBaseline,
                    baseline: Some(SessionPreparedBaselineRef {
                        baseline_id: String::from("repo-main"),
                        repo_identity: None,
                        base_ref: None,
                        status: SessionPreparedBaselineStatus::Ready,
                    }),
                    snapshot: Some(SessionWorkspaceSnapshotRef {
                        snapshot_id: String::from("snap-missing"),
                        restore_manifest_id: None,
                        source_baseline_id: None,
                    }),
                    execution_host: None,
                    provenance_note: None,
                }),
                mounted_refs: Vec::new(),
            })
            .expect("hosted transport should start a session");
        let owner = snapshot
            .session
            .runtime_owner
            .expect("hosted session should persist a runtime owner");
        assert_eq!(owner.kind, SessionRuntimeOwnerKind::HostedControlPlane);
        assert_eq!(owner.owner_id, "probe-hosted-test");
        assert_eq!(owner.attach_transport, SessionAttachTransport::TcpJsonl);
        assert_eq!(owner.attach_target.as_deref(), Some(attach_target.as_str()));
        let workspace_state = snapshot
            .session
            .workspace_state
            .expect("hosted session should persist workspace provenance");
        assert_eq!(
            workspace_state.boot_mode,
            SessionWorkspaceBootMode::PreparedBaseline
        );
        let baseline = workspace_state
            .baseline
            .expect("hosted session should persist baseline provenance");
        assert_eq!(baseline.baseline_id, "repo-main");
        assert_eq!(
            baseline.repo_identity.as_deref(),
            Some("github.com/OpenAgentsInc/probe")
        );
        assert_eq!(baseline.base_ref.as_deref(), Some("main"));
        assert_eq!(baseline.status, SessionPreparedBaselineStatus::Ready);
        let execution_host = workspace_state
            .execution_host
            .expect("hosted session should expose execution-host provenance");
        assert_eq!(execution_host.kind, SessionExecutionHostKind::HostedWorker);
        assert_eq!(execution_host.host_id, "probe-hosted-test");
        assert_eq!(
            workspace_state.provenance_note.as_deref(),
            Some("snapshot snap-missing has no manifest in hosted/snapshots")
        );
        let receipts = snapshot
            .session
            .hosted_receipts
            .expect("hosted session should expose hosted receipts");
        let auth = receipts
            .auth
            .expect("hosted session should persist auth receipt");
        assert_eq!(auth.authority, "probe-hosted-test");
        assert_eq!(auth.subject, "gcp-internal-dogfood");
        assert_eq!(auth.auth_kind, SessionHostedAuthKind::ControlPlaneAssertion);
        let worker = receipts
            .worker
            .expect("hosted session should persist worker receipt");
        assert_eq!(worker.owner_id, "probe-hosted-test");
        assert_eq!(worker.execution_host_id, "probe-hosted-test");
        let checkout = receipts
            .checkout
            .expect("hosted session should persist checkout receipt");
        assert_eq!(checkout.kind, SessionHostedCheckoutKind::PlainWorkspace);
        assert_eq!(
            checkout.workspace_root,
            environment.workspace().to_path_buf()
        );
        let cost = receipts
            .cost
            .expect("hosted session should persist cost receipt");
        assert_eq!(cost.observed_turn_count, 0);
        assert_eq!(cost.wallclock_ms, 0);
        let cleanup = receipts
            .cleanup
            .expect("hosted session should persist cleanup receipt");
        assert_eq!(cleanup.status, SessionHostedCleanupStatus::NotRequired);
        let detached_summary = client
            .list_detached_sessions()
            .expect("hosted client should list detached sessions")
            .into_iter()
            .find(|summary| summary.session_id == snapshot.session.id)
            .expect("hosted detached registry should include started session");
        assert_eq!(
            detached_summary
                .workspace_state
                .as_ref()
                .and_then(|state| state.baseline.as_ref())
                .map(|baseline| baseline.status),
            Some(SessionPreparedBaselineStatus::Ready)
        );

        client
            .shutdown()
            .expect("idle hosted server should accept shutdown");
        server.wait();
    }

    #[test]
    fn hosted_gcp_iap_transport_can_discover_and_attach_to_existing_hosted_sessions() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let address = reserve_loopback_addr();
        let attach_target = format!("tcp://{address}");
        let mut server = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );

        let mut starter_config =
            ProbeClientConfig::new(environment.probe_home(), "probe-client-starter");
        starter_config.transport = ProbeClientTransportConfig::HostedTcp {
            address: address.clone(),
        };
        let mut starter = wait_for_hosted_client(starter_config);
        let snapshot = starter
            .start_session(StartSessionRequest {
                title: Some(String::from("discoverable hosted session")),
                cwd: environment.workspace().to_path_buf(),
                profile: test_profile("http://127.0.0.1:9/v1"),
                system_prompt: None,
                harness_profile: None,
                workspace_state: None,
                mounted_refs: Vec::new(),
            })
            .expect("hosted session should start");
        let session_id = snapshot.session.id.clone();
        drop(starter);

        let (fake_gcloud, log_path) = write_fake_gcloud_tunnel_script(&environment);
        let remote_port = address
            .rsplit_once(':')
            .expect("hosted address should include a port")
            .1
            .parse::<u16>()
            .expect("hosted address port should parse");
        let mut config = ProbeClientConfig::new(environment.probe_home(), "probe-client-test");
        let mut iap = HostedGcpIapTransportConfig::new(
            "openagentsgemini",
            "us-central1-a",
            "probe-hosted-forge-1",
        );
        iap.remote_port = remote_port;
        iap.local_port = Some(17789);
        iap.gcloud_binary = Some(fake_gcloud);
        config.transport = ProbeClientTransportConfig::HostedGcpIap(iap);

        let mut teammate = ProbeClient::connect(config)
            .expect("hosted gcp iap transport should connect through the fake tunnel");
        let detached = teammate
            .list_detached_sessions()
            .expect("gcp iap hosted client should discover detached sessions");
        assert!(detached.iter().any(|summary| {
            summary.session_id == session_id
                && summary
                    .runtime_owner
                    .as_ref()
                    .and_then(|owner| owner.attach_target.as_deref())
                    == Some(attach_target.as_str())
        }));

        let inspected = teammate
            .inspect_detached_session(&session_id)
            .expect("gcp iap hosted client should inspect the discovered session");
        assert_eq!(inspected.summary.session_id, session_id);

        let logged = fs::read_to_string(&log_path).expect("fake gcloud command log should exist");
        assert!(logged.contains("compute start-iap-tunnel probe-hosted-forge-1"));
        assert!(logged.contains("--project=openagentsgemini"));
        assert!(logged.contains("--zone=us-central1-a"));
        assert!(logged.contains("--local-host-port=127.0.0.1:17789"));

        drop(teammate);

        let mut closer_config =
            ProbeClientConfig::new(environment.probe_home(), "probe-client-closer");
        closer_config.transport = ProbeClientTransportConfig::HostedTcp {
            address: address.clone(),
        };
        let mut closer = wait_for_hosted_client(closer_config);
        closer
            .shutdown()
            .expect("idle hosted server should accept shutdown directly");
        server.wait();
    }

    #[test]
    fn hosted_turn_receipts_capture_git_checkout_and_cost_observability() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        init_git_workspace(environment.workspace());
        let fake_backend = FakeOpenAiServer::from_responses(vec![
            FakeHttpResponse::text_event_stream(
                200,
                concat!(
                    "data: {\"id\":\"chatcmpl_hosted_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hosted\"}}]}\n\n",
                    "data: {\"id\":\"chatcmpl_hosted_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" receipts\"}}]}\n\n",
                    "data: {\"id\":\"chatcmpl_hosted_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":3,\"total_tokens\":6}}\n\n"
                ),
            ),
        ]);
        let address = reserve_loopback_addr();
        let attach_target = format!("tcp://{address}");
        let mut server = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );
        let mut config = ProbeClientConfig::new(environment.probe_home(), "probe-client-test");
        config.transport = ProbeClientTransportConfig::HostedTcp {
            address: address.clone(),
        };
        let mut client = wait_for_hosted_client(config);
        let outcome = client
            .exec_plain_text(PlainTextExecRequest {
                profile: test_profile(fake_backend.base_url()),
                prompt: String::from("summarize hosted receipts"),
                title: Some(String::from("hosted receipts")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: None,
                harness_profile: None,
                tool_loop: None,
            })
            .expect("hosted turn should succeed");
        let receipts = outcome
            .session
            .hosted_receipts
            .expect("completed hosted turn should expose hosted receipts");
        let checkout = receipts
            .checkout
            .expect("hosted git workspace should persist checkout receipt");
        assert_eq!(checkout.kind, SessionHostedCheckoutKind::GitRepository);
        assert_eq!(
            checkout.repo_identity.as_deref(),
            Some("https://github.com/OpenAgentsInc/probe.git")
        );
        assert_eq!(checkout.head_ref.as_deref(), Some("main"));
        assert!(
            checkout.head_commit.is_some(),
            "git checkout receipt should include the resolved commit"
        );
        let cost = receipts
            .cost
            .expect("hosted turn should persist cost receipt");
        assert_eq!(cost.observed_turn_count, 1);
        assert!(cost.wallclock_ms > 0);
        assert_eq!(cost.prompt_tokens, Some(3));
        assert_eq!(cost.completion_tokens, Some(3));
        assert_eq!(cost.total_tokens, Some(6));
        let cleanup = receipts
            .cleanup
            .expect("hosted turn should persist cleanup receipt");
        assert_eq!(cleanup.status, SessionHostedCleanupStatus::NotRequired);

        client
            .shutdown()
            .expect("idle hosted server should accept shutdown");
        server.wait();
        let requests = fake_backend.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("summarize hosted receipts"));
    }

    #[test]
    fn hosted_detached_summary_projects_checkout_worker_and_cleanup_receipts() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        init_git_workspace(environment.workspace());
        let fake_backend = FakeOpenAiServer::from_responses(vec![
            FakeHttpResponse::text_event_stream(
                200,
                concat!(
                    "data: {\"id\":\"chatcmpl_hosted_detached\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"detached\"}}]}\n\n",
                    "data: {\"id\":\"chatcmpl_hosted_detached\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" summary\"}}]}\n\n",
                    "data: {\"id\":\"chatcmpl_hosted_detached\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2,\"total_tokens\":6}}\n\n"
                ),
            ),
        ]);
        let address = reserve_loopback_addr();
        let attach_target = format!("tcp://{address}");
        let mut server = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );
        let mut config = ProbeClientConfig::new(environment.probe_home(), "probe-client-test");
        config.transport = ProbeClientTransportConfig::HostedTcp {
            address: address.clone(),
        };
        let mut client = wait_for_hosted_client(config);
        let outcome = client
            .exec_plain_text(PlainTextExecRequest {
                profile: test_profile(fake_backend.base_url()),
                prompt: String::from("project detached receipts"),
                title: Some(String::from("detached receipts")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: None,
                harness_profile: None,
                tool_loop: None,
            })
            .expect("hosted turn should succeed");
        let detached = client
            .inspect_detached_session(&outcome.session.id)
            .expect("hosted detached session should be inspectable");
        let receipts = detached
            .summary
            .hosted_receipts
            .expect("detached summary should project hosted receipts");
        let checkout = receipts
            .checkout
            .expect("detached summary should keep checkout receipt");
        assert_eq!(checkout.kind, SessionHostedCheckoutKind::GitRepository);
        assert_eq!(
            checkout.repo_identity.as_deref(),
            Some("https://github.com/OpenAgentsInc/probe.git")
        );
        let worker = receipts
            .worker
            .expect("detached summary should keep worker receipt");
        assert_eq!(worker.execution_host_id, "probe-hosted-test");
        let cleanup = receipts
            .cleanup
            .expect("detached summary should keep cleanup receipt");
        assert_eq!(cleanup.status, SessionHostedCleanupStatus::NotRequired);

        client
            .shutdown()
            .expect("idle hosted server should accept shutdown");
        server.wait();
        let requests = fake_backend.finish();
        assert_eq!(requests.len(), 1);
    }

    #[test]
    fn hosted_session_tracks_participants_controller_handoff_and_conflicts() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let address = reserve_loopback_addr();
        let attach_target = format!("tcp://{address}");
        let mut server = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );

        let mut starter = wait_for_hosted_client(hosted_client_config(
            environment.probe_home(),
            address.as_str(),
            "teammate-a",
            "Teammate A",
        ));
        let session = starter
            .start_session(StartSessionRequest {
                title: Some(String::from("shared hosted session")),
                cwd: environment.workspace().to_path_buf(),
                profile: test_profile("http://127.0.0.1:9/v1"),
                system_prompt: None,
                harness_profile: None,
                workspace_state: None,
                mounted_refs: Vec::new(),
            })
            .expect("hosted shared session should start");
        let session_id = session.session.id.clone();
        let attached = starter
            .attach_session_participant(&session_id, true)
            .expect("first participant should attach and claim control");
        assert_eq!(attached.participants.len(), 1);
        assert_eq!(
            attached
                .controller_lease
                .as_ref()
                .map(|lease| lease.participant_id.as_str()),
            Some("teammate-a")
        );
        drop(starter);

        let mut teammate_b = wait_for_hosted_client(hosted_client_config(
            environment.probe_home(),
            address.as_str(),
            "teammate-b",
            "Teammate B",
        ));
        let attached = teammate_b
            .attach_session_participant(&session_id, false)
            .expect("second participant should attach without claiming control");
        assert_eq!(attached.participants.len(), 2);
        assert_eq!(
            attached
                .controller_lease
                .as_ref()
                .map(|lease| lease.participant_id.as_str()),
            Some("teammate-a")
        );
        drop(teammate_b);

        let mut starter = wait_for_hosted_client(hosted_client_config(
            environment.probe_home(),
            address.as_str(),
            "teammate-a",
            "Teammate A",
        ));
        starter
            .update_session_controller(
                &session_id,
                SessionControllerAction::Handoff,
                Some(String::from("teammate-b")),
            )
            .expect("controller handoff should succeed");
        drop(starter);

        let mut teammate_b = wait_for_hosted_client(hosted_client_config(
            environment.probe_home(),
            address.as_str(),
            "teammate-b",
            "Teammate B",
        ));
        let inspected = teammate_b
            .inspect_detached_session(&session_id)
            .expect("hosted shared session should be inspectable");
        assert_eq!(inspected.summary.participants.len(), 2);
        assert_eq!(
            inspected
                .summary
                .controller_lease
                .as_ref()
                .map(|lease| lease.participant_id.as_str()),
            Some("teammate-b")
        );
        assert!(
            inspected
                .summary
                .hosted_receipts
                .as_ref()
                .expect("shared session should keep hosted receipts")
                .history
                .iter()
                .any(|event| matches!(
                    event,
                    SessionHostedLifecycleEvent::ControllerLeaseChanged {
                        action: SessionControllerAction::Handoff,
                        actor_participant_id,
                        target_participant_id,
                        ..
                    } if actor_participant_id == "teammate-a"
                        && target_participant_id.as_deref() == Some("teammate-b")
                ))
        );
        drop(teammate_b);

        let mut starter = wait_for_hosted_client(hosted_client_config(
            environment.probe_home(),
            address.as_str(),
            "teammate-a",
            "Teammate A",
        ));

        let error = starter
            .queue_plain_text_session_turn(PlainTextResumeRequest {
                session_id: session_id.clone(),
                profile: test_profile("http://127.0.0.1:9/v1"),
                prompt: String::from("this should be rejected"),
                tool_loop: None,
            })
            .expect_err("non-controller should not queue a hosted turn");
        assert!(matches!(
            error,
            super::ProbeClientError::SessionScopedProtocol { ref source, .. }
                if source.code == "session_controller_conflict"
        ));

        starter
            .update_session_controller(&session_id, SessionControllerAction::Takeover, None)
            .expect("takeover should succeed");
        let inspected = starter
            .inspect_detached_session(&session_id)
            .expect("taken-over hosted session should remain inspectable");
        assert_eq!(
            inspected
                .summary
                .controller_lease
                .as_ref()
                .map(|lease| lease.participant_id.as_str()),
            Some("teammate-a")
        );
        assert!(
            inspected
                .summary
                .hosted_receipts
                .as_ref()
                .expect("taken-over shared session should keep hosted receipts")
                .history
                .iter()
                .any(|event| matches!(
                    event,
                    SessionHostedLifecycleEvent::ControllerLeaseChanged {
                        action: SessionControllerAction::Takeover,
                        actor_participant_id,
                        target_participant_id,
                        ..
                    } if actor_participant_id == "teammate-a"
                        && target_participant_id.as_deref() == Some("teammate-b")
                ))
        );

        starter
            .shutdown()
            .expect("idle hosted server should accept shutdown");
        server.wait();
    }

    #[test]
    fn managed_hosted_workspace_cleanup_receipt_marks_completed_once_path_is_gone() {
        let environment = ProbeTestEnvironment::new();
        let managed_workspace = environment
            .probe_home()
            .join("hosted")
            .join("workspaces")
            .join("cleanup-proof");
        fs::create_dir_all(&managed_workspace).expect("create managed hosted workspace");
        fs::write(managed_workspace.join("README.md"), "# cleanup proof\n")
            .expect("seed managed workspace");
        let address = reserve_loopback_addr();
        let attach_target = format!("tcp://{address}");
        let mut server = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );
        let mut config = ProbeClientConfig::new(environment.probe_home(), "probe-client-test");
        config.transport = ProbeClientTransportConfig::HostedTcp {
            address: address.clone(),
        };
        let mut client = wait_for_hosted_client(config);
        let snapshot = client
            .start_session(StartSessionRequest {
                title: Some(String::from("managed cleanup proof")),
                cwd: managed_workspace.clone(),
                profile: test_profile("http://127.0.0.1:9/v1"),
                system_prompt: None,
                harness_profile: None,
                workspace_state: None,
                mounted_refs: Vec::new(),
            })
            .expect("managed hosted session should start");
        fs::remove_dir_all(&managed_workspace).expect("remove managed workspace");
        let inspected = client
            .inspect_detached_session(&snapshot.session.id)
            .expect("managed hosted detached session should be inspectable");
        let receipts = inspected
            .summary
            .hosted_receipts
            .expect("managed hosted session should keep hosted receipts");
        let cleanup = receipts
            .cleanup
            .expect("managed hosted session should keep cleanup receipt");
        assert_eq!(cleanup.status, SessionHostedCleanupStatus::Completed);
        assert_eq!(cleanup.strategy, "managed_hosted_workspace");
        let cleanup_events: Vec<_> = receipts
            .history
            .iter()
            .filter_map(|event| match event {
                SessionHostedLifecycleEvent::CleanupStateChanged {
                    previous_status,
                    status,
                    strategy,
                    ..
                } => Some((*previous_status, *status, strategy.as_str())),
                _ => None,
            })
            .collect();
        assert!(
            cleanup_events.contains(&(
                None,
                SessionHostedCleanupStatus::Pending,
                "managed_hosted_workspace"
            )),
            "hosted history should retain the initial managed cleanup status"
        );
        assert!(
            cleanup_events.contains(&(
                Some(SessionHostedCleanupStatus::Pending),
                SessionHostedCleanupStatus::Completed,
                "managed_hosted_workspace"
            )),
            "hosted history should retain the cleanup completion transition"
        );

        client
            .shutdown()
            .expect("idle hosted server should accept shutdown");
        server.wait();
    }

    #[test]
    fn hosted_restart_records_restart_history_and_reaps_managed_workspace() {
        let environment = ProbeTestEnvironment::new();
        let managed_workspace = environment
            .probe_home()
            .join("hosted")
            .join("workspaces")
            .join("restart-reap");
        fs::create_dir_all(&managed_workspace).expect("create managed hosted workspace");
        fs::write(
            managed_workspace.join("README.md"),
            "# restart cleanup proof\n",
        )
        .expect("seed managed workspace");
        let fake_backend = delayed_completion_backend(Duration::from_millis(25), "completed");
        let profile = test_profile(fake_backend.base_url());
        let address = reserve_loopback_addr();
        let attach_target = format!("tcp://{address}");
        let mut server = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );
        let mut config = ProbeClientConfig::new(environment.probe_home(), "probe-client-test");
        config.transport = ProbeClientTransportConfig::HostedTcp {
            address: address.clone(),
        };
        let mut client = wait_for_hosted_client(config.clone());
        let snapshot = client
            .start_session(StartSessionRequest {
                title: Some(String::from("hosted restart cleanup")),
                cwd: managed_workspace.clone(),
                profile: profile.clone(),
                system_prompt: None,
                harness_profile: None,
                workspace_state: None,
                mounted_refs: Vec::new(),
            })
            .expect("managed hosted session should start");
        let session_id = snapshot.session.id.clone();

        client
            .queue_plain_text_session_turn(PlainTextResumeRequest {
                session_id: session_id.clone(),
                profile: profile.clone(),
                prompt: String::from("complete this hosted turn"),
                tool_loop: None,
            })
            .expect("hosted queue turn should be accepted");
        drop(client);

        let completed = wait_for_detached_status(
            config.clone(),
            &session_id,
            DetachedSessionStatus::Completed,
        );
        assert_eq!(completed.status, DetachedSessionStatus::Completed);
        assert!(
            managed_workspace.exists(),
            "managed workspace should still exist before restart reconciliation"
        );

        server.kill_ungraceful();
        let mut restarted = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );
        let mut reattached = wait_for_hosted_client(config);
        let inspected = reattached
            .inspect_detached_session(&session_id)
            .expect("restarted hosted session should remain inspectable");
        assert_eq!(inspected.summary.status, DetachedSessionStatus::Completed);
        assert!(
            !managed_workspace.exists(),
            "restart reconciliation should reap the managed hosted workspace"
        );

        let receipts = inspected
            .summary
            .hosted_receipts
            .expect("restarted hosted summary should keep hosted receipts");
        let cleanup = receipts
            .cleanup
            .expect("restarted hosted summary should keep cleanup receipt");
        assert_eq!(cleanup.status, SessionHostedCleanupStatus::Completed);
        assert!(receipts.history.iter().any(|event| {
            matches!(
                event,
                SessionHostedLifecycleEvent::ControlPlaneRestartObserved { summary, .. }
                    if summary.contains("control-plane restart")
            )
        }));
        assert!(receipts.history.iter().any(|event| {
            matches!(
                event,
                SessionHostedLifecycleEvent::OrphanedManagedWorkspaceReaped {
                    workspace_root,
                    ..
                } if workspace_root == &managed_workspace
            )
        }));
        assert!(receipts.history.iter().any(|event| {
            matches!(
                event,
                SessionHostedLifecycleEvent::CleanupStateChanged {
                    previous_status: Some(SessionHostedCleanupStatus::Pending),
                    status: SessionHostedCleanupStatus::Completed,
                    workspace_root,
                    strategy,
                    ..
                } if workspace_root == &managed_workspace
                    && strategy == "managed_hosted_workspace"
            )
        }));

        reattached
            .shutdown()
            .expect("restarted hosted server should accept shutdown");
        restarted.wait();
        let _ = fake_backend.finish();
    }

    #[test]
    fn hosted_restart_keeps_approval_paused_sessions_resumable() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let fake_backend = approval_pause_backend(Duration::from_millis(50));
        let profile = test_profile(fake_backend.base_url());
        let address = reserve_loopback_addr();
        let attach_target = format!("tcp://{address}");
        let mut server = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );
        let mut config = ProbeClientConfig::new(environment.probe_home(), "probe-client-test");
        config.transport = ProbeClientTransportConfig::HostedTcp {
            address: address.clone(),
        };
        let mut client = wait_for_hosted_client(config.clone());
        let session = client
            .start_session(StartSessionRequest {
                title: Some(String::from("hosted approval pause")),
                cwd: environment.workspace().to_path_buf(),
                profile: profile.clone(),
                system_prompt: None,
                harness_profile: None,
                workspace_state: None,
                mounted_refs: Vec::new(),
            })
            .expect("hosted session should start");
        let session_id = session.session.id.clone();

        let error = client
            .continue_plain_text_session(PlainTextResumeRequest {
                session_id: session_id.clone(),
                profile: profile.clone(),
                prompt: String::from("patch hello.txt"),
                tool_loop: Some(approval_pause_tool_loop()),
            })
            .expect_err("approval pause should surface through hosted probe-client");
        assert!(matches!(
            error,
            super::ProbeClientError::ToolApprovalPending { .. }
        ));

        drop(client);
        server.kill_ungraceful();
        let mut restarted = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );
        let mut reattached = wait_for_hosted_client(config);
        let inspected = reattached
            .inspect_detached_session(&session_id)
            .expect("approval-paused hosted session should remain inspectable after restart");
        assert_eq!(
            inspected.summary.status,
            DetachedSessionStatus::ApprovalPaused
        );
        assert_eq!(
            inspected.summary.recovery_state,
            DetachedSessionRecoveryState::ApprovalPausedResumable
        );
        assert!(
            inspected
                .summary
                .recovery_note
                .as_deref()
                .is_some_and(|note| note.contains("hosted control plane can resume this session"))
        );
        assert_eq!(inspected.summary.pending_approval_count, 1);
        assert!(inspected.turn_control.active_turn.is_some());
        let receipts = inspected
            .summary
            .hosted_receipts
            .expect("approval-paused hosted summary should keep hosted receipts");
        assert!(receipts.history.iter().any(|event| {
            matches!(
                event,
                SessionHostedLifecycleEvent::ApprovalPausedTakeoverAvailable {
                    turn_id,
                    pending_approval_count,
                    ..
                } if Some(turn_id.as_str()) == inspected.summary.active_turn_id.as_deref()
                    && *pending_approval_count == 1
            )
        }));

        drop(reattached);
        restarted.kill_ungraceful();
        let _ = fake_backend.finish();
    }

    #[test]
    fn hosted_restart_marks_running_turns_failed_when_process_dies() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let fake_backend =
            delayed_completion_backend(Duration::from_millis(500), "this should never complete");
        let profile = test_profile(fake_backend.base_url());
        let address = reserve_loopback_addr();
        let attach_target = format!("tcp://{address}");
        let mut server = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );
        let mut config = ProbeClientConfig::new(environment.probe_home(), "probe-client-test");
        config.transport = ProbeClientTransportConfig::HostedTcp {
            address: address.clone(),
        };
        let mut client = wait_for_hosted_client(config.clone());
        let session = client
            .start_session(StartSessionRequest {
                title: Some(String::from("hosted restart failure")),
                cwd: environment.workspace().to_path_buf(),
                profile: profile.clone(),
                system_prompt: None,
                harness_profile: None,
                workspace_state: None,
                mounted_refs: Vec::new(),
            })
            .expect("hosted session should start");
        let session_id = session.session.id.clone();

        client
            .queue_plain_text_session_turn(PlainTextResumeRequest {
                session_id: session_id.clone(),
                profile: profile.clone(),
                prompt: String::from("run through a hosted restart"),
                tool_loop: None,
            })
            .expect("queue turn should be accepted");
        drop(client);

        let running =
            wait_for_detached_status(config.clone(), &session_id, DetachedSessionStatus::Running);
        assert!(running.active_turn_id.is_some());

        server.kill_ungraceful();
        let mut restarted = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );
        let mut reattached = wait_for_hosted_client(config);
        let inspected = reattached
            .inspect_detached_session(&session_id)
            .expect("restarted hosted server should report failed running turn");
        assert_eq!(inspected.summary.status, DetachedSessionStatus::Failed);
        assert_eq!(
            inspected.summary.recovery_state,
            DetachedSessionRecoveryState::RunningTurnFailedOnRestart
        );
        assert!(
            inspected
                .summary
                .recovery_note
                .as_deref()
                .is_some_and(|note| note.contains("restarted before this running turn completed"))
        );
        assert!(
            inspected
                .turn_control
                .recent_turns
                .first()
                .and_then(|turn| turn.failure_message.as_deref())
                .is_some_and(
                    |message| message.contains("restarted before this running turn completed")
                )
        );
        let receipts = inspected
            .summary
            .hosted_receipts
            .expect("restart-failed hosted summary should keep hosted receipts");
        assert!(receipts.history.iter().any(|event| {
            matches!(
                event,
                SessionHostedLifecycleEvent::RunningTurnFailedOnRestart { turn_id, .. }
                    if Some(turn_id.as_str()) == inspected.summary.last_terminal_turn_id.as_deref()
            )
        }));

        reattached
            .shutdown()
            .expect("restarted hosted server should accept shutdown");
        restarted.wait();
        let _ = fake_backend.finish();
    }

    #[test]
    fn hosted_startup_reaps_orphaned_detached_registry_entries_without_session_metadata() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let address = reserve_loopback_addr();
        let attach_target = format!("tcp://{address}");
        let mut server = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );
        let mut config = ProbeClientConfig::new(environment.probe_home(), "probe-client-test");
        config.transport = ProbeClientTransportConfig::HostedTcp {
            address: address.clone(),
        };
        let mut client = wait_for_hosted_client(config.clone());
        let snapshot = client
            .start_session(StartSessionRequest {
                title: Some(String::from("hosted orphan cleanup")),
                cwd: environment.workspace().to_path_buf(),
                profile: test_profile("http://127.0.0.1:9/v1"),
                system_prompt: None,
                harness_profile: None,
                workspace_state: None,
                mounted_refs: Vec::new(),
            })
            .expect("hosted session should start");
        let session_dir = snapshot
            .session
            .transcript_path
            .parent()
            .expect("session transcript should live inside a session dir")
            .to_path_buf();

        client
            .shutdown()
            .expect("hosted server should accept shutdown");
        server.wait();

        fs::remove_dir_all(&session_dir)
            .expect("remove session metadata to simulate orphaned registry");

        let mut restarted = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );
        let mut reattached = wait_for_hosted_client(config);
        let detached = reattached
            .list_detached_sessions()
            .expect("restarted hosted server should list detached sessions");
        assert!(
            detached.is_empty(),
            "startup reconciliation should drop orphaned detached registry entries"
        );

        drop(reattached);
        restarted.kill_ungraceful();
    }

    #[test]
    fn hosted_session_falls_back_to_fresh_when_prepared_baseline_is_missing() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let address = reserve_loopback_addr();
        let attach_target = format!("tcp://{address}");
        let mut server = HostedProbeServer::start(
            environment.probe_home(),
            address.as_str(),
            "probe-hosted-test",
            attach_target.as_str(),
        );

        let mut config = ProbeClientConfig::new(environment.probe_home(), "probe-client-test");
        config.transport = ProbeClientTransportConfig::HostedTcp {
            address: address.clone(),
        };
        let mut client = wait_for_hosted_client(config);
        let snapshot = client
            .start_session(StartSessionRequest {
                title: Some(String::from("missing baseline fallback")),
                cwd: environment.workspace().to_path_buf(),
                profile: test_profile("http://127.0.0.1:9/v1"),
                system_prompt: None,
                harness_profile: None,
                workspace_state: Some(SessionWorkspaceState {
                    boot_mode: SessionWorkspaceBootMode::PreparedBaseline,
                    baseline: Some(SessionPreparedBaselineRef {
                        baseline_id: String::from("missing-baseline"),
                        repo_identity: None,
                        base_ref: None,
                        status: SessionPreparedBaselineStatus::Ready,
                    }),
                    snapshot: None,
                    execution_host: None,
                    provenance_note: None,
                }),
                mounted_refs: Vec::new(),
            })
            .expect("hosted transport should still start a session when the baseline is missing");
        let workspace_state = snapshot
            .session
            .workspace_state
            .expect("hosted session should still expose workspace provenance");
        assert_eq!(workspace_state.boot_mode, SessionWorkspaceBootMode::Fresh);
        assert_eq!(
            workspace_state
                .baseline
                .as_ref()
                .map(|baseline| baseline.status),
            Some(SessionPreparedBaselineStatus::Missing)
        );
        assert_eq!(
            workspace_state.provenance_note.as_deref(),
            Some(
                "prepared baseline missing-baseline has no manifest in hosted/baselines prepared baseline missing-baseline was Missing; Probe fell back to a fresh workspace start"
            )
        );

        client
            .shutdown()
            .expect("idle hosted server should accept shutdown");
        server.wait();
    }

    fn workspace_binary(name: &str) -> std::path::PathBuf {
        let current_exe = std::env::current_exe().expect("test binary path should resolve");
        let base_dir = current_exe
            .parent()
            .and_then(|parent| {
                if parent.file_name().is_some_and(|value| value == "deps") {
                    parent.parent()
                } else {
                    Some(parent)
                }
            })
            .expect("test binary should have a parent directory");
        let candidate = base_dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        assert!(
            candidate.exists(),
            "expected workspace binary at {}",
            candidate.display()
        );
        candidate
    }

    fn reserve_loopback_addr() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback port should bind");
        let address = listener
            .local_addr()
            .expect("loopback listener should expose an address");
        drop(listener);
        address.to_string()
    }

    fn write_fake_gcloud_tunnel_script(
        environment: &ProbeTestEnvironment,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let script_path = environment.probe_home().join("fake-gcloud.sh");
        let log_path = environment.probe_home().join("fake-gcloud.log");
        let script = format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" > "{log_path}"
instance="${{3:-}}"
remote_port="${{4:-}}"
local_spec=""
for arg in "$@"; do
  case "$arg" in
    --local-host-port=*)
      local_spec="${{arg#*=}}"
      ;;
  esac
done
[ -n "$instance" ] || exit 1
[ -n "$remote_port" ] || exit 1
[ -n "$local_spec" ] || exit 1
local_port="${{local_spec##*:}}"
exec python3 - "$local_port" "$remote_port" <<'PY'
import select
import socket
import sys

local_port = int(sys.argv[1])
remote_port = int(sys.argv[2])

listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("127.0.0.1", local_port))
listener.listen(1)

client, _ = listener.accept()
upstream = socket.create_connection(("127.0.0.1", remote_port))
sockets = [client, upstream]

while sockets:
    readable, _, exceptional = select.select(sockets, [], sockets, 0.5)
    for sock in exceptional:
        if sock in sockets:
            sockets.remove(sock)
    for sock in readable:
        data = sock.recv(65536)
        if not data:
            if sock in sockets:
                sockets.remove(sock)
            peer = upstream if sock is client else client
            try:
                peer.shutdown(socket.SHUT_WR)
            except OSError:
                pass
            continue
        peer = upstream if sock is client else client
        peer.sendall(data)

for stream in (client, upstream, listener):
    try:
        stream.close()
    except OSError:
        pass
PY
"#,
            log_path = log_path.display()
        );
        fs::write(&script_path, script).expect("write fake gcloud script");
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&script_path)
                .expect("fake gcloud script metadata")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script_path, permissions)
                .expect("fake gcloud script should be executable");
        }
        (script_path, log_path)
    }

    fn init_git_workspace(workspace: &std::path::Path) {
        let status = Command::new("git")
            .arg("init")
            .arg("-b")
            .arg("main")
            .current_dir(workspace)
            .status()
            .expect("git init should succeed");
        assert!(status.success(), "git init should succeed");
        let status = Command::new("git")
            .args(["config", "user.name", "Probe Test"])
            .current_dir(workspace)
            .status()
            .expect("git config user.name should succeed");
        assert!(status.success(), "git config user.name should succeed");
        let status = Command::new("git")
            .args(["config", "user.email", "probe@example.com"])
            .current_dir(workspace)
            .status()
            .expect("git config user.email should succeed");
        assert!(status.success(), "git config user.email should succeed");
        let status = Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/OpenAgentsInc/probe.git",
            ])
            .current_dir(workspace)
            .status()
            .expect("git remote add should succeed");
        assert!(status.success(), "git remote add should succeed");
        let status = Command::new("git")
            .args(["add", "."])
            .current_dir(workspace)
            .status()
            .expect("git add should succeed");
        assert!(status.success(), "git add should succeed");
        let status = Command::new("git")
            .args(["commit", "-m", "initial fixture"])
            .current_dir(workspace)
            .status()
            .expect("git commit should succeed");
        assert!(status.success(), "git commit should succeed");
    }

    fn delayed_completion_backend(delay: Duration, assistant_text: &str) -> FakeOpenAiServer {
        let assistant_text = String::from(assistant_text);
        FakeOpenAiServer::from_handler(move |_request| {
            thread::sleep(delay);
            FakeHttpResponse::json_ok(serde_json::json!({
                "id": "chatcmpl_hosted_complete",
                "model": TEST_MODEL,
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": assistant_text.clone()
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 3,
                    "completion_tokens": 3,
                    "total_tokens": 6
                }
            }))
        })
    }

    fn approval_pause_backend(delay: Duration) -> FakeOpenAiServer {
        FakeOpenAiServer::from_handler(move |_request| {
            thread::sleep(delay);
            FakeHttpResponse::json_ok(serde_json::json!({
                "id": "chatcmpl_hosted_pause",
                "model": TEST_MODEL,
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
            }))
        })
    }

    fn approval_pause_tool_loop() -> probe_core::tools::ToolLoopConfig {
        let mut tool_loop = probe_core::tools::ToolLoopConfig::coding_bootstrap(
            probe_core::tools::ProbeToolChoice::Required,
            false,
        );
        tool_loop.approval = probe_core::tools::ToolApprovalConfig {
            allow_write_tools: false,
            allow_network_shell: false,
            allow_destructive_shell: false,
            denied_action: probe_core::tools::ToolDeniedAction::Pause,
        };
        tool_loop
    }

    fn wait_for_hosted_client(config: ProbeClientConfig) -> ProbeClient {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match ProbeClient::connect(config.clone()) {
                Ok(client) => return client,
                Err(super::ProbeClientError::ConnectHosted(_)) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(error) => panic!("hosted transport should accept connections: {error}"),
            }
        }
    }

    fn hosted_client_config(
        probe_home: &std::path::Path,
        address: &str,
        participant_id: &str,
        display_name: &str,
    ) -> ProbeClientConfig {
        let mut config = ProbeClientConfig::new(probe_home.to_path_buf(), "probe-client-test");
        config.transport = ProbeClientTransportConfig::HostedTcp {
            address: String::from(address),
        };
        config.display_name = Some(String::from(display_name));
        config.participant_id = Some(String::from(participant_id));
        config
    }

    fn wait_for_detached_status(
        config: ProbeClientConfig,
        session_id: &super::SessionId,
        expected: DetachedSessionStatus,
    ) -> super::DetachedSessionSummary {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut last_summary = None;
        while Instant::now() < deadline {
            let mut client = wait_for_hosted_client(config.clone());
            let response = client
                .inspect_detached_session(session_id)
                .expect("hosted detached session should be inspectable");
            last_summary = Some(response.summary.clone());
            if response.summary.status == expected {
                return response.summary;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("timed out waiting for detached status {expected:?}: {last_summary:?}");
    }

    struct HostedProbeServer {
        child: Child,
    }

    impl HostedProbeServer {
        fn start(
            probe_home: &std::path::Path,
            address: &str,
            owner_id: &str,
            attach_target: &str,
        ) -> Self {
            let child = Command::new(workspace_binary("probe-server"))
                .arg("--probe-home")
                .arg(probe_home)
                .arg("--listen-tcp")
                .arg(address)
                .arg("--hosted-owner-id")
                .arg(owner_id)
                .arg("--hosted-attach-target")
                .arg(attach_target)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("hosted probe-server should spawn");
            Self { child }
        }

        fn wait(&mut self) {
            let status = self.child.wait().expect("hosted probe-server should exit");
            assert!(status.success(), "hosted probe-server should exit cleanly");
        }

        fn kill_ungraceful(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    impl Drop for HostedProbeServer {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn test_profile(base_url: &str) -> BackendProfile {
        BackendProfile {
            name: String::from("client-test"),
            kind: BackendKind::OpenAiChatCompletions,
            base_url: String::from(base_url),
            model: String::from(TEST_MODEL),
            reasoning_level: None,
            api_key_env: String::from("PROBE_OPENAI_API_KEY"),
            timeout_secs: 30,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
            control_plane: None,
            psionic_mesh: None,
        }
    }
}
