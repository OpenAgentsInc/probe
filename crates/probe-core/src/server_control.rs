use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use probe_protocol::backend::{
    BackendControlPlaneKind, BackendKind, BackendProfile, PsionicMeshAttachInfo,
    PsionicMeshTargetableModel,
};
use probe_provider_apple_fm::{AppleFmProviderClient, AppleFmProviderConfig};
use psionic_apple_fm::AppleFmSystemLanguageModelAvailability;
use serde::{Deserialize, Serialize};

use crate::backend_profiles::{
    OPENAI_CODEX_SUBSCRIPTION_MODEL, PSIONIC_APPLE_FM_MODEL, PSIONIC_INFERENCE_MESH_DEFAULT_MODEL,
    PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL, persisted_reasoning_level_for_backend,
    persisted_service_tier_for_backend, resolved_reasoning_level_for_backend,
    resolved_service_tier_for_backend,
};

const DEFAULT_SERVER_CONFIG_PATH: &str = "server/psionic-local.json";
const DEFAULT_OPENAI_SERVER_CONFIG_PATH: &str = "server/psionic-openai-chat-completions.json";
const DEFAULT_CODEX_SERVER_CONFIG_PATH: &str = "server/openai-codex-subscription.json";
const DEFAULT_APPLE_FM_SERVER_CONFIG_PATH: &str = "server/psionic-apple-fm.json";
const DEFAULT_SERVER_HOST: &str = "127.0.0.1";
const DEFAULT_CODEX_SERVER_HOST: &str = "chatgpt.com";
const DEFAULT_OPENAI_SERVER_PORT: u16 = 8080;
const DEFAULT_CODEX_SERVER_PORT: u16 = 443;
const DEFAULT_APPLE_FM_SERVER_PORT: u16 = 11435;
const DEFAULT_SERVER_BACKEND: &str = "cpu";
const OPENAI_COMPAT_LOCAL_WORKER_ID: &str = "openai_compat";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PsionicServerMode {
    Attach,
    Launch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PsionicServerConfig {
    pub mode: PsionicServerMode,
    #[serde(default = "default_server_api_kind")]
    pub api_kind: BackendKind,
    pub host: String,
    pub port: u16,
    pub backend: String,
    pub binary_path: Option<PathBuf>,
    pub model_path: Option<PathBuf>,
    pub model_id: Option<String>,
    pub reasoning_budget: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_plane: Option<BackendControlPlaneKind>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerTargetKind {
    ManagedLaunch,
    LoopbackAttach,
    TailnetAttach,
    RemoteAttach,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerOperatorSummary {
    pub backend_kind: BackendKind,
    pub mode: PsionicServerMode,
    pub target_kind: ServerTargetKind,
    pub host: String,
    pub port: u16,
    pub base_url: String,
    pub model_id: Option<String>,
    pub reasoning_level: Option<String>,
    pub service_tier: Option<String>,
    pub control_plane: Option<BackendControlPlaneKind>,
    pub psionic_mesh: Option<PsionicMeshAttachInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct ServerConfigOverrides {
    pub mode: Option<PsionicServerMode>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub backend: Option<String>,
    pub binary_path: Option<PathBuf>,
    pub model_path: Option<PathBuf>,
    pub model_id: Option<String>,
    pub reasoning_budget: Option<u8>,
}

#[derive(Debug)]
pub enum ServerControlError {
    Io(std::io::Error),
    Json(serde_json::Error),
    InvalidBackend(String),
    UnsupportedManagedLaunch {
        backend: BackendKind,
    },
    UnsupportedManagedLaunchControlPlane {
        control_plane: BackendControlPlaneKind,
    },
    MissingBinaryPath,
    MissingModelPath,
    SpawnFailed(String),
    ReadinessTimeout {
        base_url: String,
        timeout_secs: u64,
    },
    BackendUnavailable {
        base_url: String,
        detail: String,
    },
    Http(String),
}

impl Display for ServerControlError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::InvalidBackend(value) => {
                write!(f, "invalid backend `{value}`; expected `cpu` or `cuda`")
            }
            Self::UnsupportedManagedLaunch { backend } => write!(
                f,
                "managed launch is not supported for backend {:?}; attach to an already-running bridge instead",
                backend
            ),
            Self::UnsupportedManagedLaunchControlPlane { control_plane } => write!(
                f,
                "managed launch is not supported for control plane {:?}; attach to an existing Psionic mesh instead",
                control_plane
            ),
            Self::MissingBinaryPath => write!(
                f,
                "launch mode requires `binary_path`; set it in the server config or via CLI"
            ),
            Self::MissingModelPath => write!(
                f,
                "launch mode requires `model_path`; set it in the server config or via CLI"
            ),
            Self::SpawnFailed(message) => write!(f, "{message}"),
            Self::ReadinessTimeout {
                base_url,
                timeout_secs,
            } => write!(
                f,
                "server at {base_url} did not become ready within {}s",
                timeout_secs
            ),
            Self::BackendUnavailable { base_url, detail } => {
                write!(f, "backend at {base_url} is not ready: {detail}")
            }
            Self::Http(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ServerControlError {}

impl From<std::io::Error> for ServerControlError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ServerControlError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Debug)]
pub struct ServerProcessGuard {
    config: PsionicServerConfig,
    child: Option<Child>,
    operator_summary: ServerOperatorSummary,
}

#[derive(Clone, Debug, Deserialize)]
struct PsionicMeshManagementStatusResponse {
    topology_digest: String,
    default_model: String,
    nodes: Vec<PsionicMeshManagementNodeStatus>,
}

#[derive(Clone, Debug, Deserialize)]
struct PsionicMeshManagementNodeStatus {
    worker_id: String,
    served_mesh_role: PsionicMeshManagementRoleState,
    execution_mode_label: String,
    execution_engine_label: String,
    models: Vec<PsionicMeshManagementModelStatus>,
}

#[derive(Clone, Debug, Deserialize)]
struct PsionicMeshManagementRoleState {
    role: String,
    posture: String,
    #[serde(default)]
    reasons: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct PsionicMeshManagementModelStatus {
    canonical_name: String,
    family: String,
    supported_endpoints: Vec<String>,
    warm_state: String,
    structured_outputs: bool,
    tool_calling: bool,
    response_state: bool,
}

enum PsionicMeshDiscoveryError {
    Retryable(String),
    Fatal(ServerControlError),
}

impl Drop for ServerProcessGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Default for PsionicServerConfig {
    fn default() -> Self {
        Self {
            mode: PsionicServerMode::Attach,
            api_kind: BackendKind::OpenAiChatCompletions,
            host: String::from(DEFAULT_SERVER_HOST),
            port: DEFAULT_OPENAI_SERVER_PORT,
            backend: String::from(DEFAULT_SERVER_BACKEND),
            binary_path: None,
            model_path: None,
            model_id: Some(String::from(PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL)),
            reasoning_budget: None,
            reasoning_level: None,
            service_tier: None,
            control_plane: None,
        }
    }
}

impl PsionicServerConfig {
    #[must_use]
    pub fn from_backend_profile(profile: &BackendProfile) -> Self {
        let (host, port) = parse_profile_host_port(profile);
        let mut config = Self {
            mode: PsionicServerMode::Attach,
            api_kind: profile.kind,
            host,
            port,
            backend: String::from(DEFAULT_SERVER_BACKEND),
            binary_path: None,
            model_path: None,
            model_id: Some(profile.model.clone()),
            reasoning_budget: None,
            reasoning_level: profile.reasoning_level.clone(),
            service_tier: profile.service_tier.clone(),
            control_plane: profile.control_plane,
        };
        config.set_api_kind(profile.kind);
        config.model_id = Some(profile.model.clone());
        config.reasoning_level =
            persisted_reasoning_level_for_backend(profile.kind, profile.reasoning_level.as_deref());
        config.service_tier =
            persisted_service_tier_for_backend(profile.kind, profile.service_tier.as_deref());
        config
    }

    #[must_use]
    pub fn config_path(probe_home: &Path) -> PathBuf {
        probe_home.join(DEFAULT_SERVER_CONFIG_PATH)
    }

    #[must_use]
    pub fn backend_config_path(probe_home: &Path, api_kind: BackendKind) -> PathBuf {
        probe_home.join(match api_kind {
            BackendKind::OpenAiChatCompletions => DEFAULT_OPENAI_SERVER_CONFIG_PATH,
            BackendKind::OpenAiCodexSubscription => DEFAULT_CODEX_SERVER_CONFIG_PATH,
            BackendKind::AppleFmBridge => DEFAULT_APPLE_FM_SERVER_CONFIG_PATH,
        })
    }

    #[must_use]
    pub fn base_url(&self) -> String {
        match self.api_kind {
            BackendKind::OpenAiChatCompletions => format!("http://{}:{}/v1", self.host, self.port),
            BackendKind::OpenAiCodexSubscription => {
                if self.port == DEFAULT_CODEX_SERVER_PORT {
                    format!("https://{}/backend-api/codex", self.host)
                } else {
                    format!("https://{}:{}/backend-api/codex", self.host, self.port)
                }
            }
            BackendKind::AppleFmBridge => format!("http://{}:{}", self.host, self.port),
        }
    }

    #[must_use]
    pub fn resolved_model_id(&self) -> Option<String> {
        self.model_id
            .clone()
            .or_else(|| {
                self.model_path.as_ref().and_then(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().to_string())
                })
            })
            .or_else(|| default_model_id_for(self.api_kind, self.control_plane))
    }

    pub fn set_api_kind(&mut self, api_kind: BackendKind) {
        if self.api_kind == api_kind {
            self.ensure_kind_defaults();
            return;
        }

        let previous_default_port = default_port_for(self.api_kind);
        let previous_default_host = default_host_for(self.api_kind);
        let previous_default_model = default_model_id_for(self.api_kind, self.control_plane);
        if self.host == previous_default_host {
            self.host = String::from(default_host_for(api_kind));
        }
        if self.port == previous_default_port {
            self.port = default_port_for(api_kind);
        }
        if self.model_id.is_none() || self.model_id == previous_default_model {
            self.model_id = default_model_id_for(api_kind, self.control_plane);
        }
        self.api_kind = api_kind;
        self.ensure_kind_defaults();
    }

    fn ensure_kind_defaults(&mut self) {
        if self.api_kind != BackendKind::OpenAiChatCompletions {
            self.control_plane = None;
        }
        if self.model_id.is_none()
            || self.model_id.as_deref().is_some_and(|model_id| {
                is_legacy_default_model(self.api_kind, self.control_plane, model_id)
            })
        {
            self.model_id = default_model_id_for(self.api_kind, self.control_plane);
        }
        self.reasoning_level =
            persisted_reasoning_level_for_backend(self.api_kind, self.reasoning_level.as_deref());
        self.service_tier =
            persisted_service_tier_for_backend(self.api_kind, self.service_tier.as_deref());
    }

    #[must_use]
    pub fn operator_summary(&self) -> ServerOperatorSummary {
        self.operator_summary_with(None, self.resolved_model_id())
    }

    fn operator_summary_with(
        &self,
        psionic_mesh: Option<PsionicMeshAttachInfo>,
        model_id: Option<String>,
    ) -> ServerOperatorSummary {
        ServerOperatorSummary {
            backend_kind: self.api_kind,
            mode: self.mode.clone(),
            target_kind: classify_target_kind(self.mode.clone(), self.host.as_str()),
            host: self.host.clone(),
            port: self.port,
            base_url: self.base_url(),
            model_id,
            reasoning_level: resolved_reasoning_level_for_backend(
                self.api_kind,
                self.reasoning_level.as_deref(),
            )
            .map(str::to_string),
            service_tier: resolved_service_tier_for_backend(
                self.api_kind,
                self.service_tier.as_deref(),
            )
            .map(str::to_string),
            control_plane: self.control_plane,
            psionic_mesh,
        }
    }

    pub fn load_or_create(path: &Path) -> Result<Self, ServerControlError> {
        if path.exists() {
            let file = fs::File::open(path)?;
            return Ok(serde_json::from_reader(file)?);
        }

        let config = Self::default();
        config.save(path)?;
        Ok(config)
    }

    pub fn load_or_default_for_backend(
        probe_home: &Path,
        api_kind: BackendKind,
    ) -> Result<Self, ServerControlError> {
        let path = Self::backend_config_path(probe_home, api_kind);
        let mut config = if path.exists() {
            let file = fs::File::open(path)?;
            serde_json::from_reader(file)?
        } else {
            Self::default()
        };
        config.set_api_kind(api_kind);
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<(), ServerControlError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = fs::File::create(path)?;
        serde_json::to_writer_pretty(file, self)?;
        Ok(())
    }

    pub fn apply_overrides(
        &mut self,
        overrides: &ServerConfigOverrides,
    ) -> Result<(), ServerControlError> {
        if let Some(mode) = &overrides.mode {
            self.mode = mode.clone();
        }
        if let Some(host) = &overrides.host {
            self.host = host.clone();
        }
        if let Some(port) = overrides.port {
            self.port = port;
        }
        if let Some(backend) = &overrides.backend {
            if !matches!(backend.as_str(), "cpu" | "cuda") {
                return Err(ServerControlError::InvalidBackend(backend.clone()));
            }
            self.backend = backend.clone();
        }
        if let Some(binary_path) = &overrides.binary_path {
            self.binary_path = Some(binary_path.clone());
        }
        if let Some(model_path) = &overrides.model_path {
            self.model_path = Some(model_path.clone());
        }
        if let Some(model_id) = &overrides.model_id {
            self.model_id = Some(model_id.clone());
        }
        if let Some(reasoning_budget) = overrides.reasoning_budget {
            self.reasoning_budget = Some(reasoning_budget);
        }
        self.ensure_kind_defaults();
        Ok(())
    }

    pub fn prepare(
        &self,
        startup_timeout: Duration,
    ) -> Result<ServerProcessGuard, ServerControlError> {
        match self.mode {
            PsionicServerMode::Attach => {
                let operator_summary = wait_for_ready(self, startup_timeout)?;
                Ok(ServerProcessGuard {
                    config: self.clone(),
                    child: None,
                    operator_summary,
                })
            }
            PsionicServerMode::Launch => {
                if let Some(control_plane) = self.control_plane {
                    return Err(ServerControlError::UnsupportedManagedLaunchControlPlane {
                        control_plane,
                    });
                }
                if self.api_kind != BackendKind::OpenAiChatCompletions {
                    return Err(ServerControlError::UnsupportedManagedLaunch {
                        backend: self.api_kind,
                    });
                }
                let binary_path = self
                    .binary_path
                    .clone()
                    .ok_or(ServerControlError::MissingBinaryPath)?;
                let model_path = self
                    .model_path
                    .clone()
                    .ok_or(ServerControlError::MissingModelPath)?;
                let mut command = Command::new(binary_path);
                command
                    .arg("-m")
                    .arg(model_path)
                    .arg("--backend")
                    .arg(self.backend.as_str())
                    .arg("--host")
                    .arg(self.host.as_str())
                    .arg("--port")
                    .arg(self.port.to_string())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                if let Some(reasoning_budget) = self.reasoning_budget {
                    command
                        .arg("--reasoning-budget")
                        .arg(reasoning_budget.to_string());
                }

                let mut child = command.spawn().map_err(|error| {
                    ServerControlError::SpawnFailed(format!("failed to launch server: {error}"))
                })?;
                let operator_summary = match wait_for_ready(self, startup_timeout) {
                    Ok(summary) => summary,
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(error);
                    }
                };
                Ok(ServerProcessGuard {
                    config: self.clone(),
                    child: Some(child),
                    operator_summary,
                })
            }
        }
    }
}

impl ServerProcessGuard {
    #[must_use]
    pub fn base_url(&self) -> String {
        self.operator_summary.base_url.clone()
    }

    #[must_use]
    pub fn model_id(&self) -> Option<String> {
        self.operator_summary.model_id.clone()
    }

    #[must_use]
    pub fn api_kind(&self) -> BackendKind {
        self.config.api_kind
    }

    #[must_use]
    pub fn mode(&self) -> &PsionicServerMode {
        &self.config.mode
    }

    #[must_use]
    pub fn operator_summary(&self) -> ServerOperatorSummary {
        self.operator_summary.clone()
    }
}

impl ServerOperatorSummary {
    #[must_use]
    pub const fn is_remote_target(&self) -> bool {
        matches!(
            self.target_kind,
            ServerTargetKind::TailnetAttach | ServerTargetKind::RemoteAttach
        )
    }

    #[must_use]
    pub const fn attach_mode_label(&self) -> &'static str {
        match self.mode {
            PsionicServerMode::Attach => "attach",
            PsionicServerMode::Launch => "launch",
        }
    }

    #[must_use]
    pub const fn target_kind_label(&self) -> &'static str {
        match self.target_kind {
            ServerTargetKind::ManagedLaunch => "managed_launch",
            ServerTargetKind::LoopbackAttach => "loopback_or_ssh_forward",
            ServerTargetKind::TailnetAttach => "tailnet_attach",
            ServerTargetKind::RemoteAttach => "remote_attach",
        }
    }

    #[must_use]
    pub fn endpoint_label(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

fn classify_target_kind(mode: PsionicServerMode, host: &str) -> ServerTargetKind {
    match mode {
        PsionicServerMode::Launch => ServerTargetKind::ManagedLaunch,
        PsionicServerMode::Attach if is_loopback_host(host) => ServerTargetKind::LoopbackAttach,
        PsionicServerMode::Attach if is_tailnet_host(host) => ServerTargetKind::TailnetAttach,
        PsionicServerMode::Attach => ServerTargetKind::RemoteAttach,
    }
}

fn parse_profile_host_port(profile: &BackendProfile) -> (String, u16) {
    let without_scheme = profile
        .base_url
        .strip_prefix("http://")
        .or_else(|| profile.base_url.strip_prefix("https://"))
        .unwrap_or(profile.base_url.as_str());
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    let (host, port) = authority
        .rsplit_once(':')
        .map(|(host, port)| {
            (
                host.to_string(),
                port.parse::<u16>()
                    .unwrap_or_else(|_| default_port_for(profile.kind)),
            )
        })
        .unwrap_or_else(|| (authority.to_string(), default_port_for(profile.kind)));
    (host, port)
}

fn is_loopback_host(host: &str) -> bool {
    if matches!(host, "localhost" | "ip6-localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .map(|address| address.is_loopback())
        .unwrap_or(false)
}

fn is_tailnet_host(host: &str) -> bool {
    let Ok(IpAddr::V4(address)) = host.parse::<IpAddr>() else {
        return false;
    };
    let octets = address.octets();
    octets[0] == 100 && (octets[1] & 0b1100_0000) == 0b0100_0000
}

fn wait_for_ready(
    config: &PsionicServerConfig,
    timeout: Duration,
) -> Result<ServerOperatorSummary, ServerControlError> {
    if matches!(
        config.control_plane,
        Some(BackendControlPlaneKind::PsionicInferenceMesh)
    ) {
        return wait_for_psionic_mesh_ready(config, timeout);
    }

    let deadline = Instant::now() + timeout;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .map_err(|error| ServerControlError::Http(error.to_string()))?;
    let readiness_url = match config.api_kind {
        BackendKind::OpenAiChatCompletions => {
            format!("{}/models", config.base_url().trim_end_matches('/'))
        }
        BackendKind::OpenAiCodexSubscription => return Ok(config.operator_summary()),
        BackendKind::AppleFmBridge => format!("{}/health", config.base_url().trim_end_matches('/')),
    };

    loop {
        match config.api_kind {
            BackendKind::OpenAiChatCompletions => match client.get(readiness_url.as_str()).send() {
                Ok(response) if response.status().is_success() => {
                    return Ok(config.operator_summary());
                }
                Ok(_) | Err(_) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(250));
                }
                Ok(_) | Err(_) => {
                    return Err(ServerControlError::ReadinessTimeout {
                        base_url: config.base_url(),
                        timeout_secs: timeout.as_secs(),
                    });
                }
            },
            BackendKind::OpenAiCodexSubscription => {
                unreachable!("codex readiness returns early before entering the retry loop")
            }
            BackendKind::AppleFmBridge => {
                let apple_client = AppleFmProviderClient::new(AppleFmProviderConfig {
                    base_url: config.base_url(),
                    model: config
                        .resolved_model_id()
                        .unwrap_or_else(|| String::from(PSIONIC_APPLE_FM_MODEL)),
                    timeout: Duration::from_secs(1),
                })
                .map_err(|error| ServerControlError::Http(error.to_string()))?;
                match apple_client.system_model_availability() {
                    Ok(availability) if availability.is_ready() => {
                        return Ok(config.operator_summary());
                    }
                    Ok(availability) => {
                        return Err(ServerControlError::BackendUnavailable {
                            base_url: config.base_url(),
                            detail: format_apple_fm_unavailability(&availability),
                        });
                    }
                    Err(_) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(250));
                    }
                    Err(_) => {
                        return Err(ServerControlError::ReadinessTimeout {
                            base_url: config.base_url(),
                            timeout_secs: timeout.as_secs(),
                        });
                    }
                }
            }
        }
    }
}

