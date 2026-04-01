use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::{self, BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use probe_core::runtime::{
    PlainTextResumeRequest, ProbeRuntime, ResolvePendingToolApprovalOutcome,
    ResolvePendingToolApprovalRequest, RuntimeError, RuntimeEvent, RuntimeEventSink,
    default_probe_home,
};
use probe_core::session_store::{NewItem, NewSession, SessionStoreError};
use probe_core::tools::{
    ExecutedToolCall, ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction as CoreDeniedAction,
    ToolLongContextConfig, ToolLoopConfig, ToolOracleConfig,
};
use probe_protocol::default_local_daemon_socket_path;
use probe_protocol::runtime::{
    CancelQueuedTurnRequest, CancelQueuedTurnResponse, ClientMessage, DetachedSessionRecoveryState,
    DetachedSessionStatus, DetachedSessionSummary, EventDeliveryGuarantee, EventEnvelope,
    InitializeResponse, InspectDetachedSessionResponse, InspectSessionTurnsResponse,
    InterruptTurnResponse, ListDetachedSessionsResponse, ListPendingApprovalsRequest,
    ListPendingApprovalsResponse, ListSessionsResponse, QueueTurnResponse, QueuedTurnStatus,
    RequestEnvelope, ResolvePendingApprovalResponse, ResponseBody, ResponseEnvelope,
    RuntimeCapabilities, RuntimeProgressEvent, RuntimeProtocolError, RuntimeRequest,
    RuntimeResponse, RuntimeToolCallDelta, RuntimeUsage, ServerEvent, ServerMessage,
    SessionLookupRequest, SessionSnapshot, ShutdownResponse, StartSessionRequest,
    ToolApprovalRecipe, ToolCallResult, ToolChoice, ToolDeniedAction, ToolLongContextRecipe,
    ToolLoopRecipe, ToolOracleRecipe, ToolSetKind, TransportKind, TurnCompleted, TurnPaused,
    TurnRequest, TurnResponse, TurnSubmissionKind,
};
use probe_protocol::session::{
    SessionBackendTarget, SessionId, SessionMetadata, UsageMeasurement, UsageTruth,
};
use probe_protocol::{PROBE_PROTOCOL_VERSION, PROBE_RUNTIME_NAME};

use crate::detached_registry::{DetachedRegistryError, DetachedSessionRegistry};
use crate::turn_control::{SessionTurnControlState, StoredTurnControlRecord, now_ms};

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

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
pub(crate) struct SharedJsonlWriter {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl SharedJsonlWriter {
    pub(crate) fn new(writer: Box<dyn Write + Send>) -> Self {
        Self {
            writer: Arc::new(Mutex::new(writer)),
        }
    }

    fn stdio() -> Self {
        Self::new(Box::new(BufWriter::new(io::stdout())))
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
        let mut writer = self
            .writer
            .lock()
            .expect("probe-server writer mutex should not be poisoned");
        serde_json::to_writer(&mut *writer, &message)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        Ok(())
    }
}

pub fn run_stdio_server(probe_home: Option<PathBuf>) -> Result<(), ServerError> {
    let home = resolve_probe_home(probe_home, "probe-server")?;
    let server = ProbeServerConnection::new(
        ProbeServerCore::new(ProbeRuntime::new(home)),
        SharedJsonlWriter::stdio(),
        TransportKind::StdioJsonl,
    );
    let stdin = io::stdin();
    let _ = server.run(stdin.lock())?;
    Ok(())
}

#[cfg(unix)]
pub fn run_local_daemon(
    probe_home: Option<PathBuf>,
    socket_path: Option<PathBuf>,
) -> Result<(), ServerError> {
    let home = resolve_probe_home(probe_home, "probe-daemon")?;
    let socket_path = socket_path.unwrap_or_else(|| default_local_daemon_socket_path(&home));
    prepare_daemon_socket(socket_path.as_path())?;
    let _cleanup_guard = SocketCleanupGuard::new(socket_path.clone());
    let listener = UnixListener::bind(&socket_path)?;
    let core = ProbeServerCore::daemon(ProbeRuntime::new(home));
    core.reconcile_detached_sessions()
        .map_err(runtime_protocol_error_to_io)?;

    loop {
        let (stream, _) = listener.accept()?;
        let server = ProbeServerConnection::new(
            core.clone(),
            SharedJsonlWriter::new(Box::new(BufWriter::new(stream.try_clone()?))),
            TransportKind::UnixSocketJsonl,
        );
        match server.run(io::BufReader::new(stream))? {
            ConnectionRunOutcome::ClientDisconnected => {}
            ConnectionRunOutcome::ServerShutdown => break,
        }
    }

    Ok(())
}

#[cfg(not(unix))]
pub fn run_local_daemon(
    _probe_home: Option<PathBuf>,
    _socket_path: Option<PathBuf>,
) -> Result<(), ServerError> {
    Err(ServerError::Io(io::Error::other(
        "local probe-daemon socket transport is only implemented on unix platforms",
    )))
}

fn resolve_probe_home(
    probe_home: Option<PathBuf>,
    process_name: &str,
) -> Result<PathBuf, ServerError> {
    match probe_home {
        Some(home) => Ok(home),
        None => default_probe_home().map_err(|error| {
            io::Error::other(format!(
                "failed to resolve probe home for {process_name}: {error}"
            ))
            .into()
        }),
    }
}

#[cfg(unix)]
fn prepare_daemon_socket(socket_path: &Path) -> Result<(), ServerError> {
    let Some(parent) = socket_path.parent() else {
        return Err(ServerError::Io(io::Error::other(format!(
            "daemon socket path {} has no parent directory",
            socket_path.display()
        ))));
    };
    fs::create_dir_all(parent)?;

    if !socket_path.exists() {
        return Ok(());
    }

    let file_type = fs::symlink_metadata(socket_path)?.file_type();
    if !file_type.is_socket() {
        return Err(ServerError::Io(io::Error::other(format!(
            "daemon socket path {} already exists and is not a unix socket",
            socket_path.display()
        ))));
    }

    match UnixStream::connect(socket_path) {
        Ok(_) => Err(ServerError::Io(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "probe-daemon is already listening at {}",
                socket_path.display()
            ),
        ))),
        Err(_) => {
            fs::remove_file(socket_path)?;
            Ok(())
        }
    }
}

