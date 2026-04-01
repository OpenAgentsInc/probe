use std::fmt::{Display, Formatter};
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use probe_protocol::backend::BackendKind;
use probe_provider_apple_fm::{AppleFmProviderClient, AppleFmProviderConfig};
use psionic_apple_fm::AppleFmSystemLanguageModelAvailability;
use serde::{Deserialize, Serialize};

use crate::backend_profiles::{PSIONIC_APPLE_FM_MODEL, PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL};

const DEFAULT_SERVER_CONFIG_PATH: &str = "server/psionic-local.json";
const DEFAULT_OPENAI_SERVER_CONFIG_PATH: &str = "server/psionic-openai-chat-completions.json";
const DEFAULT_APPLE_FM_SERVER_CONFIG_PATH: &str = "server/psionic-apple-fm.json";
const DEFAULT_SERVER_HOST: &str = "127.0.0.1";
const DEFAULT_OPENAI_SERVER_PORT: u16 = 8080;
const DEFAULT_APPLE_FM_SERVER_PORT: u16 = 11435;
const DEFAULT_SERVER_BACKEND: &str = "cpu";

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
    UnsupportedManagedLaunch { backend: BackendKind },
    MissingBinaryPath,
    MissingModelPath,
    SpawnFailed(String),
    ReadinessTimeout { base_url: String, timeout_secs: u64 },
    BackendUnavailable { base_url: String, detail: String },
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
        }
    }
}

impl PsionicServerConfig {
    #[must_use]
    pub fn config_path(probe_home: &Path) -> PathBuf {
        probe_home.join(DEFAULT_SERVER_CONFIG_PATH)
    }

    #[must_use]
    pub fn backend_config_path(probe_home: &Path, api_kind: BackendKind) -> PathBuf {
        probe_home.join(match api_kind {
            BackendKind::OpenAiChatCompletions => DEFAULT_OPENAI_SERVER_CONFIG_PATH,
            BackendKind::AppleFmBridge => DEFAULT_APPLE_FM_SERVER_CONFIG_PATH,
        })
    }

    #[must_use]
    pub fn base_url(&self) -> String {
        match self.api_kind {
            BackendKind::OpenAiChatCompletions => format!("http://{}:{}/v1", self.host, self.port),
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
            .or_else(|| default_model_id_for(self.api_kind))
    }

    pub fn set_api_kind(&mut self, api_kind: BackendKind) {
        if self.api_kind == api_kind {
            self.ensure_kind_defaults();
            return;
        }

        let previous_default_port = default_port_for(self.api_kind);
        let previous_default_model = default_model_id_for(self.api_kind);
        if self.port == previous_default_port {
            self.port = default_port_for(api_kind);
        }
        if self.model_id.is_none() || self.model_id == previous_default_model {
            self.model_id = default_model_id_for(api_kind);
        }
        self.api_kind = api_kind;
        self.ensure_kind_defaults();
    }

    fn ensure_kind_defaults(&mut self) {
        if self.model_id.is_none() {
            self.model_id = default_model_id_for(self.api_kind);
        }
    }

    #[must_use]
    pub fn operator_summary(&self) -> ServerOperatorSummary {
        ServerOperatorSummary {
            backend_kind: self.api_kind,
            mode: self.mode.clone(),
            target_kind: classify_target_kind(self.mode.clone(), self.host.as_str()),
            host: self.host.clone(),
            port: self.port,
            base_url: self.base_url(),
            model_id: self.resolved_model_id(),
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
                wait_for_ready(self, startup_timeout)?;
                Ok(ServerProcessGuard {
                    config: self.clone(),
                    child: None,
                })
            }
            PsionicServerMode::Launch => {
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
                if let Err(error) = wait_for_ready(self, startup_timeout) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
                Ok(ServerProcessGuard {
                    config: self.clone(),
                    child: Some(child),
                })
            }
        }
    }
}

impl ServerProcessGuard {
    #[must_use]
    pub fn base_url(&self) -> String {
        self.config.base_url()
    }

    #[must_use]
    pub fn model_id(&self) -> Option<String> {
        self.config.resolved_model_id()
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
        self.config.operator_summary()
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
) -> Result<(), ServerControlError> {
    let deadline = Instant::now() + timeout;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .map_err(|error| ServerControlError::Http(error.to_string()))?;
    let readiness_url = match config.api_kind {
        BackendKind::OpenAiChatCompletions => {
            format!("{}/models", config.base_url().trim_end_matches('/'))
        }
        BackendKind::AppleFmBridge => format!("{}/health", config.base_url().trim_end_matches('/')),
    };

    loop {
        match config.api_kind {
            BackendKind::OpenAiChatCompletions => match client.get(readiness_url.as_str()).send() {
                Ok(response) if response.status().is_success() => return Ok(()),
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
                    Ok(availability) if availability.is_ready() => return Ok(()),
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

const fn default_server_api_kind() -> BackendKind {
    BackendKind::OpenAiChatCompletions
}

const fn default_port_for(api_kind: BackendKind) -> u16 {
    match api_kind {
        BackendKind::OpenAiChatCompletions => DEFAULT_OPENAI_SERVER_PORT,
        BackendKind::AppleFmBridge => DEFAULT_APPLE_FM_SERVER_PORT,
    }
}

fn default_model_id_for(api_kind: BackendKind) -> Option<String> {
    match api_kind {
        BackendKind::OpenAiChatCompletions => {
            Some(String::from(PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL))
        }
        BackendKind::AppleFmBridge => Some(String::from(PSIONIC_APPLE_FM_MODEL)),
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
    use probe_test_support::{FakeAppleFmServer, FakeHttpResponse};
    use serde_json::json;

    use super::{
        DEFAULT_APPLE_FM_SERVER_PORT, PSIONIC_APPLE_FM_MODEL, PsionicServerConfig,
        PsionicServerMode, ServerConfigOverrides, ServerProcessGuard, ServerTargetKind,
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
            temp.path().join("server/psionic-openai-chat-completions.json")
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
        qwen
            .save(
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
        assert_eq!(loaded.resolved_model_id().as_deref(), Some("remote-qwen.gguf"));

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
        };

        let guard: ServerProcessGuard = config
            .prepare(Duration::from_secs(5))
            .expect("launch should succeed");
        assert!(matches!(guard.mode(), PsionicServerMode::Launch));
    }
}