fn wait_for_psionic_mesh_ready(
    config: &PsionicServerConfig,
    timeout: Duration,
) -> Result<ServerOperatorSummary, ServerControlError> {
    let deadline = Instant::now() + timeout;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .map_err(|error| ServerControlError::Http(error.to_string()))?;
    let management_base_url = psionic_mesh_management_base_url(config);
    let mut last_retryable_detail = None;
    loop {
        match probe_psionic_mesh_once(config, &client) {
            Ok(summary) => return Ok(summary),
            Err(PsionicMeshDiscoveryError::Fatal(error)) => return Err(error),
            Err(PsionicMeshDiscoveryError::Retryable(detail)) if Instant::now() < deadline => {
                last_retryable_detail = Some(detail);
                thread::sleep(Duration::from_millis(250));
            }
            Err(PsionicMeshDiscoveryError::Retryable(detail)) => {
                return Err(ServerControlError::BackendUnavailable {
                    base_url: management_base_url,
                    detail: last_retryable_detail.unwrap_or(detail),
                });
            }
        }
    }
}

fn probe_psionic_mesh_once(
    config: &PsionicServerConfig,
    client: &reqwest::blocking::Client,
) -> Result<ServerOperatorSummary, PsionicMeshDiscoveryError> {
    let management_base_url = psionic_mesh_management_base_url(config);
    let management_url = format!("{management_base_url}/psionic/management/status");
    let response = client
        .get(management_url.as_str())
        .send()
        .map_err(|error| PsionicMeshDiscoveryError::Retryable(error.to_string()))?;
    let response = response
        .error_for_status()
        .map_err(|error| PsionicMeshDiscoveryError::Retryable(error.to_string()))?;
    let status = response
        .json::<PsionicMeshManagementStatusResponse>()
        .map_err(|error| PsionicMeshDiscoveryError::Retryable(error.to_string()))?;
    let attach_info = psionic_mesh_attach_info(&management_base_url, &status);
    if attach_info.targetable_models.is_empty() {
        return Err(PsionicMeshDiscoveryError::Retryable(format!(
            "mesh management at {} does not advertise any warm targetable models yet{}",
            management_base_url,
            attach_info
                .served_mesh_role
                .as_deref()
                .map(|role| format!(
                    " (role={} posture={} reasons={})",
                    role,
                    attach_info
                        .served_mesh_posture
                        .as_deref()
                        .unwrap_or("unknown"),
                    if attach_info.served_mesh_reasons.is_empty() {
                        String::from("none")
                    } else {
                        attach_info.served_mesh_reasons.join(",")
                    }
                ))
                .unwrap_or_default()
        )));
    }
    let selected_model = resolve_psionic_mesh_model(config, &attach_info)
        .map_err(PsionicMeshDiscoveryError::Fatal)?;
    Ok(config.operator_summary_with(Some(attach_info), Some(selected_model)))
}