struct SocketCleanupGuard {
    path: PathBuf,
}

impl SocketCleanupGuard {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for SocketCleanupGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Clone)]
pub struct ProbeServerCore {
    runtime: ProbeRuntime,
    turn_control: Arc<TurnControlPlane>,
    detached_registry: Option<Arc<DetachedSessionRegistry>>,
}

struct ProbeServerConnection {
    core: ProbeServerCore,
    writer: SharedJsonlWriter,
    transport: TransportKind,
}

enum ConnectionRunOutcome {
    ClientDisconnected,
    ServerShutdown,
}

enum RequestHandlingOutcome {
    Continue,
    ShutdownAccepted,
}

#[derive(Clone)]
struct TurnControlPlane {
    runtime: ProbeRuntime,
    coordination: Arc<Mutex<HashSet<String>>>,
    detached_registry: Option<Arc<DetachedSessionRegistry>>,
}

struct QueuedTurnReservation {
    response: QueueTurnResponse,
    turn_id: String,
    should_start: bool,
}

struct BackgroundTurnWorkItem {
    turn_id: String,
    request: TurnRequest,
    mode: TurnMode,
}

struct InterruptOutcome {
    response: InterruptTurnResponse,
    should_start_next: bool,
}

impl ProbeServerCore {
    pub fn new(runtime: ProbeRuntime) -> Self {
        Self::with_mode(runtime, ServerOwnershipMode::ForegroundStdio)
    }

    fn daemon(runtime: ProbeRuntime) -> Self {
        Self::with_mode(runtime, ServerOwnershipMode::DetachedDaemon)
    }

    fn with_mode(runtime: ProbeRuntime, mode: ServerOwnershipMode) -> Self {
        let detached_registry = mode
            .is_daemon()
            .then(|| Arc::new(DetachedSessionRegistry::new(runtime.session_store().root())));
        Self {
            turn_control: Arc::new(TurnControlPlane::new(
                runtime.clone(),
                detached_registry.clone(),
            )),
            runtime,
            detached_registry,
        }
    }

    fn reconcile_detached_sessions(&self) -> Result<(), RuntimeProtocolError> {
        let Some(registry) = self.detached_registry.as_ref() else {
            return Ok(());
        };
        for session_id in registry
            .tracked_session_ids()
            .map_err(detached_registry_error_to_protocol)?
        {
            if self
                .runtime
                .session_store()
                .read_metadata(&session_id)
                .is_err()
            {
                registry
                    .remove(&session_id)
                    .map_err(detached_registry_error_to_protocol)?;
                continue;
            }
            self.sync_detached_session_if_tracked(&session_id)?;
        }
        Ok(())
    }

    fn ensure_detached_session_registered(
        &self,
        metadata: &SessionMetadata,
    ) -> Result<(), RuntimeProtocolError> {
        let Some(registry) = self.detached_registry.as_ref() else {
            return Ok(());
        };
        registry
            .register_session(metadata, now_ms())
            .map_err(detached_registry_error_to_protocol)?;
        self.turn_control
            .sync_detached_session_if_tracked(&metadata.id)
    }

