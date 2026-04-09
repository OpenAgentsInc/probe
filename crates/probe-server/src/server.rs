use std::collections::{BTreeMap, HashSet};
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use probe_core::runtime::{
    PlainTextResumeRequest, ProbeRuntime, ResolvePendingToolApprovalOutcome,
    ResolvePendingToolApprovalRequest, RuntimeError, RuntimeEvent, RuntimeEventSink,
    default_probe_home, derived_runtime_activity, is_validation_command,
};
use probe_core::session_store::{NewItem, NewSession, SessionStoreError};
use probe_core::session_summary_artifacts::{
    SessionSummaryArtifactError, refresh_session_summary_artifacts,
};
use probe_core::tools::{
    ExecutedToolCall, ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction as CoreDeniedAction,
    ToolLongContextConfig, ToolLoopConfig, ToolOracleConfig, ToolRegistry,
};
use probe_protocol::default_local_daemon_socket_path;
use probe_protocol::runtime::{
    AttachSessionParticipantRequest, AttachSessionParticipantResponse, CancelQueuedTurnRequest,
    CancelQueuedTurnResponse, ClientMessage, DetachedSessionEventPayload,
    DetachedSessionEventTruth, DetachedSessionRecoveryState, DetachedSessionStatus,
    DetachedSessionSummary, EventDeliveryGuarantee, EventEnvelope, InitializeResponse,
    InspectDetachedSessionResponse, InspectSessionMeshCoordinationRequest,
    InspectSessionMeshCoordinationResponse, InspectSessionMeshPluginOffersRequest,
    InspectSessionMeshPluginOffersResponse, InspectSessionTurnsResponse, InterruptTurnResponse,
    ListDetachedSessionsResponse, ListPendingApprovalsRequest, ListPendingApprovalsResponse,
    ListSessionsResponse, PostSessionMeshCoordinationRequest, PostSessionMeshCoordinationResponse,
    PublishSessionMeshPluginOfferRequest, PublishSessionMeshPluginOfferResponse, QueueTurnResponse,
    QueuedTurnStatus, ReadDetachedSessionLogRequest, ReadDetachedSessionLogResponse,
    RequestEnvelope, ResolvePendingApprovalResponse, ResponseBody, ResponseEnvelope,
    RevertLastTaskResponse, RuntimeActivity, RuntimeCapabilities, RuntimeProgressEvent,
    RuntimeProtocolError, RuntimeRequest, RuntimeResponse, RuntimeToolCallDelta, RuntimeUsage,
    ServerEvent, ServerMessage, SessionLookupRequest, SessionSnapshot, ShutdownResponse,
    SpawnChildSessionRequest, SpawnChildSessionResponse, StartSessionRequest, ToolApprovalRecipe,
    ToolCallResult, ToolChoice, ToolDeniedAction, ToolLongContextRecipe, ToolLoopRecipe,
    ToolOracleRecipe, ToolSetKind, TransportKind, TurnAuthor, TurnCompleted, TurnPaused,
    TurnRequest, TurnResponse, TurnSubmissionKind, UpdateSessionControllerRequest,
    UpdateSessionControllerResponse, WatchDetachedSessionRequest, WatchDetachedSessionResponse,
};
use probe_protocol::session::{
    SessionAttachTransport, SessionBackendTarget, SessionBranchState, SessionChildClosureSummary,
    SessionChildLink, SessionChildStatus, SessionChildSummary, SessionControllerAction,
    SessionControllerLease, SessionDeliveryArtifact, SessionDeliveryState, SessionDeliveryStatus,
    SessionExecutionHost, SessionExecutionHostKind, SessionHostedAuthKind,
    SessionHostedAuthReceipt, SessionHostedCheckoutKind, SessionHostedCheckoutReceipt,
    SessionHostedCleanupReceipt, SessionHostedCleanupStatus, SessionHostedCostReceipt,
    SessionHostedLifecycleEvent, SessionHostedReceipts, SessionHostedWorkerReceipt, SessionId,
    SessionInitiator, SessionMcpConnectionStatus, SessionMcpServer, SessionMcpServerSource,
    SessionMcpServerTransport, SessionMcpState, SessionMcpTool, SessionMeshCoordinationEntry,
    SessionMeshCoordinationKind, SessionMeshCoordinationMode, SessionMeshCoordinationStatus,
    SessionMeshCoordinationVisibility, SessionMeshPluginOffer, SessionMeshPluginTool,
    SessionMetadata, SessionMountKind, SessionMountRef, SessionParentLink, SessionParticipant,
    SessionPreparedBaselineRef, SessionPreparedBaselineStatus, SessionRuntimeOwner,
    SessionRuntimeOwnerKind, SessionSummaryArtifact, SessionSummaryArtifactRef,
    SessionWorkspaceBootMode, SessionWorkspaceSnapshotRef, SessionWorkspaceState,
    TaskCheckpointStatus, TaskCheckpointSummary, TaskDiffPreview, TaskFinalReceipt,
    TaskReceiptDisposition, TaskRevertibilityStatus, TaskRevertibilitySummary,
    TaskVerificationCommandStatus, TaskVerificationCommandSummary, TaskVerificationStatus,
    TaskWorkspaceSummary, TaskWorkspaceSummaryStatus, ToolPolicyDecision, ToolRiskClass,
    TranscriptEvent, TranscriptItem, TranscriptItemKind, UsageMeasurement, UsageTruth,
};
use probe_protocol::{PROBE_PROTOCOL_VERSION, PROBE_RUNTIME_NAME};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::detached_events::{DetachedEventError, DetachedSessionEventHub};
use crate::detached_registry::{DetachedRegistryError, DetachedSessionRegistry};
use crate::detached_watchdog::{
    DetachedTurnWatchdogPolicy, DetachedTurnWatchdogTrigger, evaluate_detached_turn_watchdog,
};
use crate::turn_control::{SessionTurnControlState, StoredTurnControlRecord, now_ms};

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;