fn psionic_mesh_attach_info(
    management_base_url: &str,
    status: &PsionicMeshManagementStatusResponse,
) -> PsionicMeshAttachInfo {
    let local_node = status
        .nodes
        .iter()
        .find(|node| node.worker_id == OPENAI_COMPAT_LOCAL_WORKER_ID);
    PsionicMeshAttachInfo {
        management_base_url: management_base_url.to_string(),
        topology_digest: status.topology_digest.clone(),
        default_model: status.default_model.clone(),
        targetable_models: targetable_psionic_mesh_models(status),
        local_worker_id: local_node.map(|node| node.worker_id.clone()),
        served_mesh_role: local_node.map(|node| node.served_mesh_role.role.clone()),
        served_mesh_posture: local_node.map(|node| node.served_mesh_role.posture.clone()),
        served_mesh_reasons: local_node
            .map(|node| node.served_mesh_role.reasons.clone())
            .unwrap_or_default(),
        execution_mode: local_node.map(|node| node.execution_mode_label.clone()),
        execution_engine: local_node.map(|node| node.execution_engine_label.clone()),
        fallback_posture: local_node.and_then(psionic_mesh_fallback_posture),
    }
}

fn targetable_psionic_mesh_models(
    status: &PsionicMeshManagementStatusResponse,
) -> Vec<PsionicMeshTargetableModel> {
    let mut merged = BTreeMap::<String, PsionicMeshTargetableModel>::new();
    for node in &status.nodes {
        for model in &node.models {
            if model.warm_state != "warm" {
                continue;
            }
            let entry = merged
                .entry(model.canonical_name.clone())
                .or_insert_with(|| PsionicMeshTargetableModel {
                    model: model.canonical_name.clone(),
                    family: model.family.clone(),
                    supported_endpoints: Vec::new(),
                    structured_outputs: model.structured_outputs,
                    tool_calling: model.tool_calling,
                    response_state: model.response_state,
                });
            let mut endpoints = entry
                .supported_endpoints
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            endpoints.extend(model.supported_endpoints.iter().cloned());
            entry.supported_endpoints = endpoints.into_iter().collect();
            entry.structured_outputs |= model.structured_outputs;
            entry.tool_calling |= model.tool_calling;
            entry.response_state |= model.response_state;
        }
    }
    merged.into_values().collect()
}

