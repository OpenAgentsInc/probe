use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::backend_profiles::PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL;

const DEFAULT_SERVER_CONFIG_PATH: &str = "server/psionic-local.json";
const DEFAULT_SERVER_HOST: &str = "127.0.0.1";
const DEFAULT_SERVER_PORT: u16 = 8080;
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
    pub host: String,
    pub port: u16,
    pub backend: String,
    pub binary_path: Option<PathBuf>,
    pub model_path: Option<PathBuf>,
    pub model_id: Option<String>,
    pub reasoning_budget: Option<u8>,
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
    MissingBinaryPath,
    MissingModelPath,
    SpawnFailed(String),
    ReadinessTimeout { base_url: String, timeout_secs: u64 },
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
            host: String::from(DEFAULT_SERVER_HOST),
            port: DEFAULT_SERVER_PORT,
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
    pub fn base_url(&self) -> String {
        format!("http://{}:{}/v1", self.host, self.port)
    }

    #[must_use]
    pub fn resolved_model_id(&self) -> Option<String> {
        self.model_id.clone().or_else(|| {
            self.model_path.as_ref().and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
            })
        })
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
        Ok(())
    }

    pub fn prepare(
        &self,
        startup_timeout: Duration,
    ) -> Result<ServerProcessGuard, ServerControlError> {
        match self.mode {
            PsionicServerMode::Attach => {
                wait_for_ready(self.base_url().as_str(), startup_timeout)?;
                Ok(ServerProcessGuard {
                    config: self.clone(),
                    child: None,
                })
            }
            PsionicServerMode::Launch => {
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
                if let Err(error) = wait_for_ready(self.base_url().as_str(), startup_timeout) {
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
    pub fn mode(&self) -> &PsionicServerMode {
        &self.config.mode
    }
}

fn wait_for_ready(base_url: &str, timeout: Duration) -> Result<(), ServerControlError> {
    let deadline = Instant::now() + timeout;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .map_err(|error| ServerControlError::Http(error.to_string()))?;
    let models_url = format!("{}/models", base_url.trim_end_matches('/'));

    loop {
        match client.get(models_url.as_str()).send() {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(_) | Err(_) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(250));
            }
            Ok(_) | Err(_) => {
                return Err(ServerControlError::ReadinessTimeout {
                    base_url: String::from(base_url),
                    timeout_secs: timeout.as_secs(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;

    use super::{
        PsionicServerConfig, PsionicServerMode, ServerConfigOverrides, ServerProcessGuard,
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