    fn ensure_detached_session_registered_by_id(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeProtocolError> {
        if self.detached_registry.is_none() {
            return Ok(());
        }
        let metadata = self
            .runtime
            .session_store()
            .read_metadata(session_id)
            .map_err(session_store_error_to_protocol)?;
        self.ensure_detached_session_registered(&metadata)
    }

    fn sync_detached_session_if_tracked(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeProtocolError> {
        self.turn_control
            .sync_detached_session_if_tracked(session_id)
    }

    fn list_detached_sessions(&self) -> Result<ListDetachedSessionsResponse, RuntimeProtocolError> {
        let Some(registry) = self.detached_registry.as_ref() else {
            return Err(protocol_error(
                "unsupported_transport",
                "detached session registry is only available through the daemon transport",
            ));
        };
        for session_id in registry
            .tracked_session_ids()
            .map_err(detached_registry_error_to_protocol)?
        {
            self.sync_detached_session_if_tracked(&session_id)?;
        }
        let sessions = registry
            .list()
            .map_err(detached_registry_error_to_protocol)?;
        Ok(ListDetachedSessionsResponse { sessions })
    }

    fn inspect_detached_session(
        &self,
        session_id: &SessionId,
    ) -> Result<InspectDetachedSessionResponse, RuntimeProtocolError> {
        let Some(registry) = self.detached_registry.as_ref() else {
            return Err(protocol_error(
                "unsupported_transport",
                "detached session registry is only available through the daemon transport",
            ));
        };
        let metadata = self
            .runtime
            .session_store()
            .read_metadata(session_id)
            .map_err(session_store_error_to_protocol)?;
        registry
            .register_session(&metadata, now_ms())
            .map_err(detached_registry_error_to_protocol)?;
        self.sync_detached_session_if_tracked(session_id)?;
        let summary = registry
            .read(session_id)
            .map_err(detached_registry_error_to_protocol)?
            .ok_or_else(|| {
                protocol_error(
                    "session_not_found",
                    format!(
                        "daemon registry has no detached session for {}",
                        session_id.as_str()
                    ),
                )
            })?;
        let turn_control = self.turn_control.inspect_session_turns(session_id)?;
        let session = session_snapshot_from_runtime(&self.runtime, session_id)?;
        Ok(InspectDetachedSessionResponse {
            summary,
            session,
            turn_control,
        })
    }
}

#[derive(Clone, Copy)]
enum ServerOwnershipMode {
    ForegroundStdio,
    DetachedDaemon,
}

impl ServerOwnershipMode {
    fn is_daemon(self) -> bool {
        matches!(self, Self::DetachedDaemon)
    }
}

impl TurnControlPlane {
    fn new(runtime: ProbeRuntime, detached_registry: Option<Arc<DetachedSessionRegistry>>) -> Self {
        Self {
            runtime,
            coordination: Arc::new(Mutex::new(HashSet::new())),
            detached_registry,
        }
    }

    fn save_state_and_sync(
        &self,
        session_id: &SessionId,
        state: &SessionTurnControlState,
    ) -> Result<(), RuntimeProtocolError> {
        state
            .save(&self.runtime, session_id)
            .map_err(session_store_error_to_protocol)?;
        self.sync_detached_session_summary(session_id, state)
    }

    fn sync_detached_session_summary(
        &self,
        session_id: &SessionId,
        state: &SessionTurnControlState,
    ) -> Result<(), RuntimeProtocolError> {
        let Some(registry) = self.detached_registry.as_ref() else {
            return Ok(());
        };
        let metadata = self
            .runtime
            .session_store()
            .read_metadata(session_id)
            .map_err(session_store_error_to_protocol)?;
        let pending_approval_count = self
            .runtime
            .pending_tool_approvals(session_id)
            .map_err(runtime_error_to_protocol)?
            .len();
        let previous = registry
            .read(session_id)
            .map_err(detached_registry_error_to_protocol)?;
        let summary = detached_session_summary_from_state(
            &metadata,
            state,
            pending_approval_count,
            previous.as_ref(),
            now_ms(),
        );
        registry
            .upsert(summary)
            .map_err(detached_registry_error_to_protocol)
    }

    fn sync_detached_session_if_tracked(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeProtocolError> {
        let Some(registry) = self.detached_registry.as_ref() else {
            return Ok(());
        };
        if registry
            .read(session_id)
            .map_err(detached_registry_error_to_protocol)?
            .is_none()
        {
            return Ok(());
        }
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let state = self.load_state_locked(session_id.as_str(), &mut coordination)?;
        self.sync_detached_session_summary(session_id, &state)
    }

    fn reserve_direct_turn(
        &self,
        request: &TurnRequest,
        mode: TurnMode,
    ) -> Result<String, RuntimeProtocolError> {
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let mut state = self.load_state_locked(request.session_id.as_str(), &mut coordination)?;
        if state.unfinished_turn_count() != 0 {
            return Err(protocol_error(
                "session_busy",
                format!(
                    "session {} already has active or queued work; use queue_turn to submit follow-up prompts",
                    request.session_id.as_str()
                ),
            ));
        }
        let requested_at_ms = now_ms();
        let turn_id = state
            .push_turn(
                &request.session_id,
                mode.submission_kind(),
                QueuedTurnStatus::Running,
                request,
                requested_at_ms,
                Some(requested_at_ms),
            )
            .turn_id;
        self.save_state_and_sync(&request.session_id, &state)?;
        coordination.insert(String::from(request.session_id.as_str()));
        Ok(turn_id)
    }

    fn reserve_queue_turn(
        &self,
        request: &TurnRequest,
    ) -> Result<QueuedTurnReservation, RuntimeProtocolError> {
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let mut state = self.load_state_locked(request.session_id.as_str(), &mut coordination)?;
        let should_start = state.unfinished_turn_count() == 0
            && !coordination.contains(request.session_id.as_str());
        let requested_at_ms = now_ms();
        let status = if should_start {
            QueuedTurnStatus::Running
        } else {
            QueuedTurnStatus::Queued
        };
        let turn_id = state
            .push_turn(
                &request.session_id,
                TurnSubmissionKind::Continue,
                status,
                request,
                requested_at_ms,
                should_start.then_some(requested_at_ms),
            )
            .turn_id;
        let response = QueueTurnResponse {
            turn: state
                .view_for(turn_id.as_str())
                .expect("queued turn should be visible after insert"),
        };
        self.save_state_and_sync(&request.session_id, &state)?;
        if should_start {
            coordination.insert(String::from(request.session_id.as_str()));
        }
        Ok(QueuedTurnReservation {
            response,
            turn_id,
            should_start,
        })
    }

    fn reserve_pending_approval_resolution(
        &self,
        request: &probe_protocol::runtime::ResolvePendingApprovalRequest,
    ) -> Result<String, RuntimeProtocolError> {
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let mut state = self.load_state_locked(request.session_id.as_str(), &mut coordination)?;
        let Some(active_turn) = state.active_turn_mut() else {
            return Err(protocol_error(
                "not_running",
                format!(
                    "session {} does not have an active turn waiting for approval",
                    request.session_id.as_str()
                ),
            ));
        };
        if !active_turn.record.awaiting_approval {
            return Err(protocol_error(
                "approval_not_pending",
                format!(
                    "session {} is not currently paused on a pending approval",
                    request.session_id.as_str()
                ),
            ));
        }
        active_turn.record.awaiting_approval = false;
        let turn_id = active_turn.record.turn_id.clone();
        self.save_state_and_sync(&request.session_id, &state)?;
        coordination.insert(String::from(request.session_id.as_str()));
        Ok(turn_id)
    }

    fn inspect_session_turns(
        &self,
        session_id: &SessionId,
    ) -> Result<InspectSessionTurnsResponse, RuntimeProtocolError> {
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let state = self.load_state_locked(session_id.as_str(), &mut coordination)?;
        Ok(state.inspect_view(session_id))
    }

    fn cancel_queued_turn(
        &self,
        request: CancelQueuedTurnRequest,
    ) -> Result<CancelQueuedTurnResponse, RuntimeProtocolError> {
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let mut state = self.load_state_locked(request.session_id.as_str(), &mut coordination)?;
        let Some(turn) = state.queued_turn_mut(request.turn_id.as_str()) else {
            let reason_code = if state
                .turns
                .iter()
                .any(|turn| turn.record.turn_id == request.turn_id)
            {
                Some(String::from("not_queued"))
            } else {
                Some(String::from("not_found"))
            };
            return Ok(CancelQueuedTurnResponse {
                session_id: request.session_id,
                turn_id: request.turn_id,
                cancelled: false,
                reason_code,
                message: String::from("turn is not a queued turn that can be cancelled"),
            });
        };
        let turn_note = queued_turn_note(turn);
        let turn_id = turn.record.turn_id.clone();
        turn.record.status = QueuedTurnStatus::Cancelled;
        turn.record.finished_at_ms = Some(now_ms());
        turn.record.cancellation_reason = Some(String::from("cancelled before execution"));
        self.runtime
            .session_store()
            .append_turn(
                &request.session_id,
                &[NewItem::new(
                    probe_protocol::session::TranscriptItemKind::Note,
                    turn_note,
                )],
            )
            .map_err(session_store_error_to_protocol)?;
        self.save_state_and_sync(&request.session_id, &state)?;
        Ok(CancelQueuedTurnResponse {
            session_id: request.session_id,
            turn_id,
            cancelled: true,
            reason_code: None,
            message: String::from("queued turn cancelled before execution"),
        })
    }

    fn interrupt_turn(
        &self,
        session_id: SessionId,
    ) -> Result<InterruptOutcome, RuntimeProtocolError> {
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let mut state = match self.load_state_locked(session_id.as_str(), &mut coordination) {
            Ok(state) => state,
            Err(error) if error.code == "session_not_found" => {
                return Ok(InterruptOutcome {
                    response: InterruptTurnResponse {
                        session_id,
                        turn_id: None,
                        interrupted: false,
                        reason_code: Some(String::from("not_running")),
                        message: String::from("session is not currently running a turn"),
                    },
                    should_start_next: false,
                });
            }
            Err(error) => return Err(error),
        };
        let Some(active_turn) = state.active_turn_mut() else {
            return Ok(InterruptOutcome {
                response: InterruptTurnResponse {
                    session_id,
                    turn_id: None,
                    interrupted: false,
                    reason_code: Some(String::from("not_running")),
                    message: String::from("session is not currently running a turn"),
                },
                should_start_next: false,
            });
        };
        let turn_id = active_turn.record.turn_id.clone();
        if coordination.contains(session_id.as_str()) {
            return Ok(InterruptOutcome {
                response: InterruptTurnResponse {
                    session_id,
                    turn_id: Some(turn_id),
                    interrupted: false,
                    reason_code: Some(String::from("unsupported")),
                    message: String::from(
                        "probe-server still cannot cooperatively interrupt an in-flight runtime turn",
                    ),
                },
                should_start_next: false,
            });
        }
        if !active_turn.record.awaiting_approval {
            return Ok(InterruptOutcome {
                response: InterruptTurnResponse {
                    session_id,
                    turn_id: Some(turn_id),
                    interrupted: false,
                    reason_code: Some(String::from("not_interruptible")),
                    message: String::from(
                        "the active turn is not paused on approval, so there is nothing honest to interrupt yet",
                    ),
                },
                should_start_next: false,
            });
        }

        let pending_approvals = self
            .runtime
            .pending_tool_approvals(&session_id)
            .map_err(runtime_error_to_protocol)?;
        for approval in &pending_approvals {
            self.runtime
                .session_store()
                .resolve_pending_tool_approval(
                    &session_id,
                    approval.tool_call_id.as_str(),
                    probe_protocol::session::ToolApprovalResolution::Rejected,
                )
                .map_err(session_store_error_to_protocol)?;
        }
        let note = interrupted_turn_note(active_turn, pending_approvals.len());
        self.runtime
            .session_store()
            .append_turn(
                &session_id,
                &[NewItem::new(
                    probe_protocol::session::TranscriptItemKind::Note,
                    note,
                )],
            )
            .map_err(session_store_error_to_protocol)?;
        active_turn.record.status = QueuedTurnStatus::Cancelled;
        active_turn.record.awaiting_approval = false;
        active_turn.record.finished_at_ms = Some(now_ms());
        active_turn.record.cancellation_reason =
            Some(String::from("interrupted while waiting for tool approval"));
        let should_start_next = state.queued_turn_count() > 0;
        self.save_state_and_sync(&session_id, &state)?;
        Ok(InterruptOutcome {
            response: InterruptTurnResponse {
                session_id,
                turn_id: Some(turn_id),
                interrupted: true,
                reason_code: None,
                message: String::from(
                    "cancelled the approval-paused turn, rejected its pending approvals, and preserved the interruption in the transcript",
                ),
            },
            should_start_next,
        })
    }

    fn mark_turn_completed(
        &self,
        session_id: &SessionId,
        turn_id: &str,
    ) -> Result<bool, RuntimeProtocolError> {
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let mut state = self.load_state_locked(session_id.as_str(), &mut coordination)?;
        if let Some(turn) = state.turn_by_id_mut(turn_id) {
            turn.record.status = QueuedTurnStatus::Completed;
            turn.record.awaiting_approval = false;
            turn.record.finished_at_ms = Some(now_ms());
            turn.record.failure_message = None;
            turn.record.cancellation_reason = None;
        }
        coordination.remove(session_id.as_str());
        let should_start_next = state.queued_turn_count() > 0;
        self.save_state_and_sync(session_id, &state)?;
        Ok(should_start_next)
    }

    fn mark_turn_paused(
        &self,
        session_id: &SessionId,
        turn_id: &str,
    ) -> Result<(), RuntimeProtocolError> {
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let mut state = self.load_state_locked(session_id.as_str(), &mut coordination)?;
        if let Some(turn) = state.turn_by_id_mut(turn_id) {
            turn.record.status = QueuedTurnStatus::Running;
            turn.record.awaiting_approval = true;
            turn.record.finished_at_ms = None;
        }
        coordination.remove(session_id.as_str());
        self.save_state_and_sync(session_id, &state)
    }

    fn restore_paused_turn(
        &self,
        session_id: &SessionId,
        turn_id: &str,
    ) -> Result<(), RuntimeProtocolError> {
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let mut state = self.load_state_locked(session_id.as_str(), &mut coordination)?;
        if let Some(turn) = state.turn_by_id_mut(turn_id) {
            turn.record.status = QueuedTurnStatus::Running;
            turn.record.awaiting_approval = true;
        }
        coordination.remove(session_id.as_str());
        self.save_state_and_sync(session_id, &state)
    }

    fn mark_turn_failed(
        &self,
        session_id: &SessionId,
        turn_id: &str,
        message: String,
    ) -> Result<bool, RuntimeProtocolError> {
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let mut state = self.load_state_locked(session_id.as_str(), &mut coordination)?;
        if let Some(turn) = state.turn_by_id_mut(turn_id) {
            turn.record.status = QueuedTurnStatus::Failed;
            turn.record.awaiting_approval = false;
            turn.record.finished_at_ms = Some(now_ms());
            turn.record.failure_message = Some(message);
            turn.record.cancellation_reason = None;
        }
        coordination.remove(session_id.as_str());
        let should_start_next = state.queued_turn_count() > 0;
        self.save_state_and_sync(session_id, &state)?;
        Ok(should_start_next)
    }

    fn maybe_start_next_queued_turn(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<BackgroundTurnWorkItem>, RuntimeProtocolError> {
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let mut state = self.load_state_locked(session_id.as_str(), &mut coordination)?;
        if state.active_turn().is_some() || coordination.contains(session_id.as_str()) {
            return Ok(None);
        }
        let Some(next_turn) = state.next_queued_turn() else {
            return Ok(None);
        };
        let turn_id = next_turn.record.turn_id.clone();
        if let Some(turn) = state.turn_by_id_mut(turn_id.as_str()) {
            turn.record.status = QueuedTurnStatus::Running;
            turn.record.started_at_ms = Some(now_ms());
            turn.record.awaiting_approval = false;
            turn.record.failure_message = None;
            turn.record.cancellation_reason = None;
        }
        self.save_state_and_sync(session_id, &state)?;
        coordination.insert(String::from(session_id.as_str()));
        Ok(Some(BackgroundTurnWorkItem {
            turn_id,
            request: next_turn.to_turn_request(),
            mode: TurnMode::Continue,
        }))
    }

    fn unfinished_turn_count(&self) -> Result<usize, RuntimeProtocolError> {
        let sessions = self
            .runtime
            .session_store()
            .list_sessions()
            .map_err(session_store_error_to_protocol)?;
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let mut active_turns = 0usize;
        for session in sessions {
            let state = self.load_state_locked(session.id.as_str(), &mut coordination)?;
            active_turns += state.unfinished_turn_count();
        }
        Ok(active_turns)
    }

    fn load_state_locked(
        &self,
        session_id: &str,
        coordination: &mut HashSet<String>,
    ) -> Result<SessionTurnControlState, RuntimeProtocolError> {
        let session_id = SessionId::new(session_id);
        let mut state = SessionTurnControlState::load(&self.runtime, &session_id)
            .map_err(session_store_error_to_protocol)?;
        let approvals_pending = !self
            .runtime
            .pending_tool_approvals(&session_id)
            .map_err(runtime_error_to_protocol)?
            .is_empty();
        if !coordination.contains(session_id.as_str())
            && state.recover_orphaned_running_turns(approvals_pending, now_ms())
        {
            self.save_state_and_sync(&session_id, &state)?;
        }
        Ok(state)
    }
}

impl ProbeServerConnection {
    fn new(core: ProbeServerCore, writer: SharedJsonlWriter, transport: TransportKind) -> Self {
        Self {
            core,
            writer,
            transport,
        }
    }

    fn run(&self, reader: impl BufRead) -> Result<ConnectionRunOutcome, ServerError> {
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
            if matches!(
                self.handle_request(envelope)?,
                RequestHandlingOutcome::ShutdownAccepted
            ) {
                return Ok(ConnectionRunOutcome::ServerShutdown);
            }
        }
        Ok(ConnectionRunOutcome::ClientDisconnected)
    }

    fn handle_request(
        &self,
        envelope: RequestEnvelope,
    ) -> Result<RequestHandlingOutcome, ServerError> {
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
                                transport: self.transport,
                                supports_stdio_child_process: matches!(
                                    self.transport,
                                    TransportKind::StdioJsonl
                                ),
                                supports_local_daemon_socket: matches!(
                                    self.transport,
                                    TransportKind::UnixSocketJsonl
                                ),
                                supports_session_resume: true,
                                supports_session_inspect: true,
                                supports_pending_approval_resolution: true,
                                supports_interrupt_requests: true,
                                supports_queued_turns: true,
                                supports_detached_session_registry: self
                                    .core
                                    .detached_registry
                                    .is_some(),
                            },
                        }),
                    )?;
                }
                Ok(RequestHandlingOutcome::Continue)
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
                Ok(RequestHandlingOutcome::Continue)
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
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::ListSessions => {
                match self.core.runtime.session_store().list_sessions() {
                    Ok(sessions) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::ListSessions(ListSessionsResponse { sessions }),
                    )?,
                    Err(error) => self.writer.send_response_error(
                        request_id.as_str(),
                        session_store_error_to_protocol(error),
                    )?,
                }
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::ListDetachedSessions => {
                match self.core.list_detached_sessions() {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::ListDetachedSessions(response),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(RequestHandlingOutcome::Continue)
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
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::InspectDetachedSession(SessionLookupRequest { session_id }) => {
                match self.core.inspect_detached_session(&session_id) {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::InspectDetachedSession(response),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::StartTurn(request) => {
                self.spawn_turn_request(request_id, request, TurnMode::Start)?;
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::ContinueTurn(request) => {
                self.spawn_turn_request(request_id, request, TurnMode::Continue)?;
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::QueueTurn(request) => {
                match self.queue_turn(request) {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::QueueTurn(response),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::InspectSessionTurns(SessionLookupRequest { session_id }) => {
                match self.inspect_session_turns(&session_id) {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::InspectSessionTurns(response),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::InterruptTurn(request) => {
                match self.interrupt_turn(request.session_id) {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::InterruptTurn(response),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::CancelQueuedTurn(request) => {
                match self.cancel_queued_turn(request) {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::CancelQueuedTurn(response),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(RequestHandlingOutcome::Continue)
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
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::ResolvePendingApproval(request) => {
                self.spawn_resolve_pending_approval(request_id, request)?;
                Ok(RequestHandlingOutcome::Continue)
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
                if accepted {
                    Ok(RequestHandlingOutcome::ShutdownAccepted)
                } else {
                    Ok(RequestHandlingOutcome::Continue)
                }
            }
        }
    }

    fn start_session(
        &self,
        request: StartSessionRequest,
    ) -> Result<SessionSnapshot, RuntimeProtocolError> {
        let session = self
            .core
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
        let snapshot = session_snapshot_from_runtime(&self.core.runtime, session_id)?;
        self.core
            .ensure_detached_session_registered(&snapshot.session)?;
        Ok(snapshot)
    }

    fn list_pending_approvals(
        &self,
        request: ListPendingApprovalsRequest,
    ) -> Result<Vec<probe_protocol::session::PendingToolApproval>, RuntimeProtocolError> {
        if let Some(session_id) = request.session_id {
            return self
                .core
                .runtime
                .pending_tool_approvals(&session_id)
                .map_err(runtime_error_to_protocol);
        }

        let mut approvals = Vec::new();
        let sessions = self
            .core
            .runtime
            .session_store()
            .list_sessions()
            .map_err(session_store_error_to_protocol)?;
        for session in sessions {
            approvals.extend(
                self.core
                    .runtime
                    .clone()
                    .pending_tool_approvals(&session.id)
                    .map_err(runtime_error_to_protocol)?,
            );
        }
        approvals.sort_by(|left, right| right.requested_at_ms.cmp(&left.requested_at_ms));
        Ok(approvals)
    }

    fn inspect_session_turns(
        &self,
        session_id: &SessionId,
    ) -> Result<InspectSessionTurnsResponse, RuntimeProtocolError> {
        self.core.sync_detached_session_if_tracked(session_id)?;
        self.core.turn_control.inspect_session_turns(session_id)
    }

    fn queue_turn(&self, request: TurnRequest) -> Result<QueueTurnResponse, RuntimeProtocolError> {
        self.core
            .ensure_detached_session_registered_by_id(&request.session_id)?;
        let reservation = self.core.turn_control.reserve_queue_turn(&request)?;
        if reservation.should_start {
            spawn_turn_worker(
                Arc::clone(&self.core.turn_control),
                self.core.runtime.clone(),
                self.writer.clone(),
                None,
                request,
                TurnMode::Continue,
                reservation.turn_id,
            )
            .map_err(|error| {
                protocol_error(
                    "turn_spawn_failed",
                    format!("failed to spawn queued turn: {error}"),
                )
            })?;
        }
        Ok(reservation.response)
    }

    fn interrupt_turn(
        &self,
        session_id: SessionId,
    ) -> Result<InterruptTurnResponse, RuntimeProtocolError> {
        self.core
            .ensure_detached_session_registered_by_id(&session_id)?;
        let outcome = self.core.turn_control.interrupt_turn(session_id.clone())?;
        if outcome.should_start_next {
            spawn_next_queued_turn_if_ready(
                Arc::clone(&self.core.turn_control),
                self.core.runtime.clone(),
                self.writer.clone(),
                session_id,
            );
        }
        Ok(outcome.response)
    }

    fn cancel_queued_turn(
        &self,
        request: CancelQueuedTurnRequest,
    ) -> Result<CancelQueuedTurnResponse, RuntimeProtocolError> {
        self.core
            .ensure_detached_session_registered_by_id(&request.session_id)?;
        self.core.turn_control.cancel_queued_turn(request)
    }

    fn spawn_turn_request(
        &self,
        request_id: String,
        request: TurnRequest,
        mode: TurnMode,
    ) -> Result<(), ServerError> {
        if let Err(error) = self
            .core
            .ensure_detached_session_registered_by_id(&request.session_id)
        {
            self.writer
                .send_response_error(request_id.as_str(), error)?;
            return Ok(());
        }
        let turn_id = match self.core.turn_control.reserve_direct_turn(&request, mode) {
            Ok(turn_id) => turn_id,
            Err(error) => {
                self.writer
                    .send_response_error(request_id.as_str(), error)?;
                return Ok(());
            }
        };

        if let Err(error) = spawn_turn_worker(
            Arc::clone(&self.core.turn_control),
            self.core.runtime.clone(),
            self.writer.clone(),
            Some(request_id.clone()),
            request.clone(),
            mode,
            turn_id.clone(),
        ) {
            let _ = self.core.turn_control.mark_turn_failed(
                &request.session_id,
                turn_id.as_str(),
                format!("failed to spawn turn worker: {error}"),
            );
            self.writer.send_response_error(
                request_id.as_str(),
                protocol_error(
                    "turn_spawn_failed",
                    format!("failed to spawn turn worker: {error}"),
                ),
            )?;
        }
        Ok(())
    }

    fn spawn_resolve_pending_approval(
        &self,
        request_id: String,
        request: probe_protocol::runtime::ResolvePendingApprovalRequest,
    ) -> Result<(), ServerError> {
        if let Err(error) = self
            .core
            .ensure_detached_session_registered_by_id(&request.session_id)
        {
            self.writer
                .send_response_error(request_id.as_str(), error)?;
            return Ok(());
        }
        let turn_id = match self
            .core
            .turn_control
            .reserve_pending_approval_resolution(&request)
        {
            Ok(turn_id) => turn_id,
            Err(error) => {
                self.writer
                    .send_response_error(request_id.as_str(), error)?;
                return Ok(());
            }
        };

        if let Err(error) = spawn_approval_resolution_worker(
            Arc::clone(&self.core.turn_control),
            self.core.runtime.clone(),
            self.writer.clone(),
            request_id.clone(),
            request.clone(),
            turn_id.clone(),
        ) {
            let _ = self
                .core
                .turn_control
                .restore_paused_turn(&request.session_id, turn_id.as_str());
            self.writer.send_response_error(
                request_id.as_str(),
                protocol_error(
                    "turn_spawn_failed",
                    format!("failed to spawn approval-resolution worker: {error}"),
                ),
            )?;
        }
        Ok(())
    }

    fn active_turn_count(&self) -> usize {
        self.core.turn_control.unfinished_turn_count().unwrap_or(1)
    }
}

#[derive(Clone, Copy)]
enum TurnMode {
    Start,
    Continue,
}

impl TurnMode {
    fn submission_kind(self) -> TurnSubmissionKind {
        match self {
            Self::Start => TurnSubmissionKind::Start,
            Self::Continue => TurnSubmissionKind::Continue,
        }
    }
}

fn spawn_turn_worker(
    turn_control: Arc<TurnControlPlane>,
    runtime: ProbeRuntime,
    writer: SharedJsonlWriter,
    request_id: Option<String>,
    request: TurnRequest,
    mode: TurnMode,
    turn_id: String,
) -> Result<(), ServerError> {
    let session_id = request.session_id.clone();
    let thread_name = format!("probe-server-turn-{}", session_id.as_str());
    thread::Builder::new().name(thread_name).spawn(move || {
        let response = run_turn_request(&runtime, &writer, request_id.as_deref(), request, mode);

        let should_start_next = match &response {
            Ok(RuntimeResponse::StartTurn(TurnResponse::Completed(_)))
            | Ok(RuntimeResponse::ContinueTurn(TurnResponse::Completed(_))) => turn_control
                .mark_turn_completed(&session_id, turn_id.as_str())
                .unwrap_or(false),
            Ok(RuntimeResponse::StartTurn(TurnResponse::Paused(_)))
            | Ok(RuntimeResponse::ContinueTurn(TurnResponse::Paused(_))) => {
                let _ = turn_control.mark_turn_paused(&session_id, turn_id.as_str());
                false
            }
            Ok(other) => {
                let _ = turn_control.mark_turn_failed(
                    &session_id,
                    turn_id.as_str(),
                    format!("unexpected turn response shape: {other:?}"),
                );
                false
            }
            Err(error) => turn_control
                .mark_turn_failed(&session_id, turn_id.as_str(), error.message.clone())
                .unwrap_or(false),
        };

        if let Some(request_id) = request_id.as_deref() {
            match response {
                Ok(response) => {
                    let _ = writer.send_response_ok(request_id, response);
                }
                Err(error) => {
                    let _ = writer.send_response_error(request_id, error);
                }
            }
        }

        if should_start_next {
            spawn_next_queued_turn_if_ready(turn_control, runtime, writer, session_id);
        }
    })?;
    Ok(())
}

fn spawn_approval_resolution_worker(
    turn_control: Arc<TurnControlPlane>,
    runtime: ProbeRuntime,
    writer: SharedJsonlWriter,
    request_id: String,
    request: probe_protocol::runtime::ResolvePendingApprovalRequest,
    turn_id: String,
) -> Result<(), ServerError> {
    let session_id = request.session_id.clone();
    let thread_name = format!("probe-server-approval-{}", session_id.as_str());
    thread::Builder::new().name(thread_name).spawn(move || {
        let response =
            run_pending_approval_resolution(&runtime, &writer, Some(request_id.as_str()), request);

        let should_start_next = match &response {
            Ok(RuntimeResponse::ResolvePendingApproval(
                ResolvePendingApprovalResponse::StillPending { .. },
            )) => {
                let _ = turn_control.mark_turn_paused(&session_id, turn_id.as_str());
                false
            }
            Ok(RuntimeResponse::ResolvePendingApproval(
                ResolvePendingApprovalResponse::Resumed(_),
            )) => turn_control
                .mark_turn_completed(&session_id, turn_id.as_str())
                .unwrap_or(false),
            Ok(other) => {
                let _ = turn_control.mark_turn_failed(
                    &session_id,
                    turn_id.as_str(),
                    format!("unexpected approval response shape: {other:?}"),
                );
                false
            }
            Err(error) if approval_error_keeps_turn_paused(error) => {
                let _ = turn_control.restore_paused_turn(&session_id, turn_id.as_str());
                false
            }
            Err(error) => turn_control
                .mark_turn_failed(&session_id, turn_id.as_str(), error.message.clone())
                .unwrap_or(false),
        };

        match response {
            Ok(response) => {
                let _ = writer.send_response_ok(request_id.as_str(), response);
            }
            Err(error) => {
                let _ = writer.send_response_error(request_id.as_str(), error);
            }
        }

        if should_start_next {
            spawn_next_queued_turn_if_ready(turn_control, runtime, writer, session_id);
        }
    })?;
    Ok(())
}

fn spawn_next_queued_turn_if_ready(
    turn_control: Arc<TurnControlPlane>,
    runtime: ProbeRuntime,
    writer: SharedJsonlWriter,
    session_id: SessionId,
) {
    let work_item = match turn_control.maybe_start_next_queued_turn(&session_id) {
        Ok(work_item) => work_item,
        Err(_) => None,
    };
    if let Some(work_item) = work_item {
        if let Err(error) = spawn_turn_worker(
            Arc::clone(&turn_control),
            runtime,
            writer,
            None,
            work_item.request,
            work_item.mode,
            work_item.turn_id.clone(),
        ) {
            let _ = turn_control.mark_turn_failed(
                &session_id,
                work_item.turn_id.as_str(),
                format!("failed to spawn queued turn worker: {error}"),
            );
        }
    }
}

fn approval_error_keeps_turn_paused(error: &RuntimeProtocolError) -> bool {
    matches!(
        error.code.as_str(),
        "approval_not_found" | "approval_already_resolved"
    )
}

fn queued_turn_note(turn: &StoredTurnControlRecord) -> String {
    format!(
        "probe-server cancelled queued turn {} from {} before execution: {}",
        turn.record.turn_id,
        render_turn_author(&turn.record.author),
        render_prompt_excerpt(turn.record.prompt.as_str()),
    )
}

fn interrupted_turn_note(turn: &StoredTurnControlRecord, pending_approvals: usize) -> String {
    format!(
        "probe-server interrupted approval-paused turn {} from {} and rejected {} pending approval(s): {}",
        turn.record.turn_id,
        render_turn_author(&turn.record.author),
        pending_approvals,
        render_prompt_excerpt(turn.record.prompt.as_str()),
    )
}

fn render_turn_author(author: &probe_protocol::runtime::TurnAuthor) -> String {
    author
        .display_name
        .clone()
        .unwrap_or_else(|| match author.client_version.as_deref() {
            Some(version) => format!("{} {}", author.client_name, version),
            None => author.client_name.clone(),
        })
}

fn render_prompt_excerpt(prompt: &str) -> String {
    const MAX_CHARS: usize = 96;
    let mut excerpt = prompt.trim().replace('\n', " ");
    if excerpt.chars().count() <= MAX_CHARS {
        return format!("\"{excerpt}\"");
    }
    excerpt = excerpt.chars().take(MAX_CHARS - 1).collect::<String>();
    format!("\"{}...\"", excerpt.trim_end())
}

fn run_turn_request(
    runtime: &ProbeRuntime,
    writer: &SharedJsonlWriter,
    request_id: Option<&str>,
    request: TurnRequest,
    mode: TurnMode,
) -> Result<RuntimeResponse, RuntimeProtocolError> {
    let tool_loop = request.tool_loop.map(tool_loop_from_recipe).transpose()?;
    let event_sink = request_id.map(|request_id| {
        let writer_for_events = writer.clone();
        let request_id_for_events = String::from(request_id);
        Arc::new(move |event| {
            let delivery = delivery_for_runtime_event(&event);
            let encoded = encode_runtime_event(event);
            let _ = writer_for_events.send_event(
                request_id_for_events.as_str(),
                ServerEvent::RuntimeProgress {
                    delivery,
                    event: encoded,
                },
            );
        }) as Arc<dyn RuntimeEventSink>
    });

    let resume_request = PlainTextResumeRequest {
        session_id: request.session_id.clone(),
        profile: request.profile,
        prompt: request.prompt,
        tool_loop,
    };
    let result = match event_sink {
        Some(event_sink) => {
            runtime.continue_plain_text_session_with_events(resume_request, event_sink)
        }
        None => runtime.continue_plain_text_session(resume_request),
    };

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
            if let Some(request_id) = request_id {
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
            }
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
    request_id: Option<&str>,
    request: probe_protocol::runtime::ResolvePendingApprovalRequest,
) -> Result<RuntimeResponse, RuntimeProtocolError> {
    let tool_loop = tool_loop_from_recipe(request.tool_loop)?;
    let event_sink = request_id.map(|request_id| {
        let writer_for_events = writer.clone();
        let request_id_for_events = String::from(request_id);
        Arc::new(move |event| {
            let delivery = delivery_for_runtime_event(&event);
            let encoded = encode_runtime_event(event);
            let _ = writer_for_events.send_event(
                request_id_for_events.as_str(),
                ServerEvent::RuntimeProgress {
                    delivery,
                    event: encoded,
                },
            );
        }) as Arc<dyn RuntimeEventSink>
    });

    let approval_request = ResolvePendingToolApprovalRequest {
        session_id: request.session_id.clone(),
        profile: request.profile,
        tool_loop,
        call_id: request.call_id,
        resolution: request.resolution,
    };
    let result = match event_sink {
        Some(event_sink) => {
            runtime.resolve_pending_tool_approval_with_events(approval_request, event_sink)
        }
        None => runtime.resolve_pending_tool_approval(approval_request),
    };

    match result.map_err(runtime_error_to_protocol)? {
        ResolvePendingToolApprovalOutcome::StillPending {
            session,
            pending_approvals,
        } => {
            if let Some(request_id) = request_id {
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
            }
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

fn session_snapshot_from_runtime(
    runtime: &ProbeRuntime,
    session_id: &SessionId,
) -> Result<SessionSnapshot, RuntimeProtocolError> {
    let session = runtime
        .session_store()
        .read_metadata(session_id)
        .map_err(session_store_error_to_protocol)?;
    let transcript = runtime
        .session_store()
        .read_transcript(session_id)
        .map_err(session_store_error_to_protocol)?;
    let pending_approvals = runtime
        .pending_tool_approvals(session_id)
        .map_err(runtime_error_to_protocol)?;
    Ok(SessionSnapshot {
        session,
        transcript,
        pending_approvals,
    })
}

fn detached_session_summary_from_state(
    metadata: &SessionMetadata,
    state: &SessionTurnControlState,
    pending_approval_count: usize,
    previous: Option<&DetachedSessionSummary>,
    now_ms: u64,
) -> DetachedSessionSummary {
    let view = state.inspect_view(&metadata.id);
    let active_turn = view.active_turn.as_ref();
    let last_terminal_turn = view.recent_turns.first();
    let (status, recovery_state, recovery_note) = if let Some(active_turn) = active_turn {
        if active_turn.awaiting_approval {
            (
                DetachedSessionStatus::ApprovalPaused,
                DetachedSessionRecoveryState::ApprovalPausedResumable,
                Some(String::from(
                    "daemon restart can resume this session after the pending approval is resolved",
                )),
            )
        } else {
            (
                DetachedSessionStatus::Running,
                DetachedSessionRecoveryState::Clean,
                None,
            )
        }
    } else if !view.queued_turns.is_empty() {
        (
            DetachedSessionStatus::Queued,
            DetachedSessionRecoveryState::Clean,
            None,
        )
    } else if let Some(last_terminal_turn) = last_terminal_turn {
        let status = match last_terminal_turn.status {
            QueuedTurnStatus::Completed => DetachedSessionStatus::Completed,
            QueuedTurnStatus::Failed => DetachedSessionStatus::Failed,
            QueuedTurnStatus::Cancelled => DetachedSessionStatus::Cancelled,
            QueuedTurnStatus::Queued | QueuedTurnStatus::Running => DetachedSessionStatus::Idle,
        };
        let recovery_state = if last_terminal_turn
            .failure_message
            .as_deref()
            .is_some_and(|message| message.contains("restarted before this running turn completed"))
        {
            DetachedSessionRecoveryState::RunningTurnFailedOnRestart
        } else {
            DetachedSessionRecoveryState::Clean
        };
        let recovery_note = if matches!(
            recovery_state,
            DetachedSessionRecoveryState::RunningTurnFailedOnRestart
        ) {
            last_terminal_turn.failure_message.clone()
        } else {
            None
        };
        (status, recovery_state, recovery_note)
    } else {
        (
            DetachedSessionStatus::Idle,
            DetachedSessionRecoveryState::Clean,
            None,
        )
    };

    DetachedSessionSummary {
        session_id: metadata.id.clone(),
        title: metadata.title.clone(),
        cwd: metadata.cwd.clone(),
        status,
        active_turn_id: active_turn.map(|turn| turn.turn_id.clone()),
        queued_turn_count: view.queued_turns.len(),
        pending_approval_count,
        last_terminal_turn_id: last_terminal_turn.map(|turn| turn.turn_id.clone()),
        last_terminal_status: last_terminal_turn.map(|turn| turn.status),
        registered_at_ms: previous
            .map(|summary| summary.registered_at_ms)
            .unwrap_or(now_ms),
        updated_at_ms: now_ms,
        recovery_state,
        recovery_note,
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

fn runtime_protocol_error_to_io(error: RuntimeProtocolError) -> io::Error {
    io::Error::other(format!("{} ({})", error.message, error.code))
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

fn detached_registry_error_to_protocol(error: DetachedRegistryError) -> RuntimeProtocolError {
    protocol_error("detached_registry_error", error.to_string())
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
        ProbeServerCore, ProbeToolChoice, ToolLoopRecipe, ToolSetKind, approval_from_recipe,
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
        let _server = ProbeServerCore::new(runtime);
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
            reasoning_level: None,
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