const MCP_REGISTRY_RELATIVE_PATH: &str = "mcp/servers.json";
const MCP_STDIO_STARTUP_TIMEOUT: Duration = Duration::from_millis(1200);

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
struct StoredMcpRegistryFile {
    #[serde(default)]
    servers: Vec<StoredMcpServerRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredMcpServerSource {
    ManualLaunch,
    ProviderCommandRecipe,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredMcpServerTransport {
    Stdio,
    Http,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct StoredMcpServerRecord {
    id: String,
    name: String,
    enabled: bool,
    #[serde(default = "default_stored_mcp_source")]
    source: StoredMcpServerSource,
    #[serde(default)]
    transport: Option<StoredMcpServerTransport>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    provider_setup_command: Option<String>,
    #[serde(default)]
    provider_hint: Option<String>,
    #[serde(default)]
    client_hint: Option<String>,
}

fn default_stored_mcp_source() -> StoredMcpServerSource {
    StoredMcpServerSource::ManualLaunch
}
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedApiServerConfig {
    pub owner_id: String,
    pub display_name: Option<String>,
    pub attach_target: Option<String>,
    pub auth_authority: Option<String>,
    pub auth_subject: Option<String>,
    pub auth_kind: SessionHostedAuthKind,
    pub auth_scope: Option<String>,
}

impl HostedApiServerConfig {
    #[must_use]
    pub fn new(owner_id: impl Into<String>) -> Self {
        Self {
            owner_id: owner_id.into(),
            display_name: None,
            attach_target: None,
            auth_authority: None,
            auth_subject: None,
            auth_kind: SessionHostedAuthKind::ControlPlaneAssertion,
            auth_scope: Some(String::from("probe.hosted.session")),
        }
    }
}

pub fn run_hosted_tcp_server(
    probe_home: Option<PathBuf>,
    bind_addr: String,
    config: HostedApiServerConfig,
    watchdog_policy: DetachedTurnWatchdogPolicy,
) -> Result<(), ServerError> {
    let home = resolve_probe_home(probe_home, "probe-server hosted tcp")?;
    let listener = TcpListener::bind(bind_addr.as_str())?;
    let resolved_addr = listener.local_addr()?.to_string();
    let core = ProbeServerCore::hosted(
        ProbeRuntime::new(home),
        HostedApiServerConfig {
            attach_target: Some(
                config
                    .attach_target
                    .unwrap_or_else(|| format!("tcp://{resolved_addr}")),
            ),
            ..config
        },
        watchdog_policy,
    );
    core.reconcile_detached_sessions()
        .map_err(runtime_protocol_error_to_io)?;
    spawn_detached_watchdog(core.clone(), watchdog_policy)
        .map_err(|error| ServerError::Io(io::Error::other(error.to_string())))?;

    loop {
        let (stream, _) = listener.accept()?;
        let server = ProbeServerConnection::new(
            core.clone(),
            SharedJsonlWriter::new(Box::new(BufWriter::new(stream.try_clone()?))),
            TransportKind::TcpJsonl,
        );
        match server.run(io::BufReader::new(stream))? {
            ConnectionRunOutcome::ClientDisconnected => {}
            ConnectionRunOutcome::ServerShutdown => break,
        }
    }

    Ok(())
}

#[cfg(unix)]
pub fn run_local_daemon(
    probe_home: Option<PathBuf>,
    socket_path: Option<PathBuf>,
) -> Result<(), ServerError> {
    run_local_daemon_with_watchdog_policy(
        probe_home,
        socket_path,
        DetachedTurnWatchdogPolicy::default(),
    )
}

#[cfg(unix)]
pub fn run_local_daemon_with_watchdog_policy(
    probe_home: Option<PathBuf>,
    socket_path: Option<PathBuf>,
    watchdog_policy: DetachedTurnWatchdogPolicy,
) -> Result<(), ServerError> {
    let home = resolve_probe_home(probe_home, "probe-daemon")?;
    let socket_path = socket_path.unwrap_or_else(|| default_local_daemon_socket_path(&home));
    prepare_daemon_socket(socket_path.as_path())?;
    let _cleanup_guard = SocketCleanupGuard::new(socket_path.clone());
    let listener = UnixListener::bind(&socket_path)?;
    let core = ProbeServerCore::daemon(ProbeRuntime::new(home), watchdog_policy);
    core.reconcile_detached_sessions()
        .map_err(runtime_protocol_error_to_io)?;
    spawn_detached_watchdog(core.clone(), watchdog_policy)
        .map_err(|error| ServerError::Io(io::Error::other(error.to_string())))?;

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

#[cfg(not(unix))]
pub fn run_local_daemon_with_watchdog_policy(
    _probe_home: Option<PathBuf>,
    _socket_path: Option<PathBuf>,
    _watchdog_policy: DetachedTurnWatchdogPolicy,
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

const HOSTED_BASELINES_DIR: &str = "hosted/baselines";
const HOSTED_SNAPSHOTS_DIR: &str = "hosted/snapshots";
const PSIONIC_MESH_COORDINATION_STATUS_PATH: &str = "/psionic/management/coordination/status";
const PSIONIC_MESH_COORDINATION_FEED_PATH: &str = "/psionic/management/coordination/feed";
const PSIONIC_MESH_COORDINATION_SEARCH_PATH: &str = "/psionic/management/coordination/search";
const PSIONIC_MESH_COORDINATION_POST_PATH: &str = "/psionic/management/coordination/post";
const PROBE_MESH_PLUGIN_OFFER_SCHEMA: &str = "probe.mesh_plugin_offer.v1";
const PROBE_MESH_PLUGIN_CODING_BOOTSTRAP_ID: &str = "probe.coding_bootstrap.local";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProbeMeshPluginOfferEnvelope {
    schema: String,
    offer: SessionMeshPluginOffer,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HostedBaselineManifest {
    baseline_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repo_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_ref: Option<String>,
    #[serde(default)]
    stale: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HostedSnapshotManifest {
    snapshot_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restore_manifest_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_baseline_id: Option<String>,
}

#[derive(Clone)]
struct HostedReceiptConfig {
    auth_authority: String,
    auth_subject: String,
    auth_kind: SessionHostedAuthKind,
    auth_scope: Option<String>,
}

#[derive(Clone)]
pub struct ProbeServerCore {
    runtime: ProbeRuntime,
    turn_control: Arc<TurnControlPlane>,
    detached_registry: Option<Arc<DetachedSessionRegistry>>,
    detached_event_hub: Option<Arc<DetachedSessionEventHub>>,
    runtime_owner: SessionRuntimeOwner,
    hosted_receipt_config: Option<HostedReceiptConfig>,
    started_at_ms: u64,
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

#[derive(Clone, Debug, Serialize)]
struct PsionicMeshCoordinationFeedQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    since_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<SessionMeshCoordinationKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    visibility: Option<SessionMeshCoordinationVisibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
struct PsionicMeshCoordinationSearchQuery {
    query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    since_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<SessionMeshCoordinationKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    visibility: Option<SessionMeshCoordinationVisibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
struct PsionicMeshCoordinationPostBody {
    kind: SessionMeshCoordinationKind,
    body: String,
    author: String,
    visibility: SessionMeshCoordinationVisibility,
}

#[derive(Clone, Copy)]
enum HostedCleanupTrigger {
    ControlPlaneShutdown,
    ControlPlaneRestart,
}

#[derive(Clone)]
struct TurnControlPlane {
    runtime: ProbeRuntime,
    coordination: Arc<Mutex<HashSet<String>>>,
    detached_registry: Option<Arc<DetachedSessionRegistry>>,
    detached_event_hub: Option<Arc<DetachedSessionEventHub>>,
    hosted_receipt_config: Option<HostedReceiptConfig>,
    watchdog_policy: DetachedTurnWatchdogPolicy,
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
        Self::with_mode(
            runtime,
            ServerOwnershipMode::ForegroundStdio,
            SessionRuntimeOwner {
                kind: SessionRuntimeOwnerKind::ForegroundChild,
                owner_id: String::from("probe-server-foreground"),
                attach_transport: SessionAttachTransport::StdioJsonl,
                display_name: Some(String::from("probe-server")),
                attach_target: None,
            },
            None,
            DetachedTurnWatchdogPolicy::default(),
        )
    }

    fn daemon(runtime: ProbeRuntime, watchdog_policy: DetachedTurnWatchdogPolicy) -> Self {
        let socket_path = default_local_daemon_socket_path(runtime.session_store().root());
        Self::with_mode(
            runtime,
            ServerOwnershipMode::DetachedDaemon,
            SessionRuntimeOwner {
                kind: SessionRuntimeOwnerKind::LocalDaemon,
                owner_id: socket_path.display().to_string(),
                attach_transport: SessionAttachTransport::UnixSocketJsonl,
                display_name: Some(String::from("probe-daemon")),
                attach_target: Some(socket_path.display().to_string()),
            },
            None,
            watchdog_policy,
        )
    }

    fn hosted(
        runtime: ProbeRuntime,
        config: HostedApiServerConfig,
        watchdog_policy: DetachedTurnWatchdogPolicy,
    ) -> Self {
        let HostedApiServerConfig {
            owner_id,
            display_name,
            attach_target,
            auth_authority,
            auth_subject,
            auth_kind,
            auth_scope,
        } = config;
        Self::with_mode(
            runtime,
            ServerOwnershipMode::HostedControlPlane,
            SessionRuntimeOwner {
                kind: SessionRuntimeOwnerKind::HostedControlPlane,
                owner_id: owner_id.clone(),
                attach_transport: SessionAttachTransport::TcpJsonl,
                display_name,
                attach_target,
            },
            Some(HostedReceiptConfig {
                auth_authority: auth_authority.unwrap_or_else(|| owner_id.clone()),
                auth_subject: auth_subject.unwrap_or_else(|| String::from("gcp-internal-dogfood")),
                auth_kind,
                auth_scope,
            }),
            watchdog_policy,
        )
    }

    fn with_mode(
        runtime: ProbeRuntime,
        mode: ServerOwnershipMode,
        runtime_owner: SessionRuntimeOwner,
        hosted_receipt_config: Option<HostedReceiptConfig>,
        watchdog_policy: DetachedTurnWatchdogPolicy,
    ) -> Self {
        let detached_registry = mode
            .is_detached_owner()
            .then(|| Arc::new(DetachedSessionRegistry::new(runtime.session_store().root())));
        let detached_event_hub = mode
            .is_detached_owner()
            .then(|| Arc::new(DetachedSessionEventHub::new(runtime.session_store().root())));
        Self {
            turn_control: Arc::new(TurnControlPlane::new(
                runtime.clone(),
                detached_registry.clone(),
                detached_event_hub.clone(),
                hosted_receipt_config.clone(),
                watchdog_policy,
            )),
            runtime,
            detached_registry,
            detached_event_hub,
            runtime_owner,
            hosted_receipt_config,
            started_at_ms: now_ms(),
        }
    }

    fn inspect_session_mesh_coordination(
        &self,
        request: InspectSessionMeshCoordinationRequest,
    ) -> Result<InspectSessionMeshCoordinationResponse, RuntimeProtocolError> {
        let session = self
            .runtime
            .session_store()
            .read_metadata(&request.session_id)
            .map_err(session_store_error_to_protocol)?;
        let management_base_url = psionic_mesh_management_base_url_from_session(&session)?;
        let client = mesh_coordination_http_client()?;
        let status: SessionMeshCoordinationStatus = psionic_management_get(
            &client,
            &management_base_url,
            PSIONIC_MESH_COORDINATION_STATUS_PATH,
            None::<&PsionicMeshCoordinationFeedQuery>,
        )?;
        let entries = if status.mode == SessionMeshCoordinationMode::Disabled {
            Vec::new()
        } else if let Some(query) = request
            .query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            psionic_management_get(
                &client,
                &management_base_url,
                PSIONIC_MESH_COORDINATION_SEARCH_PATH,
                Some(&PsionicMeshCoordinationSearchQuery {
                    query: query.to_string(),
                    since_ms: request.since_ms,
                    author: request.author.clone(),
                    kind: request.kind,
                    visibility: request.visibility,
                    limit: request.limit,
                }),
            )?
        } else {
            psionic_management_get(
                &client,
                &management_base_url,
                PSIONIC_MESH_COORDINATION_FEED_PATH,
                Some(&PsionicMeshCoordinationFeedQuery {
                    since_ms: request.since_ms,
                    author: request.author.clone(),
                    kind: request.kind,
                    visibility: request.visibility,
                    limit: request.limit,
                }),
            )?
        };
        Ok(InspectSessionMeshCoordinationResponse {
            session_id: request.session_id,
            status,
            entries,
        })
    }

    fn post_session_mesh_coordination(
        &self,
        request: PostSessionMeshCoordinationRequest,
    ) -> Result<PostSessionMeshCoordinationResponse, RuntimeProtocolError> {
        let session = self
            .runtime
            .session_store()
            .read_metadata(&request.session_id)
            .map_err(session_store_error_to_protocol)?;
        let management_base_url = psionic_mesh_management_base_url_from_session(&session)?;
        let client = mesh_coordination_http_client()?;
        let status: SessionMeshCoordinationStatus = psionic_management_get(
            &client,
            &management_base_url,
            PSIONIC_MESH_COORDINATION_STATUS_PATH,
            None::<&PsionicMeshCoordinationFeedQuery>,
        )?;
        if status.mode == SessionMeshCoordinationMode::Disabled {
            return Err(protocol_error(
                "mesh_coordination_unavailable",
                format!(
                    "mesh coordination is disabled for session {}",
                    request.session_id.as_str()
                ),
            ));
        }
        let entry: SessionMeshCoordinationEntry = psionic_management_post(
            &client,
            &management_base_url,
            PSIONIC_MESH_COORDINATION_POST_PATH,
            &PsionicMeshCoordinationPostBody {
                kind: request.kind,
                body: request.body,
                author: request.author.unwrap_or_else(|| String::from("probe")),
                visibility: request
                    .visibility
                    .unwrap_or(SessionMeshCoordinationVisibility::Mesh),
            },
        )?;
        Ok(PostSessionMeshCoordinationResponse {
            session_id: request.session_id,
            entry,
        })
    }

    fn inspect_session_mesh_plugin_offers(
        &self,
        request: InspectSessionMeshPluginOffersRequest,
    ) -> Result<InspectSessionMeshPluginOffersResponse, RuntimeProtocolError> {
        let response =
            self.inspect_session_mesh_coordination(InspectSessionMeshCoordinationRequest {
                session_id: request.session_id.clone(),
                query: None,
                since_ms: None,
                author: None,
                kind: Some(SessionMeshCoordinationKind::Tip),
                visibility: None,
                limit: request.limit,
            })?;
        let offers = response
            .entries
            .iter()
            .filter_map(mesh_plugin_offer_from_entry)
            .collect::<Vec<_>>();
        Ok(InspectSessionMeshPluginOffersResponse {
            session_id: request.session_id,
            status: response.status,
            offers,
        })
    }

    fn publish_session_mesh_plugin_offer(
        &self,
        request: PublishSessionMeshPluginOfferRequest,
    ) -> Result<PublishSessionMeshPluginOfferResponse, RuntimeProtocolError> {
        let session = self
            .runtime
            .session_store()
            .read_metadata(&request.session_id)
            .map_err(session_store_error_to_protocol)?;
        let mut offer = build_probe_mesh_plugin_offer(&session, request.tool_set.as_str())?;
        let body = serde_json::to_string(&ProbeMeshPluginOfferEnvelope {
            schema: String::from(PROBE_MESH_PLUGIN_OFFER_SCHEMA),
            offer: offer.clone(),
        })
        .map_err(|error| {
            protocol_error(
                "mesh_plugin_encode_error",
                format!("failed to encode mesh plugin offer: {error}"),
            )
        })?;
        let response = self.post_session_mesh_coordination(PostSessionMeshCoordinationRequest {
            session_id: request.session_id.clone(),
            kind: SessionMeshCoordinationKind::Tip,
            body,
            author: request.author,
            visibility: Some(
                request
                    .visibility
                    .unwrap_or(SessionMeshCoordinationVisibility::Mesh),
            ),
        })?;
        offer.entry_id = Some(response.entry.id);
        offer.published_at_ms = Some(response.entry.created_at_ms);
        Ok(PublishSessionMeshPluginOfferResponse {
            session_id: request.session_id,
            entry: response.entry,
            offer,
        })
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
            self.record_hosted_restart_history_if_needed(&session_id)?;
            self.finalize_hosted_cleanup_for_session(
                &session_id,
                HostedCleanupTrigger::ControlPlaneRestart,
            )?;
        }
        Ok(())
    }

    fn record_hosted_restart_history_if_needed(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeProtocolError> {
        let metadata = load_session_metadata_for_collaboration(self, session_id)?;
        if !is_hosted_session(&metadata) || metadata.created_at_ms >= self.started_at_ms {
            return Ok(());
        }
        let Some(runtime_owner) = metadata.runtime_owner.as_ref() else {
            return Ok(());
        };
        let execution_host_id = hosted_execution_host_id(&metadata, runtime_owner);
        let summary = format!(
            "hosted Probe reconciled this session after control-plane restart at {}",
            self.started_at_ms
        );
        let event = SessionHostedLifecycleEvent::ControlPlaneRestartObserved {
            control_plane_started_at_ms: self.started_at_ms,
            session_owner_id: runtime_owner.owner_id.clone(),
            execution_host_id,
            summary,
            recorded_at_ms: self.started_at_ms,
        };
        let _ = persist_session_collaboration_metadata(self, metadata, Some(event))?;
        Ok(())
    }

    fn finalize_hosted_cleanup_for_all_sessions(
        &self,
        trigger: HostedCleanupTrigger,
    ) -> Result<(), RuntimeProtocolError> {
        let Some(registry) = self.detached_registry.as_ref() else {
            return Ok(());
        };
        for summary in registry
            .list()
            .map_err(detached_registry_error_to_protocol)?
        {
            self.finalize_hosted_cleanup_for_session(&summary.session_id, trigger)?;
        }
        Ok(())
    }

    fn finalize_hosted_cleanup_for_session(
        &self,
        session_id: &SessionId,
        trigger: HostedCleanupTrigger,
    ) -> Result<(), RuntimeProtocolError> {
        let Some(registry) = self.detached_registry.as_ref() else {
            return Ok(());
        };
        let Some(summary) = registry
            .read(session_id)
            .map_err(detached_registry_error_to_protocol)?
        else {
            return Ok(());
        };
        if !hosted_cleanup_ready(&summary) {
            return Ok(());
        }
        let metadata = load_session_metadata_for_collaboration(self, session_id)?;
        let metadata = finalize_managed_hosted_cleanup(self, metadata, trigger, now_ms())?;
        let _ = persist_session_collaboration_metadata(self, metadata, None)?;
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
        let session = session_snapshot_from_core(self, session_id)?;
        Ok(InspectDetachedSessionResponse {
            summary,
            session,
            turn_control,
        })
    }

    fn attach_session_participant(
        &self,
        request: AttachSessionParticipantRequest,
    ) -> Result<AttachSessionParticipantResponse, RuntimeProtocolError> {
        self.ensure_detached_session_registered_by_id(&request.session_id)?;
        let mut metadata = load_session_metadata_for_collaboration(self, &request.session_id)?;
        let now = now_ms();
        upsert_session_participant(&mut metadata, &request.participant, now, true)?;
        let history_event = if request.claim_controller {
            apply_session_controller_action(
                &mut metadata,
                &request.participant,
                SessionControllerAction::Claim,
                None,
                now,
            )?
        } else {
            None
        };
        let metadata = persist_session_collaboration_metadata(self, metadata, history_event)?;
        Ok(AttachSessionParticipantResponse {
            session_id: request.session_id,
            participants: metadata.participants,
            controller_lease: metadata.controller_lease,
        })
    }

    fn update_session_controller(
        &self,
        request: UpdateSessionControllerRequest,
    ) -> Result<UpdateSessionControllerResponse, RuntimeProtocolError> {
        self.ensure_detached_session_registered_by_id(&request.session_id)?;
        let mut metadata = load_session_metadata_for_collaboration(self, &request.session_id)?;
        let now = now_ms();
        let history_event = apply_session_controller_action(
            &mut metadata,
            &request.actor,
            request.action,
            request.target_participant_id.clone(),
            now,
        )?;
        let metadata = persist_session_collaboration_metadata(self, metadata, history_event)?;
        Ok(UpdateSessionControllerResponse {
            session_id: request.session_id,
            participants: metadata.participants,
            controller_lease: metadata.controller_lease,
        })
    }

    fn read_detached_session_log(
        &self,
        request: ReadDetachedSessionLogRequest,
    ) -> Result<ReadDetachedSessionLogResponse, RuntimeProtocolError> {
        let Some(event_hub) = self.detached_event_hub.as_ref() else {
            return Err(protocol_error(
                "unsupported_transport",
                "detached session event logs are only available through the daemon transport",
            ));
        };
        self.ensure_detached_session_registered_by_id(&request.session_id)?;
        let events = event_hub
            .read(&request.session_id, request.after_cursor, request.limit)
            .map_err(detached_event_error_to_protocol)?;
        let newest_cursor = event_hub
            .newest_cursor(&request.session_id)
            .map_err(detached_event_error_to_protocol)?;
        Ok(ReadDetachedSessionLogResponse {
            session_id: request.session_id,
            events,
            newest_cursor,
        })
    }
}

#[derive(Clone, Copy)]
enum ServerOwnershipMode {
    ForegroundStdio,
    DetachedDaemon,
    HostedControlPlane,
}

const MAX_CHILD_SESSION_DEPTH: usize = 2;
const MAX_CHILD_SESSIONS_PER_PARENT: usize = 8;

impl ServerOwnershipMode {
    fn is_detached_owner(self) -> bool {
        matches!(self, Self::DetachedDaemon | Self::HostedControlPlane)
    }
}

impl TurnControlPlane {
    fn new(
        runtime: ProbeRuntime,
        detached_registry: Option<Arc<DetachedSessionRegistry>>,
        detached_event_hub: Option<Arc<DetachedSessionEventHub>>,
        hosted_receipt_config: Option<HostedReceiptConfig>,
        watchdog_policy: DetachedTurnWatchdogPolicy,
    ) -> Self {
        Self {
            runtime,
            coordination: Arc::new(Mutex::new(HashSet::new())),
            detached_registry,
            detached_event_hub,
            hosted_receipt_config,
            watchdog_policy,
        }
    }

    fn save_state_only(
        &self,
        session_id: &SessionId,
        state: &SessionTurnControlState,
    ) -> Result<(), RuntimeProtocolError> {
        state
            .save(&self.runtime, session_id)
            .map_err(session_store_error_to_protocol)
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
        let transcript = self
            .runtime
            .session_store()
            .read_transcript(session_id)
            .map_err(session_store_error_to_protocol)?;
        let metadata = sync_hosted_session_metadata_from_store(
            self.runtime.session_store(),
            self.hosted_receipt_config.as_ref(),
            metadata,
            transcript.as_slice(),
            now_ms(),
        )?;
        let branch_state = session_branch_state(metadata.cwd.as_path());
        let delivery_state = branch_state
            .as_ref()
            .map(|branch_state| session_delivery_state(branch_state, now_ms()));
        let summary_artifacts = refresh_session_summary_artifacts(
            self.runtime.session_store(),
            &metadata,
            transcript.as_slice(),
            branch_state.as_ref(),
            delivery_state.as_ref(),
        )
        .map_err(session_summary_artifact_error_to_protocol)?;
        let now = now_ms();
        let mut summary = detached_session_summary_from_state(
            &metadata,
            state,
            pending_approval_count,
            summary_artifact_refs(summary_artifacts.as_slice()),
            previous.as_ref(),
            now,
        );
        let metadata = sync_hosted_history_from_detached_summary(
            self.runtime.session_store(),
            metadata,
            &summary,
            now,
        )?;
        summary = detached_session_summary_from_state(
            &metadata,
            state,
            pending_approval_count,
            summary_artifact_refs(summary_artifacts.as_slice()),
            previous.as_ref(),
            now,
        );
        let turn_control = state.inspect_view(session_id);
        registry
            .upsert(summary.clone())
            .map_err(detached_registry_error_to_protocol)?;
        if let Some(event_hub) = self.detached_event_hub.as_ref() {
            event_hub
                .append(
                    session_id,
                    DetachedSessionEventTruth::Authoritative,
                    DetachedSessionEventPayload::SummaryUpdated {
                        summary: summary.clone(),
                        turn_control,
                    },
                    now,
                )
                .map_err(detached_event_error_to_protocol)?;
            if branch_state.is_some() || delivery_state.is_some() {
                event_hub
                    .append(
                        session_id,
                        DetachedSessionEventTruth::Authoritative,
                        DetachedSessionEventPayload::WorkspaceStateUpdated {
                            workspace_state: metadata.workspace_state.clone(),
                            branch_state: branch_state.clone(),
                            delivery_state: delivery_state.clone(),
                        },
                        now,
                    )
                    .map_err(detached_event_error_to_protocol)?;
            }
            if let Some(parent_link) = metadata.parent_link.as_ref() {
                event_hub
                    .append(
                        &parent_link.session_id,
                        DetachedSessionEventTruth::Authoritative,
                        DetachedSessionEventPayload::ChildSessionUpdated {
                            child: SessionChildSummary {
                                session_id: metadata.id.clone(),
                                title: metadata.title.clone(),
                                cwd: metadata.cwd.clone(),
                                state: metadata.state,
                                status: session_child_status_from_detached(summary.status),
                                initiator: parent_link.initiator.clone(),
                                purpose: parent_link.purpose.clone(),
                                parent_turn_id: parent_link.turn_id.clone(),
                                parent_turn_index: parent_link.turn_index,
                                closure: session_child_closure(
                                    session_child_status_from_detached(summary.status),
                                    branch_state.as_ref(),
                                    delivery_state.as_ref(),
                                    metadata.updated_at_ms,
                                ),
                                created_at_ms: metadata.created_at_ms,
                                updated_at_ms: metadata.updated_at_ms,
                            },
                        },
                        now,
                    )
                    .map_err(detached_event_error_to_protocol)?;
            }
        }
        Ok(())
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
        if coordination.contains(request.session_id.as_str()) {
            return Err(protocol_error(
                "session_draining",
                format!(
                    "session {} is still draining a previously timed-out detached turn; retry after the worker exits",
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
                Some(self.watchdog_policy.execution_timeout_ms),
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
        if state.unfinished_turn_count() == 0 && coordination.contains(request.session_id.as_str())
        {
            return Err(protocol_error(
                "session_draining",
                format!(
                    "session {} is still draining a previously timed-out detached turn; retry after the worker exits",
                    request.session_id.as_str()
                ),
            ));
        }
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
                should_start.then_some(self.watchdog_policy.execution_timeout_ms),
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
        let resumed_at_ms = now_ms();
        active_turn.record.awaiting_approval = false;
        active_turn.record.last_progress_at_ms = Some(resumed_at_ms);
        active_turn.record.execution_timeout_at_ms =
            Some(resumed_at_ms.saturating_add(self.watchdog_policy.execution_timeout_ms));
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
        let _ = persist_latest_task_receipt_from_pending_baseline(
            &self.runtime,
            &session_id,
            TaskReceiptDisposition::Stopped,
        );
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
        let mut completed = false;
        if let Some(turn) = state.turn_by_id_mut(turn_id) {
            if turn.record.status == QueuedTurnStatus::Running {
                turn.record.status = QueuedTurnStatus::Completed;
                turn.record.awaiting_approval = false;
                turn.record.finished_at_ms = Some(now_ms());
                turn.record.failure_message = None;
                turn.record.cancellation_reason = None;
                completed = true;
            }
        }
        coordination.remove(session_id.as_str());
        let should_start_next = completed && state.queued_turn_count() > 0;
        if completed {
            self.save_state_and_sync(session_id, &state)?;
        }
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
            if turn.record.status == QueuedTurnStatus::Running {
                turn.record.status = QueuedTurnStatus::Running;
                turn.record.awaiting_approval = true;
                turn.record.finished_at_ms = None;
                turn.record.last_progress_at_ms = Some(now_ms());
                turn.record.execution_timeout_at_ms = None;
            }
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
            if turn.record.status == QueuedTurnStatus::Running {
                turn.record.status = QueuedTurnStatus::Running;
                turn.record.awaiting_approval = true;
                turn.record.last_progress_at_ms = Some(now_ms());
                turn.record.execution_timeout_at_ms = None;
            }
        }
        coordination.remove(session_id.as_str());
        self.save_state_and_sync(session_id, &state)
    }

    fn record_runtime_progress(
        &self,
        session_id: &SessionId,
        turn_id: &str,
    ) -> Result<(), RuntimeProtocolError> {
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let mut state = self.load_state_locked(session_id.as_str(), &mut coordination)?;
        if state.record_runtime_progress(turn_id, now_ms()) {
            self.save_state_only(session_id, &state)?;
        }
        Ok(())
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
        let mut failed = false;
        if let Some(turn) = state.turn_by_id_mut(turn_id) {
            if turn.record.status == QueuedTurnStatus::Running {
                turn.record.status = QueuedTurnStatus::Failed;
                turn.record.awaiting_approval = false;
                turn.record.finished_at_ms = Some(now_ms());
                turn.record.failure_message = Some(message);
                turn.record.cancellation_reason = None;
                failed = true;
            }
        }
        coordination.remove(session_id.as_str());
        let should_start_next = failed && state.queued_turn_count() > 0;
        if failed {
            self.save_state_and_sync(session_id, &state)?;
        }
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
        state.mark_turn_running(
            turn_id.as_str(),
            now_ms(),
            self.watchdog_policy.execution_timeout_ms,
        );
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
            let unfinished = state.unfinished_turn_count();
            if unfinished == 0 && coordination.contains(session.id.as_str()) {
                active_turns += 1;
            } else {
                active_turns += unfinished;
            }
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

    fn enforce_detached_watchdog_policy(&self) -> Result<(), RuntimeProtocolError> {
        let Some(registry) = self.detached_registry.as_ref() else {
            return Ok(());
        };
        for session_id in registry
            .tracked_session_ids()
            .map_err(detached_registry_error_to_protocol)?
        {
            self.enforce_detached_watchdog_for_session(&session_id)?;
        }
        Ok(())
    }

    fn enforce_detached_watchdog_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeProtocolError> {
        let now_ms = now_ms();
        let mut coordination = self
            .coordination
            .lock()
            .expect("probe-server coordination mutex should not be poisoned");
        let mut state = self.load_state_locked(session_id.as_str(), &mut coordination)?;
        let Some(active_turn) = state.active_turn().cloned() else {
            return Ok(());
        };
        let Some(trigger) =
            evaluate_detached_turn_watchdog(&active_turn.record, now_ms, self.watchdog_policy)
        else {
            return Ok(());
        };

        let failure_message = watchdog_failure_message(&active_turn, trigger);
        let transcript_note = watchdog_transcript_note(&active_turn, trigger);
        let queued_cancelled = cancel_queued_turns_after_watchdog(
            &mut state,
            active_turn.record.turn_id.as_str(),
            now_ms,
        );

        if let Some(turn) = state.turn_by_id_mut(active_turn.record.turn_id.as_str()) {
            if turn.record.status != QueuedTurnStatus::Running {
                return Ok(());
            }
            turn.record.status = QueuedTurnStatus::TimedOut;
            turn.record.awaiting_approval = false;
            turn.record.finished_at_ms = Some(now_ms);
            turn.record.failure_message = Some(failure_message.clone());
            turn.record.cancellation_reason = None;
        }

        let mut notes = vec![NewItem::new(
            probe_protocol::session::TranscriptItemKind::Note,
            transcript_note,
        )];
        notes.extend(queued_cancelled.iter().map(|note| {
            NewItem::new(
                probe_protocol::session::TranscriptItemKind::Note,
                note.clone(),
            )
        }));
        self.runtime
            .session_store()
            .append_turn(session_id, &notes)
            .map_err(session_store_error_to_protocol)?;
        self.save_state_and_sync(session_id, &state)?;
        if let Some(event_hub) = self.detached_event_hub.as_ref() {
            event_hub
                .append(
                    session_id,
                    DetachedSessionEventTruth::Authoritative,
                    DetachedSessionEventPayload::Note {
                        code: watchdog_note_code(trigger).to_string(),
                        message: watchdog_event_message(
                            &active_turn,
                            trigger,
                            queued_cancelled.len(),
                        ),
                    },
                    now_ms,
                )
                .map_err(detached_event_error_to_protocol)?;
        }
        Ok(())
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
                                supports_hosted_tcp_jsonl: matches!(
                                    self.transport,
                                    TransportKind::TcpJsonl
                                ),
                                supports_session_resume: true,
                                supports_session_inspect: true,
                                supports_session_mounts: true,
                                supports_child_sessions: true,
                                supports_pending_approval_resolution: true,
                                supports_interrupt_requests: true,
                                supports_queued_turns: true,
                                supports_detached_session_registry: self
                                    .core
                                    .detached_registry
                                    .is_some(),
                                supports_detached_watch_subscriptions: self
                                    .core
                                    .detached_event_hub
                                    .is_some(),
                                supports_mesh_coordination_adjunct: true,
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
            RuntimeRequest::SpawnChildSession(request) => {
                match self.spawn_child_session(request) {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::SpawnChildSession(response),
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
            RuntimeRequest::ReadDetachedSessionLog(request) => {
                match self.core.read_detached_session_log(request) {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::ReadDetachedSessionLog(response),
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
            RuntimeRequest::InspectSessionMeshCoordination(request) => {
                match self.core.inspect_session_mesh_coordination(request) {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::InspectSessionMeshCoordination(response),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::PostSessionMeshCoordination(request) => {
                match self.core.post_session_mesh_coordination(request) {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::PostSessionMeshCoordination(response),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::InspectSessionMeshPluginOffers(request) => {
                match self.core.inspect_session_mesh_plugin_offers(request) {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::InspectSessionMeshPluginOffers(response),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::PublishSessionMeshPluginOffer(request) => {
                match self.core.publish_session_mesh_plugin_offer(request) {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::PublishSessionMeshPluginOffer(response),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::AttachSessionParticipant(request) => {
                match self.core.attach_session_participant(request) {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::AttachSessionParticipant(response),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::UpdateSessionController(request) => {
                match self.core.update_session_controller(request) {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::UpdateSessionController(response),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::WatchDetachedSession(request) => {
                self.watch_detached_session(request_id, request)?;
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
                match self.interrupt_turn(request) {
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
            RuntimeRequest::RevertLastTask(request) => {
                match self.revert_last_task(request.session_id, request.author.as_ref()) {
                    Ok(response) => self.writer.send_response_ok(
                        request_id.as_str(),
                        RuntimeResponse::RevertLastTask(response),
                    )?,
                    Err(error) => self
                        .writer
                        .send_response_error(request_id.as_str(), error)?,
                }
                Ok(RequestHandlingOutcome::Continue)
            }
            RuntimeRequest::Shutdown => {
                let active_turns = self.active_turn_count();
                let accepted = active_turns == 0;
                if accepted {
                    self.core
                        .finalize_hosted_cleanup_for_all_sessions(
                            HostedCleanupTrigger::ControlPlaneShutdown,
                        )
                        .map_err(runtime_protocol_error_to_io)?;
                }
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
        let StartSessionRequest {
            title,
            cwd,
            profile,
            system_prompt,
            harness_profile,
            workspace_state,
            mounted_refs,
        } = request;
        let workspace_state = resolve_workspace_state(
            self.core.runtime.session_store().root(),
            &self.core.runtime_owner,
            workspace_state,
        );
        let mcp_state = Some(load_session_mcp_state(
            self.core.runtime.session_store().root(),
        ));
        let mounted_refs = validate_session_mounts(mounted_refs)?;
        let session = self
            .core
            .runtime
            .session_store()
            .create_session_with(
                NewSession::new(normalize_session_title(title), cwd)
                    .with_system_prompt(system_prompt)
                    .with_harness_profile(harness_profile)
                    .with_backend(SessionBackendTarget::from_profile(&profile))
                    .with_runtime_owner(Some(self.core.runtime_owner.clone()))
                    .with_workspace_state(workspace_state)
                    .with_mcp_state(mcp_state)
                    .with_mounted_refs(mounted_refs),
            )
            .map_err(session_store_error_to_protocol)?;
        self.session_snapshot(&session.id)
    }

    fn spawn_child_session(
        &self,
        request: SpawnChildSessionRequest,
    ) -> Result<SpawnChildSessionResponse, RuntimeProtocolError> {
        let parent = self
            .core
            .runtime
            .session_store()
            .read_metadata(&request.parent_session_id)
            .map_err(session_store_error_to_protocol)?;
        if parent.child_links.len() >= MAX_CHILD_SESSIONS_PER_PARENT {
            return Err(protocol_error(
                "child_session_limit_reached",
                format!(
                    "session {} already owns {} child sessions; Probe currently caps direct children at {}",
                    request.parent_session_id.as_str(),
                    parent.child_links.len(),
                    MAX_CHILD_SESSIONS_PER_PARENT,
                ),
            ));
        }
        let parent_depth = session_depth(&self.core.runtime, &parent)?;
        if parent_depth >= MAX_CHILD_SESSION_DEPTH {
            return Err(protocol_error(
                "child_session_depth_exceeded",
                format!(
                    "session {} is already at child-session depth {}; Probe currently caps delegation depth at {}",
                    request.parent_session_id.as_str(),
                    parent_depth,
                    MAX_CHILD_SESSION_DEPTH,
                ),
            ));
        }

        let child_cwd = request.cwd.unwrap_or_else(|| parent.cwd.clone());
        enforce_same_workspace_boundary(parent.cwd.as_path(), child_cwd.as_path())?;
        let child_title = normalize_session_title(
            request
                .title
                .or_else(|| Some(format!("{} Child", parent.title))),
        );
        let initiator = request
            .author
            .as_ref()
            .map(session_initiator_from_turn_author);
        let workspace_state = parent.workspace_state.clone();
        let mcp_state = parent.mcp_state.clone();
        let mounted_refs = parent.mounted_refs.clone();
        let child = self
            .core
            .runtime
            .session_store()
            .create_session_with(
                NewSession::new(child_title, child_cwd)
                    .with_system_prompt(request.system_prompt)
                    .with_harness_profile(request.harness_profile)
                    .with_backend(SessionBackendTarget::from_profile(&request.profile))
                    .with_runtime_owner(Some(self.core.runtime_owner.clone()))
                    .with_workspace_state(workspace_state)
                    .with_mcp_state(mcp_state)
                    .with_mounted_refs(mounted_refs)
                    .with_parent_link(Some(SessionParentLink {
                        session_id: request.parent_session_id.clone(),
                        turn_id: request.parent_turn_id,
                        turn_index: request.parent_turn_index,
                        initiator,
                        purpose: request.purpose,
                    })),
            )
            .map_err(session_store_error_to_protocol)?;
        self.core
            .runtime
            .session_store()
            .append_child_link(
                &request.parent_session_id,
                SessionChildLink {
                    session_id: child.id.clone(),
                    added_at_ms: child.created_at_ms,
                },
            )
            .map_err(session_store_error_to_protocol)?;
        let snapshot = self.session_snapshot(&child.id)?;
        let child = session_child_summary_from_snapshot(&snapshot);
        let parent_state =
            SessionTurnControlState::load(&self.core.runtime, &request.parent_session_id)
                .map_err(session_store_error_to_protocol)?;
        self.core
            .turn_control
            .sync_detached_session_summary(&request.parent_session_id, &parent_state)?;
        Ok(SpawnChildSessionResponse {
            parent_session_id: request.parent_session_id,
            child,
            session: snapshot,
        })
    }

    fn session_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionSnapshot, RuntimeProtocolError> {
        let snapshot = session_snapshot_from_core(&self.core, session_id)?;
        self.core
            .ensure_detached_session_registered(&snapshot.session)?;
        self.core
            .turn_control
            .sync_detached_session_if_tracked(session_id)?;
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

    fn revert_last_task(
        &self,
        session_id: SessionId,
        author: Option<&TurnAuthor>,
    ) -> Result<RevertLastTaskResponse, RuntimeProtocolError> {
        self.core
            .ensure_detached_session_registered_by_id(&session_id)?;
        self.prepare_turn_authority(&session_id, author, "revert last task")?;
        let turns = self.core.turn_control.inspect_session_turns(&session_id)?;
        if turns.active_turn.is_some() {
            return Err(protocol_error(
                "turn_in_progress",
                String::from(
                    "finish the active turn before asking Probe to revert the latest task",
                ),
            ));
        }
        if !turns.queued_turns.is_empty() {
            return Err(protocol_error(
                "queued_turns_pending",
                String::from(
                    "clear or run queued turns before asking Probe to revert the latest task",
                ),
            ));
        }

        let reverted_files = revert_latest_task_in_session(&self.core.runtime, &session_id)?;
        let session = self.session_snapshot(&session_id)?;
        let message = if reverted_files.is_empty() {
            String::from("Probe completed the requested revert.")
        } else {
            format!(
                "Probe reverted the latest task in {}.",
                summarize_task_paths(reverted_files.as_slice(), 6)
            )
        };
        Ok(RevertLastTaskResponse {
            session,
            reverted_files,
            message,
        })
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
        self.prepare_turn_authority(&request.session_id, request.author.as_ref(), "queue turn")?;
        let reservation = self.core.turn_control.reserve_queue_turn(&request)?;
        if reservation.should_start {
            spawn_turn_worker(
                Arc::clone(&self.core.turn_control),
                self.core.runtime.clone(),
                self.writer.clone(),
                self.core.detached_event_hub.clone(),
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
        request: probe_protocol::runtime::InterruptTurnRequest,
    ) -> Result<InterruptTurnResponse, RuntimeProtocolError> {
        let session_exists = self
            .core
            .runtime
            .session_store()
            .read_metadata(&request.session_id)
            .is_ok();
        if session_exists {
            self.prepare_turn_authority(
                &request.session_id,
                request.author.as_ref(),
                "interrupt turn",
            )?;
            self.core
                .ensure_detached_session_registered_by_id(&request.session_id)?;
        }
        let outcome = self
            .core
            .turn_control
            .interrupt_turn(request.session_id.clone())?;
        if outcome.should_start_next {
            spawn_next_queued_turn_if_ready(
                Arc::clone(&self.core.turn_control),
                self.core.runtime.clone(),
                self.writer.clone(),
                self.core.detached_event_hub.clone(),
                request.session_id,
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
        self.prepare_turn_authority(
            &request.session_id,
            request.author.as_ref(),
            "cancel queued turn",
        )?;
        self.core.turn_control.cancel_queued_turn(request)
    }

    fn watch_detached_session(
        &self,
        request_id: String,
        request: WatchDetachedSessionRequest,
    ) -> Result<(), ServerError> {
        let Some(event_hub) = self.core.detached_event_hub.as_ref() else {
            self.writer.send_response_error(
                request_id.as_str(),
                protocol_error(
                    "unsupported_transport",
                    "detached session watch is only available through the daemon transport",
                ),
            )?;
            return Ok(());
        };
        if let Err(error) = self
            .core
            .ensure_detached_session_registered_by_id(&request.session_id)
        {
            self.writer
                .send_response_error(request_id.as_str(), error)?;
            return Ok(());
        }
        let receiver = event_hub.subscribe(&request.session_id);
        if let Err(error) = self
            .core
            .sync_detached_session_if_tracked(&request.session_id)
        {
            self.writer
                .send_response_error(request_id.as_str(), error)?;
            return Ok(());
        }
        let replay = match self
            .core
            .read_detached_session_log(ReadDetachedSessionLogRequest {
                session_id: request.session_id.clone(),
                after_cursor: request.after_cursor,
                limit: request.replay_limit,
            }) {
            Ok(replay) => replay,
            Err(error) => {
                self.writer
                    .send_response_error(request_id.as_str(), error)?;
                return Ok(());
            }
        };
        for record in &replay.events {
            if self
                .writer
                .send_event(
                    request_id.as_str(),
                    ServerEvent::DetachedSessionStream {
                        record: record.clone(),
                    },
                )
                .is_err()
            {
                return Ok(());
            }
        }

        let mut last_cursor = replay.newest_cursor;
        loop {
            match receiver.recv_timeout(Duration::from_millis(250)) {
                Ok(record) => {
                    if last_cursor.is_some_and(|cursor| record.cursor <= cursor) {
                        continue;
                    }
                    last_cursor = Some(record.cursor);
                    if self
                        .writer
                        .send_event(
                            request_id.as_str(),
                            ServerEvent::DetachedSessionStream { record },
                        )
                        .is_err()
                    {
                        return Ok(());
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        self.writer.send_response_ok(
            request_id.as_str(),
            RuntimeResponse::WatchDetachedSession(WatchDetachedSessionResponse {
                session_id: request.session_id,
                replayed_events: replay.events.len(),
                last_cursor,
            }),
        )?;
        Ok(())
    }

    fn spawn_turn_request(
        &self,
        request_id: String,
        request: TurnRequest,
        mode: TurnMode,
    ) -> Result<(), ServerError> {
        if let Err(error) =
            self.prepare_turn_authority(&request.session_id, request.author.as_ref(), "submit turn")
        {
            self.writer
                .send_response_error(request_id.as_str(), error)?;
            return Ok(());
        }
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
            self.core.detached_event_hub.clone(),
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
        if let Err(error) = self.prepare_turn_authority(
            &request.session_id,
            request.author.as_ref(),
            "resolve pending approval",
        ) {
            self.writer
                .send_response_error(request_id.as_str(), error)?;
            return Ok(());
        }
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
            self.core.detached_event_hub.clone(),
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

    fn prepare_turn_authority(
        &self,
        session_id: &SessionId,
        author: Option<&TurnAuthor>,
        action_label: &str,
    ) -> Result<(), RuntimeProtocolError> {
        self.core
            .ensure_detached_session_registered_by_id(session_id)?;
        let metadata =
            prepare_session_authority_for_action(&self.core, session_id, author, action_label)?;
        let _ = metadata;
        Ok(())
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

#[derive(Clone, Debug)]
struct TaskWorkspaceBaseline {
    task_start_turn_index: u64,
    repo_root: Option<PathBuf>,
    preexisting_dirty_files: Vec<String>,
}

fn capture_task_workspace_baseline(
    runtime: &ProbeRuntime,
    session_id: &SessionId,
) -> Result<TaskWorkspaceBaseline, RuntimeProtocolError> {
    let metadata = runtime
        .session_store()
        .read_metadata(session_id)
        .map_err(session_store_error_to_protocol)?;
    Ok(capture_task_workspace_baseline_from_metadata(&metadata))
}

fn capture_resumable_task_workspace_baseline(
    runtime: &ProbeRuntime,
    session_id: &SessionId,
) -> Result<TaskWorkspaceBaseline, RuntimeProtocolError> {
    let metadata = runtime
        .session_store()
        .read_metadata(session_id)
        .map_err(session_store_error_to_protocol)?;
    Ok(pending_task_workspace_baseline(&metadata)
        .unwrap_or_else(|| capture_task_workspace_baseline_from_metadata(&metadata)))
}

fn capture_task_workspace_baseline_from_metadata(
    metadata: &SessionMetadata,
) -> TaskWorkspaceBaseline {
    let repo_root = resolve_git_repo_root(metadata.cwd.as_path());
    let preexisting_dirty_files =
        dirty_worktree_paths(metadata.cwd.as_path(), repo_root.as_deref());
    TaskWorkspaceBaseline {
        task_start_turn_index: metadata.next_turn_index,
        repo_root,
        preexisting_dirty_files,
    }
}

fn pending_task_workspace_baseline(metadata: &SessionMetadata) -> Option<TaskWorkspaceBaseline> {
    metadata
        .latest_task_receipt
        .as_ref()
        .filter(|receipt| receipt.disposition == TaskReceiptDisposition::PendingApproval)
        .map(|receipt| task_workspace_baseline_from_summary(&receipt.workspace))
        .or_else(|| {
            metadata
                .latest_task_workspace_summary
                .as_ref()
                .filter(|summary| summary.status == TaskWorkspaceSummaryStatus::PendingApproval)
                .map(task_workspace_baseline_from_summary)
        })
}

fn task_workspace_baseline_from_summary(summary: &TaskWorkspaceSummary) -> TaskWorkspaceBaseline {
    TaskWorkspaceBaseline {
        task_start_turn_index: summary.task_start_turn_index,
        repo_root: summary.repo_root.clone(),
        preexisting_dirty_files: summary.preexisting_dirty_files.clone(),
    }
}

fn persist_latest_task_receipt(
    runtime: &ProbeRuntime,
    session_id: &SessionId,
    baseline: &TaskWorkspaceBaseline,
    disposition: TaskReceiptDisposition,
) -> Result<SessionMetadata, RuntimeProtocolError> {
    let mut metadata = runtime
        .session_store()
        .read_metadata(session_id)
        .map_err(session_store_error_to_protocol)?;
    let transcript = runtime
        .session_store()
        .read_transcript(session_id)
        .map_err(session_store_error_to_protocol)?;
    let observed_dirty_files_after_task =
        dirty_worktree_paths(metadata.cwd.as_path(), baseline.repo_root.as_deref());
    let workspace = build_task_workspace_summary(
        transcript.as_slice(),
        baseline,
        workspace_summary_status_for_receipt(disposition),
        observed_dirty_files_after_task.as_slice(),
    );
    metadata.latest_task_workspace_summary = Some(workspace.clone());
    metadata.latest_task_receipt = Some(build_task_final_receipt(
        transcript.as_slice(),
        disposition,
        &workspace,
    ));
    runtime
        .session_store()
        .replace_metadata(metadata)
        .map_err(session_store_error_to_protocol)
}

fn persist_latest_task_receipt_from_pending_baseline(
    runtime: &ProbeRuntime,
    session_id: &SessionId,
    disposition: TaskReceiptDisposition,
) -> Result<Option<SessionMetadata>, RuntimeProtocolError> {
    let metadata = runtime
        .session_store()
        .read_metadata(session_id)
        .map_err(session_store_error_to_protocol)?;
    let Some(baseline) = pending_task_workspace_baseline(&metadata) else {
        return Ok(None);
    };
    persist_latest_task_receipt(runtime, session_id, &baseline, disposition).map(Some)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RevertPatchOperation {
    path: String,
    old_text: String,
    new_text: String,
    replace_all: bool,
    create_if_missing: bool,
}

fn revert_latest_task_in_session(
    runtime: &ProbeRuntime,
    session_id: &SessionId,
) -> Result<Vec<String>, RuntimeProtocolError> {
    let metadata = runtime
        .session_store()
        .read_metadata(session_id)
        .map_err(session_store_error_to_protocol)?;
    let Some(summary) = metadata.latest_task_workspace_summary.as_ref() else {
        return Err(protocol_error(
            "revert_unavailable",
            String::from("no recent applied task is available to revert on this lane yet"),
        ));
    };
    if summary.status == TaskWorkspaceSummaryStatus::PendingApproval {
        return Err(protocol_error(
            "revert_pending_approval",
            String::from(
                "the latest task is still pending approval, so there is nothing applied to revert yet",
            ),
        ));
    }
    if summary.revertibility.status != TaskRevertibilityStatus::Exact {
        return Err(protocol_error(
            "revert_not_safe",
            summary.revertibility.summary_text.clone(),
        ));
    }

    let transcript = runtime
        .session_store()
        .read_transcript(session_id)
        .map_err(session_store_error_to_protocol)?;
    let operations = collect_revert_patch_operations(
        transcript.as_slice(),
        summary.task_start_turn_index,
        summary.changed_files.as_slice(),
    )?;
    let repo_root = summary
        .repo_root
        .clone()
        .or_else(|| resolve_git_repo_root(metadata.cwd.as_path()));
    let dirty_before_revert = dirty_worktree_paths(metadata.cwd.as_path(), repo_root.as_deref());
    let reverted_files =
        apply_revert_patch_operations(metadata.cwd.as_path(), operations.as_slice())?;
    let revert_turn = runtime
        .session_store()
        .append_turn(
            session_id,
            &[NewItem::new(
                probe_protocol::session::TranscriptItemKind::Note,
                format!(
                    "Probe reverted the latest task in {}.",
                    summarize_task_paths(reverted_files.as_slice(), 6)
                ),
            )],
        )
        .map_err(session_store_error_to_protocol)?;
    let dirty_after_revert = dirty_worktree_paths(metadata.cwd.as_path(), repo_root.as_deref());
    let outside_tracking_dirty_files = dirty_files_outside_tool_tracking(
        dirty_after_revert.as_slice(),
        dirty_before_revert.as_slice(),
        reverted_files.as_slice(),
    );
    let workspace = build_reverted_task_workspace_summary(
        revert_turn.index,
        repo_root,
        dirty_before_revert,
        reverted_files.clone(),
        outside_tracking_dirty_files,
    );
    let receipt = build_reverted_task_receipt(&workspace);
    let mut refreshed = runtime
        .session_store()
        .read_metadata(session_id)
        .map_err(session_store_error_to_protocol)?;
    refreshed.latest_task_workspace_summary = Some(workspace);
    refreshed.latest_task_receipt = Some(receipt);
    runtime
        .session_store()
        .replace_metadata(refreshed)
        .map_err(session_store_error_to_protocol)?;
    Ok(reverted_files)
}

fn collect_revert_patch_operations(
    transcript: &[TranscriptEvent],
    task_start_turn_index: u64,
    changed_files: &[String],
) -> Result<Vec<RevertPatchOperation>, RuntimeProtocolError> {
    let mut apply_patch_calls = BTreeMap::<String, serde_json::Value>::new();
    let mut operations = Vec::new();

    for event in transcript
        .iter()
        .filter(|event| event.turn.index >= task_start_turn_index)
    {
        for item in &event.turn.items {
            if item.kind == TranscriptItemKind::ToolCall
                && item.name.as_deref() == Some("apply_patch")
                && let (Some(call_id), Some(arguments)) =
                    (item.tool_call_id.as_ref(), item.arguments.as_ref())
            {
                apply_patch_calls.insert(call_id.clone(), arguments.clone());
                continue;
            }

            if item.kind != TranscriptItemKind::ToolResult || !tool_result_applied(item) {
                continue;
            }
            let Some(tool_execution) = item.tool_execution.as_ref() else {
                continue;
            };
            if !matches!(
                tool_execution.risk_class,
                ToolRiskClass::Write | ToolRiskClass::Network | ToolRiskClass::Destructive
            ) {
                continue;
            }

            let Some(tool_name) = item.name.as_deref() else {
                return Err(protocol_error(
                    "revert_missing_tool_name",
                    String::from(
                        "the latest task includes an applied tool result without a tool name, so Probe cannot restore it safely",
                    ),
                ));
            };
            if tool_name != "apply_patch" {
                return Err(protocol_error(
                    "revert_not_supported",
                    format!(
                        "the latest task includes applied `{tool_name}` work, so Probe cannot promise an exact automated revert yet"
                    ),
                ));
            }

            let Some(call_id) = item.tool_call_id.as_ref() else {
                return Err(protocol_error(
                    "revert_missing_call_id",
                    String::from("Probe could not find the apply_patch call id needed for revert"),
                ));
            };
            let Some(arguments) = apply_patch_calls.get(call_id) else {
                return Err(protocol_error(
                    "revert_missing_arguments",
                    String::from(
                        "Probe could not find the recorded apply_patch arguments needed for revert",
                    ),
                ));
            };
            operations.push(parse_revert_patch_operation(arguments, changed_files)?);
        }
    }

    if operations.is_empty() {
        return Err(protocol_error(
            "revert_unavailable",
            String::from(
                "the latest task does not contain an exact apply_patch history Probe can restore yet",
            ),
        ));
    }

    Ok(operations)
}

fn parse_revert_patch_operation(
    arguments: &serde_json::Value,
    changed_files: &[String],
) -> Result<RevertPatchOperation, RuntimeProtocolError> {
    let path = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            protocol_error(
                "revert_invalid_arguments",
                String::from("Probe recorded an apply_patch call without a valid path"),
            )
        })?;
    let old_text = arguments
        .get("old_text")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            protocol_error(
                "revert_invalid_arguments",
                String::from("Probe recorded an apply_patch call without old_text"),
            )
        })?;
    let new_text = arguments
        .get("new_text")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            protocol_error(
                "revert_invalid_arguments",
                String::from("Probe recorded an apply_patch call without new_text"),
            )
        })?;
    let replace_all = arguments
        .get("replace_all")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let create_if_missing = arguments
        .get("create_if_missing")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    if replace_all {
        return Err(protocol_error(
            "revert_not_supported",
            format!(
                "Probe cannot safely auto-revert `{path}` yet because the original apply_patch used replace_all"
            ),
        ));
    }
    if !changed_files.iter().any(|candidate| candidate == path) {
        return Err(protocol_error(
            "revert_scope_mismatch",
            format!(
                "Probe recorded a reversible patch for `{path}`, but that path was not retained in the latest task summary"
            ),
        ));
    }

    Ok(RevertPatchOperation {
        path: path.to_string(),
        old_text: old_text.to_string(),
        new_text: new_text.to_string(),
        replace_all,
        create_if_missing,
    })
}

fn apply_revert_patch_operations(
    cwd: &Path,
    operations: &[RevertPatchOperation],
) -> Result<Vec<String>, RuntimeProtocolError> {
    let mut reverted_files = Vec::new();
    for operation in operations.iter().rev() {
        let resolved = resolve_revert_workspace_path(cwd, operation.path.as_str())?;
        if operation.old_text.is_empty() && operation.create_if_missing {
            let current = fs::read_to_string(&resolved).map_err(|error| {
                protocol_error(
                    "revert_read_failed",
                    format!(
                        "failed to read `{}` while restoring the latest task: {error}",
                        operation.path
                    ),
                )
            })?;
            if current != operation.new_text {
                return Err(protocol_error(
                    "revert_conflict",
                    format!(
                        "Probe cannot safely delete `{}` because the file no longer matches the contents it created",
                        operation.path
                    ),
                ));
            }
            fs::remove_file(&resolved).map_err(|error| {
                protocol_error(
                    "revert_write_failed",
                    format!(
                        "failed to remove `{}` while restoring the latest task: {error}",
                        operation.path
                    ),
                )
            })?;
        } else if operation.old_text.is_empty() {
            let current = fs::read_to_string(&resolved).map_err(|error| {
                protocol_error(
                    "revert_read_failed",
                    format!(
                        "failed to read `{}` while restoring the latest task: {error}",
                        operation.path
                    ),
                )
            })?;
            if current != operation.new_text {
                return Err(protocol_error(
                    "revert_conflict",
                    format!(
                        "Probe cannot safely restore `{}` because the file no longer matches the text it wrote",
                        operation.path
                    ),
                ));
            }
            fs::write(&resolved, "").map_err(|error| {
                protocol_error(
                    "revert_write_failed",
                    format!(
                        "failed to restore `{}` to its previous empty contents: {error}",
                        operation.path
                    ),
                )
            })?;
        } else {
            let current = fs::read_to_string(&resolved).map_err(|error| {
                protocol_error(
                    "revert_read_failed",
                    format!(
                        "failed to read `{}` while restoring the latest task: {error}",
                        operation.path
                    ),
                )
            })?;
            let occurrences = current.matches(operation.new_text.as_str()).count();
            if occurrences != 1 {
                return Err(protocol_error(
                    "revert_conflict",
                    format!(
                        "Probe cannot safely restore `{}` because the expected patched text is no longer present exactly once",
                        operation.path
                    ),
                ));
            }
            let restored =
                current.replacen(operation.new_text.as_str(), operation.old_text.as_str(), 1);
            fs::write(&resolved, restored).map_err(|error| {
                protocol_error(
                    "revert_write_failed",
                    format!(
                        "failed to write the restored contents for `{}`: {error}",
                        operation.path
                    ),
                )
            })?;
        }
        push_unique_path(&mut reverted_files, operation.path.as_str());
    }
    Ok(reverted_files)
}

fn resolve_revert_workspace_path(
    base: &Path,
    requested_path: &str,
) -> Result<PathBuf, RuntimeProtocolError> {
    if requested_path.trim().is_empty() {
        return Err(protocol_error(
            "revert_invalid_path",
            String::from("Probe cannot revert a task with an empty recorded path"),
        ));
    }
    let base_dir = if base.is_absolute() {
        base.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(base)
    };
    let mut resolved = base_dir.clone();
    for component in Path::new(requested_path).components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => resolved.push(part),
            std::path::Component::ParentDir => {
                if resolved == base_dir {
                    return Err(protocol_error(
                        "revert_invalid_path",
                        format!("path `{requested_path}` escapes the session cwd"),
                    ));
                }
                resolved.pop();
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(protocol_error(
                    "revert_invalid_path",
                    format!("path `{requested_path}` must stay relative to the session cwd"),
                ));
            }
        }
    }
    Ok(resolved)
}

fn build_reverted_task_workspace_summary(
    task_start_turn_index: u64,
    repo_root: Option<PathBuf>,
    preexisting_dirty_files: Vec<String>,
    changed_files: Vec<String>,
    outside_tracking_dirty_files: Vec<String>,
) -> TaskWorkspaceSummary {
    let checkpoint_status = if outside_tracking_dirty_files.is_empty() {
        TaskCheckpointStatus::Captured
    } else {
        TaskCheckpointStatus::Limited
    };
    let checkpoint_summary = match checkpoint_status {
        TaskCheckpointStatus::Captured => format!(
            "Probe restored the previous contents for {} using the latest task's recorded patch history.",
            summarize_task_paths(changed_files.as_slice(), 6)
        ),
        TaskCheckpointStatus::Limited => format!(
            "Probe restored {} but observed additional dirty files during the revert, so manual review is still recommended.",
            summarize_task_paths(changed_files.as_slice(), 6)
        ),
        TaskCheckpointStatus::NotCaptured => String::new(),
    };
    TaskWorkspaceSummary {
        task_start_turn_index,
        status: TaskWorkspaceSummaryStatus::Reverted,
        changed_files: changed_files.clone(),
        touched_but_unchanged_files: Vec::new(),
        preexisting_dirty_files,
        outside_tracking_dirty_files,
        repo_root,
        change_accounting_limited: false,
        checkpoint: TaskCheckpointSummary {
            status: checkpoint_status,
            summary_text: checkpoint_summary,
        },
        revertibility: TaskRevertibilitySummary {
            status: TaskRevertibilityStatus::Unavailable,
            summary_text: String::from(
                "The latest task already represents a completed revert, so there is nothing newer for Probe to auto-restore right now.",
            ),
        },
        diff_previews: Vec::new(),
        summary_text: format!(
            "This task reverted the previous task in {}.",
            summarize_task_paths(changed_files.as_slice(), 6)
        ),
    }
}

fn build_reverted_task_receipt(workspace: &TaskWorkspaceSummary) -> TaskFinalReceipt {
    let mut uncertainty_reasons = Vec::new();
    if !workspace.outside_tracking_dirty_files.is_empty() {
        uncertainty_reasons.push(format!(
            "Additional dirty files were still present after the revert: {}.",
            summarize_task_paths(workspace.outside_tracking_dirty_files.as_slice(), 3)
        ));
    }
    TaskFinalReceipt {
        disposition: TaskReceiptDisposition::Succeeded,
        workspace: workspace.clone(),
        verification_status: TaskVerificationStatus::NotRun,
        verification_commands: Vec::new(),
        uncertainty_reasons: uncertainty_reasons.clone(),
        summary_text: if uncertainty_reasons.is_empty() {
            format!(
                "Reverted the latest task in {}. No validation command was observed after the restore.",
                summarize_task_paths(workspace.changed_files.as_slice(), 6)
            )
        } else {
            format!(
                "Reverted the latest task in {}. No validation command was observed after the restore. Remaining uncertainty: {}.",
                summarize_task_paths(workspace.changed_files.as_slice(), 6),
                summarize_task_paths(uncertainty_reasons.as_slice(), 2)
            )
        },
    }
}

fn build_task_workspace_summary(
    transcript: &[TranscriptEvent],
    baseline: &TaskWorkspaceBaseline,
    requested_status: TaskWorkspaceSummaryStatus,
    observed_dirty_files_after_task: &[String],
) -> TaskWorkspaceSummary {
    let mut changed_files = Vec::new();
    let mut touched_files = Vec::new();
    let mut change_accounting_limited = false;

    for event in transcript
        .iter()
        .filter(|event| event.turn.index >= baseline.task_start_turn_index)
    {
        for item in &event.turn.items {
            if item.kind != TranscriptItemKind::ToolResult || !tool_result_applied(item) {
                continue;
            }
            let Some(tool_execution) = item.tool_execution.as_ref() else {
                continue;
            };
            if matches!(
                tool_execution.risk_class,
                ToolRiskClass::Write | ToolRiskClass::Network | ToolRiskClass::Destructive
            ) {
                for path in &tool_execution.files_touched {
                    push_unique_path(&mut touched_files, path);
                }
                for path in &tool_execution.files_changed {
                    push_unique_path(&mut changed_files, path);
                }
                if item.name.as_deref() == Some("shell")
                    && tool_execution.files_touched.is_empty()
                    && tool_execution.files_changed.is_empty()
                {
                    change_accounting_limited = true;
                }
            }
        }
    }

    let changed_set = changed_files.iter().cloned().collect::<HashSet<_>>();
    let touched_but_unchanged_files = touched_files
        .into_iter()
        .filter(|path| !changed_set.contains(path))
        .collect::<Vec<_>>();
    let outside_tracking_dirty_files = dirty_files_outside_tool_tracking(
        observed_dirty_files_after_task,
        baseline.preexisting_dirty_files.as_slice(),
        changed_files.as_slice(),
    );
    let status = resolve_task_workspace_summary_status(
        requested_status,
        changed_files.is_empty(),
        change_accounting_limited,
    );
    let checkpoint = build_task_checkpoint_summary(
        status,
        baseline,
        changed_files.as_slice(),
        outside_tracking_dirty_files.as_slice(),
        change_accounting_limited,
    );
    let summary_text = task_workspace_summary_text(
        status,
        changed_files.as_slice(),
        touched_but_unchanged_files.as_slice(),
        baseline.preexisting_dirty_files.as_slice(),
        outside_tracking_dirty_files.as_slice(),
        change_accounting_limited,
    );
    let revertibility = build_task_revertibility_summary(
        transcript,
        baseline.task_start_turn_index,
        status,
        &checkpoint,
        changed_files.as_slice(),
        outside_tracking_dirty_files.as_slice(),
        change_accounting_limited,
    );
    let diff_previews =
        build_task_diff_previews(baseline.repo_root.as_deref(), changed_files.as_slice());

    TaskWorkspaceSummary {
        task_start_turn_index: baseline.task_start_turn_index,
        status,
        changed_files,
        touched_but_unchanged_files,
        preexisting_dirty_files: baseline.preexisting_dirty_files.clone(),
        outside_tracking_dirty_files,
        repo_root: baseline.repo_root.clone(),
        change_accounting_limited,
        checkpoint,
        revertibility,
        diff_previews,
        summary_text,
    }
}

fn build_task_diff_previews(
    repo_root: Option<&Path>,
    changed_files: &[String],
) -> Vec<TaskDiffPreview> {
    let Some(repo_root) = repo_root else {
        return Vec::new();
    };
    changed_files
        .iter()
        .filter_map(|path| build_task_diff_preview(repo_root, path))
        .collect()
}

fn build_task_diff_preview(repo_root: &Path, path: &str) -> Option<TaskDiffPreview> {
    let mut diff_text = git_diff_for_path(repo_root, path);
    if diff_text.trim().is_empty() {
        diff_text = git_diff_for_untracked_path(repo_root, path).unwrap_or_default();
    }
    if diff_text.trim().is_empty() {
        return None;
    }
    let mut diff_lines = diff_text.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let truncated = diff_lines.len() > 180;
    if truncated {
        diff_lines.truncate(180);
    }
    Some(TaskDiffPreview {
        path: path.to_string(),
        diff_lines,
        truncated,
    })
}

fn git_diff_for_path(repo_root: &Path, path: &str) -> String {
    run_git_output(repo_root, &["diff", "--", path]).unwrap_or_default()
}

fn git_diff_for_untracked_path(repo_root: &Path, path: &str) -> Option<String> {
    let absolute_path = repo_root.join(path);
    if !absolute_path.exists() {
        return None;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("diff")
        .arg("--no-index")
        .arg("--")
        .arg("/dev/null")
        .arg(&absolute_path)
        .output()
        .ok()?;
    if !output.status.success() && output.stdout.is_empty() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).into_owned();
    (!value.trim().is_empty()).then_some(value)
}

fn build_task_checkpoint_summary(
    status: TaskWorkspaceSummaryStatus,
    baseline: &TaskWorkspaceBaseline,
    changed_files: &[String],
    outside_tracking_dirty_files: &[String],
    change_accounting_limited: bool,
) -> TaskCheckpointSummary {
    let status = resolve_task_checkpoint_status(
        status,
        baseline.repo_root.is_some(),
        changed_files.is_empty(),
        outside_tracking_dirty_files.is_empty(),
        change_accounting_limited,
    );
    TaskCheckpointSummary {
        status,
        summary_text: task_checkpoint_summary_text(
            status,
            changed_files,
            outside_tracking_dirty_files,
        ),
    }
}

fn resolve_task_checkpoint_status(
    workspace_status: TaskWorkspaceSummaryStatus,
    repo_root_available: bool,
    changed_files_empty: bool,
    outside_tracking_dirty_files_empty: bool,
    change_accounting_limited: bool,
) -> TaskCheckpointStatus {
    if changed_files_empty {
        return if workspace_status == TaskWorkspaceSummaryStatus::ChangeAccountingLimited {
            TaskCheckpointStatus::Limited
        } else {
            TaskCheckpointStatus::NotCaptured
        };
    }
    if !repo_root_available || change_accounting_limited || !outside_tracking_dirty_files_empty {
        TaskCheckpointStatus::Limited
    } else {
        TaskCheckpointStatus::Captured
    }
}

fn task_checkpoint_summary_text(
    status: TaskCheckpointStatus,
    changed_files: &[String],
    outside_tracking_dirty_files: &[String],
) -> String {
    match status {
        TaskCheckpointStatus::NotCaptured => {
            String::from("No pre-edit checkpoint was needed because no repo changes landed.")
        }
        TaskCheckpointStatus::Captured => format!(
            "Probe captured a pre-edit checkpoint before changes landed in {}.",
            summarize_task_paths(changed_files, 6)
        ),
        TaskCheckpointStatus::Limited => {
            if changed_files.is_empty() {
                String::from(
                    "Probe observed write-capable work but cannot confirm a complete pre-edit checkpoint for this task.",
                )
            } else if outside_tracking_dirty_files.is_empty() {
                format!(
                    "Probe captured only a limited pre-edit checkpoint for changes in {}.",
                    summarize_task_paths(changed_files, 6)
                )
            } else {
                format!(
                    "Probe captured only a limited pre-edit checkpoint for {} because additional dirty files appeared outside tracked tool results: {}.",
                    summarize_task_paths(changed_files, 6),
                    summarize_task_paths(outside_tracking_dirty_files, 6)
                )
            }
        }
    }
}

fn build_task_revertibility_summary(
    transcript: &[TranscriptEvent],
    task_start_turn_index: u64,
    status: TaskWorkspaceSummaryStatus,
    checkpoint: &TaskCheckpointSummary,
    changed_files: &[String],
    outside_tracking_dirty_files: &[String],
    change_accounting_limited: bool,
) -> TaskRevertibilitySummary {
    let revert_blocker =
        detect_revert_support_blocker(transcript, task_start_turn_index, changed_files);
    let mut status = resolve_task_revertibility_status(
        status,
        checkpoint.status,
        changed_files.is_empty(),
        outside_tracking_dirty_files.is_empty(),
        change_accounting_limited,
    );
    if revert_blocker.is_some() && !changed_files.is_empty() {
        status = TaskRevertibilityStatus::Limited;
    }
    TaskRevertibilitySummary {
        status,
        summary_text: revert_blocker.unwrap_or_else(|| {
            task_revertibility_summary_text(status, changed_files, outside_tracking_dirty_files)
        }),
    }
}

fn detect_revert_support_blocker(
    transcript: &[TranscriptEvent],
    task_start_turn_index: u64,
    changed_files: &[String],
) -> Option<String> {
    let mut apply_patch_calls = BTreeMap::<String, serde_json::Value>::new();
    for event in transcript
        .iter()
        .filter(|event| event.turn.index >= task_start_turn_index)
    {
        for item in &event.turn.items {
            if item.kind == TranscriptItemKind::ToolCall
                && item.name.as_deref() == Some("apply_patch")
                && let (Some(call_id), Some(arguments)) =
                    (item.tool_call_id.as_ref(), item.arguments.as_ref())
            {
                apply_patch_calls.insert(call_id.clone(), arguments.clone());
                continue;
            }

            if item.kind != TranscriptItemKind::ToolResult || !tool_result_applied(item) {
                continue;
            }
            let Some(tool_execution) = item.tool_execution.as_ref() else {
                continue;
            };
            if !matches!(
                tool_execution.risk_class,
                ToolRiskClass::Write | ToolRiskClass::Network | ToolRiskClass::Destructive
            ) {
                continue;
            }
            let Some(tool_name) = item.name.as_deref() else {
                return Some(String::from(
                    "Probe is missing the tool name for part of the latest task, so auto-revert is not safe yet.",
                ));
            };
            if tool_name != "apply_patch" {
                return Some(format!(
                    "The latest task used `{tool_name}`, so Probe cannot auto-revert it safely yet."
                ));
            }
            let Some(call_id) = item.tool_call_id.as_ref() else {
                return Some(String::from(
                    "Probe is missing the apply_patch call id needed for auto-revert.",
                ));
            };
            let Some(arguments) = apply_patch_calls.get(call_id) else {
                return Some(String::from(
                    "Probe is missing the recorded apply_patch arguments needed for auto-revert.",
                ));
            };
            let path = arguments
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("[unknown path]");
            let replace_all = arguments
                .get("replace_all")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if replace_all {
                return Some(format!(
                    "The latest task edited `{path}` with replace_all, so Probe will not auto-revert it yet."
                ));
            }
            if !changed_files.iter().any(|candidate| candidate == path) {
                return Some(format!(
                    "Probe's latest task summary is missing `{path}`, so auto-revert is not safe yet."
                ));
            }
        }
    }
    None
}

fn resolve_task_revertibility_status(
    workspace_status: TaskWorkspaceSummaryStatus,
    checkpoint_status: TaskCheckpointStatus,
    changed_files_empty: bool,
    outside_tracking_dirty_files_empty: bool,
    change_accounting_limited: bool,
) -> TaskRevertibilityStatus {
    if matches!(
        workspace_status,
        TaskWorkspaceSummaryStatus::NoRepoChanges
            | TaskWorkspaceSummaryStatus::PendingApproval
            | TaskWorkspaceSummaryStatus::Reverted
    ) {
        return TaskRevertibilityStatus::Unavailable;
    }
    if matches!(
        workspace_status,
        TaskWorkspaceSummaryStatus::ChangeAccountingLimited
    ) || change_accounting_limited
    {
        return TaskRevertibilityStatus::Limited;
    }
    if changed_files_empty {
        return TaskRevertibilityStatus::Unavailable;
    }
    match checkpoint_status {
        TaskCheckpointStatus::Captured if outside_tracking_dirty_files_empty => {
            TaskRevertibilityStatus::Exact
        }
        TaskCheckpointStatus::Captured | TaskCheckpointStatus::Limited => {
            TaskRevertibilityStatus::Limited
        }
        TaskCheckpointStatus::NotCaptured => TaskRevertibilityStatus::Unavailable,
    }
}

fn task_revertibility_summary_text(
    status: TaskRevertibilityStatus,
    changed_files: &[String],
    outside_tracking_dirty_files: &[String],
) -> String {
    match status {
        TaskRevertibilityStatus::Unavailable => {
            if changed_files.is_empty() {
                String::from(
                    "No applied repo changes are currently available for automated revert.",
                )
            } else {
                String::from(
                    "Probe does not currently have enough checkpoint coverage to promise a safe automated revert for this task.",
                )
            }
        }
        TaskRevertibilityStatus::Exact => format!(
            "Probe has enough checkpoint coverage to attempt an exact restore for {}.",
            summarize_task_paths(changed_files, 6)
        ),
        TaskRevertibilityStatus::Limited => {
            if changed_files.is_empty() {
                String::from(
                    "Probe observed write-capable work but can only offer limited revert guidance, so manual review is still required.",
                )
            } else if outside_tracking_dirty_files.is_empty() {
                format!(
                    "Probe may be able to help revert {} later, but checkpoint coverage is limited and manual review is still required.",
                    summarize_task_paths(changed_files, 6)
                )
            } else {
                format!(
                    "Probe may be able to help revert {}, but additional dirty files appeared outside tracked tool results: {}. Manual review is still required.",
                    summarize_task_paths(changed_files, 6),
                    summarize_task_paths(outside_tracking_dirty_files, 6)
                )
            }
        }
    }
}

fn workspace_summary_status_for_receipt(
    disposition: TaskReceiptDisposition,
) -> TaskWorkspaceSummaryStatus {
    match disposition {
        TaskReceiptDisposition::Succeeded => TaskWorkspaceSummaryStatus::Changed,
        TaskReceiptDisposition::PendingApproval => TaskWorkspaceSummaryStatus::PendingApproval,
        TaskReceiptDisposition::Failed | TaskReceiptDisposition::Stopped => {
            TaskWorkspaceSummaryStatus::PartialChangesBeforeFailure
        }
    }
}

fn resolve_task_workspace_summary_status(
    requested_status: TaskWorkspaceSummaryStatus,
    changed_files_empty: bool,
    change_accounting_limited: bool,
) -> TaskWorkspaceSummaryStatus {
    match requested_status {
        TaskWorkspaceSummaryStatus::PendingApproval => TaskWorkspaceSummaryStatus::PendingApproval,
        TaskWorkspaceSummaryStatus::PartialChangesBeforeFailure => {
            if changed_files_empty {
                if change_accounting_limited {
                    TaskWorkspaceSummaryStatus::ChangeAccountingLimited
                } else {
                    TaskWorkspaceSummaryStatus::NoRepoChanges
                }
            } else {
                TaskWorkspaceSummaryStatus::PartialChangesBeforeFailure
            }
        }
        TaskWorkspaceSummaryStatus::Changed => {
            if changed_files_empty {
                if change_accounting_limited {
                    TaskWorkspaceSummaryStatus::ChangeAccountingLimited
                } else {
                    TaskWorkspaceSummaryStatus::NoRepoChanges
                }
            } else {
                TaskWorkspaceSummaryStatus::Changed
            }
        }
        TaskWorkspaceSummaryStatus::Reverted => TaskWorkspaceSummaryStatus::Reverted,
        TaskWorkspaceSummaryStatus::NoRepoChanges
        | TaskWorkspaceSummaryStatus::ChangeAccountingLimited => requested_status,
    }
}

fn task_workspace_summary_text(
    status: TaskWorkspaceSummaryStatus,
    changed_files: &[String],
    touched_but_unchanged_files: &[String],
    preexisting_dirty_files: &[String],
    outside_tracking_dirty_files: &[String],
    change_accounting_limited: bool,
) -> String {
    let mut sentences = vec![match status {
        TaskWorkspaceSummaryStatus::NoRepoChanges => {
            String::from("No repo changes were made by this task.")
        }
        TaskWorkspaceSummaryStatus::Changed => format!(
            "This task changed {} file(s): {}.",
            changed_files.len(),
            summarize_task_paths(changed_files, 6)
        ),
        TaskWorkspaceSummaryStatus::Reverted => format!(
            "This task reverted the previous task in {}.",
            summarize_task_paths(changed_files, 6)
        ),
        TaskWorkspaceSummaryStatus::PartialChangesBeforeFailure => format!(
            "Partial edits landed before failure in {} file(s): {}.",
            changed_files.len(),
            summarize_task_paths(changed_files, 6)
        ),
        TaskWorkspaceSummaryStatus::PendingApproval => {
            if changed_files.is_empty() {
                String::from("This task is waiting for approval. No repo changes have landed yet.")
            } else {
                format!(
                    "This task is waiting for approval. Changes have already landed in {} file(s): {}.",
                    changed_files.len(),
                    summarize_task_paths(changed_files, 6)
                )
            }
        }
        TaskWorkspaceSummaryStatus::ChangeAccountingLimited => String::from(
            "Probe cannot confirm whether repo changes landed for this task because write-capable shell commands ran without file-level change accounting.",
        ),
    }];

    if !touched_but_unchanged_files.is_empty() {
        sentences.push(format!(
            "Touched without lasting changes: {}.",
            summarize_task_paths(touched_but_unchanged_files, 6)
        ));
    }
    if !preexisting_dirty_files.is_empty() {
        sentences.push(format!(
            "Dirty before task start: {}.",
            summarize_task_paths(preexisting_dirty_files, 6)
        ));
    }
    if !outside_tracking_dirty_files.is_empty() {
        sentences.push(format!(
            "Additional dirty files appeared during the task outside tracked tool results: {}.",
            summarize_task_paths(outside_tracking_dirty_files, 6)
        ));
    }
    if change_accounting_limited && status != TaskWorkspaceSummaryStatus::ChangeAccountingLimited {
        sentences.push(String::from(
            "Changed-file accounting may be incomplete because write-capable shell commands ran without file-level accounting.",
        ));
    }

    sentences.join(" ")
}

fn build_task_final_receipt(
    transcript: &[TranscriptEvent],
    disposition: TaskReceiptDisposition,
    workspace: &TaskWorkspaceSummary,
) -> TaskFinalReceipt {
    let verification_commands =
        collect_task_verification_commands(transcript, workspace.task_start_turn_index);
    let verification_status = aggregate_task_verification_status(verification_commands.as_slice());
    let uncertainty_reasons = task_receipt_uncertainty_reasons(
        disposition,
        workspace,
        verification_status,
        verification_commands.as_slice(),
    );
    let summary_text = task_final_receipt_text(
        disposition,
        workspace,
        verification_status,
        verification_commands.as_slice(),
        uncertainty_reasons.as_slice(),
    );
    TaskFinalReceipt {
        disposition,
        workspace: workspace.clone(),
        verification_status,
        verification_commands,
        uncertainty_reasons,
        summary_text,
    }
}

fn collect_task_verification_commands(
    transcript: &[TranscriptEvent],
    task_start_turn_index: u64,
) -> Vec<TaskVerificationCommandSummary> {
    let mut commands = Vec::new();
    for event in transcript
        .iter()
        .filter(|event| event.turn.index >= task_start_turn_index)
    {
        for item in &event.turn.items {
            if item.kind != TranscriptItemKind::ToolResult {
                continue;
            }
            let Some(tool_execution) = item.tool_execution.as_ref() else {
                continue;
            };
            if !matches!(
                tool_execution.policy_decision,
                ToolPolicyDecision::AutoAllow | ToolPolicyDecision::Approved
            ) {
                continue;
            }
            if item.name.as_deref() != Some("shell") {
                continue;
            }
            let Some(command) = tool_execution.command.as_deref() else {
                continue;
            };
            if !is_validation_command(command) {
                continue;
            }
            commands.push(TaskVerificationCommandSummary {
                command: String::from(command),
                status: task_verification_command_status(item),
                exit_code: tool_execution.exit_code,
                truncated_output: tool_execution.truncated == Some(true),
            });
        }
    }
    commands
}

fn task_verification_command_status(item: &TranscriptItem) -> TaskVerificationCommandStatus {
    let Some(tool_execution) = item.tool_execution.as_ref() else {
        return TaskVerificationCommandStatus::Failed;
    };
    if tool_execution.timed_out == Some(true) {
        return TaskVerificationCommandStatus::TimedOut;
    }
    if tool_result_has_error(item) {
        return TaskVerificationCommandStatus::Failed;
    }
    match tool_execution.exit_code {
        Some(0) => TaskVerificationCommandStatus::Passed,
        Some(_) | None => TaskVerificationCommandStatus::Failed,
    }
}

fn aggregate_task_verification_status(
    commands: &[TaskVerificationCommandSummary],
) -> TaskVerificationStatus {
    if commands.is_empty() {
        return TaskVerificationStatus::NotRun;
    }
    let all_passed = commands
        .iter()
        .all(|command| command.status == TaskVerificationCommandStatus::Passed);
    if all_passed {
        return TaskVerificationStatus::Passed;
    }
    let all_failed = commands
        .iter()
        .all(|command| command.status == TaskVerificationCommandStatus::Failed);
    if all_failed {
        return TaskVerificationStatus::Failed;
    }
    let all_timed_out = commands
        .iter()
        .all(|command| command.status == TaskVerificationCommandStatus::TimedOut);
    if all_timed_out {
        return TaskVerificationStatus::TimedOut;
    }
    let any_timed_out = commands
        .iter()
        .any(|command| command.status == TaskVerificationCommandStatus::TimedOut);
    if any_timed_out
        && commands
            .iter()
            .all(|command| command.status != TaskVerificationCommandStatus::Failed)
    {
        return TaskVerificationStatus::TimedOut;
    }
    TaskVerificationStatus::Mixed
}

fn task_receipt_uncertainty_reasons(
    disposition: TaskReceiptDisposition,
    workspace: &TaskWorkspaceSummary,
    verification_status: TaskVerificationStatus,
    verification_commands: &[TaskVerificationCommandSummary],
) -> Vec<String> {
    let mut reasons = Vec::new();
    if !workspace.outside_tracking_dirty_files.is_empty() {
        reasons.push(format!(
            "Probe observed dirty files outside tracked tool results during this task: {}.",
            summarize_task_paths(workspace.outside_tracking_dirty_files.as_slice(), 3)
        ));
    }
    if workspace.checkpoint.status == TaskCheckpointStatus::Limited {
        reasons.push(String::from(
            "Probe's pre-edit checkpoint coverage for this task is limited, so any later revert flow may require manual review.",
        ));
    }
    if workspace.change_accounting_limited {
        reasons.push(String::from(
            "Changed-file accounting is limited because write-capable shell commands ran without file-level tracking.",
        ));
    }
    if !workspace.changed_files.is_empty() && verification_status == TaskVerificationStatus::NotRun
    {
        reasons.push(String::from(
            "Edits landed without an observed validation command.",
        ));
    }
    if verification_status == TaskVerificationStatus::TimedOut {
        reasons.push(String::from(
            "At least one validation command timed out before Probe observed a full result.",
        ));
    }
    if verification_status == TaskVerificationStatus::Mixed {
        reasons.push(String::from(
            "Validation results were mixed across the commands Probe observed.",
        ));
    }
    let truncated = verification_commands
        .iter()
        .filter(|command| command.truncated_output)
        .map(|command| summarize_task_command(command.command.as_str()))
        .collect::<Vec<_>>();
    if !truncated.is_empty() {
        reasons.push(format!(
            "Validation output was truncated for {}.",
            summarize_task_paths(truncated.as_slice(), 3)
        ));
    }
    if disposition == TaskReceiptDisposition::PendingApproval {
        reasons.push(String::from(
            "The task is still waiting for approval and may continue changing the workspace after approval.",
        ));
    }
    if disposition == TaskReceiptDisposition::Stopped {
        reasons.push(String::from(
            "The task was stopped before completion, so follow-up work may still be needed.",
        ));
    }
    reasons
}

fn task_final_receipt_text(
    disposition: TaskReceiptDisposition,
    workspace: &TaskWorkspaceSummary,
    verification_status: TaskVerificationStatus,
    verification_commands: &[TaskVerificationCommandSummary],
    uncertainty_reasons: &[String],
) -> String {
    let mut sentences = vec![task_receipt_workspace_sentence(disposition, workspace)];
    sentences.push(task_receipt_verification_sentence(
        disposition,
        workspace,
        verification_status,
        verification_commands,
    ));
    if !uncertainty_reasons.is_empty() {
        sentences.push(format!(
            "Remaining uncertainty: {}.",
            summarize_task_paths(uncertainty_reasons, 2)
        ));
    }
    sentences.join(" ")
}

fn task_receipt_workspace_sentence(
    disposition: TaskReceiptDisposition,
    workspace: &TaskWorkspaceSummary,
) -> String {
    match (disposition, workspace.status) {
        (TaskReceiptDisposition::Failed, TaskWorkspaceSummaryStatus::NoRepoChanges) => {
            String::from("Task failed before repo changes landed.")
        }
        (TaskReceiptDisposition::Failed, TaskWorkspaceSummaryStatus::ChangeAccountingLimited) => {
            String::from(
                "Task failed after write-capable shell work, but Probe cannot confirm whether repo changes landed.",
            )
        }
        (TaskReceiptDisposition::Stopped, TaskWorkspaceSummaryStatus::NoRepoChanges) => {
            String::from("Task was stopped before repo changes landed.")
        }
        (TaskReceiptDisposition::Stopped, TaskWorkspaceSummaryStatus::ChangeAccountingLimited) => {
            String::from(
                "Task was stopped after write-capable shell work, but Probe cannot confirm whether repo changes landed.",
            )
        }
        (_, TaskWorkspaceSummaryStatus::Reverted) => workspace.summary_text.clone(),
        (TaskReceiptDisposition::Stopped, _) => {
            if workspace.changed_files.is_empty() {
                String::from("Task was stopped before repo changes landed.")
            } else {
                format!(
                    "Task was stopped after changes landed in {} file(s): {}.",
                    workspace.changed_files.len(),
                    summarize_task_paths(workspace.changed_files.as_slice(), 6)
                )
            }
        }
        _ => workspace.summary_text.clone(),
    }
}

fn task_receipt_verification_sentence(
    disposition: TaskReceiptDisposition,
    workspace: &TaskWorkspaceSummary,
    verification_status: TaskVerificationStatus,
    verification_commands: &[TaskVerificationCommandSummary],
) -> String {
    match verification_status {
        TaskVerificationStatus::NotRun => {
            if disposition == TaskReceiptDisposition::PendingApproval {
                String::from("No validation command has completed yet.")
            } else if disposition == TaskReceiptDisposition::Stopped {
                String::from("No validation command completed before the task was stopped.")
            } else if workspace.changed_files.is_empty() {
                String::from("No validation command was observed for this task.")
            } else {
                String::from("No validation command was observed after the edits landed.")
            }
        }
        TaskVerificationStatus::Passed => {
            let prefix = if disposition == TaskReceiptDisposition::Stopped {
                "Validation passed before the task was stopped"
            } else {
                "Validation passed"
            };
            format!(
                "{prefix}: {}.",
                summarize_task_verification_commands(verification_commands, 3)
            )
        }
        TaskVerificationStatus::Failed => {
            let prefix = if disposition == TaskReceiptDisposition::Stopped {
                "Validation failed before the task was stopped"
            } else {
                "Validation failed"
            };
            format!(
                "{prefix}: {}.",
                summarize_task_verification_commands(verification_commands, 3)
            )
        }
        TaskVerificationStatus::TimedOut => {
            let prefix = if disposition == TaskReceiptDisposition::Stopped {
                "Validation timed out before the task was stopped"
            } else {
                "Validation timed out"
            };
            format!(
                "{prefix}: {}.",
                summarize_task_verification_commands(verification_commands, 3)
            )
        }
        TaskVerificationStatus::Mixed => {
            let prefix = if disposition == TaskReceiptDisposition::Stopped {
                "Validation results were mixed before the task was stopped"
            } else {
                "Validation results were mixed"
            };
            format!(
                "{prefix}: {}.",
                summarize_task_verification_commands(verification_commands, 3)
            )
        }
    }
}

fn summarize_task_paths(paths: &[String], max_items: usize) -> String {
    let mut items = paths.iter().take(max_items).cloned().collect::<Vec<_>>();
    let remaining = paths.len().saturating_sub(items.len());
    if remaining > 0 {
        items.push(format!("and {remaining} more"));
    }
    items.join(", ")
}

fn summarize_task_verification_commands(
    commands: &[TaskVerificationCommandSummary],
    max_items: usize,
) -> String {
    let mut items = commands
        .iter()
        .take(max_items)
        .map(|command| {
            let status = match command.status {
                TaskVerificationCommandStatus::Passed => "passed",
                TaskVerificationCommandStatus::Failed => "failed",
                TaskVerificationCommandStatus::TimedOut => "timed out",
            };
            if command.truncated_output {
                format!(
                    "{} ({status}, output truncated)",
                    summarize_task_command(command.command.as_str())
                )
            } else {
                format!(
                    "{} ({status})",
                    summarize_task_command(command.command.as_str())
                )
            }
        })
        .collect::<Vec<_>>();
    let remaining = commands.len().saturating_sub(items.len());
    if remaining > 0 {
        items.push(format!("and {remaining} more"));
    }
    items.join(", ")
}

fn summarize_task_command(command: &str) -> String {
    let command = command.trim();
    let mut chars = command.chars();
    let preview = chars.by_ref().take(60).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}...")
    } else if preview.is_empty() {
        String::from("[empty command]")
    } else {
        preview
    }
}

fn dirty_files_outside_tool_tracking(
    observed_dirty_files_after_task: &[String],
    preexisting_dirty_files: &[String],
    changed_files: &[String],
) -> Vec<String> {
    let tracked = preexisting_dirty_files
        .iter()
        .chain(changed_files.iter())
        .cloned()
        .collect::<HashSet<_>>();
    observed_dirty_files_after_task
        .iter()
        .filter(|path| !tracked.contains(*path))
        .cloned()
        .collect()
}

fn dirty_worktree_paths(cwd: &Path, repo_root: Option<&Path>) -> Vec<String> {
    let Some(repo_root) = repo_root else {
        return Vec::new();
    };
    let Some(output) = run_git_output(
        repo_root,
        &["status", "--porcelain", "--untracked-files=all"],
    ) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for line in output.lines() {
        let Some(raw_path) = parse_git_status_path(line) else {
            continue;
        };
        let absolute_path = repo_root.join(raw_path);
        push_unique_path(
            &mut paths,
            &render_task_workspace_path(cwd, repo_root, absolute_path.as_path()),
        );
    }
    paths
}

fn parse_git_status_path(line: &str) -> Option<&str> {
    if line.len() < 4 {
        return None;
    }
    let path = line.get(3..)?.trim();
    if path.is_empty() {
        return None;
    }
    Some(path.rsplit_once(" -> ").map_or(path, |(_, next)| next))
}

fn render_task_workspace_path(cwd: &Path, repo_root: &Path, path: &Path) -> String {
    let cwd_abs = if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(cwd)
    };
    if let Ok(relative) = path.strip_prefix(&cwd_abs) {
        return relative.display().to_string();
    }
    if let Ok(relative) = path.strip_prefix(repo_root) {
        return relative.display().to_string();
    }
    path.display().to_string()
}

fn push_unique_path(paths: &mut Vec<String>, path: &str) {
    if paths.iter().any(|existing| existing == path) {
        return;
    }
    paths.push(path.to_string());
}

fn tool_result_applied(item: &TranscriptItem) -> bool {
    let Some(tool_execution) = item.tool_execution.as_ref() else {
        return false;
    };
    if !matches!(
        tool_execution.policy_decision,
        ToolPolicyDecision::AutoAllow | ToolPolicyDecision::Approved
    ) {
        return false;
    }
    !tool_result_has_error(item)
}

fn tool_result_has_error(item: &TranscriptItem) -> bool {
    serde_json::from_str::<serde_json::Value>(&item.text)
        .ok()
        .and_then(|value| value.get("error").cloned())
        .is_some()
}

fn spawn_turn_worker(
    turn_control: Arc<TurnControlPlane>,
    runtime: ProbeRuntime,
    writer: SharedJsonlWriter,
    detached_event_hub: Option<Arc<DetachedSessionEventHub>>,
    request_id: Option<String>,
    request: TurnRequest,
    mode: TurnMode,
    turn_id: String,
) -> Result<(), ServerError> {
    let session_id = request.session_id.clone();
    let turn_id_for_worker = turn_id.clone();
    let thread_name = format!("probe-server-turn-{}", session_id.as_str());
    thread::Builder::new().name(thread_name).spawn(move || {
        let response = run_turn_request(
            Arc::clone(&turn_control),
            &runtime,
            &writer,
            detached_event_hub.clone(),
            request_id.as_deref(),
            request,
            mode,
            turn_id_for_worker.as_str(),
        );

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
            spawn_next_queued_turn_if_ready(
                turn_control,
                runtime,
                writer,
                detached_event_hub,
                session_id,
            );
        }
    })?;
    Ok(())
}

fn spawn_approval_resolution_worker(
    turn_control: Arc<TurnControlPlane>,
    runtime: ProbeRuntime,
    writer: SharedJsonlWriter,
    detached_event_hub: Option<Arc<DetachedSessionEventHub>>,
    request_id: String,
    request: probe_protocol::runtime::ResolvePendingApprovalRequest,
    turn_id: String,
) -> Result<(), ServerError> {
    let session_id = request.session_id.clone();
    let thread_name = format!("probe-server-approval-{}", session_id.as_str());
    thread::Builder::new().name(thread_name).spawn(move || {
        let response = run_pending_approval_resolution(
            Arc::clone(&turn_control),
            &runtime,
            &writer,
            detached_event_hub.clone(),
            Some(request_id.as_str()),
            request,
            turn_id.as_str(),
        );

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
            spawn_next_queued_turn_if_ready(
                turn_control,
                runtime,
                writer,
                detached_event_hub,
                session_id,
            );
        }
    })?;
    Ok(())
}

fn spawn_next_queued_turn_if_ready(
    turn_control: Arc<TurnControlPlane>,
    runtime: ProbeRuntime,
    writer: SharedJsonlWriter,
    detached_event_hub: Option<Arc<DetachedSessionEventHub>>,
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
            detached_event_hub,
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

fn spawn_detached_watchdog(
    core: ProbeServerCore,
    watchdog_policy: DetachedTurnWatchdogPolicy,
) -> Result<(), io::Error> {
    thread::Builder::new()
        .name(String::from("probe-daemon-watchdog"))
        .spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(watchdog_policy.poll_interval_ms));
                let _ = core.turn_control.enforce_detached_watchdog_policy();
            }
        })?;
    Ok(())
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

fn cancel_queued_turns_after_watchdog(
    state: &mut SessionTurnControlState,
    active_turn_id: &str,
    now_ms: u64,
) -> Vec<String> {
    let mut notes = Vec::new();
    for turn in &mut state.turns {
        if turn.record.status != QueuedTurnStatus::Queued {
            continue;
        }
        turn.record.status = QueuedTurnStatus::Cancelled;
        turn.record.finished_at_ms = Some(now_ms);
        turn.record.cancellation_reason = Some(format!(
            "cancelled because detached turn {} timed out before this queued turn could start",
            active_turn_id
        ));
        notes.push(format!(
            "probe-daemon cancelled queued turn {} from {} after detached turn {} timed out: {}",
            turn.record.turn_id,
            render_turn_author(&turn.record.author),
            active_turn_id,
            render_prompt_excerpt(turn.record.prompt.as_str()),
        ));
    }
    notes
}

fn watchdog_note_code(trigger: DetachedTurnWatchdogTrigger) -> &'static str {
    match trigger {
        DetachedTurnWatchdogTrigger::ProgressStalled { .. } => "detached_turn_watchdog_stalled",
        DetachedTurnWatchdogTrigger::ExecutionTimedOut { .. } => "detached_turn_execution_timeout",
    }
}

fn watchdog_failure_message(
    turn: &StoredTurnControlRecord,
    trigger: DetachedTurnWatchdogTrigger,
) -> String {
    match trigger {
        DetachedTurnWatchdogTrigger::ProgressStalled {
            last_progress_at_ms,
            stall_timeout_ms,
        } => format!(
            "probe-daemon watchdog marked detached turn {} as timed out after {}ms without runtime progress since {}",
            turn.record.turn_id, stall_timeout_ms, last_progress_at_ms
        ),
        DetachedTurnWatchdogTrigger::ExecutionTimedOut {
            timeout_at_ms,
            execution_timeout_ms,
        } => format!(
            "probe-daemon timed out detached turn {} after exceeding the {}ms execution limit at {}",
            turn.record.turn_id, execution_timeout_ms, timeout_at_ms
        ),
    }
}

fn watchdog_transcript_note(
    turn: &StoredTurnControlRecord,
    trigger: DetachedTurnWatchdogTrigger,
) -> String {
    match trigger {
        DetachedTurnWatchdogTrigger::ProgressStalled {
            last_progress_at_ms,
            stall_timeout_ms,
        } => format!(
            "probe-daemon watchdog timed out detached turn {} from {} after {}ms without runtime progress since {}: {}",
            turn.record.turn_id,
            render_turn_author(&turn.record.author),
            stall_timeout_ms,
            last_progress_at_ms,
            render_prompt_excerpt(turn.record.prompt.as_str()),
        ),
        DetachedTurnWatchdogTrigger::ExecutionTimedOut {
            timeout_at_ms,
            execution_timeout_ms,
        } => format!(
            "probe-daemon timed out detached turn {} from {} after exceeding the {}ms execution limit at {}: {}",
            turn.record.turn_id,
            render_turn_author(&turn.record.author),
            execution_timeout_ms,
            timeout_at_ms,
            render_prompt_excerpt(turn.record.prompt.as_str()),
        ),
    }
}

fn watchdog_event_message(
    turn: &StoredTurnControlRecord,
    trigger: DetachedTurnWatchdogTrigger,
    queued_cancelled: usize,
) -> String {
    let summary = match trigger {
        DetachedTurnWatchdogTrigger::ProgressStalled {
            last_progress_at_ms,
            stall_timeout_ms,
        } => format!(
            "detached turn {} stalled for {}ms after last progress at {}",
            turn.record.turn_id, stall_timeout_ms, last_progress_at_ms
        ),
        DetachedTurnWatchdogTrigger::ExecutionTimedOut {
            timeout_at_ms,
            execution_timeout_ms,
        } => format!(
            "detached turn {} exceeded the {}ms execution limit at {}",
            turn.record.turn_id, execution_timeout_ms, timeout_at_ms
        ),
    };
    if queued_cancelled == 0 {
        summary
    } else {
        format!("{summary}; cancelled {queued_cancelled} queued follow-up turn(s)")
    }
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
    turn_control: Arc<TurnControlPlane>,
    runtime: &ProbeRuntime,
    writer: &SharedJsonlWriter,
    detached_event_hub: Option<Arc<DetachedSessionEventHub>>,
    request_id: Option<&str>,
    request: TurnRequest,
    mode: TurnMode,
    turn_id: &str,
) -> Result<RuntimeResponse, RuntimeProtocolError> {
    let task_baseline = capture_resumable_task_workspace_baseline(runtime, &request.session_id)?;
    let tool_loop = request.tool_loop.map(tool_loop_from_recipe).transpose()?;
    let event_sink = request_id
        .map(|request_id| (Some(String::from(request_id)), detached_event_hub.clone()))
        .or_else(|| detached_event_hub.clone().map(|hub| (None, Some(hub))))
        .map(|(request_id, detached_event_hub)| {
            let writer_for_events = writer.clone();
            let session_id_for_events = request.session_id.clone();
            let turn_control_for_events = Arc::clone(&turn_control);
            let turn_id_for_events = String::from(turn_id);
            let last_activity_for_events = Arc::new(Mutex::new(None::<RuntimeActivity>));
            Arc::new(move |event| {
                let _ = turn_control_for_events
                    .record_runtime_progress(&session_id_for_events, turn_id_for_events.as_str());
                let activity_update = derived_runtime_activity(&event);
                emit_runtime_progress(
                    &writer_for_events,
                    request_id.as_ref(),
                    detached_event_hub.as_ref(),
                    &session_id_for_events,
                    delivery_for_runtime_event(&event),
                    encode_runtime_event(event),
                );
                if let Some((activity_session_id, activity)) = activity_update {
                    let mut last_activity = last_activity_for_events
                        .lock()
                        .expect("runtime activity mutex should not be poisoned");
                    if last_activity.as_ref() != Some(&activity) {
                        *last_activity = Some(activity.clone());
                        emit_runtime_progress(
                            &writer_for_events,
                            request_id.as_ref(),
                            detached_event_hub.as_ref(),
                            &activity_session_id,
                            EventDeliveryGuarantee::Lossless,
                            RuntimeProgressEvent::ActivityUpdated {
                                session_id: activity_session_id.clone(),
                                activity,
                            },
                        );
                    }
                }
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
        Ok(outcome) => {
            let _ = persist_latest_task_receipt(
                runtime,
                &request.session_id,
                &task_baseline,
                TaskReceiptDisposition::Succeeded,
            );
            turn_response_to_runtime_response(
                TurnResponse::Completed(turn_completed(
                    runtime,
                    turn_control.hosted_receipt_config.as_ref(),
                    outcome,
                )?),
                mode,
            )
        }
        Err(RuntimeError::ToolApprovalPending {
            session_id,
            tool_name,
            call_id,
            reason,
        }) => {
            let session = persist_latest_task_receipt(
                runtime,
                &session_id,
                &task_baseline,
                TaskReceiptDisposition::PendingApproval,
            )
            .unwrap_or_else(|_| {
                runtime
                    .session_store()
                    .read_metadata(&session_id)
                    .expect("session metadata should remain readable after pending approval")
            });
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
            if let Some(detached_event_hub) = detached_event_hub.as_ref() {
                detached_event_hub
                    .append(
                        &session_id,
                        DetachedSessionEventTruth::Authoritative,
                        DetachedSessionEventPayload::PendingApprovalsUpdated {
                            approvals: pending_approvals.clone(),
                        },
                        now_ms(),
                    )
                    .map_err(detached_event_error_to_protocol)?;
            }
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
        Err(error) => {
            let _ = persist_latest_task_receipt(
                runtime,
                &request.session_id,
                &task_baseline,
                TaskReceiptDisposition::Failed,
            );
            return Err(runtime_error_to_protocol(error));
        }
    };
    Ok(response)
}

fn run_pending_approval_resolution(
    turn_control: Arc<TurnControlPlane>,
    runtime: &ProbeRuntime,
    writer: &SharedJsonlWriter,
    detached_event_hub: Option<Arc<DetachedSessionEventHub>>,
    request_id: Option<&str>,
    request: probe_protocol::runtime::ResolvePendingApprovalRequest,
    turn_id: &str,
) -> Result<RuntimeResponse, RuntimeProtocolError> {
    let task_baseline = capture_task_workspace_baseline(runtime, &request.session_id)?;
    let tool_loop = tool_loop_from_recipe(request.tool_loop)?;
    let event_sink = request_id
        .map(|request_id| (Some(String::from(request_id)), detached_event_hub.clone()))
        .or_else(|| detached_event_hub.clone().map(|hub| (None, Some(hub))))
        .map(|(request_id, detached_event_hub)| {
            let writer_for_events = writer.clone();
            let session_id_for_events = request.session_id.clone();
            let turn_control_for_events = Arc::clone(&turn_control);
            let turn_id_for_events = String::from(turn_id);
            let last_activity_for_events = Arc::new(Mutex::new(None::<RuntimeActivity>));
            Arc::new(move |event| {
                let _ = turn_control_for_events
                    .record_runtime_progress(&session_id_for_events, turn_id_for_events.as_str());
                let activity_update = derived_runtime_activity(&event);
                emit_runtime_progress(
                    &writer_for_events,
                    request_id.as_ref(),
                    detached_event_hub.as_ref(),
                    &session_id_for_events,
                    delivery_for_runtime_event(&event),
                    encode_runtime_event(event),
                );
                if let Some((activity_session_id, activity)) = activity_update {
                    let mut last_activity = last_activity_for_events
                        .lock()
                        .expect("runtime activity mutex should not be poisoned");
                    if last_activity.as_ref() != Some(&activity) {
                        *last_activity = Some(activity.clone());
                        emit_runtime_progress(
                            &writer_for_events,
                            request_id.as_ref(),
                            detached_event_hub.as_ref(),
                            &activity_session_id,
                            EventDeliveryGuarantee::Lossless,
                            RuntimeProgressEvent::ActivityUpdated {
                                session_id: activity_session_id.clone(),
                                activity,
                            },
                        );
                    }
                }
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

    match result {
        Ok(ResolvePendingToolApprovalOutcome::StillPending {
            session: _,
            pending_approvals,
        }) => {
            let session = persist_latest_task_receipt(
                runtime,
                &request.session_id,
                &task_baseline,
                TaskReceiptDisposition::PendingApproval,
            )
            .unwrap_or_else(|_| {
                runtime
                    .session_store()
                    .read_metadata(&request.session_id)
                    .expect("session metadata should remain readable after pending approval")
            });
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
            if let Some(detached_event_hub) = detached_event_hub.as_ref() {
                detached_event_hub
                    .append(
                        &session.id,
                        DetachedSessionEventTruth::Authoritative,
                        DetachedSessionEventPayload::PendingApprovalsUpdated {
                            approvals: pending_approvals.clone(),
                        },
                        now_ms(),
                    )
                    .map_err(detached_event_error_to_protocol)?;
            }
            Ok(RuntimeResponse::ResolvePendingApproval(
                ResolvePendingApprovalResponse::StillPending {
                    session,
                    pending_approvals,
                },
            ))
        }
        Ok(ResolvePendingToolApprovalOutcome::Resumed { outcome }) => {
            let _ = persist_latest_task_receipt(
                runtime,
                &request.session_id,
                &task_baseline,
                TaskReceiptDisposition::Succeeded,
            );
            Ok(RuntimeResponse::ResolvePendingApproval(
                ResolvePendingApprovalResponse::Resumed(turn_completed(
                    runtime,
                    turn_control.hosted_receipt_config.as_ref(),
                    outcome,
                )?),
            ))
        }
        Err(error) => {
            let _ = persist_latest_task_receipt(
                runtime,
                &request.session_id,
                &task_baseline,
                TaskReceiptDisposition::Failed,
            );
            Err(runtime_error_to_protocol(error))
        }
    }
}

fn session_snapshot_from_core(
    core: &ProbeServerCore,
    session_id: &SessionId,
) -> Result<SessionSnapshot, RuntimeProtocolError> {
    let session = core
        .runtime
        .session_store()
        .read_metadata(session_id)
        .map_err(session_store_error_to_protocol)?;
    let transcript = core
        .runtime
        .session_store()
        .read_transcript(session_id)
        .map_err(session_store_error_to_protocol)?;
    let session = sync_hosted_session_metadata_from_store(
        core.runtime.session_store(),
        core.hosted_receipt_config.as_ref(),
        session,
        transcript.as_slice(),
        now_ms(),
    )?;
    let child_sessions = session
        .child_links
        .iter()
        .filter_map(
            |link| match session_child_summary_from_runtime(core, &link.session_id) {
                Ok(summary) => Some(summary),
                Err(error) if error.code == "session_not_found" => None,
                Err(_error) => Some(SessionChildSummary {
                    session_id: link.session_id.clone(),
                    title: String::from("[missing child session]"),
                    cwd: session.cwd.clone(),
                    state: probe_protocol::session::SessionState::Archived,
                    status: SessionChildStatus::Failed,
                    initiator: None,
                    purpose: None,
                    parent_turn_id: None,
                    parent_turn_index: None,
                    closure: Some(SessionChildClosureSummary {
                        status: SessionChildStatus::Failed,
                        delivery_status: None,
                        branch_name: None,
                        head_commit: None,
                        compare_ref: None,
                        updated_at_ms: session.updated_at_ms,
                    }),
                    created_at_ms: session.created_at_ms,
                    updated_at_ms: session.updated_at_ms,
                }),
            },
        )
        .collect();
    let pending_approvals = core
        .runtime
        .pending_tool_approvals(session_id)
        .map_err(runtime_error_to_protocol)?;
    let branch_state = session_branch_state(session.cwd.as_path());
    let delivery_state = branch_state
        .as_ref()
        .map(|branch_state| session_delivery_state(branch_state, now_ms()));
    let summary_artifacts = refresh_session_summary_artifacts(
        core.runtime.session_store(),
        &session,
        transcript.as_slice(),
        branch_state.as_ref(),
        delivery_state.as_ref(),
    )
    .map_err(session_summary_artifact_error_to_protocol)?;
    Ok(SessionSnapshot {
        session,
        branch_state,
        delivery_state,
        summary_artifacts,
        child_sessions,
        transcript,
        pending_approvals,
    })
}

fn session_child_summary_from_runtime(
    core: &ProbeServerCore,
    session_id: &SessionId,
) -> Result<SessionChildSummary, RuntimeProtocolError> {
    let session = core
        .runtime
        .session_store()
        .read_metadata(session_id)
        .map_err(session_store_error_to_protocol)?;
    let status = if let Some(registry) = core.detached_registry.as_ref() {
        registry
            .read(session_id)
            .map_err(detached_registry_error_to_protocol)?
            .map(|summary| session_child_status_from_detached(summary.status))
            .unwrap_or_else(|| {
                session_child_status_from_state(core, session_id)
                    .unwrap_or(SessionChildStatus::Idle)
            })
    } else {
        session_child_status_from_state(core, session_id)?
    };
    let branch_state = session_branch_state(session.cwd.as_path());
    let delivery_state = branch_state
        .as_ref()
        .map(|branch_state| session_delivery_state(branch_state, now_ms()));
    Ok(SessionChildSummary {
        session_id: session.id.clone(),
        title: session.title.clone(),
        cwd: session.cwd.clone(),
        state: session.state,
        status,
        initiator: session
            .parent_link
            .as_ref()
            .and_then(|link| link.initiator.clone()),
        purpose: session
            .parent_link
            .as_ref()
            .and_then(|link| link.purpose.clone()),
        parent_turn_id: session
            .parent_link
            .as_ref()
            .and_then(|link| link.turn_id.clone()),
        parent_turn_index: session
            .parent_link
            .as_ref()
            .and_then(|link| link.turn_index),
        closure: session_child_closure(
            status,
            branch_state.as_ref(),
            delivery_state.as_ref(),
            session.updated_at_ms,
        ),
        created_at_ms: session.created_at_ms,
        updated_at_ms: session.updated_at_ms,
    })
}

fn session_child_summary_from_snapshot(snapshot: &SessionSnapshot) -> SessionChildSummary {
    SessionChildSummary {
        session_id: snapshot.session.id.clone(),
        title: snapshot.session.title.clone(),
        cwd: snapshot.session.cwd.clone(),
        state: snapshot.session.state,
        status: SessionChildStatus::Idle,
        initiator: snapshot
            .session
            .parent_link
            .as_ref()
            .and_then(|link| link.initiator.clone()),
        purpose: snapshot
            .session
            .parent_link
            .as_ref()
            .and_then(|link| link.purpose.clone()),
        parent_turn_id: snapshot
            .session
            .parent_link
            .as_ref()
            .and_then(|link| link.turn_id.clone()),
        parent_turn_index: snapshot
            .session
            .parent_link
            .as_ref()
            .and_then(|link| link.turn_index),
        closure: None,
        created_at_ms: snapshot.session.created_at_ms,
        updated_at_ms: snapshot.session.updated_at_ms,
    }
}

fn session_child_status_from_state(
    core: &ProbeServerCore,
    session_id: &SessionId,
) -> Result<SessionChildStatus, RuntimeProtocolError> {
    let state = SessionTurnControlState::load(&core.runtime, session_id)
        .map_err(session_store_error_to_protocol)?;
    let view = state.inspect_view(session_id);
    if let Some(active_turn) = view.active_turn.as_ref() {
        return Ok(if active_turn.awaiting_approval {
            SessionChildStatus::ApprovalPaused
        } else {
            SessionChildStatus::Running
        });
    }
    if !view.queued_turns.is_empty() {
        return Ok(SessionChildStatus::Queued);
    }
    Ok(preferred_terminal_turn(view.recent_turns.as_slice())
        .map(|turn| session_child_status_from_terminal(turn.status))
        .unwrap_or(SessionChildStatus::Idle))
}

fn session_child_status_from_terminal(status: QueuedTurnStatus) -> SessionChildStatus {
    match status {
        QueuedTurnStatus::Queued => SessionChildStatus::Queued,
        QueuedTurnStatus::Running => SessionChildStatus::Running,
        QueuedTurnStatus::Completed => SessionChildStatus::Completed,
        QueuedTurnStatus::Failed => SessionChildStatus::Failed,
        QueuedTurnStatus::Cancelled => SessionChildStatus::Cancelled,
        QueuedTurnStatus::TimedOut => SessionChildStatus::TimedOut,
    }
}

fn session_child_status_from_detached(status: DetachedSessionStatus) -> SessionChildStatus {
    match status {
        DetachedSessionStatus::Idle => SessionChildStatus::Idle,
        DetachedSessionStatus::Running => SessionChildStatus::Running,
        DetachedSessionStatus::Queued => SessionChildStatus::Queued,
        DetachedSessionStatus::ApprovalPaused => SessionChildStatus::ApprovalPaused,
        DetachedSessionStatus::Completed => SessionChildStatus::Completed,
        DetachedSessionStatus::Failed => SessionChildStatus::Failed,
        DetachedSessionStatus::Cancelled => SessionChildStatus::Cancelled,
        DetachedSessionStatus::TimedOut => SessionChildStatus::TimedOut,
    }
}

fn session_child_closure(
    status: SessionChildStatus,
    branch_state: Option<&SessionBranchState>,
    delivery_state: Option<&SessionDeliveryState>,
    updated_at_ms: u64,
) -> Option<SessionChildClosureSummary> {
    match status {
        SessionChildStatus::Completed
        | SessionChildStatus::Failed
        | SessionChildStatus::Cancelled
        | SessionChildStatus::TimedOut => Some(SessionChildClosureSummary {
            status,
            delivery_status: delivery_state.map(|state| state.status),
            branch_name: delivery_state
                .and_then(|state| state.branch_name.clone())
                .or_else(|| branch_state.map(|state| state.head_ref.clone())),
            head_commit: branch_state.map(|state| state.head_commit.clone()),
            compare_ref: delivery_state.and_then(|state| state.compare_ref.clone()),
            updated_at_ms,
        }),
        SessionChildStatus::Idle
        | SessionChildStatus::Running
        | SessionChildStatus::Queued
        | SessionChildStatus::ApprovalPaused => None,
    }
}

fn session_initiator_from_turn_author(
    author: &probe_protocol::runtime::TurnAuthor,
) -> SessionInitiator {
    SessionInitiator {
        client_name: author.client_name.clone(),
        client_version: author.client_version.clone(),
        display_name: author.display_name.clone(),
        participant_id: author.participant_id.clone(),
    }
}

fn load_session_metadata_for_collaboration(
    core: &ProbeServerCore,
    session_id: &SessionId,
) -> Result<SessionMetadata, RuntimeProtocolError> {
    let metadata = core
        .runtime
        .session_store()
        .read_metadata(session_id)
        .map_err(session_store_error_to_protocol)?;
    let transcript = core
        .runtime
        .session_store()
        .read_transcript(session_id)
        .map_err(session_store_error_to_protocol)?;
    sync_hosted_session_metadata_from_store(
        core.runtime.session_store(),
        core.hosted_receipt_config.as_ref(),
        metadata,
        transcript.as_slice(),
        now_ms(),
    )
}

fn persist_session_collaboration_metadata(
    core: &ProbeServerCore,
    mut metadata: SessionMetadata,
    history_event: Option<SessionHostedLifecycleEvent>,
) -> Result<SessionMetadata, RuntimeProtocolError> {
    if let (Some(receipts), Some(event)) = (metadata.hosted_receipts.as_mut(), history_event) {
        push_hosted_history_event(&mut receipts.history, event);
    }
    let metadata = core
        .runtime
        .session_store()
        .replace_metadata(metadata)
        .map_err(session_store_error_to_protocol)?;
    core.turn_control
        .sync_detached_session_if_tracked(&metadata.id)?;
    Ok(metadata)
}

fn prepare_session_authority_for_action(
    core: &ProbeServerCore,
    session_id: &SessionId,
    author: Option<&TurnAuthor>,
    action_label: &str,
) -> Result<SessionMetadata, RuntimeProtocolError> {
    let mut metadata = load_session_metadata_for_collaboration(core, session_id)?;
    let now = now_ms();
    let shared_mode = !metadata.participants.is_empty() || metadata.controller_lease.is_some();
    if let Some(author) = author {
        upsert_session_participant(&mut metadata, author, now, false)?;
    } else if is_hosted_shared_mode(&metadata) {
        return Err(participant_required_error(session_id, action_label));
    }

    let mut history_event = None;
    if is_hosted_session(&metadata) {
        if let Some(controller) = metadata.controller_lease.clone() {
            let Some(actor_id) = author.and_then(normalized_participant_id) else {
                return Err(participant_required_error(session_id, action_label));
            };
            if actor_id != controller.participant_id {
                return Err(controller_conflict_error(
                    &metadata,
                    controller.participant_id.as_str(),
                    action_label,
                ));
            }
        } else if let Some(author) = author {
            if normalized_participant_id(author).is_some() {
                history_event = apply_session_controller_action(
                    &mut metadata,
                    author,
                    SessionControllerAction::Claim,
                    None,
                    now,
                )?;
            }
        } else if shared_mode {
            return Err(participant_required_error(session_id, action_label));
        }
    }

    if author.is_none() && history_event.is_none() {
        return Ok(metadata);
    }
    persist_session_collaboration_metadata(core, metadata, history_event)
}

fn normalized_participant_id(author: &TurnAuthor) -> Option<String> {
    author
        .participant_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from)
}

fn is_hosted_session(metadata: &SessionMetadata) -> bool {
    metadata
        .runtime_owner
        .as_ref()
        .is_some_and(|owner| owner.kind == SessionRuntimeOwnerKind::HostedControlPlane)
}

fn is_hosted_shared_mode(metadata: &SessionMetadata) -> bool {
    is_hosted_session(metadata)
        && (!metadata.participants.is_empty() || metadata.controller_lease.is_some())
}

fn participant_required_error(session_id: &SessionId, action_label: &str) -> RuntimeProtocolError {
    protocol_error(
        "participant_id_required",
        format!(
            "cannot {action_label} for hosted session {} without a participant_id once shared session control is active",
            session_id.as_str()
        ),
    )
}

fn controller_conflict_error(
    metadata: &SessionMetadata,
    controller_participant_id: &str,
    action_label: &str,
) -> RuntimeProtocolError {
    protocol_error(
        "session_controller_conflict",
        format!(
            "cannot {action_label} because hosted session {} is currently controlled by {}; hand off or take over control first",
            metadata.id.as_str(),
            session_participant_label(metadata, controller_participant_id),
        ),
    )
}

fn upsert_session_participant(
    metadata: &mut SessionMetadata,
    author: &TurnAuthor,
    recorded_at_ms: u64,
    require_participant_id: bool,
) -> Result<Option<String>, RuntimeProtocolError> {
    let Some(participant_id) = normalized_participant_id(author) else {
        return if require_participant_id {
            Err(protocol_error(
                "participant_id_required",
                "hosted shared-session actions require a non-empty participant_id",
            ))
        } else {
            Ok(None)
        };
    };

    if let Some(existing) = metadata
        .participants
        .iter_mut()
        .find(|participant| participant.participant_id == participant_id)
    {
        existing.client_name = author.client_name.clone();
        existing.client_version = author
            .client_version
            .clone()
            .or_else(|| existing.client_version.clone());
        existing.display_name = author
            .display_name
            .clone()
            .or_else(|| existing.display_name.clone());
        existing.last_seen_at_ms = recorded_at_ms;
    } else {
        metadata.participants.push(SessionParticipant {
            participant_id: participant_id.clone(),
            client_name: author.client_name.clone(),
            client_version: author.client_version.clone(),
            display_name: author.display_name.clone(),
            attached_at_ms: recorded_at_ms,
            last_seen_at_ms: recorded_at_ms,
        });
    }
    Ok(Some(participant_id))
}

fn apply_session_controller_action(
    metadata: &mut SessionMetadata,
    actor: &TurnAuthor,
    action: SessionControllerAction,
    target_participant_id: Option<String>,
    recorded_at_ms: u64,
) -> Result<Option<SessionHostedLifecycleEvent>, RuntimeProtocolError> {
    let actor_participant_id = upsert_session_participant(metadata, actor, recorded_at_ms, true)?
        .expect("required participant id should be present");
    let current_controller = metadata.controller_lease.clone();
    let history_event = match action {
        SessionControllerAction::Claim => {
            if let Some(controller) = current_controller {
                if controller.participant_id == actor_participant_id {
                    return Ok(None);
                }
                return Err(controller_conflict_error(
                    metadata,
                    controller.participant_id.as_str(),
                    "claim control",
                ));
            }
            metadata.controller_lease = Some(SessionControllerLease {
                participant_id: actor_participant_id.clone(),
                acquired_at_ms: recorded_at_ms,
            });
            hosted_controller_history_event(
                metadata,
                action,
                actor_participant_id,
                None,
                recorded_at_ms,
            )
        }
        SessionControllerAction::Release => {
            let Some(controller) = current_controller else {
                return Ok(None);
            };
            if controller.participant_id != actor_participant_id {
                return Err(controller_conflict_error(
                    metadata,
                    controller.participant_id.as_str(),
                    "release control",
                ));
            }
            metadata.controller_lease = None;
            hosted_controller_history_event(
                metadata,
                action,
                actor_participant_id,
                None,
                recorded_at_ms,
            )
        }
        SessionControllerAction::Handoff => {
            let Some(controller) = current_controller else {
                return Err(protocol_error(
                    "session_controller_missing",
                    format!(
                        "cannot hand off control for hosted session {} because no active controller lease exists",
                        metadata.id.as_str()
                    ),
                ));
            };
            if controller.participant_id != actor_participant_id {
                return Err(controller_conflict_error(
                    metadata,
                    controller.participant_id.as_str(),
                    "hand off control",
                ));
            }
            let Some(target_participant_id) = target_participant_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(String::from)
            else {
                return Err(protocol_error(
                    "target_participant_required",
                    "handoff requires a non-empty target_participant_id",
                ));
            };
            if target_participant_id == actor_participant_id {
                return Ok(None);
            }
            if !metadata
                .participants
                .iter()
                .any(|participant| participant.participant_id == target_participant_id)
            {
                return Err(protocol_error(
                    "participant_not_attached",
                    format!(
                        "cannot hand off hosted session {} to participant {} because that participant is not attached",
                        metadata.id.as_str(),
                        target_participant_id
                    ),
                ));
            }
            metadata.controller_lease = Some(SessionControllerLease {
                participant_id: target_participant_id.clone(),
                acquired_at_ms: recorded_at_ms,
            });
            hosted_controller_history_event(
                metadata,
                action,
                actor_participant_id,
                Some(target_participant_id),
                recorded_at_ms,
            )
        }
        SessionControllerAction::Takeover => {
            if let Some(controller) = current_controller {
                if controller.participant_id == actor_participant_id {
                    return Ok(None);
                }
                metadata.controller_lease = Some(SessionControllerLease {
                    participant_id: actor_participant_id.clone(),
                    acquired_at_ms: recorded_at_ms,
                });
                hosted_controller_history_event(
                    metadata,
                    action,
                    actor_participant_id,
                    Some(controller.participant_id),
                    recorded_at_ms,
                )
            } else {
                metadata.controller_lease = Some(SessionControllerLease {
                    participant_id: actor_participant_id.clone(),
                    acquired_at_ms: recorded_at_ms,
                });
                hosted_controller_history_event(
                    metadata,
                    SessionControllerAction::Claim,
                    actor_participant_id,
                    None,
                    recorded_at_ms,
                )
            }
        }
    };
    Ok(history_event)
}

fn hosted_controller_history_event(
    metadata: &SessionMetadata,
    action: SessionControllerAction,
    actor_participant_id: String,
    target_participant_id: Option<String>,
    recorded_at_ms: u64,
) -> Option<SessionHostedLifecycleEvent> {
    let runtime_owner = metadata.runtime_owner.as_ref()?;
    if runtime_owner.kind != SessionRuntimeOwnerKind::HostedControlPlane {
        return None;
    }
    let execution_host_id = hosted_execution_host_id(metadata, runtime_owner);
    let actor_label = session_participant_label(metadata, actor_participant_id.as_str());
    let summary = match (action, target_participant_id.as_deref()) {
        (SessionControllerAction::Claim, _) => {
            format!("participant {actor_label} claimed hosted session control")
        }
        (SessionControllerAction::Release, _) => {
            format!("participant {actor_label} released hosted session control")
        }
        (SessionControllerAction::Handoff, Some(target)) => format!(
            "participant {actor_label} handed hosted session control to {}",
            session_participant_label(metadata, target)
        ),
        (SessionControllerAction::Takeover, Some(previous)) => format!(
            "participant {actor_label} took over hosted session control from {}",
            session_participant_label(metadata, previous)
        ),
        (SessionControllerAction::Takeover, None) => {
            format!("participant {actor_label} took over hosted session control")
        }
        (SessionControllerAction::Handoff, None) => {
            format!("participant {actor_label} handed off hosted session control")
        }
    };
    Some(SessionHostedLifecycleEvent::ControllerLeaseChanged {
        action,
        actor_participant_id,
        target_participant_id,
        session_owner_id: runtime_owner.owner_id.clone(),
        execution_host_id,
        summary,
        recorded_at_ms,
    })
}

fn session_participant_label(metadata: &SessionMetadata, participant_id: &str) -> String {
    metadata
        .participants
        .iter()
        .find(|participant| participant.participant_id == participant_id)
        .and_then(|participant| {
            participant
                .display_name
                .as_ref()
                .map(|display_name| format!("{display_name} ({participant_id})"))
        })
        .unwrap_or_else(|| String::from(participant_id))
}

fn hosted_execution_host_id(
    metadata: &SessionMetadata,
    runtime_owner: &SessionRuntimeOwner,
) -> String {
    metadata
        .hosted_receipts
        .as_ref()
        .and_then(|receipts| receipts.worker.as_ref())
        .map(|worker| worker.execution_host_id.clone())
        .or_else(|| {
            metadata
                .workspace_state
                .as_ref()
                .and_then(|state| state.execution_host.as_ref())
                .map(|host| host.host_id.clone())
        })
        .unwrap_or_else(|| runtime_owner.owner_id.clone())
}

fn hosted_cleanup_ready(summary: &DetachedSessionSummary) -> bool {
    matches!(
        summary.status,
        DetachedSessionStatus::Completed
            | DetachedSessionStatus::Failed
            | DetachedSessionStatus::Cancelled
            | DetachedSessionStatus::TimedOut
    ) && summary.controller_lease.is_none()
}

fn finalize_managed_hosted_cleanup(
    _core: &ProbeServerCore,
    mut metadata: SessionMetadata,
    trigger: HostedCleanupTrigger,
    recorded_at_ms: u64,
) -> Result<SessionMetadata, RuntimeProtocolError> {
    if !is_hosted_session(&metadata) {
        return Ok(metadata);
    }
    let Some(runtime_owner) = metadata.runtime_owner.as_ref() else {
        return Ok(metadata);
    };
    let execution_host_id = hosted_execution_host_id(&metadata, runtime_owner);
    let Some(receipts) = metadata.hosted_receipts.as_mut() else {
        return Ok(metadata);
    };
    let Some(cleanup) = receipts.cleanup.as_mut() else {
        return Ok(metadata);
    };
    if cleanup.strategy != "managed_hosted_workspace"
        || matches!(
            cleanup.status,
            SessionHostedCleanupStatus::Completed | SessionHostedCleanupStatus::NotRequired
        )
    {
        return Ok(metadata);
    }

    if metadata.cwd.exists() {
        match fs::remove_dir_all(&metadata.cwd) {
            Ok(()) => {
                cleanup.status = SessionHostedCleanupStatus::Completed;
                cleanup.recorded_at_ms = recorded_at_ms;
                cleanup.note = Some(match trigger {
                    HostedCleanupTrigger::ControlPlaneShutdown => String::from(
                        "Probe removed the managed hosted workspace during clean control-plane shutdown",
                    ),
                    HostedCleanupTrigger::ControlPlaneRestart => String::from(
                        "Probe reaped the orphaned managed hosted workspace during control-plane restart reconciliation",
                    ),
                });
                push_hosted_history_event(
                    &mut receipts.history,
                    SessionHostedLifecycleEvent::CleanupStateChanged {
                        previous_status: Some(SessionHostedCleanupStatus::Pending),
                        status: SessionHostedCleanupStatus::Completed,
                        workspace_root: metadata.cwd.clone(),
                        strategy: cleanup.strategy.clone(),
                        execution_host_id: Some(execution_host_id.clone()),
                        summary: cleanup
                            .note
                            .clone()
                            .unwrap_or_else(|| String::from("managed hosted cleanup completed")),
                        recorded_at_ms,
                    },
                );
                if matches!(trigger, HostedCleanupTrigger::ControlPlaneRestart) {
                    push_hosted_history_event(
                        &mut receipts.history,
                        SessionHostedLifecycleEvent::OrphanedManagedWorkspaceReaped {
                            workspace_root: metadata.cwd.clone(),
                            session_owner_id: runtime_owner.owner_id.clone(),
                            execution_host_id,
                            summary: String::from(
                                "hosted Probe reaped an orphaned managed workspace during restart reconciliation",
                            ),
                            recorded_at_ms,
                        },
                    );
                }
            }
            Err(error) => {
                cleanup.status = SessionHostedCleanupStatus::Failed;
                cleanup.recorded_at_ms = recorded_at_ms;
                cleanup.note = Some(format!(
                    "Probe failed to remove the managed hosted workspace: {error}"
                ));
                push_hosted_history_event(
                    &mut receipts.history,
                    SessionHostedLifecycleEvent::CleanupStateChanged {
                        previous_status: Some(SessionHostedCleanupStatus::Pending),
                        status: SessionHostedCleanupStatus::Failed,
                        workspace_root: metadata.cwd.clone(),
                        strategy: cleanup.strategy.clone(),
                        execution_host_id: Some(execution_host_id),
                        summary: cleanup
                            .note
                            .clone()
                            .unwrap_or_else(|| String::from("managed hosted cleanup failed")),
                        recorded_at_ms,
                    },
                );
            }
        }
    } else {
        cleanup.status = SessionHostedCleanupStatus::Completed;
        cleanup.recorded_at_ms = recorded_at_ms;
        cleanup.note = Some(String::from(
            "Probe no longer sees the managed hosted workspace path and records cleanup as complete",
        ));
        push_hosted_history_event(
            &mut receipts.history,
            SessionHostedLifecycleEvent::CleanupStateChanged {
                previous_status: Some(SessionHostedCleanupStatus::Pending),
                status: SessionHostedCleanupStatus::Completed,
                workspace_root: metadata.cwd.clone(),
                strategy: cleanup.strategy.clone(),
                execution_host_id: Some(execution_host_id),
                summary: cleanup
                    .note
                    .clone()
                    .unwrap_or_else(|| String::from("managed hosted cleanup completed")),
                recorded_at_ms,
            },
        );
    }

    Ok(metadata)
}

fn session_branch_state(cwd: &Path) -> Option<SessionBranchState> {
    let repo_root = resolve_git_repo_root(cwd)?;
    let head_commit = run_git_string(cwd, &["rev-parse", "HEAD"])?;
    let head_ref = run_git_string(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .or_else(|| run_git_string(cwd, &["rev-parse", "--short", "HEAD"]))?;
    let detached_head =
        run_git_string(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"]).is_none();
    let working_tree_dirty = run_git_string(cwd, &["status", "--porcelain"])
        .is_some_and(|output| !output.trim().is_empty());
    let upstream_ref = run_git_string(
        cwd,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    );
    let (ahead_by, behind_by) = upstream_ref
        .as_ref()
        .and_then(|_| {
            run_git_string(
                cwd,
                &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
            )
        })
        .and_then(|counts| parse_ahead_behind(counts.as_str()))
        .unwrap_or((None, None));
    Some(SessionBranchState {
        repo_root,
        head_ref,
        head_commit,
        detached_head,
        working_tree_dirty,
        upstream_ref,
        ahead_by,
        behind_by,
    })
}

fn session_delivery_state(branch_state: &SessionBranchState, now_ms: u64) -> SessionDeliveryState {
    let status = if branch_state.working_tree_dirty {
        SessionDeliveryStatus::NeedsCommit
    } else if branch_state.behind_by.unwrap_or(0) > 0 {
        SessionDeliveryStatus::Diverged
    } else if branch_state.ahead_by.unwrap_or(0) > 0 {
        SessionDeliveryStatus::NeedsPush
    } else if branch_state.upstream_ref.is_some() {
        SessionDeliveryStatus::Synced
    } else {
        SessionDeliveryStatus::LocalOnly
    };
    let branch_name = (!branch_state.detached_head).then(|| branch_state.head_ref.clone());
    let compare_ref = branch_state.upstream_ref.as_ref().and_then(|upstream_ref| {
        branch_name
            .as_ref()
            .map(|branch_name| format!("{upstream_ref}...{branch_name}"))
    });
    let mut artifacts = vec![SessionDeliveryArtifact {
        kind: String::from("head_commit"),
        value: branch_state.head_commit.clone(),
        label: Some(String::from("Head commit")),
    }];
    artifacts.push(SessionDeliveryArtifact {
        kind: String::from("head_ref"),
        value: branch_state.head_ref.clone(),
        label: Some(String::from("Head ref")),
    });
    if let Some(upstream_ref) = branch_state.upstream_ref.as_ref() {
        artifacts.push(SessionDeliveryArtifact {
            kind: String::from("upstream_ref"),
            value: upstream_ref.clone(),
            label: Some(String::from("Upstream ref")),
        });
    }
    if let Some(compare_ref) = compare_ref.as_ref() {
        artifacts.push(SessionDeliveryArtifact {
            kind: String::from("compare_ref"),
            value: compare_ref.clone(),
            label: Some(String::from("Compare ref")),
        });
    }
    SessionDeliveryState {
        status,
        branch_name,
        remote_tracking_ref: branch_state.upstream_ref.clone(),
        compare_ref,
        updated_at_ms: now_ms,
        artifacts,
    }
}

fn run_git_string(cwd: &Path, args: &[&str]) -> Option<String> {
    let value = run_git_output(cwd, args)?;
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn run_git_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn parse_ahead_behind(counts: &str) -> Option<(Option<u64>, Option<u64>)> {
    let mut parts = counts.split_whitespace();
    let ahead_by = parts.next()?.parse::<u64>().ok();
    let behind_by = parts.next()?.parse::<u64>().ok();
    Some((ahead_by, behind_by))
}

fn detached_session_summary_from_state(
    metadata: &SessionMetadata,
    state: &SessionTurnControlState,
    pending_approval_count: usize,
    summary_artifact_refs: Vec<SessionSummaryArtifactRef>,
    previous: Option<&DetachedSessionSummary>,
    now_ms: u64,
) -> DetachedSessionSummary {
    let view = state.inspect_view(&metadata.id);
    let active_turn = view.active_turn.as_ref();
    let last_terminal_turn = preferred_terminal_turn(view.recent_turns.as_slice());
    let approval_recovery_note = match metadata.runtime_owner.as_ref().map(|owner| owner.kind) {
        Some(SessionRuntimeOwnerKind::HostedControlPlane) => {
            "hosted control plane can resume this session after the pending approval is resolved"
        }
        _ => "daemon restart can resume this session after the pending approval is resolved",
    };
    let (status, recovery_state, recovery_note) = if let Some(active_turn) = active_turn {
        if active_turn.awaiting_approval {
            (
                DetachedSessionStatus::ApprovalPaused,
                DetachedSessionRecoveryState::ApprovalPausedResumable,
                Some(String::from(approval_recovery_note)),
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
            QueuedTurnStatus::TimedOut => DetachedSessionStatus::TimedOut,
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
        runtime_owner: metadata.runtime_owner.clone(),
        workspace_state: metadata.workspace_state.clone(),
        hosted_receipts: metadata.hosted_receipts.clone(),
        mounted_refs: metadata.mounted_refs.clone(),
        summary_artifact_refs,
        participants: metadata.participants.clone(),
        controller_lease: metadata.controller_lease.clone(),
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

fn sync_hosted_session_metadata_from_store(
    store: &probe_core::session_store::FilesystemSessionStore,
    hosted_receipt_config: Option<&HostedReceiptConfig>,
    mut metadata: SessionMetadata,
    transcript: &[TranscriptEvent],
    recorded_at_ms: u64,
) -> Result<SessionMetadata, RuntimeProtocolError> {
    let Some(runtime_owner) = metadata.runtime_owner.as_ref() else {
        return Ok(metadata);
    };
    if runtime_owner.kind != SessionRuntimeOwnerKind::HostedControlPlane {
        return Ok(metadata);
    }
    let updated_receipts = hosted_receipts_for_session(
        store.root(),
        hosted_receipt_config,
        &metadata,
        transcript,
        recorded_at_ms,
    );
    if metadata.hosted_receipts.as_ref() == Some(&updated_receipts) {
        return Ok(metadata);
    }
    metadata.hosted_receipts = Some(updated_receipts);
    store
        .replace_metadata(metadata)
        .map_err(session_store_error_to_protocol)
}

fn hosted_receipts_for_session(
    probe_home: &Path,
    hosted_receipt_config: Option<&HostedReceiptConfig>,
    metadata: &SessionMetadata,
    transcript: &[TranscriptEvent],
    recorded_at_ms: u64,
) -> SessionHostedReceipts {
    let previous_receipts = metadata.hosted_receipts.as_ref();
    let runtime_owner = metadata
        .runtime_owner
        .as_ref()
        .expect("hosted receipts require runtime owner");
    let execution_host = metadata
        .workspace_state
        .as_ref()
        .and_then(|state| state.execution_host.as_ref())
        .cloned()
        .unwrap_or_else(|| execution_host_for_owner(runtime_owner));
    let worker = SessionHostedWorkerReceipt {
        owner_kind: runtime_owner.kind,
        owner_id: runtime_owner.owner_id.clone(),
        attach_transport: runtime_owner.attach_transport,
        attach_target: runtime_owner.attach_target.clone(),
        execution_host_kind: execution_host.kind,
        execution_host_id: execution_host.host_id.clone(),
        execution_host_label: execution_host
            .display_name
            .clone()
            .or_else(|| Some(execution_host.host_id.clone())),
        recorded_at_ms,
    };
    let cleanup = hosted_cleanup_receipt(
        probe_home,
        metadata,
        previous_receipts.and_then(|receipts| receipts.cleanup.as_ref()),
        recorded_at_ms,
    );
    let auth = hosted_receipt_config.map(|config| SessionHostedAuthReceipt {
        authority: config.auth_authority.clone(),
        subject: config.auth_subject.clone(),
        auth_kind: config.auth_kind,
        scope: config.auth_scope.clone(),
        recorded_at_ms,
    });
    SessionHostedReceipts {
        auth,
        checkout: Some(hosted_checkout_receipt(metadata, recorded_at_ms)),
        worker: Some(worker.clone()),
        cost: Some(hosted_cost_receipt(transcript, recorded_at_ms)),
        cleanup: Some(cleanup.clone()),
        history: hosted_receipt_history(previous_receipts, &worker, &cleanup, recorded_at_ms),
    }
}

fn hosted_checkout_receipt(
    metadata: &SessionMetadata,
    recorded_at_ms: u64,
) -> SessionHostedCheckoutReceipt {
    let branch_state = session_branch_state(metadata.cwd.as_path());
    let repo_identity = metadata
        .workspace_state
        .as_ref()
        .and_then(|state| state.baseline.as_ref())
        .and_then(|baseline| baseline.repo_identity.clone())
        .or_else(|| {
            run_git_string(
                metadata.cwd.as_path(),
                &["config", "--get", "remote.origin.url"],
            )
        });
    match branch_state {
        Some(branch_state) => SessionHostedCheckoutReceipt {
            kind: SessionHostedCheckoutKind::GitRepository,
            workspace_root: metadata.cwd.clone(),
            repo_root: Some(branch_state.repo_root),
            repo_identity,
            head_ref: Some(branch_state.head_ref),
            head_commit: Some(branch_state.head_commit),
            note: None,
            recorded_at_ms,
        },
        None => SessionHostedCheckoutReceipt {
            kind: SessionHostedCheckoutKind::PlainWorkspace,
            workspace_root: metadata.cwd.clone(),
            repo_root: None,
            repo_identity,
            head_ref: None,
            head_commit: None,
            note: Some(String::from(
                "cwd is not inside a git repository; Probe cannot emit branch or commit checkout truth for this session",
            )),
            recorded_at_ms,
        },
    }
}

fn hosted_cost_receipt(
    transcript: &[TranscriptEvent],
    recorded_at_ms: u64,
) -> SessionHostedCostReceipt {
    let mut wallclock_ms = 0_u64;
    let mut prompt_tokens = 0_u64;
    let mut completion_tokens = 0_u64;
    let mut total_tokens = 0_u64;
    let mut saw_prompt_tokens = false;
    let mut saw_completion_tokens = false;
    let mut saw_total_tokens = false;
    for event in transcript {
        if let Some(observability) = event.turn.observability.as_ref() {
            wallclock_ms = wallclock_ms.saturating_add(observability.wallclock_ms);
            if let Some(value) = observability.prompt_tokens {
                prompt_tokens = prompt_tokens.saturating_add(value);
                saw_prompt_tokens = true;
            }
            if let Some(value) = observability.completion_tokens {
                completion_tokens = completion_tokens.saturating_add(value);
                saw_completion_tokens = true;
            }
            if let Some(value) = observability.total_tokens {
                total_tokens = total_tokens.saturating_add(value);
                saw_total_tokens = true;
            }
        }
    }
    SessionHostedCostReceipt {
        observed_turn_count: transcript.len() as u64,
        wallclock_ms,
        prompt_tokens: saw_prompt_tokens.then_some(prompt_tokens),
        completion_tokens: saw_completion_tokens.then_some(completion_tokens),
        total_tokens: saw_total_tokens.then_some(total_tokens),
        note: Some(String::from(
            "Probe reports raw hosted turn observability for operator cost estimation; provider billing is not wired into the runtime contract yet",
        )),
        recorded_at_ms,
    }
}

fn hosted_cleanup_receipt(
    probe_home: &Path,
    metadata: &SessionMetadata,
    previous: Option<&SessionHostedCleanupReceipt>,
    recorded_at_ms: u64,
) -> SessionHostedCleanupReceipt {
    let managed_root = probe_home.join("hosted").join("workspaces");
    let managed_workspace = same_path_prefix(metadata.cwd.as_path(), managed_root.as_path());
    let (status, strategy, note) = if managed_workspace {
        let workspace_exists = metadata.cwd.exists();
        let status = match (previous.map(|receipt| receipt.status), workspace_exists) {
            (Some(SessionHostedCleanupStatus::Failed), true) => SessionHostedCleanupStatus::Failed,
            (Some(SessionHostedCleanupStatus::Completed), false) => {
                SessionHostedCleanupStatus::Completed
            }
            (_, true) => SessionHostedCleanupStatus::Pending,
            (_, false) => SessionHostedCleanupStatus::Completed,
        };
        let note = match status {
            SessionHostedCleanupStatus::Failed => previous
                .and_then(|receipt| receipt.note.clone())
                .or_else(|| {
                    Some(String::from(
                        "Probe previously failed to clean up this managed hosted workspace",
                    ))
                }),
            SessionHostedCleanupStatus::Pending => Some(String::from(
                "Probe marked this workspace as managed hosted state and will keep cleanup pending until hosted closeout runs",
            )),
            SessionHostedCleanupStatus::Completed => Some(String::from(
                "Probe no longer sees the managed hosted workspace path and records cleanup as complete",
            )),
            SessionHostedCleanupStatus::NotRequired => None,
        };
        (status, String::from("managed_hosted_workspace"), note)
    } else {
        (
            SessionHostedCleanupStatus::NotRequired,
            String::from("operator_supplied_workspace"),
            Some(String::from(
                "Probe is attached to an existing workspace path and does not delete it during hosted session cleanup",
            )),
        )
    };
    SessionHostedCleanupReceipt {
        status,
        workspace_root: metadata.cwd.clone(),
        strategy,
        note,
        recorded_at_ms,
    }
}

fn hosted_receipt_history(
    previous: Option<&SessionHostedReceipts>,
    worker: &SessionHostedWorkerReceipt,
    cleanup: &SessionHostedCleanupReceipt,
    recorded_at_ms: u64,
) -> Vec<SessionHostedLifecycleEvent> {
    let mut history = previous
        .map(|receipts| receipts.history.clone())
        .unwrap_or_default();
    let previous_cleanup = previous.and_then(|receipts| receipts.cleanup.as_ref());
    let cleanup_changed = previous_cleanup.map(|receipt| receipt.status) != Some(cleanup.status)
        || previous_cleanup.map(|receipt| receipt.strategy.as_str())
            != Some(cleanup.strategy.as_str())
        || previous_cleanup.map(|receipt| receipt.workspace_root.as_path())
            != Some(cleanup.workspace_root.as_path());
    if cleanup_changed {
        push_hosted_history_event(
            &mut history,
            SessionHostedLifecycleEvent::CleanupStateChanged {
                previous_status: previous_cleanup.map(|receipt| receipt.status),
                status: cleanup.status,
                workspace_root: cleanup.workspace_root.clone(),
                strategy: cleanup.strategy.clone(),
                execution_host_id: Some(worker.execution_host_id.clone()),
                summary: cleanup.note.clone().unwrap_or_else(|| {
                    format!(
                        "Probe recorded hosted cleanup status `{}` for strategy `{}`",
                        hosted_cleanup_status_label(cleanup.status),
                        cleanup.strategy
                    )
                }),
                recorded_at_ms,
            },
        );
    }
    history
}

fn sync_hosted_history_from_detached_summary(
    store: &probe_core::session_store::FilesystemSessionStore,
    mut metadata: SessionMetadata,
    summary: &DetachedSessionSummary,
    recorded_at_ms: u64,
) -> Result<SessionMetadata, RuntimeProtocolError> {
    let Some(runtime_owner) = metadata.runtime_owner.as_ref() else {
        return Ok(metadata);
    };
    if runtime_owner.kind != SessionRuntimeOwnerKind::HostedControlPlane {
        return Ok(metadata);
    }
    let Some(mut receipts) = metadata.hosted_receipts.clone() else {
        return Ok(metadata);
    };
    let execution_host_id = receipts
        .worker
        .as_ref()
        .map(|worker| worker.execution_host_id.clone())
        .unwrap_or_else(|| runtime_owner.owner_id.clone());
    let event = match summary.recovery_state {
        DetachedSessionRecoveryState::ApprovalPausedResumable => {
            summary
                .active_turn_id
                .as_ref()
                .map(|turn_id| SessionHostedLifecycleEvent::ApprovalPausedTakeoverAvailable {
                    turn_id: turn_id.clone(),
                    session_owner_id: runtime_owner.owner_id.clone(),
                    execution_host_id,
                    pending_approval_count: summary.pending_approval_count,
                    summary: summary.recovery_note.clone().unwrap_or_else(|| {
                        String::from(
                            "hosted Probe kept this approval-paused turn reattachable for operator takeover",
                        )
                    }),
                    recorded_at_ms,
                })
        }
        DetachedSessionRecoveryState::RunningTurnFailedOnRestart => summary
            .last_terminal_turn_id
            .as_ref()
            .map(|turn_id| SessionHostedLifecycleEvent::RunningTurnFailedOnRestart {
                turn_id: turn_id.clone(),
                session_owner_id: runtime_owner.owner_id.clone(),
                execution_host_id,
                summary: summary.recovery_note.clone().unwrap_or_else(|| {
                    String::from(
                        "hosted Probe marked the interrupted running turn as failed after restart",
                    )
                }),
                recorded_at_ms,
            }),
        DetachedSessionRecoveryState::Clean => None,
    };
    let Some(event) = event else {
        return Ok(metadata);
    };
    let original_receipts = receipts.clone();
    push_hosted_history_event(&mut receipts.history, event);
    if receipts == original_receipts {
        return Ok(metadata);
    }
    metadata.hosted_receipts = Some(receipts);
    store
        .replace_metadata(metadata)
        .map_err(session_store_error_to_protocol)
}

fn push_hosted_history_event(
    history: &mut Vec<SessionHostedLifecycleEvent>,
    event: SessionHostedLifecycleEvent,
) {
    if history
        .iter()
        .any(|existing| hosted_history_event_matches(existing, &event))
    {
        return;
    }
    history.push(event);
}

fn hosted_history_event_matches(
    left: &SessionHostedLifecycleEvent,
    right: &SessionHostedLifecycleEvent,
) -> bool {
    match (left, right) {
        (
            SessionHostedLifecycleEvent::ControlPlaneRestartObserved {
                control_plane_started_at_ms: left_started_at,
                ..
            },
            SessionHostedLifecycleEvent::ControlPlaneRestartObserved {
                control_plane_started_at_ms: right_started_at,
                ..
            },
        ) => left_started_at == right_started_at,
        (
            SessionHostedLifecycleEvent::RunningTurnFailedOnRestart {
                turn_id: left_turn, ..
            },
            SessionHostedLifecycleEvent::RunningTurnFailedOnRestart {
                turn_id: right_turn,
                ..
            },
        ) => left_turn == right_turn,
        (
            SessionHostedLifecycleEvent::ApprovalPausedTakeoverAvailable {
                turn_id: left_turn, ..
            },
            SessionHostedLifecycleEvent::ApprovalPausedTakeoverAvailable {
                turn_id: right_turn,
                ..
            },
        ) => left_turn == right_turn,
        (
            SessionHostedLifecycleEvent::CleanupStateChanged {
                previous_status: left_previous,
                status: left_status,
                workspace_root: left_root,
                strategy: left_strategy,
                ..
            },
            SessionHostedLifecycleEvent::CleanupStateChanged {
                previous_status: right_previous,
                status: right_status,
                workspace_root: right_root,
                strategy: right_strategy,
                ..
            },
        ) => {
            left_previous == right_previous
                && left_status == right_status
                && left_root == right_root
                && left_strategy == right_strategy
        }
        (
            SessionHostedLifecycleEvent::OrphanedManagedWorkspaceReaped {
                workspace_root: left_root,
                ..
            },
            SessionHostedLifecycleEvent::OrphanedManagedWorkspaceReaped {
                workspace_root: right_root,
                ..
            },
        ) => left_root == right_root,
        (
            SessionHostedLifecycleEvent::ControllerLeaseChanged {
                action: left_action,
                actor_participant_id: left_actor,
                target_participant_id: left_target,
                ..
            },
            SessionHostedLifecycleEvent::ControllerLeaseChanged {
                action: right_action,
                actor_participant_id: right_actor,
                target_participant_id: right_target,
                ..
            },
        ) => {
            left_action == right_action && left_actor == right_actor && left_target == right_target
        }
        _ => false,
    }
}

fn hosted_cleanup_status_label(status: SessionHostedCleanupStatus) -> &'static str {
    match status {
        SessionHostedCleanupStatus::NotRequired => "not_required",
        SessionHostedCleanupStatus::Pending => "pending",
        SessionHostedCleanupStatus::Completed => "completed",
        SessionHostedCleanupStatus::Failed => "failed",
    }
}

fn turn_response_to_runtime_response(response: TurnResponse, mode: TurnMode) -> RuntimeResponse {
    match mode {
        TurnMode::Start => RuntimeResponse::StartTurn(response),
        TurnMode::Continue => RuntimeResponse::ContinueTurn(response),
    }
}

fn summary_artifact_refs(artifacts: &[SessionSummaryArtifact]) -> Vec<SessionSummaryArtifactRef> {
    artifacts
        .iter()
        .map(|artifact| artifact.artifact_ref().clone())
        .collect()
}

fn resolve_workspace_state(
    probe_home: &Path,
    runtime_owner: &SessionRuntimeOwner,
    requested: Option<SessionWorkspaceState>,
) -> Option<SessionWorkspaceState> {
    if requested.is_none()
        && !matches!(
            runtime_owner.kind,
            SessionRuntimeOwnerKind::HostedControlPlane
        )
    {
        return None;
    }

    let mut notes = Vec::new();
    let mut workspace_state = requested.unwrap_or(SessionWorkspaceState {
        boot_mode: SessionWorkspaceBootMode::Fresh,
        baseline: None,
        snapshot: None,
        execution_host: None,
        provenance_note: None,
    });

    if workspace_state.execution_host.is_none() {
        workspace_state.execution_host = Some(execution_host_for_owner(runtime_owner));
    }

    workspace_state.baseline = workspace_state
        .baseline
        .take()
        .map(|baseline| resolve_baseline_ref(probe_home, baseline, &mut notes));
    workspace_state.snapshot = workspace_state
        .snapshot
        .take()
        .map(|snapshot| resolve_snapshot_ref(probe_home, snapshot, &mut notes));

    match workspace_state.boot_mode {
        SessionWorkspaceBootMode::Fresh => {}
        SessionWorkspaceBootMode::PreparedBaseline => match workspace_state.baseline.as_ref() {
            Some(baseline) if matches!(baseline.status, SessionPreparedBaselineStatus::Ready) => {}
            Some(baseline) => {
                workspace_state.boot_mode = SessionWorkspaceBootMode::Fresh;
                notes.push(format!(
                    "prepared baseline {} was {:?}; Probe fell back to a fresh workspace start",
                    baseline.baseline_id, baseline.status
                ));
            }
            None => {
                workspace_state.boot_mode = SessionWorkspaceBootMode::Fresh;
                notes.push(String::from(
                    "prepared baseline boot was requested without a baseline ref; Probe fell back to a fresh workspace start",
                ));
            }
        },
        SessionWorkspaceBootMode::SnapshotRestore => {
            if workspace_state.snapshot.is_none() {
                workspace_state.boot_mode = SessionWorkspaceBootMode::Fresh;
                notes.push(String::from(
                    "snapshot restore was requested without a snapshot ref; Probe fell back to a fresh workspace start",
                ));
            }
        }
    }

    workspace_state.provenance_note =
        combine_provenance_note(workspace_state.provenance_note.take(), notes);
    Some(workspace_state)
}

fn load_session_mcp_state(probe_home: &Path) -> SessionMcpState {
    let path = probe_home.join(MCP_REGISTRY_RELATIVE_PATH);
    if !path.exists() {
        return SessionMcpState::default();
    }
    match fs::read_to_string(&path) {
        Ok(body) => match serde_json::from_str::<StoredMcpRegistryFile>(&body) {
            Ok(registry) => SessionMcpState {
                load_error: None,
                servers: registry
                    .servers
                    .into_iter()
                    .filter(|server| server.enabled)
                    .map(session_mcp_server_from_record)
                    .collect(),
            },
            Err(error) => SessionMcpState {
                load_error: Some(format!(
                    "Probe could not read {}: {}",
                    path.display(),
                    error
                )),
                servers: Vec::new(),
            },
        },
        Err(error) => SessionMcpState {
            load_error: Some(format!(
                "Probe could not read {}: {}",
                path.display(),
                error
            )),
            servers: Vec::new(),
        },
    }
}

fn session_mcp_server_from_record(server: StoredMcpServerRecord) -> SessionMcpServer {
    let mut snapshot = SessionMcpServer {
        id: server.id,
        name: server.name,
        enabled: server.enabled,
        source: match server.source {
            StoredMcpServerSource::ManualLaunch => SessionMcpServerSource::ManualLaunch,
            StoredMcpServerSource::ProviderCommandRecipe => {
                SessionMcpServerSource::ProviderCommandRecipe
            }
        },
        transport: server.transport.as_ref().map(|transport| match transport {
            StoredMcpServerTransport::Stdio => SessionMcpServerTransport::Stdio,
            StoredMcpServerTransport::Http => SessionMcpServerTransport::Http,
        }),
        target: server.target.clone(),
        provider_setup_command: server.provider_setup_command.clone(),
        provider_hint: server.provider_hint.clone(),
        client_hint: server.client_hint.clone(),
        connection_status: None,
        connection_note: None,
        discovered_tools: Vec::new(),
    };

    match (
        &server.source,
        server.transport.as_ref(),
        server.target.as_deref(),
    ) {
        (
            StoredMcpServerSource::ManualLaunch,
            Some(StoredMcpServerTransport::Stdio),
            Some(target),
        ) => {
            let (status, note, tools) = inspect_stdio_mcp_server(target);
            snapshot.connection_status = Some(status);
            snapshot.connection_note = note;
            snapshot.discovered_tools = tools;
        }
        (StoredMcpServerSource::ManualLaunch, Some(StoredMcpServerTransport::Http), Some(_)) => {
            snapshot.connection_status = Some(SessionMcpConnectionStatus::Unsupported);
            snapshot.connection_note = Some(String::from(
                "HTTP MCP mounting is not implemented yet; this entry will not attach until that transport ships.",
            ));
        }
        (StoredMcpServerSource::ProviderCommandRecipe, _, _) => {
            snapshot.connection_status = Some(SessionMcpConnectionStatus::Unsupported);
            snapshot.connection_note = Some(String::from(
                "Provider-command recipes are saved, but Probe cannot launch them as runtime MCP servers yet.",
            ));
        }
        (StoredMcpServerSource::ManualLaunch, _, _) => {
            snapshot.connection_status = Some(SessionMcpConnectionStatus::Failed);
            snapshot.connection_note = Some(String::from(
                "This MCP entry is missing the launch details Probe needs to attach it.",
            ));
        }
    }

    snapshot
}

fn inspect_stdio_mcp_server(
    target: &str,
) -> (
    SessionMcpConnectionStatus,
    Option<String>,
    Vec<SessionMcpTool>,
) {
    let mut child = match Command::new("/bin/sh")
        .arg("-lc")
        .arg(target)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return (
                SessionMcpConnectionStatus::Failed,
                Some(format!(
                    "Probe could not launch the stdio MCP server: {error}"
                )),
                Vec::new(),
            );
        }
    };

    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return (
            SessionMcpConnectionStatus::Failed,
            Some(String::from(
                "Probe launched the stdio MCP server, but stdin was unavailable.",
            )),
            Vec::new(),
        );
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return (
            SessionMcpConnectionStatus::Failed,
            Some(String::from(
                "Probe launched the stdio MCP server, but stdout was unavailable.",
            )),
            Vec::new(),
        );
    };

    let (tx, rx) = mpsc::channel::<Result<serde_json::Value, String>>();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_mcp_framed_message(&mut reader) {
                Ok(Some(message)) => {
                    if tx.send(Ok(message)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = tx.send(Err(error.to_string()));
                    break;
                }
            }
        }
    });

    if let Err(error) = write_mcp_framed_message(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {
                    "name": "probe",
                    "version": "0.1.0"
                }
            }
        }),
    ) {
        let _ = child.kill();
        let _ = child.wait();
        return (
            SessionMcpConnectionStatus::Failed,
            Some(format!(
                "Probe could not send the MCP initialize request: {error}"
            )),
            Vec::new(),
        );
    }

    let initialize = match rx.recv_timeout(MCP_STDIO_STARTUP_TIMEOUT) {
        Ok(Ok(message)) => message,
        Ok(Err(error)) => {
            let _ = child.kill();
            let _ = child.wait();
            return (
                SessionMcpConnectionStatus::Failed,
                Some(format!(
                    "Probe could not read the MCP initialize response: {error}"
                )),
                Vec::new(),
            );
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return (
                SessionMcpConnectionStatus::Failed,
                Some(String::from(
                    "The stdio MCP server did not answer Probe's initialize request in time.",
                )),
                Vec::new(),
            );
        }
    };

    if let Some(error) = initialize.get("error") {
        let _ = child.kill();
        let _ = child.wait();
        return (
            SessionMcpConnectionStatus::Failed,
            Some(format!(
                "The stdio MCP server rejected initialization: {}",
                compact_json_value(error)
            )),
            Vec::new(),
        );
    }

    let _ = write_mcp_framed_message(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
    );
    if let Err(error) = write_mcp_framed_message(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
    ) {
        let _ = child.kill();
        let _ = child.wait();
        return (
            SessionMcpConnectionStatus::Failed,
            Some(format!(
                "Probe initialized the MCP server, but tools/list failed to send: {error}"
            )),
            Vec::new(),
        );
    }

    let tools_message = match rx.recv_timeout(MCP_STDIO_STARTUP_TIMEOUT) {
        Ok(Ok(message)) => message,
        Ok(Err(error)) => {
            let _ = child.kill();
            let _ = child.wait();
            return (
                SessionMcpConnectionStatus::Failed,
                Some(format!(
                    "Probe could not read the MCP tool inventory response: {error}"
                )),
                Vec::new(),
            );
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return (
                SessionMcpConnectionStatus::Failed,
                Some(String::from(
                    "The stdio MCP server initialized, but Probe did not receive a tools/list response in time.",
                )),
                Vec::new(),
            );
        }
    };

    let tools = if let Some(error) = tools_message.get("error") {
        let _ = child.kill();
        let _ = child.wait();
        return (
            SessionMcpConnectionStatus::Failed,
            Some(format!(
                "The MCP server answered initialize, but tools/list failed: {}",
                compact_json_value(error)
            )),
            Vec::new(),
        );
    } else {
        extract_mcp_tools_from_message(&tools_message)
    };

    let _ = child.kill();
    let _ = child.wait();

    (
        SessionMcpConnectionStatus::Connected,
        Some(format!(
            "Attached at session start and discovered {} tool(s).",
            tools.len()
        )),
        tools,
    )
}

fn write_mcp_framed_message(
    writer: &mut impl Write,
    message: &serde_json::Value,
) -> io::Result<()> {
    let body = serde_json::to_vec(message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

fn read_mcp_framed_message(reader: &mut impl BufRead) -> io::Result<Option<serde_json::Value>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return if content_length.is_some() {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "unexpected EOF while reading MCP frame headers",
                ))
            } else {
                Ok(None)
            };
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            let parsed = rest.trim().parse::<usize>().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid MCP content length: {error}"),
                )
            })?;
            content_length = Some(parsed);
        }
    }

    let Some(content_length) = content_length else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing Content-Length header in MCP frame",
        ));
    };
    let mut body = vec![0_u8; content_length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn extract_mcp_tools_from_message(message: &serde_json::Value) -> Vec<SessionMcpTool> {
    message
        .get("result")
        .and_then(|result| result.get("tools"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            let name = tool.get("name")?.as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            Some(SessionMcpTool {
                name: name.to_string(),
                description: tool
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                input_schema: tool.get("inputSchema").cloned(),
            })
        })
        .collect()
}

fn compact_json_value(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| String::from("unavailable error payload"))
}

fn validate_session_mounts(
    mounted_refs: Vec<SessionMountRef>,
) -> Result<Vec<SessionMountRef>, RuntimeProtocolError> {
    let mut seen_mount_ids = HashSet::new();
    for mount in &mounted_refs {
        if matches!(mount.kind, SessionMountKind::Unsupported) {
            return Err(protocol_error(
                "unsupported_session_mount_kind",
                format!(
                    "session mount `{}` uses an unsupported kind; Probe currently accepts only `knowledge_pack` and `eval_pack`",
                    mount.mount_id
                ),
            ));
        }
        if mount.mount_id.trim().is_empty() {
            return Err(protocol_error(
                "invalid_session_mount",
                "session mounts must include a non-empty mount_id".to_string(),
            ));
        }
        if !seen_mount_ids.insert(mount.mount_id.clone()) {
            return Err(protocol_error(
                "duplicate_session_mount_id",
                format!(
                    "session mount id `{}` was provided more than once",
                    mount.mount_id
                ),
            ));
        }
        if mount.resource_ref.trim().is_empty() {
            return Err(protocol_error(
                "invalid_session_mount",
                format!(
                    "session mount `{}` must include a non-empty resource_ref",
                    mount.mount_id
                ),
            ));
        }
        if mount.provenance.publisher.trim().is_empty() {
            return Err(protocol_error(
                "invalid_session_mount",
                format!(
                    "session mount `{}` must include a non-empty provenance.publisher",
                    mount.mount_id
                ),
            ));
        }
        if mount.provenance.source_ref.trim().is_empty() {
            return Err(protocol_error(
                "invalid_session_mount",
                format!(
                    "session mount `{}` must include a non-empty provenance.source_ref",
                    mount.mount_id
                ),
            ));
        }
    }
    Ok(mounted_refs)
}

fn execution_host_for_owner(runtime_owner: &SessionRuntimeOwner) -> SessionExecutionHost {
    let kind = match runtime_owner.kind {
        SessionRuntimeOwnerKind::HostedControlPlane => SessionExecutionHostKind::HostedWorker,
        SessionRuntimeOwnerKind::ForegroundChild | SessionRuntimeOwnerKind::LocalDaemon => {
            SessionExecutionHostKind::LocalMachine
        }
    };
    SessionExecutionHost {
        kind,
        host_id: runtime_owner.owner_id.clone(),
        display_name: runtime_owner.display_name.clone(),
        location: None,
    }
}

fn resolve_baseline_ref(
    probe_home: &Path,
    mut baseline: SessionPreparedBaselineRef,
    notes: &mut Vec<String>,
) -> SessionPreparedBaselineRef {
    match read_manifest::<HostedBaselineManifest>(
        probe_home,
        HOSTED_BASELINES_DIR,
        baseline.baseline_id.as_str(),
    ) {
        Ok(Some(manifest)) => {
            baseline.repo_identity = baseline.repo_identity.or(manifest.repo_identity);
            baseline.base_ref = baseline.base_ref.or(manifest.base_ref);
            baseline.status = if manifest.stale {
                SessionPreparedBaselineStatus::Stale
            } else {
                SessionPreparedBaselineStatus::Ready
            };
        }
        Ok(None) => {
            baseline.status = SessionPreparedBaselineStatus::Missing;
            notes.push(format!(
                "prepared baseline {} has no manifest in {}",
                baseline.baseline_id, HOSTED_BASELINES_DIR
            ));
        }
        Err(message) => {
            baseline.status = SessionPreparedBaselineStatus::Missing;
            notes.push(format!(
                "prepared baseline {} could not be read: {}",
                baseline.baseline_id, message
            ));
        }
    }
    baseline
}

fn resolve_snapshot_ref(
    probe_home: &Path,
    mut snapshot: SessionWorkspaceSnapshotRef,
    notes: &mut Vec<String>,
) -> SessionWorkspaceSnapshotRef {
    match read_manifest::<HostedSnapshotManifest>(
        probe_home,
        HOSTED_SNAPSHOTS_DIR,
        snapshot.snapshot_id.as_str(),
    ) {
        Ok(Some(manifest)) => {
            snapshot.restore_manifest_id = snapshot
                .restore_manifest_id
                .or(manifest.restore_manifest_id);
            snapshot.source_baseline_id =
                snapshot.source_baseline_id.or(manifest.source_baseline_id);
        }
        Ok(None) => notes.push(format!(
            "snapshot {} has no manifest in {}",
            snapshot.snapshot_id, HOSTED_SNAPSHOTS_DIR
        )),
        Err(message) => notes.push(format!(
            "snapshot {} could not be read: {}",
            snapshot.snapshot_id, message
        )),
    }
    snapshot
}

fn combine_provenance_note(existing: Option<String>, notes: Vec<String>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(existing) = existing.filter(|value| !value.trim().is_empty()) {
        parts.push(existing);
    }
    parts.extend(notes.into_iter().filter(|value| !value.trim().is_empty()));
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn read_manifest<T>(probe_home: &Path, subdir: &str, manifest_id: &str) -> Result<Option<T>, String>
where
    T: for<'de> Deserialize<'de>,
{
    let path = probe_home
        .join(subdir)
        .join(manifest_file_name(manifest_id));
    match fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents)
            .map(Some)
            .map_err(|error| format!("{} at {}", error, path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{} at {}", error, path.display())),
    }
}

fn manifest_file_name(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|value| {
            if value.is_ascii_alphanumeric() || matches!(value, '-' | '_') {
                value
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "{}.json",
        if sanitized.is_empty() {
            String::from("manifest")
        } else {
            sanitized
        }
    )
}

fn preferred_terminal_turn(
    turns: &[probe_protocol::runtime::SessionTurnControlRecord],
) -> Option<&probe_protocol::runtime::SessionTurnControlRecord> {
    turns.iter().max_by_key(|turn| {
        (
            turn.finished_at_ms.unwrap_or(0),
            terminal_status_rank(turn.status),
        )
    })
}

fn terminal_status_rank(status: QueuedTurnStatus) -> u8 {
    match status {
        QueuedTurnStatus::TimedOut => 4,
        QueuedTurnStatus::Failed => 3,
        QueuedTurnStatus::Cancelled => 2,
        QueuedTurnStatus::Completed => 1,
        QueuedTurnStatus::Queued | QueuedTurnStatus::Running => 0,
    }
}

fn turn_completed(
    runtime: &ProbeRuntime,
    hosted_receipt_config: Option<&HostedReceiptConfig>,
    outcome: probe_core::runtime::PlainTextExecOutcome,
) -> Result<TurnCompleted, RuntimeProtocolError> {
    let session = runtime
        .session_store()
        .read_metadata(&outcome.session.id)
        .map_err(session_store_error_to_protocol)?;
    let transcript = runtime
        .session_store()
        .read_transcript(&outcome.session.id)
        .map_err(session_store_error_to_protocol)?;
    let session = sync_hosted_session_metadata_from_store(
        runtime.session_store(),
        hosted_receipt_config,
        session,
        transcript.as_slice(),
        now_ms(),
    )?;
    Ok(TurnCompleted {
        session,
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
    })
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
        RuntimeEvent::ActivityUpdated { .. } => EventDeliveryGuarantee::Lossless,
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

fn emit_runtime_progress(
    writer: &SharedJsonlWriter,
    request_id: Option<&String>,
    detached_event_hub: Option<&Arc<DetachedSessionEventHub>>,
    session_id: &SessionId,
    delivery: EventDeliveryGuarantee,
    event: RuntimeProgressEvent,
) {
    if let Some(request_id_for_events) = request_id {
        let _ = writer.send_event(
            request_id_for_events.as_str(),
            ServerEvent::RuntimeProgress {
                delivery,
                event: event.clone(),
            },
        );
    }
    if let Some(detached_event_hub) = detached_event_hub {
        let _ = detached_event_hub.append(
            session_id,
            detached_event_truth_from_delivery(delivery),
            DetachedSessionEventPayload::RuntimeProgress { delivery, event },
            now_ms(),
        );
    }
}

fn detached_event_truth_from_delivery(
    delivery: EventDeliveryGuarantee,
) -> DetachedSessionEventTruth {
    match delivery {
        EventDeliveryGuarantee::Lossless => DetachedSessionEventTruth::Authoritative,
        EventDeliveryGuarantee::BestEffort => DetachedSessionEventTruth::BestEffort,
    }
}

fn encode_runtime_event(event: RuntimeEvent) -> RuntimeProgressEvent {
    match event {
        RuntimeEvent::ActivityUpdated {
            session_id,
            activity,
        } => RuntimeProgressEvent::ActivityUpdated {
            session_id,
            activity,
        },
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

fn session_depth(
    runtime: &ProbeRuntime,
    session: &SessionMetadata,
) -> Result<usize, RuntimeProtocolError> {
    let mut depth = 0usize;
    let mut seen = HashSet::new();
    let mut cursor = session.parent_link.clone();
    while let Some(parent_link) = cursor {
        if !seen.insert(String::from(parent_link.session_id.as_str())) {
            return Err(protocol_error(
                "child_session_cycle",
                format!(
                    "session {} has a cyclic parent link chain",
                    session.id.as_str()
                ),
            ));
        }
        depth += 1;
        cursor = runtime
            .session_store()
            .read_metadata(&parent_link.session_id)
            .map_err(session_store_error_to_protocol)?
            .parent_link;
    }
    Ok(depth)
}

fn enforce_same_workspace_boundary(
    parent_cwd: &Path,
    child_cwd: &Path,
) -> Result<(), RuntimeProtocolError> {
    let parent_repo_root = resolve_git_repo_root(parent_cwd);
    let child_repo_root = resolve_git_repo_root(child_cwd);
    match (parent_repo_root, child_repo_root) {
        (Some(parent_repo_root), Some(child_repo_root)) if parent_repo_root == child_repo_root => {
            Ok(())
        }
        (Some(parent_repo_root), Some(child_repo_root)) => Err(protocol_error(
            "child_repo_mismatch",
            format!(
                "child session cwd {} resolves to repo {}, but parent session cwd {} resolves to repo {}; Probe only supports same-repo child sessions right now",
                child_cwd.display(),
                child_repo_root.display(),
                parent_cwd.display(),
                parent_repo_root.display(),
            ),
        )),
        (Some(parent_repo_root), None) => Err(protocol_error(
            "child_repo_mismatch",
            format!(
                "child session cwd {} is not inside the parent repo {}; Probe only supports same-repo child sessions right now",
                child_cwd.display(),
                parent_repo_root.display(),
            ),
        )),
        (None, Some(child_repo_root)) => Err(protocol_error(
            "child_repo_mismatch",
            format!(
                "parent session cwd {} is not inside a git repo, but child session cwd {} resolves to repo {}; Probe currently requires a shared repo boundary",
                parent_cwd.display(),
                child_cwd.display(),
                child_repo_root.display(),
            ),
        )),
        (None, None) if same_path(parent_cwd, child_cwd) => Ok(()),
        (None, None) => Err(protocol_error(
            "child_workspace_mismatch",
            format!(
                "parent session cwd {} and child session cwd {} do not share a git repo, so Probe only allows an exact cwd match for child sessions right now",
                parent_cwd.display(),
                child_cwd.display(),
            ),
        )),
    }
}

fn resolve_git_repo_root(path: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!root.is_empty()).then(|| PathBuf::from(root))
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn same_path_prefix(path: &Path, prefix: &Path) -> bool {
    match (path.canonicalize(), prefix.canonicalize()) {
        (Ok(path), Ok(prefix)) => path.starts_with(prefix),
        _ => path.starts_with(prefix),
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

fn build_probe_mesh_plugin_offer(
    session: &SessionMetadata,
    tool_set: &str,
) -> Result<SessionMeshPluginOffer, RuntimeProtocolError> {
    let registry = match tool_set {
        "coding_bootstrap" => ToolRegistry::coding_bootstrap(false, false),
        other => {
            return Err(protocol_error(
                "mesh_plugin_unsupported_tool_set",
                format!("unsupported mesh plugin tool set: {other}"),
            ));
        }
    };
    let tools = registry
        .declared_tools()
        .into_iter()
        .map(|tool| SessionMeshPluginTool {
            name: tool.function.name,
            description: tool.function.description,
        })
        .collect::<Vec<_>>();
    let worker_id = session
        .backend
        .as_ref()
        .and_then(|backend| backend.psionic_mesh.as_ref())
        .and_then(|mesh| mesh.local_worker_id.clone());
    let runtime_owner_kind = session.runtime_owner.as_ref().map(|owner| owner.kind);
    let attach_transport = session
        .runtime_owner
        .as_ref()
        .map(|owner| owner.attach_transport);
    let attach_target = session
        .runtime_owner
        .as_ref()
        .and_then(|owner| owner.attach_target.clone());
    let usage_hint = if let Some(target) = attach_target.as_deref() {
        format!(
            "Attach to Probe session {} via {} to use this local tool bundle on the published node.",
            session.id.as_str(),
            target
        )
    } else {
        format!(
            "Attach to Probe session {} to use this local tool bundle on the published node.",
            session.id.as_str()
        )
    };
    Ok(SessionMeshPluginOffer {
        plugin_id: String::from(PROBE_MESH_PLUGIN_CODING_BOOTSTRAP_ID),
        tool_set: tool_set.to_string(),
        label: String::from("Probe coding bootstrap"),
        summary: String::from(
            "Bounded Probe coding tools run locally on one node but can be discovered across the mesh.",
        ),
        session_id: session.id.clone(),
        execution_scope: String::from("local_probe_runtime"),
        usage_hint,
        worker_id,
        runtime_owner_kind,
        attach_transport,
        attach_target,
        tools,
        entry_id: None,
        published_at_ms: None,
    })
}

fn mesh_plugin_offer_from_entry(
    entry: &SessionMeshCoordinationEntry,
) -> Option<SessionMeshPluginOffer> {
    let body = entry.body.as_deref()?;
    let envelope = serde_json::from_str::<ProbeMeshPluginOfferEnvelope>(body).ok()?;
    if envelope.schema != PROBE_MESH_PLUGIN_OFFER_SCHEMA {
        return None;
    }
    let mut offer = envelope.offer;
    if offer.worker_id.is_none() {
        offer.worker_id = Some(entry.worker_id.clone());
    }
    offer.entry_id = Some(entry.id);
    offer.published_at_ms = Some(entry.created_at_ms);
    Some(offer)
}

fn mesh_coordination_http_client() -> Result<reqwest::blocking::Client, RuntimeProtocolError> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|error| {
            protocol_error(
                "mesh_coordination_transport_error",
                format!("failed to build mesh coordination client: {error}"),
            )
        })
}

fn psionic_mesh_management_base_url_from_session(
    session: &SessionMetadata,
) -> Result<String, RuntimeProtocolError> {
    let Some(backend) = session.backend.as_ref() else {
        return Err(protocol_error(
            "mesh_coordination_unavailable",
            format!(
                "session {} has no backend target to resolve mesh coordination against",
                session.id.as_str()
            ),
        ));
    };
    if let Some(mesh) = backend.psionic_mesh.as_ref() {
        return Ok(mesh.management_base_url.clone());
    }
    if backend.control_plane
        == Some(probe_protocol::backend::BackendControlPlaneKind::PsionicInferenceMesh)
    {
        return Ok(backend.base_url.trim_end_matches("/v1").to_string());
    }
    Err(protocol_error(
        "mesh_coordination_unavailable",
        format!(
            "session {} is not attached to a Psionic mesh control plane",
            session.id.as_str()
        ),
    ))
}

fn psionic_management_get<Q, T>(
    client: &reqwest::blocking::Client,
    management_base_url: &str,
    path: &str,
    query: Option<&Q>,
) -> Result<T, RuntimeProtocolError>
where
    Q: Serialize,
    T: DeserializeOwned,
{
    let url = format!("{management_base_url}{path}");
    let request = client.get(url.as_str());
    let request = if let Some(query) = query {
        request.query(query)
    } else {
        request
    };
    let response = request.send().map_err(|error| {
        protocol_error(
            "mesh_coordination_transport_error",
            format!("failed to reach mesh coordination at {url}: {error}"),
        )
    })?;
    psionic_management_json_response(response, url.as_str())
}

fn psionic_management_post<B, T>(
    client: &reqwest::blocking::Client,
    management_base_url: &str,
    path: &str,
    body: &B,
) -> Result<T, RuntimeProtocolError>
where
    B: Serialize,
    T: DeserializeOwned,
{
    let url = format!("{management_base_url}{path}");
    let response = client
        .post(url.as_str())
        .json(body)
        .send()
        .map_err(|error| {
            protocol_error(
                "mesh_coordination_transport_error",
                format!("failed to reach mesh coordination at {url}: {error}"),
            )
        })?;
    psionic_management_json_response(response, url.as_str())
}

fn psionic_management_json_response<T>(
    response: reqwest::blocking::Response,
    url: &str,
) -> Result<T, RuntimeProtocolError>
where
    T: DeserializeOwned,
{
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().unwrap_or_default();
        let code = if status == reqwest::StatusCode::NOT_FOUND {
            "mesh_coordination_unavailable"
        } else {
            "backend_unavailable"
        };
        let detail = detail.trim();
        return Err(protocol_error(
            code,
            if detail.is_empty() {
                format!("mesh coordination request to {url} failed with {status}")
            } else {
                format!("mesh coordination request to {url} failed with {status}: {detail}")
            },
        ));
    }
    response.json::<T>().map_err(|error| {
        protocol_error(
            "mesh_coordination_decode_error",
            format!("failed to decode mesh coordination response from {url}: {error}"),
        )
    })
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

fn session_summary_artifact_error_to_protocol(
    error: SessionSummaryArtifactError,
) -> RuntimeProtocolError {
    match error {
        SessionSummaryArtifactError::SessionStore(error) => session_store_error_to_protocol(error),
        SessionSummaryArtifactError::Json(error) => {
            protocol_error("summary_artifact_json_error", error.to_string())
        }
    }
}

fn detached_registry_error_to_protocol(error: DetachedRegistryError) -> RuntimeProtocolError {
    protocol_error("detached_registry_error", error.to_string())
}

fn detached_event_error_to_protocol(error: DetachedEventError) -> RuntimeProtocolError {
    protocol_error("detached_event_error", error.to_string())
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
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use probe_core::session_store::{NewItem, NewSession};
    use probe_protocol::backend::{BackendKind, PrefixCacheMode, ServerAttachMode};
    use probe_protocol::runtime::{ToolApprovalRecipe, ToolChoice, ToolDeniedAction};
    use probe_protocol::session::{
        ItemId, SessionId, SessionTurn, TaskCheckpointStatus, TaskCheckpointSummary,
        TaskFinalReceipt, TaskReceiptDisposition, TaskRevertibilityStatus,
        TaskRevertibilitySummary, TaskVerificationStatus, TaskWorkspaceSummary,
        TaskWorkspaceSummaryStatus, ToolApprovalState, ToolExecutionRecord, ToolPolicyDecision,
        ToolRiskClass, TranscriptEvent, TranscriptItem, TranscriptItemKind, TurnId,
    };
    use serde_json::json;

    use super::{
        ProbeServerCore, ProbeToolChoice, TaskWorkspaceBaseline, ToolLoopRecipe, ToolSetKind,
        approval_from_recipe, build_task_final_receipt, build_task_workspace_summary,
        dirty_worktree_paths, load_session_mcp_state, normalize_session_title,
        resolve_git_repo_root, revert_latest_task_in_session, tool_choice_from_recipe,
        tool_loop_from_recipe,
    };
    use tempfile::tempdir;

    fn tool_result_event(
        turn_index: u64,
        tool_name: &str,
        risk_class: ToolRiskClass,
        files_touched: &[&str],
        files_changed: &[&str],
    ) -> TranscriptEvent {
        TranscriptEvent {
            session_id: SessionId::new("sess_test"),
            turn: SessionTurn {
                id: TurnId(turn_index),
                index: turn_index,
                started_at_ms: turn_index,
                completed_at_ms: Some(turn_index),
                observability: None,
                backend_receipt: None,
                items: vec![TranscriptItem {
                    id: ItemId::new(format!("item_{turn_index}")),
                    turn_id: TurnId(turn_index),
                    sequence: 0,
                    kind: TranscriptItemKind::ToolResult,
                    text: String::from(r#"{"ok":true}"#),
                    name: Some(String::from(tool_name)),
                    tool_call_id: Some(format!("call_{turn_index}")),
                    arguments: None,
                    tool_execution: Some(ToolExecutionRecord {
                        risk_class,
                        policy_decision: ToolPolicyDecision::Approved,
                        approval_state: ToolApprovalState::Approved,
                        command: None,
                        exit_code: Some(0),
                        timed_out: None,
                        truncated: None,
                        bytes_returned: None,
                        files_touched: files_touched
                            .iter()
                            .map(|path| String::from(*path))
                            .collect(),
                        files_changed: files_changed
                            .iter()
                            .map(|path| String::from(*path))
                            .collect(),
                        reason: None,
                    }),
                }],
            },
        }
    }

    fn shell_validation_event(
        turn_index: u64,
        command: &str,
        exit_code: Option<i32>,
        timed_out: bool,
        truncated: bool,
    ) -> TranscriptEvent {
        TranscriptEvent {
            session_id: SessionId::new("sess_test"),
            turn: SessionTurn {
                id: TurnId(turn_index),
                index: turn_index,
                started_at_ms: turn_index,
                completed_at_ms: Some(turn_index),
                observability: None,
                backend_receipt: None,
                items: vec![TranscriptItem {
                    id: ItemId::new(format!("shell_item_{turn_index}")),
                    turn_id: TurnId(turn_index),
                    sequence: 0,
                    kind: TranscriptItemKind::ToolResult,
                    text: format!(
                        "{{\"command\":\"{command}\",\"stdout\":\"ok\",\"stderr\":\"\",\"timed_out\":{timed_out},\"exit_code\":{}}}",
                        exit_code.map_or(String::from("null"), |value| value.to_string())
                    ),
                    name: Some(String::from("shell")),
                    tool_call_id: Some(format!("shell_call_{turn_index}")),
                    arguments: None,
                    tool_execution: Some(ToolExecutionRecord {
                        risk_class: ToolRiskClass::ReadOnly,
                        policy_decision: ToolPolicyDecision::Approved,
                        approval_state: ToolApprovalState::Approved,
                        command: Some(String::from(command)),
                        exit_code,
                        timed_out: Some(timed_out),
                        truncated: Some(truncated),
                        bytes_returned: Some(2),
                        files_touched: Vec::new(),
                        files_changed: Vec::new(),
                        reason: None,
                    }),
                }],
            },
        }
    }

    #[cfg(unix)]
    fn write_fake_stdio_mcp_server_script(path: &Path, tools: &[(&str, &str)]) {
        let initialize_body = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {
                    "name": "fake-mcp",
                    "version": "1.0.0"
                }
            }
        }))
        .expect("serialize initialize body");
        let tools_body = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": tools
                    .iter()
                    .map(|(name, description)| json!({
                        "name": name,
                        "description": description,
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" }
                            }
                        }
                    }))
                    .collect::<Vec<_>>()
            }
        }))
        .expect("serialize tools body");
        let script = format!(
            "#!/bin/sh\ncat >/dev/null &\nprintf 'Content-Length: %s\\r\\n\\r\\n%s' '{init_len}' '{init_body}'\nprintf 'Content-Length: %s\\r\\n\\r\\n%s' '{tools_len}' '{tools_body}'\nsleep 1\n",
            init_len = initialize_body.len(),
            init_body = initialize_body,
            tools_len = tools_body.len(),
            tools_body = tools_body,
        );
        fs::write(path, script).expect("write fake mcp server script");
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("set execute permissions");
    }

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

    #[cfg(unix)]
    #[test]
    fn load_session_mcp_state_snapshots_enabled_entries() {
        let temp = tempdir().expect("tempdir");
        let probe_home = temp.path().join("probe-home");
        fs::create_dir_all(probe_home.join("mcp")).expect("create mcp dir");
        #[cfg(unix)]
        let server_script = temp.path().join("fake-mcp-server.sh");
        #[cfg(unix)]
        write_fake_stdio_mcp_server_script(
            &server_script,
            &[
                ("filesystem/read", "Read files"),
                ("filesystem/list", "List files"),
            ],
        );
        fs::write(
            probe_home.join("mcp/servers.json"),
            serde_json::to_string_pretty(&json!({
                "servers": [
                    {
                        "id": "filesystem",
                        "name": "Filesystem",
                        "enabled": true,
                        "source": "manual_launch",
                        "transport": "stdio",
                        "target": format!("{}", server_script.display())
                    },
                    {
                        "id": "disabled-recipe",
                        "name": "Disabled Recipe",
                        "enabled": false,
                        "source": "provider_command_recipe",
                        "provider_setup_command": "pnpm dlx shadcn@latest mcp init --client codex",
                        "client_hint": "codex"
                    }
                ]
            }))
            .expect("serialize registry"),
        )
        .expect("write registry");

        let mcp_state = load_session_mcp_state(&probe_home);
        assert!(mcp_state.load_error.is_none());
        assert_eq!(mcp_state.servers.len(), 1);
        assert_eq!(mcp_state.servers[0].id, "filesystem");
        assert_eq!(mcp_state.servers[0].name, "Filesystem");
        assert_eq!(
            mcp_state.servers[0].connection_status,
            Some(probe_protocol::session::SessionMcpConnectionStatus::Connected)
        );
        assert_eq!(
            mcp_state.servers[0].transport,
            Some(probe_protocol::session::SessionMcpServerTransport::Stdio)
        );
        assert_eq!(mcp_state.servers[0].discovered_tools.len(), 2);
        assert_eq!(
            mcp_state.servers[0].discovered_tools[0].name,
            "filesystem/read"
        );
        assert_eq!(
            mcp_state.servers[0].discovered_tools[0].input_schema,
            Some(json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                }
            }))
        );
        assert_eq!(
            mcp_state.servers[0].discovered_tools[1].name,
            "filesystem/list"
        );
    }

    #[test]
    fn load_session_mcp_state_surfaces_malformed_registry_errors() {
        let temp = tempdir().expect("tempdir");
        let probe_home = temp.path().join("probe-home");
        fs::create_dir_all(probe_home.join("mcp")).expect("create mcp dir");
        fs::write(probe_home.join("mcp/servers.json"), "{ not valid json")
            .expect("write malformed registry");

        let mcp_state = load_session_mcp_state(&probe_home);
        assert!(mcp_state.servers.is_empty());
        let error = mcp_state.load_error.expect("load error should be present");
        assert!(error.contains("Probe could not read"), "{error}");
        assert!(error.contains("servers.json"), "{error}");
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
            control_plane: None,
            psionic_mesh: None,
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

    #[test]
    fn task_workspace_summary_tracks_changed_files_and_preexisting_dirty_state() {
        let baseline = TaskWorkspaceBaseline {
            task_start_turn_index: 3,
            repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
            preexisting_dirty_files: vec![String::from("Cargo.toml")],
        };
        let transcript = vec![
            tool_result_event(
                2,
                "apply_patch",
                ToolRiskClass::Write,
                &["src/old.rs"],
                &["src/old.rs"],
            ),
            tool_result_event(
                3,
                "apply_patch",
                ToolRiskClass::Write,
                &["src/lib.rs", "README.md"],
                &["src/lib.rs"],
            ),
        ];

        let summary = build_task_workspace_summary(
            transcript.as_slice(),
            &baseline,
            TaskWorkspaceSummaryStatus::Changed,
            &[],
        );

        assert_eq!(summary.status, TaskWorkspaceSummaryStatus::Changed);
        assert_eq!(summary.changed_files, vec![String::from("src/lib.rs")]);
        assert_eq!(
            summary.touched_but_unchanged_files,
            vec![String::from("README.md")]
        );
        assert_eq!(
            summary.preexisting_dirty_files,
            vec![String::from("Cargo.toml")]
        );
        assert!(summary.outside_tracking_dirty_files.is_empty());
        assert_eq!(summary.checkpoint.status, TaskCheckpointStatus::Captured);
        assert_eq!(
            summary.revertibility.status,
            TaskRevertibilityStatus::Limited
        );
        assert!(
            summary
                .summary_text
                .contains("This task changed 1 file(s): src/lib.rs.")
        );
        assert!(
            summary
                .summary_text
                .contains("Touched without lasting changes: README.md.")
        );
        assert!(
            summary
                .summary_text
                .contains("Dirty before task start: Cargo.toml.")
        );
    }

    #[test]
    fn task_workspace_summary_captures_diff_previews_from_git_state() {
        let temp = tempdir().expect("tempdir");
        let repo = temp.path();
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo)
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .arg("-c")
            .arg("user.name=Probe")
            .arg("-c")
            .arg("user.email=probe@example.com")
            .arg("commit")
            .arg("--allow-empty")
            .arg("-m")
            .arg("initial")
            .current_dir(repo)
            .output()
            .expect("git commit");
        std::fs::create_dir_all(repo.join("src")).expect("src dir");
        std::fs::write(
            repo.join("src/lib.rs"),
            "pub fn greeting() -> &'static str {\n    \"hello from probe\"\n}\n",
        )
        .expect("write changed file");

        let baseline = TaskWorkspaceBaseline {
            task_start_turn_index: 3,
            repo_root: Some(repo.to_path_buf()),
            preexisting_dirty_files: Vec::new(),
        };
        let transcript = vec![tool_result_event(
            3,
            "apply_patch",
            ToolRiskClass::Write,
            &["src/lib.rs"],
            &["src/lib.rs"],
        )];

        let summary = build_task_workspace_summary(
            transcript.as_slice(),
            &baseline,
            TaskWorkspaceSummaryStatus::Changed,
            &[],
        );

        assert_eq!(summary.diff_previews.len(), 1);
        let preview = &summary.diff_previews[0];
        assert_eq!(preview.path, "src/lib.rs");
        assert!(!preview.truncated);
        assert!(
            preview
                .diff_lines
                .iter()
                .any(|line| line.contains("src/lib.rs"))
        );
        assert!(preview.diff_lines.iter().any(|line| line.contains("+++")));
        assert!(
            preview
                .diff_lines
                .iter()
                .any(|line| line == "+pub fn greeting() -> &'static str {")
        );
    }

    #[test]
    fn task_workspace_summary_marks_shell_writes_without_file_accounting_as_limited() {
        let baseline = TaskWorkspaceBaseline {
            task_start_turn_index: 7,
            repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
            preexisting_dirty_files: Vec::new(),
        };
        let transcript = vec![tool_result_event(
            7,
            "shell",
            ToolRiskClass::Write,
            &[],
            &[],
        )];

        let summary = build_task_workspace_summary(
            transcript.as_slice(),
            &baseline,
            TaskWorkspaceSummaryStatus::Changed,
            &[],
        );

        assert_eq!(
            summary.status,
            TaskWorkspaceSummaryStatus::ChangeAccountingLimited
        );
        assert!(summary.changed_files.is_empty());
        assert!(summary.change_accounting_limited);
        assert_eq!(summary.checkpoint.status, TaskCheckpointStatus::Limited);
        assert_eq!(
            summary.revertibility.status,
            TaskRevertibilityStatus::Limited
        );
        assert!(
            summary
                .summary_text
                .contains("cannot confirm whether repo changes landed")
        );
    }

    #[test]
    fn task_workspace_summary_marks_read_only_tasks_as_no_repo_changes() {
        let baseline = TaskWorkspaceBaseline {
            task_start_turn_index: 5,
            repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
            preexisting_dirty_files: Vec::new(),
        };
        let transcript = vec![
            tool_result_event(5, "read_file", ToolRiskClass::ReadOnly, &["README.md"], &[]),
            tool_result_event(
                6,
                "code_search",
                ToolRiskClass::ReadOnly,
                &["src/lib.rs"],
                &[],
            ),
        ];

        let summary = build_task_workspace_summary(
            transcript.as_slice(),
            &baseline,
            TaskWorkspaceSummaryStatus::Changed,
            &[],
        );

        assert_eq!(summary.status, TaskWorkspaceSummaryStatus::NoRepoChanges);
        assert!(summary.changed_files.is_empty());
        assert!(summary.touched_but_unchanged_files.is_empty());
        assert_eq!(summary.checkpoint.status, TaskCheckpointStatus::NotCaptured);
        assert_eq!(
            summary.revertibility.status,
            TaskRevertibilityStatus::Unavailable
        );
        assert_eq!(
            summary.summary_text,
            "No repo changes were made by this task."
        );
    }

    #[test]
    fn task_workspace_summary_reports_dirty_files_outside_tracked_tool_results() {
        let baseline = TaskWorkspaceBaseline {
            task_start_turn_index: 12,
            repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
            preexisting_dirty_files: vec![String::from("Cargo.toml")],
        };
        let transcript = vec![tool_result_event(
            12,
            "apply_patch",
            ToolRiskClass::Write,
            &["src/lib.rs"],
            &["src/lib.rs"],
        )];

        let summary = build_task_workspace_summary(
            transcript.as_slice(),
            &baseline,
            TaskWorkspaceSummaryStatus::Changed,
            &[
                String::from("Cargo.toml"),
                String::from("src/lib.rs"),
                String::from("generated/schema.json"),
            ],
        );

        assert_eq!(
            summary.outside_tracking_dirty_files,
            vec![String::from("generated/schema.json")]
        );
        assert_eq!(summary.checkpoint.status, TaskCheckpointStatus::Limited);
        assert_eq!(
            summary.revertibility.status,
            TaskRevertibilityStatus::Limited
        );
        assert!(summary.summary_text.contains(
            "Additional dirty files appeared during the task outside tracked tool results: generated/schema.json."
        ));
    }

    #[test]
    fn task_workspace_summary_marks_created_files_as_exactly_revertable_when_tracked() {
        let baseline = TaskWorkspaceBaseline {
            task_start_turn_index: 4,
            repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
            preexisting_dirty_files: Vec::new(),
        };
        let transcript = vec![
            TranscriptEvent {
                session_id: SessionId::new("sess_test"),
                turn: SessionTurn {
                    id: TurnId(4),
                    index: 4,
                    started_at_ms: 4,
                    completed_at_ms: Some(4),
                    observability: None,
                    backend_receipt: None,
                    items: vec![TranscriptItem {
                        id: ItemId::new("item_create_call"),
                        turn_id: TurnId(4),
                        sequence: 0,
                        kind: TranscriptItemKind::ToolCall,
                        text: String::from(
                            r##"{"path":"example.md","old_text":"","new_text":"# Example\n","create_if_missing":true}"##,
                        ),
                        name: Some(String::from("apply_patch")),
                        tool_call_id: Some(String::from("call_create")),
                        arguments: Some(json!({
                            "path": "example.md",
                            "old_text": "",
                            "new_text": "# Example\n",
                            "create_if_missing": true
                        })),
                        tool_execution: None,
                    }],
                },
            },
            tool_result_event(
                5,
                "apply_patch",
                ToolRiskClass::Write,
                &["example.md"],
                &["example.md"],
            ),
        ];

        let mut transcript = transcript;
        transcript[1].turn.items[0].tool_call_id = Some(String::from("call_create"));

        let summary = build_task_workspace_summary(
            transcript.as_slice(),
            &baseline,
            TaskWorkspaceSummaryStatus::Changed,
            &[],
        );

        assert_eq!(summary.revertibility.status, TaskRevertibilityStatus::Exact);
        assert!(
            summary
                .revertibility
                .summary_text
                .contains("Probe has enough checkpoint coverage"),
            "{}",
            summary.revertibility.summary_text
        );
    }

    #[test]
    fn revert_latest_task_in_session_restores_exact_apply_patch_edits() {
        let temp = tempdir().expect("tempdir");
        let probe_home = temp.path().join("probe-home");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(workspace.join("src")).expect("workspace layout");
        let file_path = workspace.join("src/lib.rs");
        std::fs::write(&file_path, "hello probe\n").expect("write patched file");

        let runtime = probe_core::runtime::ProbeRuntime::new(&probe_home);
        let session = runtime
            .session_store()
            .create_session_with(NewSession::new("revert test", &workspace))
            .expect("create session");
        runtime
            .session_store()
            .append_turn(
                &session.id,
                &[NewItem::tool_call(
                    "apply_patch",
                    "call_patch_1",
                    json!({
                        "path": "src/lib.rs",
                        "old_text": "hello world\n",
                        "new_text": "hello probe\n",
                    }),
                )],
            )
            .expect("append tool call");
        runtime
            .session_store()
            .append_turn(
                &session.id,
                &[NewItem::tool_result(
                    "apply_patch",
                    "call_patch_1",
                    r#"{"ok":true}"#,
                    ToolExecutionRecord {
                        risk_class: ToolRiskClass::Write,
                        policy_decision: ToolPolicyDecision::Approved,
                        approval_state: ToolApprovalState::Approved,
                        command: None,
                        exit_code: Some(0),
                        timed_out: None,
                        truncated: None,
                        bytes_returned: None,
                        files_touched: vec![String::from("src/lib.rs")],
                        files_changed: vec![String::from("src/lib.rs")],
                        reason: None,
                    },
                )],
            )
            .expect("append tool result");

        let mut metadata = runtime
            .session_store()
            .read_metadata(&session.id)
            .expect("read metadata");
        let workspace_summary = TaskWorkspaceSummary {
            task_start_turn_index: 0,
            status: TaskWorkspaceSummaryStatus::Changed,
            changed_files: vec![String::from("src/lib.rs")],
            touched_but_unchanged_files: Vec::new(),
            preexisting_dirty_files: Vec::new(),
            outside_tracking_dirty_files: Vec::new(),
            repo_root: Some(workspace.clone()),
            change_accounting_limited: false,
            checkpoint: TaskCheckpointSummary {
                status: TaskCheckpointStatus::Captured,
                summary_text: String::from("checkpoint captured"),
            },
            revertibility: TaskRevertibilitySummary {
                status: TaskRevertibilityStatus::Exact,
                summary_text: String::from("exact restore available"),
            },
            diff_previews: Vec::new(),
            summary_text: String::from("This task changed 1 file(s): src/lib.rs."),
        };
        metadata.latest_task_workspace_summary = Some(workspace_summary.clone());
        metadata.latest_task_receipt = Some(TaskFinalReceipt {
            disposition: TaskReceiptDisposition::Succeeded,
            workspace: workspace_summary,
            verification_status: TaskVerificationStatus::NotRun,
            verification_commands: Vec::new(),
            uncertainty_reasons: Vec::new(),
            summary_text: String::from("applied src/lib.rs"),
        });
        runtime
            .session_store()
            .replace_metadata(metadata)
            .expect("store metadata");

        let reverted_files =
            revert_latest_task_in_session(&runtime, &session.id).expect("revert latest task");

        assert_eq!(reverted_files, vec![String::from("src/lib.rs")]);
        assert_eq!(
            std::fs::read_to_string(&file_path).expect("read restored file"),
            "hello world\n"
        );
        let updated = runtime
            .session_store()
            .read_metadata(&session.id)
            .expect("read updated metadata");
        let receipt = updated
            .latest_task_receipt
            .expect("revert task receipt should exist");
        assert_eq!(
            receipt.workspace.status,
            TaskWorkspaceSummaryStatus::Reverted
        );
        assert!(receipt.summary_text.contains("Reverted the latest task"));
    }

    #[test]
    fn revert_latest_task_in_session_removes_exact_created_files() {
        let temp = tempdir().expect("tempdir");
        let probe_home = temp.path().join("probe-home");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace layout");
        let file_path = workspace.join("notes.md");
        std::fs::write(&file_path, "# Placeholder\n").expect("write created file");

        let runtime = probe_core::runtime::ProbeRuntime::new(&probe_home);
        let session = runtime
            .session_store()
            .create_session_with(NewSession::new("revert test", &workspace))
            .expect("create session");
        runtime
            .session_store()
            .append_turn(
                &session.id,
                &[NewItem::tool_call(
                    "apply_patch",
                    "call_patch_create",
                    json!({
                        "path": "notes.md",
                        "old_text": "",
                        "new_text": "# Placeholder\n",
                        "create_if_missing": true,
                    }),
                )],
            )
            .expect("append tool call");
        runtime
            .session_store()
            .append_turn(
                &session.id,
                &[NewItem::tool_result(
                    "apply_patch",
                    "call_patch_create",
                    r#"{"ok":true}"#,
                    ToolExecutionRecord {
                        risk_class: ToolRiskClass::Write,
                        policy_decision: ToolPolicyDecision::Approved,
                        approval_state: ToolApprovalState::Approved,
                        command: None,
                        exit_code: Some(0),
                        timed_out: None,
                        truncated: None,
                        bytes_returned: None,
                        files_touched: vec![String::from("notes.md")],
                        files_changed: vec![String::from("notes.md")],
                        reason: None,
                    },
                )],
            )
            .expect("append tool result");

        let mut metadata = runtime
            .session_store()
            .read_metadata(&session.id)
            .expect("read metadata");
        metadata.latest_task_workspace_summary = Some(TaskWorkspaceSummary {
            task_start_turn_index: 0,
            status: TaskWorkspaceSummaryStatus::Changed,
            changed_files: vec![String::from("notes.md")],
            touched_but_unchanged_files: Vec::new(),
            preexisting_dirty_files: Vec::new(),
            outside_tracking_dirty_files: Vec::new(),
            repo_root: Some(workspace.clone()),
            change_accounting_limited: false,
            checkpoint: TaskCheckpointSummary {
                status: TaskCheckpointStatus::Captured,
                summary_text: String::from("checkpoint captured"),
            },
            revertibility: TaskRevertibilitySummary {
                status: TaskRevertibilityStatus::Exact,
                summary_text: String::from("exact restore available"),
            },
            diff_previews: Vec::new(),
            summary_text: String::from("This task changed 1 file(s): notes.md."),
        });
        runtime
            .session_store()
            .replace_metadata(metadata)
            .expect("store metadata");

        let reverted_files =
            revert_latest_task_in_session(&runtime, &session.id).expect("revert latest task");
        assert_eq!(reverted_files, vec![String::from("notes.md")]);
        assert!(!file_path.exists(), "created file should be removed");
    }

    #[test]
    fn revert_latest_task_blocks_when_created_file_changed_after_probe_wrote_it() {
        let temp = tempdir().expect("tempdir");
        let probe_home = temp.path().join("probe-home");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace layout");
        let file_path = workspace.join("notes.md");
        std::fs::write(&file_path, "# Placeholder\nedited later\n").expect("write changed file");

        let runtime = probe_core::runtime::ProbeRuntime::new(&probe_home);
        let session = runtime
            .session_store()
            .create_session_with(NewSession::new("revert test", &workspace))
            .expect("create session");
        runtime
            .session_store()
            .append_turn(
                &session.id,
                &[NewItem::tool_call(
                    "apply_patch",
                    "call_patch_create",
                    json!({
                        "path": "notes.md",
                        "old_text": "",
                        "new_text": "# Placeholder\n",
                        "create_if_missing": true,
                    }),
                )],
            )
            .expect("append tool call");
        runtime
            .session_store()
            .append_turn(
                &session.id,
                &[NewItem::tool_result(
                    "apply_patch",
                    "call_patch_create",
                    r#"{"ok":true}"#,
                    ToolExecutionRecord {
                        risk_class: ToolRiskClass::Write,
                        policy_decision: ToolPolicyDecision::Approved,
                        approval_state: ToolApprovalState::Approved,
                        command: None,
                        exit_code: Some(0),
                        timed_out: None,
                        truncated: None,
                        bytes_returned: None,
                        files_touched: vec![String::from("notes.md")],
                        files_changed: vec![String::from("notes.md")],
                        reason: None,
                    },
                )],
            )
            .expect("append tool result");

        let mut metadata = runtime
            .session_store()
            .read_metadata(&session.id)
            .expect("read metadata");
        metadata.latest_task_workspace_summary = Some(TaskWorkspaceSummary {
            task_start_turn_index: 0,
            status: TaskWorkspaceSummaryStatus::Changed,
            changed_files: vec![String::from("notes.md")],
            touched_but_unchanged_files: Vec::new(),
            preexisting_dirty_files: Vec::new(),
            outside_tracking_dirty_files: Vec::new(),
            repo_root: Some(workspace.clone()),
            change_accounting_limited: false,
            checkpoint: TaskCheckpointSummary {
                status: TaskCheckpointStatus::Captured,
                summary_text: String::from("checkpoint captured"),
            },
            revertibility: TaskRevertibilitySummary {
                status: TaskRevertibilityStatus::Exact,
                summary_text: String::from("exact restore available"),
            },
            diff_previews: Vec::new(),
            summary_text: String::from("This task changed 1 file(s): notes.md."),
        });
        runtime
            .session_store()
            .replace_metadata(metadata)
            .expect("store metadata");

        let error = revert_latest_task_in_session(&runtime, &session.id)
            .expect_err("changed created file revert should fail");
        assert_eq!(error.code, "revert_conflict");
        assert!(
            error
                .message
                .contains("file no longer matches the contents it created"),
            "{}",
            error.message
        );
    }

    #[test]
    fn task_final_receipt_summarizes_passed_validation_commands() {
        let baseline = TaskWorkspaceBaseline {
            task_start_turn_index: 3,
            repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
            preexisting_dirty_files: vec![String::from("Cargo.toml")],
        };
        let transcript = vec![
            tool_result_event(
                3,
                "apply_patch",
                ToolRiskClass::Write,
                &["src/lib.rs"],
                &["src/lib.rs"],
            ),
            shell_validation_event(4, "cargo test -p probe-tui", Some(0), false, false),
        ];
        let workspace = build_task_workspace_summary(
            transcript.as_slice(),
            &baseline,
            TaskWorkspaceSummaryStatus::Changed,
            &[],
        );
        let receipt = build_task_final_receipt(
            transcript.as_slice(),
            TaskReceiptDisposition::Succeeded,
            &workspace,
        );

        assert_eq!(receipt.verification_status, TaskVerificationStatus::Passed);
        assert_eq!(receipt.verification_commands.len(), 1);
        assert!(receipt.summary_text.contains("Validation passed"));
        assert!(receipt.summary_text.contains("cargo test -p probe-tui"));
        assert!(receipt.uncertainty_reasons.is_empty());
    }

    #[test]
    fn task_final_receipt_marks_unverified_edits_explicitly() {
        let baseline = TaskWorkspaceBaseline {
            task_start_turn_index: 9,
            repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
            preexisting_dirty_files: Vec::new(),
        };
        let transcript = vec![tool_result_event(
            9,
            "apply_patch",
            ToolRiskClass::Write,
            &["src/main.rs"],
            &["src/main.rs"],
        )];
        let workspace = build_task_workspace_summary(
            transcript.as_slice(),
            &baseline,
            TaskWorkspaceSummaryStatus::Changed,
            &[],
        );
        let receipt = build_task_final_receipt(
            transcript.as_slice(),
            TaskReceiptDisposition::Succeeded,
            &workspace,
        );

        assert_eq!(receipt.verification_status, TaskVerificationStatus::NotRun);
        assert!(
            receipt
                .summary_text
                .contains("No validation command was observed")
        );
        assert!(
            receipt.uncertainty_reasons.iter().any(
                |reason| reason.contains("Edits landed without an observed validation command")
            )
        );
        assert!(
            receipt
                .uncertainty_reasons
                .iter()
                .all(|reason| !reason.contains("checkpoint coverage for this task is limited"))
        );
    }

    #[test]
    fn task_final_receipt_describes_stopped_turns_without_calling_them_failures() {
        let baseline = TaskWorkspaceBaseline {
            task_start_turn_index: 11,
            repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
            preexisting_dirty_files: Vec::new(),
        };
        let transcript = vec![tool_result_event(
            11,
            "apply_patch",
            ToolRiskClass::Write,
            &["src/main.rs"],
            &["src/main.rs"],
        )];
        let workspace = build_task_workspace_summary(
            transcript.as_slice(),
            &baseline,
            TaskWorkspaceSummaryStatus::PartialChangesBeforeFailure,
            &[],
        );
        let receipt = build_task_final_receipt(
            transcript.as_slice(),
            TaskReceiptDisposition::Stopped,
            &workspace,
        );

        assert!(
            receipt
                .summary_text
                .contains("Task was stopped after changes landed")
        );
        assert!(
            receipt
                .summary_text
                .contains("No validation command completed before the task was stopped.")
        );
        assert!(
            receipt
                .uncertainty_reasons
                .iter()
                .any(|reason| reason.contains("stopped before completion"))
        );
        assert!(
            receipt
                .uncertainty_reasons
                .iter()
                .all(|reason| !reason.contains("checkpoint coverage for this task is limited"))
        );
        assert!(!receipt.summary_text.contains("failed before"));
    }

    #[test]
    fn task_final_receipt_calls_out_backend_failure_before_repo_changes_land() {
        let baseline = TaskWorkspaceBaseline {
            task_start_turn_index: 14,
            repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
            preexisting_dirty_files: Vec::new(),
        };
        let workspace = build_task_workspace_summary(
            &[],
            &baseline,
            TaskWorkspaceSummaryStatus::PartialChangesBeforeFailure,
            &[],
        );
        let receipt = build_task_final_receipt(&[], TaskReceiptDisposition::Failed, &workspace);

        assert_eq!(
            receipt.workspace.status,
            TaskWorkspaceSummaryStatus::NoRepoChanges
        );
        assert!(
            receipt
                .summary_text
                .contains("Task failed before repo changes landed.")
        );
        assert!(
            receipt
                .summary_text
                .contains("No validation command was observed")
        );
    }

    #[test]
    fn task_final_receipt_calls_out_failure_after_edits_landed() {
        let baseline = TaskWorkspaceBaseline {
            task_start_turn_index: 16,
            repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
            preexisting_dirty_files: Vec::new(),
        };
        let transcript = vec![tool_result_event(
            16,
            "apply_patch",
            ToolRiskClass::Write,
            &["src/main.rs"],
            &["src/main.rs"],
        )];
        let workspace = build_task_workspace_summary(
            transcript.as_slice(),
            &baseline,
            TaskWorkspaceSummaryStatus::PartialChangesBeforeFailure,
            &["src/main.rs".to_string()],
        );
        let receipt = build_task_final_receipt(
            transcript.as_slice(),
            TaskReceiptDisposition::Failed,
            &workspace,
        );

        assert_eq!(
            receipt.workspace.status,
            TaskWorkspaceSummaryStatus::PartialChangesBeforeFailure
        );
        assert!(
            receipt
                .summary_text
                .contains("Partial edits landed before failure")
        );
        assert!(
            receipt.uncertainty_reasons.iter().any(
                |reason| reason.contains("Edits landed without an observed validation command")
            )
        );
    }

    #[test]
    fn task_final_receipt_calls_out_timed_out_and_truncated_validation() {
        let baseline = TaskWorkspaceBaseline {
            task_start_turn_index: 20,
            repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
            preexisting_dirty_files: Vec::new(),
        };
        let transcript = vec![
            tool_result_event(
                20,
                "apply_patch",
                ToolRiskClass::Write,
                &["src/lib.rs"],
                &["src/lib.rs"],
            ),
            shell_validation_event(21, "cargo test -p probe-tui", Some(124), true, true),
        ];
        let workspace = build_task_workspace_summary(
            transcript.as_slice(),
            &baseline,
            TaskWorkspaceSummaryStatus::Changed,
            &["src/lib.rs".to_string()],
        );
        let receipt = build_task_final_receipt(
            transcript.as_slice(),
            TaskReceiptDisposition::Succeeded,
            &workspace,
        );

        assert_eq!(
            receipt.verification_status,
            TaskVerificationStatus::TimedOut
        );
        assert!(receipt.summary_text.contains("Validation timed out"));
        assert!(receipt.summary_text.contains("output truncated"));
        assert!(
            receipt
                .uncertainty_reasons
                .iter()
                .any(|reason| { reason.contains("timed out before Probe observed a full result") })
        );
        assert!(
            receipt
                .uncertainty_reasons
                .iter()
                .any(|reason| reason.contains("Validation output was truncated"))
        );
    }

    #[test]
    fn task_workspace_baseline_omits_git_state_for_non_git_directories() {
        let temp = tempdir().expect("tempdir");
        let cwd = temp.path().join("workspace");
        fs::create_dir_all(&cwd).expect("create workspace");
        fs::write(cwd.join("notes.txt"), "hello\n").expect("write file");

        assert!(resolve_git_repo_root(&cwd).is_none());
        assert!(dirty_worktree_paths(&cwd, None).is_empty());
    }
}