fn psionic_mesh_fallback_posture(node: &PsionicMeshManagementNodeStatus) -> Option<String> {
    if node.execution_mode_label != "proxy" {
        return None;
    }
    if node.served_mesh_role.role == "thin_client"
        && node
            .served_mesh_role
            .reasons
            .iter()
            .any(|reason| reason == "remote_only")
    {
        return Some(String::from("thin_client_remote_only"));
    }
    if node
        .served_mesh_role
        .reasons
        .iter()
        .any(|reason| reason == "warming")
    {
        return Some(String::from("warming_until_local_ready"));
    }
    None
}

fn resolve_psionic_mesh_model(
    config: &PsionicServerConfig,
    attach_info: &PsionicMeshAttachInfo,
) -> Result<String, ServerControlError> {
    let requested_model = config
        .model_id
        .as_deref()
        .filter(|model| *model != PSIONIC_INFERENCE_MESH_DEFAULT_MODEL)
        .map(str::to_string);
    if let Some(requested_model) = requested_model {
        if attach_info
            .targetable_models
            .iter()
            .any(|model| model.model == requested_model)
        {
            return Ok(requested_model);
        }
        return Err(ServerControlError::BackendUnavailable {
            base_url: attach_info.management_base_url.clone(),
            detail: format!(
                "mesh does not advertise requested model `{}`; targetable_models={}",
                requested_model,
                attach_info
                    .targetable_models
                    .iter()
                    .map(|model| model.model.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        });
    }

    if attach_info
        .targetable_models
        .iter()
        .any(|model| model.model == attach_info.default_model)
    {
        return Ok(attach_info.default_model.clone());
    }

    attach_info
        .targetable_models
        .first()
        .map(|model| model.model.clone())
        .ok_or_else(|| ServerControlError::BackendUnavailable {
            base_url: attach_info.management_base_url.clone(),
            detail: String::from("mesh does not advertise any warm targetable models"),
        })
}

fn psionic_mesh_management_base_url(config: &PsionicServerConfig) -> String {
    config.base_url().trim_end_matches("/v1").to_string()
}

const fn default_server_api_kind() -> BackendKind {
    BackendKind::OpenAiChatCompletions
}

const fn default_host_for(api_kind: BackendKind) -> &'static str {
    match api_kind {
        BackendKind::OpenAiChatCompletions | BackendKind::AppleFmBridge => DEFAULT_SERVER_HOST,
        BackendKind::OpenAiCodexSubscription => DEFAULT_CODEX_SERVER_HOST,
    }
}

const fn default_port_for(api_kind: BackendKind) -> u16 {
    match api_kind {
        BackendKind::OpenAiChatCompletions => DEFAULT_OPENAI_SERVER_PORT,
        BackendKind::OpenAiCodexSubscription => DEFAULT_CODEX_SERVER_PORT,
        BackendKind::AppleFmBridge => DEFAULT_APPLE_FM_SERVER_PORT,
    }
}

fn default_model_id_for(
    api_kind: BackendKind,
    control_plane: Option<BackendControlPlaneKind>,
) -> Option<String> {
    match (api_kind, control_plane) {
        (
            BackendKind::OpenAiChatCompletions,
            Some(BackendControlPlaneKind::PsionicInferenceMesh),
        ) => Some(String::from(PSIONIC_INFERENCE_MESH_DEFAULT_MODEL)),
        (BackendKind::OpenAiChatCompletions, None) => {
            Some(String::from(PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL))
        }
        (BackendKind::OpenAiCodexSubscription, _) => {
            Some(String::from(OPENAI_CODEX_SUBSCRIPTION_MODEL))
        }
        (BackendKind::AppleFmBridge, _) => Some(String::from(PSIONIC_APPLE_FM_MODEL)),
    }
}

fn is_legacy_default_model(
    api_kind: BackendKind,
    control_plane: Option<BackendControlPlaneKind>,
    model_id: &str,
) -> bool {
    match (api_kind, control_plane) {
        (BackendKind::OpenAiCodexSubscription, _) => model_id == "gpt-5.3-codex",
        (
            BackendKind::OpenAiChatCompletions,
            Some(BackendControlPlaneKind::PsionicInferenceMesh),
        ) => model_id == PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL,
        (BackendKind::OpenAiChatCompletions, _) | (BackendKind::AppleFmBridge, _) => false,
    }
}

fn format_apple_fm_unavailability(availability: &AppleFmSystemLanguageModelAvailability) -> String {
    let mut fields = vec![format!("model={}", availability.model.id)];
    if let Some(reason) = availability.unavailable_reason {
        fields.push(format!("reason={}", reason.label()));
    }
    if let Some(message) = availability.availability_message.as_deref() {
        fields.push(format!("message={message}"));
    }
    if let Some(platform) = availability.platform.as_deref() {
        fields.push(format!("platform={platform}"));
    }
    fields.join(" ")
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;

    use probe_protocol::backend::BackendKind;
    use probe_test_support::{FakeAppleFmServer, FakeHttpResponse, FakeOpenAiServer};
    use serde_json::json;

    use super::{
        DEFAULT_APPLE_FM_SERVER_PORT, DEFAULT_CODEX_SERVER_PORT, PSIONIC_APPLE_FM_MODEL,
        PSIONIC_INFERENCE_MESH_DEFAULT_MODEL, PsionicServerConfig, PsionicServerMode,
        ServerConfigOverrides, ServerProcessGuard, ServerTargetKind,
    };

    #[test]
    fn load_or_create_writes_default_attach_config() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("server.json");
        let config = PsionicServerConfig::load_or_create(path.as_path()).expect("load config");
        assert!(matches!(config.mode, PsionicServerMode::Attach));
        assert!(path.exists());
    }

    #[test]
    fn config_apply_overrides_updates_launch_fields() {
        let mut config = PsionicServerConfig::default();
        config
            .apply_overrides(&ServerConfigOverrides {
                mode: Some(PsionicServerMode::Launch),
                host: Some(String::from("0.0.0.0")),
                port: Some(9090),
                backend: Some(String::from("cuda")),
                model_path: Some(PathBuf::from("/tmp/model.gguf")),
                binary_path: Some(PathBuf::from("/tmp/psionic-openai-server")),
                model_id: Some(String::from("custom.gguf")),
                reasoning_budget: Some(2),
            })
            .expect("apply overrides");

        assert!(matches!(config.mode, PsionicServerMode::Launch));
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 9090);
        assert_eq!(config.backend, "cuda");
        assert_eq!(config.resolved_model_id().as_deref(), Some("custom.gguf"));
    }

    #[test]
    fn operator_summary_classifies_loopback_tailnet_and_launch_targets() {
        let loopback = PsionicServerConfig::default().operator_summary();
        assert_eq!(loopback.target_kind, ServerTargetKind::LoopbackAttach);
        assert_eq!(loopback.target_kind_label(), "loopback_or_ssh_forward");

        let tailnet = PsionicServerConfig {
            host: String::from("100.88.7.9"),
            ..PsionicServerConfig::default()
        }
        .operator_summary();
        assert_eq!(tailnet.target_kind, ServerTargetKind::TailnetAttach);
        assert!(tailnet.is_remote_target());

        let launched = PsionicServerConfig {
            mode: PsionicServerMode::Launch,
            ..PsionicServerConfig::default()
        }
        .operator_summary();
        assert_eq!(launched.target_kind, ServerTargetKind::ManagedLaunch);
        assert_eq!(launched.attach_mode_label(), "launch");
    }

    #[test]
    fn backend_config_paths_are_backend_specific() {
        let temp = tempfile::tempdir().expect("temp dir");
        assert_eq!(
            PsionicServerConfig::backend_config_path(
                temp.path(),
                BackendKind::OpenAiChatCompletions
            ),
            temp.path()
                .join("server/psionic-openai-chat-completions.json")
        );
        assert_eq!(
            PsionicServerConfig::backend_config_path(temp.path(), BackendKind::AppleFmBridge),
            temp.path().join("server/psionic-apple-fm.json")
        );
    }

    #[test]
    fn load_or_default_for_backend_uses_saved_backend_snapshot() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut qwen = PsionicServerConfig::default();
        qwen.host = String::from("100.108.56.85");
        qwen.port = 8080;
        qwen.model_id = Some(String::from("remote-qwen.gguf"));
        qwen.save(
            PsionicServerConfig::backend_config_path(
                temp.path(),
                BackendKind::OpenAiChatCompletions,
            )
            .as_path(),
        )
        .expect("save qwen snapshot");

        let loaded = PsionicServerConfig::load_or_default_for_backend(
            temp.path(),
            BackendKind::OpenAiChatCompletions,
        )
        .expect("load saved qwen snapshot");
        assert_eq!(loaded.host, "100.108.56.85");
        assert_eq!(loaded.port, 8080);
        assert_eq!(
            loaded.resolved_model_id().as_deref(),
            Some("remote-qwen.gguf")
        );

        let apple = PsionicServerConfig::load_or_default_for_backend(
            temp.path(),
            BackendKind::AppleFmBridge,
        )
        .expect("default apple fm snapshot");
        assert_eq!(apple.api_kind, BackendKind::AppleFmBridge);
        assert_eq!(apple.port, DEFAULT_APPLE_FM_SERVER_PORT);
        assert_eq!(
            apple.resolved_model_id().as_deref(),
            Some(PSIONIC_APPLE_FM_MODEL)
        );
    }

    #[test]
    fn codex_backend_defaults_to_chatgpt_remote_attach_target() {
        let temp = tempfile::tempdir().expect("temp dir");
        let codex = PsionicServerConfig::load_or_default_for_backend(
            temp.path(),
            BackendKind::OpenAiCodexSubscription,
        )
        .expect("default codex snapshot");
        assert_eq!(codex.api_kind, BackendKind::OpenAiCodexSubscription);
        assert_eq!(codex.host, "chatgpt.com");
        assert_eq!(codex.port, DEFAULT_CODEX_SERVER_PORT);
        assert_eq!(codex.base_url(), "https://chatgpt.com/backend-api/codex");
        assert_eq!(codex.resolved_model_id().as_deref(), Some("gpt-5.4"));
        assert_eq!(
            codex.operator_summary().target_kind,
            ServerTargetKind::RemoteAttach
        );
    }

    #[test]
    fn codex_backend_migrates_legacy_default_model_id() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = PsionicServerConfig::backend_config_path(
            temp.path(),
            BackendKind::OpenAiCodexSubscription,
        );
        let legacy = PsionicServerConfig {
            mode: PsionicServerMode::Attach,
            api_kind: BackendKind::OpenAiCodexSubscription,
            host: String::from("chatgpt.com"),
            port: DEFAULT_CODEX_SERVER_PORT,
            backend: String::from("cpu"),
            binary_path: None,
            model_path: None,
            model_id: Some(String::from("gpt-5.3-codex")),
            reasoning_budget: None,
            reasoning_level: Some(String::from("invalid")),
            service_tier: Some(String::from("invalid")),
            control_plane: None,
        };
        legacy.save(path.as_path()).expect("save legacy config");

        let loaded = PsionicServerConfig::load_or_default_for_backend(
            temp.path(),
            BackendKind::OpenAiCodexSubscription,
        )
        .expect("load migrated codex snapshot");

        assert_eq!(loaded.resolved_model_id().as_deref(), Some("gpt-5.4"));
        assert_eq!(loaded.reasoning_level, None);
        assert_eq!(loaded.service_tier, None);
        assert_eq!(
            loaded.operator_summary().reasoning_level.as_deref(),
            Some("medium")
        );
    }

    #[test]
    fn codex_backend_retains_reasoning_level_override() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = PsionicServerConfig::backend_config_path(
            temp.path(),
            BackendKind::OpenAiCodexSubscription,
        );
        let config = PsionicServerConfig {
            mode: PsionicServerMode::Attach,
            api_kind: BackendKind::OpenAiCodexSubscription,
            host: String::from("chatgpt.com"),
            port: DEFAULT_CODEX_SERVER_PORT,
            backend: String::from("cpu"),
            binary_path: None,
            model_path: None,
            model_id: Some(String::from("gpt-5.4")),
            reasoning_budget: None,
            reasoning_level: Some(String::from("xhigh")),
            service_tier: Some(String::from("fast")),
            control_plane: None,
        };
        config.save(path.as_path()).expect("save codex config");

        let loaded = PsionicServerConfig::load_or_default_for_backend(
            temp.path(),
            BackendKind::OpenAiCodexSubscription,
        )
        .expect("load saved codex config");

        assert_eq!(loaded.reasoning_level.as_deref(), Some("xhigh"));
        assert_eq!(loaded.service_tier.as_deref(), Some("fast"));
        assert_eq!(
            loaded.operator_summary().reasoning_level.as_deref(),
            Some("xhigh")
        );
        assert_eq!(
            loaded.operator_summary().service_tier.as_deref(),
            Some("fast")
        );
    }

    #[test]
    fn attach_mode_waits_for_ready_server() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut response = String::new();
            response.push_str("HTTP/1.1 200 OK\r\n");
            response.push_str("content-type: application/json\r\n");
            response.push_str("content-length: 11\r\n");
            response.push_str("connection: close\r\n\r\n");
            response.push_str("{\"ok\":true}");
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let config = PsionicServerConfig {
            host: String::from("127.0.0.1"),
            port: address.port(),
            ..PsionicServerConfig::default()
        };
        let guard = config
            .prepare(Duration::from_secs(2))
            .expect("attach should succeed");
        assert!(matches!(guard.mode(), PsionicServerMode::Attach));
        handle.join().expect("server thread");
    }

    #[test]
    fn mesh_attach_mode_discovers_targetable_models_and_proxy_posture() {
        let server = FakeOpenAiServer::from_responses(vec![FakeHttpResponse::json_ok(json!({
            "status": "ok",
            "topology_digest": "mesh-topology-v1",
            "default_model": "gemma4:e4b",
            "nodes": [
                {
                    "worker_id": "openai_compat",
                    "served_mesh_role": {
                        "role": "thin_client",
                        "posture": "ready",
                        "reasons": ["remote_only"]
                    },
                    "execution_mode_label": "proxy",
                    "execution_engine_label": "psionic",
                    "models": []
                },
                {
                    "worker_id": "mesh-peer-gamma",
                    "served_mesh_role": {
                        "role": "worker",
                        "posture": "ready",
                        "reasons": []
                    },
                    "execution_mode_label": "native",
                    "execution_engine_label": "psionic",
                    "models": [
                        {
                            "canonical_name": "gemma4:e4b",
                            "family": "gemma4",
                            "supported_endpoints": ["/v1/chat/completions", "/v1/responses"],
                            "warm_state": "warm",
                            "structured_outputs": false,
                            "tool_calling": true,
                            "response_state": true
                        }
                    ]
                }
            ]
        }))]);
        let address = server
            .base_url()
            .strip_prefix("http://")
            .expect("base url should start with http://");
        let address = address.trim_end_matches("/v1");
        let (host, port) = address.rsplit_once(':').expect("host:port pair");
        let port = port.parse::<u16>().expect("port should parse");

        let config = PsionicServerConfig {
            host: host.to_string(),
            port,
            model_id: Some(String::from(PSIONIC_INFERENCE_MESH_DEFAULT_MODEL)),
            control_plane: Some(
                probe_protocol::backend::BackendControlPlaneKind::PsionicInferenceMesh,
            ),
            ..PsionicServerConfig::default()
        };
        let guard = config
            .prepare(Duration::from_secs(2))
            .expect("mesh attach should succeed");
        let summary = guard.operator_summary();
        let mesh = summary.psionic_mesh.expect("mesh metadata");
        assert_eq!(summary.base_url, server.base_url());
        assert_eq!(summary.model_id.as_deref(), Some("gemma4:e4b"));
        assert_eq!(summary.control_plane, config.control_plane);
        assert_eq!(
            mesh.management_base_url,
            server.base_url().trim_end_matches("/v1")
        );
        assert_eq!(mesh.topology_digest, "mesh-topology-v1");
        assert_eq!(mesh.default_model, "gemma4:e4b");
        assert_eq!(mesh.local_worker_id.as_deref(), Some("openai_compat"));
        assert_eq!(mesh.served_mesh_role.as_deref(), Some("thin_client"));
        assert_eq!(mesh.served_mesh_posture.as_deref(), Some("ready"));
        assert_eq!(mesh.served_mesh_reasons, vec![String::from("remote_only")]);
        assert_eq!(mesh.execution_mode.as_deref(), Some("proxy"));
        assert_eq!(mesh.execution_engine.as_deref(), Some("psionic"));
        assert_eq!(
            mesh.fallback_posture.as_deref(),
            Some("thin_client_remote_only")
        );
        assert_eq!(mesh.targetable_models.len(), 1);
        assert_eq!(mesh.targetable_models[0].model, "gemma4:e4b");
        assert_eq!(mesh.targetable_models[0].family, "gemma4");
        assert_eq!(
            mesh.targetable_models[0].supported_endpoints,
            vec![
                String::from("/v1/chat/completions"),
                String::from("/v1/responses")
            ]
        );
        assert!(mesh.targetable_models[0].tool_calling);
        assert!(mesh.targetable_models[0].response_state);
        assert!(!mesh.targetable_models[0].structured_outputs);

        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("GET /psionic/management/status HTTP/1.1"));
    }

    #[test]
    fn mesh_attach_profile_refuses_managed_launch_before_persisting_fake_runtime_semantics() {
        let config = PsionicServerConfig {
            mode: PsionicServerMode::Launch,
            control_plane: Some(
                probe_protocol::backend::BackendControlPlaneKind::PsionicInferenceMesh,
            ),
            ..PsionicServerConfig::default()
        };
        let error = config
            .prepare(Duration::from_millis(10))
            .expect_err("mesh profile should stay attach only");
        assert!(
            error
                .to_string()
                .contains("managed launch is not supported for control plane"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn apple_fm_attach_mode_waits_for_ready_bridge() {
        let server = FakeAppleFmServer::from_responses(vec![FakeHttpResponse::json_status(
            200,
            json!({
                "status": "ok",
                "model_available": true,
                "version": "1.0",
                "platform": "macOS"
            }),
        )]);
        let address = server
            .base_url()
            .strip_prefix("http://")
            .expect("base url should start with http://");
        let (host, port) = address.rsplit_once(':').expect("host:port pair");
        let port = port.parse::<u16>().expect("port should parse");

        let config = PsionicServerConfig {
            api_kind: BackendKind::AppleFmBridge,
            host: host.to_string(),
            port,
            model_id: Some(String::from("apple-foundation-model")),
            reasoning_level: None,
            ..PsionicServerConfig::default()
        };
        let guard = config
            .prepare(Duration::from_secs(2))
            .expect("attach should succeed");
        assert!(matches!(guard.mode(), PsionicServerMode::Attach));
        assert_eq!(guard.base_url(), server.base_url());
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("GET /health HTTP/1.1"));
    }

    #[test]
    fn apple_fm_attach_mode_surfaces_unavailability() {
        let server = FakeAppleFmServer::from_responses(vec![FakeHttpResponse::json_status(
            200,
            json!({
                "status": "ok",
                "model_available": false,
                "unavailable_reason": "model_not_ready",
                "availability_message": "Foundation model is still preparing"
            }),
        )]);
        let address = server
            .base_url()
            .strip_prefix("http://")
            .expect("base url should start with http://");
        let (host, port) = address.rsplit_once(':').expect("host:port pair");
        let port = port.parse::<u16>().expect("port should parse");

        let config = PsionicServerConfig {
            api_kind: BackendKind::AppleFmBridge,
            host: host.to_string(),
            port,
            model_id: Some(String::from("apple-foundation-model")),
            reasoning_level: None,
            ..PsionicServerConfig::default()
        };
        let error = config
            .prepare(Duration::from_secs(1))
            .expect_err("unavailable model should fail");
        assert!(
            error.to_string().contains("reason=model_not_ready"),
            "error should include the typed unavailability reason"
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
    }

    #[test]
    fn launch_mode_spawns_fake_server_script() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();
        let models_dir = root.join("v1");
        std::fs::create_dir_all(&models_dir).expect("create models dir");
        std::fs::write(models_dir.join("models"), "{\"ok\":true}").expect("write models file");

        let script_path = root.join("fake-psionic-openai-server.sh");
        std::fs::write(
            &script_path,
            r#"#!/bin/sh
HOST=127.0.0.1
PORT=8080
while [ "$#" -gt 0 ]; do
  case "$1" in
    --host) HOST="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    -m|--model|--backend|--reasoning-budget) shift 2 ;;
    *) shift ;;
  esac
done
cd "$(dirname "$0")" && exec python3 -m http.server "$PORT" --bind "$HOST"
"#,
        )
        .expect("write script");
        let mut permissions = std::fs::metadata(&script_path)
            .expect("metadata")
            .permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script_path, permissions).expect("set permissions");
        }

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("listener addr").port();
        drop(listener);

        let config = PsionicServerConfig {
            mode: PsionicServerMode::Launch,
            api_kind: BackendKind::OpenAiChatCompletions,
            host: String::from("127.0.0.1"),
            port,
            backend: String::from("cpu"),
            binary_path: Some(script_path),
            model_path: Some(root.join("fake.gguf")),
            model_id: Some(String::from("fake.gguf")),
            reasoning_budget: None,
            reasoning_level: None,
            service_tier: None,
            control_plane: None,
        };

        let guard: ServerProcessGuard = config
            .prepare(Duration::from_secs(5))
            .expect("launch should succeed");
        assert!(matches!(guard.mode(), PsionicServerMode::Launch));
    }
}
