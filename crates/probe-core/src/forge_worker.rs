use std::fmt::{Display, Formatter};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const FORGE_WORKER_FILE_RELATIVE_PATH: &str = "auth/forge-worker.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeWorkerSessionRecord {
    pub base_url: String,
    pub worker_id: String,
    pub org_id: String,
    pub project_id: Option<String>,
    pub runtime_kind: String,
    pub environment_class: Option<String>,
    pub session_id: String,
    pub session_token: String,
    pub expires_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeWorkerStatus {
    pub path: PathBuf,
    pub attached: bool,
    pub base_url: Option<String>,
    pub worker_id: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeWorkerRuntimeContext {
    pub request_id: String,
    pub worker_id: String,
    pub org_id: String,
    pub project_id: Option<String>,
    pub runtime_kind: String,
    pub environment_class: Option<String>,
    pub session_id: String,
    pub worker_state: String,
}

#[derive(Clone, Debug)]
pub struct ForgeWorkerAuthStore {
    path: PathBuf,
}

impl ForgeWorkerAuthStore {
    #[must_use]
    pub fn new(probe_home: impl AsRef<Path>) -> Self {
        Self {
            path: probe_home.as_ref().join(FORGE_WORKER_FILE_RELATIVE_PATH),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn load(&self) -> Result<Option<ForgeWorkerSessionRecord>, ForgeWorkerError> {
        if !self.path.exists() {
            return Ok(None);
        }

        let raw = fs::read_to_string(self.path.as_path()).map_err(ForgeWorkerError::Io)?;
        let record = serde_json::from_str(&raw).map_err(ForgeWorkerError::Json)?;
        Ok(Some(record))
    }

    pub fn save(&self, record: &ForgeWorkerSessionRecord) -> Result<(), ForgeWorkerError> {
        let parent = self.path.parent().ok_or_else(|| {
            ForgeWorkerError::Store(String::from("forge worker auth file must have a parent"))
        })?;
        fs::create_dir_all(parent).map_err(ForgeWorkerError::Io)?;
        write_private_file(
            self.path.as_path(),
            serde_json::to_vec_pretty(record).map_err(ForgeWorkerError::Json)?,
        )
    }

    pub fn clear(&self) -> Result<bool, ForgeWorkerError> {
        match fs::remove_file(self.path.as_path()) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(ForgeWorkerError::Io(error)),
        }
    }

    pub fn status(&self) -> Result<ForgeWorkerStatus, ForgeWorkerError> {
        let record = self.load()?;
        Ok(match record {
            Some(record) => ForgeWorkerStatus {
                path: self.path.clone(),
                attached: true,
                base_url: Some(record.base_url),
                worker_id: Some(record.worker_id),
                expires_at: Some(record.expires_at),
            },
            None => ForgeWorkerStatus {
                path: self.path.clone(),
                attached: false,
                base_url: None,
                worker_id: None,
                expires_at: None,
            },
        })
    }
}

#[derive(Clone, Debug)]
pub struct ForgeWorkerAuthController {
    client: Client,
    base_url: String,
    store: ForgeWorkerAuthStore,
}

impl ForgeWorkerAuthController {
    pub fn new(
        probe_home: impl AsRef<Path>,
        base_url: impl Into<String>,
    ) -> Result<Self, ForgeWorkerError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(ForgeWorkerError::Http)?;
        Ok(Self {
            client,
            base_url: trim_base_url(base_url.into()),
            store: ForgeWorkerAuthStore::new(probe_home),
        })
    }

    #[must_use]
    pub fn store(&self) -> &ForgeWorkerAuthStore {
        &self.store
    }

    pub fn status(&self) -> Result<ForgeWorkerStatus, ForgeWorkerError> {
        self.store.status()
    }

    pub fn attach_worker(
        &self,
        worker_id: &str,
        bootstrap_token: &str,
        attachment_metadata: Option<Value>,
    ) -> Result<ForgeWorkerSessionRecord, ForgeWorkerError> {
        let response = self
            .client
            .post(format!("{}/worker/v1/attach", self.base_url))
            .json(&json!({
                "worker_id": worker_id,
                "bootstrap_token": bootstrap_token,
                "attachment_metadata": attachment_metadata.unwrap_or_else(|| json!({})),
            }))
            .send()
            .map_err(ForgeWorkerError::Http)?;
        let payload: ForgeWorkerAttachResponse = decode_json_response(response)?;
        let record = ForgeWorkerSessionRecord {
            base_url: self.base_url.clone(),
            worker_id: payload.worker.id,
            org_id: payload.worker.org_id,
            project_id: payload.worker.project_id,
            runtime_kind: payload.worker.runtime_kind,
            environment_class: payload.worker.environment_class,
            session_id: payload.session_id,
            session_token: payload.session_token,
            expires_at: payload.expires_at,
        };
        self.store.save(&record)?;
        Ok(record)
    }

    pub fn worker_context(&self) -> Result<Option<ForgeWorkerRuntimeContext>, ForgeWorkerError> {
        let Some(record) = self.store.load()? else {
            return Ok(None);
        };

        match self
            .client
            .get(format!("{}/worker/v1/me", record.base_url))
            .header("authorization", format!("Bearer {}", record.session_token))
            .header("x-request-id", next_request_id("ctx"))
            .send()
            .map_err(ForgeWorkerError::Http)
        {
            Ok(response) if response.status() == StatusCode::UNAUTHORIZED => {
                let _ = self.store.clear();
                Err(ForgeWorkerError::WorkerSessionRevoked)
            }
            Ok(response) => {
                let payload: ForgeWorkerContextResponse = decode_json_response(response)?;
                Ok(Some(ForgeWorkerRuntimeContext::from(payload)))
            }
            Err(error) => Err(error),
        }
    }

    pub fn heartbeat(
        &self,
        state: &str,
        current_run_id: Option<&str>,
        metadata_patch: Option<Value>,
    ) -> Result<ForgeWorkerRuntimeContext, ForgeWorkerError> {
        let record = self
            .store
            .load()?
            .ok_or(ForgeWorkerError::WorkerNotAttached)?;

        match self
            .client
            .post(format!("{}/worker/v1/heartbeat", record.base_url))
            .header("authorization", format!("Bearer {}", record.session_token))
            .header("x-request-id", next_request_id("heartbeat"))
            .json(&json!({
                "state": state,
                "current_run_id": current_run_id,
                "metadata_patch": metadata_patch.unwrap_or_else(|| json!({})),
            }))
            .send()
            .map_err(ForgeWorkerError::Http)
        {
            Ok(response) if response.status() == StatusCode::UNAUTHORIZED => {
                let _ = self.store.clear();
                Err(ForgeWorkerError::WorkerSessionRevoked)
            }
            Ok(response) => {
                let payload: ForgeWorkerContextResponse = decode_json_response(response)?;
                Ok(ForgeWorkerRuntimeContext::from(payload))
            }
            Err(error) => Err(error),
        }
    }

    pub fn clear(&self) -> Result<bool, ForgeWorkerError> {
        self.store.clear()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ForgeWorkerAttachResponse {
    worker: ForgeWorkerApiWorker,
    session_id: String,
    session_token: String,
    expires_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ForgeWorkerContextResponse {
    request_id: String,
    worker_session: ForgeWorkerSessionContext,
    worker: ForgeWorkerApiWorker,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ForgeWorkerApiWorker {
    id: String,
    org_id: String,
    project_id: Option<String>,
    runtime_kind: String,
    environment_class: Option<String>,
    state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ForgeWorkerSessionContext {
    worker_id: String,
    org_id: String,
    project_id: Option<String>,
    runtime_kind: String,
    environment_class: Option<String>,
    session_id: String,
}

#[derive(Debug)]
pub enum ForgeWorkerError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Http(reqwest::Error),
    Store(String),
    UnexpectedStatus { status: u16, body: String },
    WorkerNotAttached,
    WorkerSessionRevoked,
}

impl Display for ForgeWorkerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::Http(error) => write!(f, "http error: {error}"),
            Self::Store(message) => f.write_str(message),
            Self::UnexpectedStatus { status, body } => {
                write!(f, "Forge returned http {status}: {body}")
            }
            Self::WorkerNotAttached => f.write_str("Probe worker is not attached to Forge"),
            Self::WorkerSessionRevoked => f.write_str(
                "Forge worker session was revoked or expired and has been cleared locally",
            ),
        }
    }
}

impl std::error::Error for ForgeWorkerError {}

impl From<ForgeWorkerContextResponse> for ForgeWorkerRuntimeContext {
    fn from(value: ForgeWorkerContextResponse) -> Self {
        Self {
            request_id: value.request_id,
            worker_id: value.worker_session.worker_id,
            org_id: value.worker_session.org_id,
            project_id: value.worker_session.project_id,
            runtime_kind: value.worker_session.runtime_kind,
            environment_class: value.worker_session.environment_class,
            session_id: value.worker_session.session_id,
            worker_state: value.worker.state,
        }
    }
}

fn trim_base_url(value: String) -> String {
    let trimmed = value.trim_end_matches('/').to_string();
    if let Some(root) = trimmed.strip_suffix("/v1") {
        root.to_string()
    } else {
        trimmed
    }
}

fn next_request_id(prefix: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("probe-forge-{prefix}-{now}")
}

fn decode_json_response<T: for<'de> Deserialize<'de>>(
    response: Response,
) -> Result<T, ForgeWorkerError> {
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response
            .text()
            .unwrap_or_else(|_| String::from("<unreadable body>"));
        return Err(ForgeWorkerError::UnexpectedStatus { status, body });
    }

    response.json().map_err(ForgeWorkerError::Http)
}

fn write_private_file(path: &Path, bytes: Vec<u8>) -> Result<(), ForgeWorkerError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(ForgeWorkerError::Io)?;
        file.write_all(bytes.as_slice())
            .map_err(ForgeWorkerError::Io)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(ForgeWorkerError::Io)?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        fs::write(path, bytes).map_err(ForgeWorkerError::Io)
    }
}

#[cfg(test)]
mod tests {
    use probe_test_support::{FakeHttpRequest, FakeHttpResponse, FakeOpenAiServer};
    use serde_json::json;
    use tempfile::tempdir;

    use super::{ForgeWorkerAuthController, ForgeWorkerError};

    #[test]
    fn attach_persists_worker_session_and_fetches_context() {
        let server = FakeOpenAiServer::from_handler(|request: FakeHttpRequest| {
            match request.path.as_str() {
                "/worker/v1/attach" => FakeHttpResponse::json_ok(json!({
                    "worker": {
                        "id": "forge-worker-1",
                        "org_id": "org-1",
                        "project_id": "project-1",
                        "runtime_kind": "probe",
                        "environment_class": "linux-dev",
                        "state": "attached"
                    },
                    "session_id": "forge-worker-session-1",
                    "session_token": "session-token-1",
                    "expires_at": "2026-04-14T18:00:00Z"
                })),
                "/worker/v1/me" => FakeHttpResponse::json_ok(json!({
                    "request_id": "probe-forge-ctx-1",
                    "worker_session": {
                        "worker_id": "forge-worker-1",
                        "org_id": "org-1",
                        "project_id": "project-1",
                        "runtime_kind": "probe",
                        "environment_class": "linux-dev",
                        "session_id": "forge-worker-session-1"
                    },
                    "worker": {
                        "id": "forge-worker-1",
                        "org_id": "org-1",
                        "project_id": "project-1",
                        "runtime_kind": "probe",
                        "environment_class": "linux-dev",
                        "state": "attached"
                    }
                })),
                "/worker/v1/heartbeat" => FakeHttpResponse::json_ok(json!({
                    "request_id": "probe-forge-heartbeat-1",
                    "worker_session": {
                        "worker_id": "forge-worker-1",
                        "org_id": "org-1",
                        "project_id": "project-1",
                        "runtime_kind": "probe",
                        "environment_class": "linux-dev",
                        "session_id": "forge-worker-session-1"
                    },
                    "worker": {
                        "id": "forge-worker-1",
                        "org_id": "org-1",
                        "project_id": "project-1",
                        "runtime_kind": "probe",
                        "environment_class": "linux-dev",
                        "state": "busy"
                    }
                })),
                other => panic!("unexpected path {other}"),
            }
        });
        let temp = tempdir().expect("temp dir");
        let controller = ForgeWorkerAuthController::new(temp.path(), server.base_url()).unwrap();

        let record = controller
            .attach_worker(
                "forge-worker-1",
                "bootstrap-token",
                Some(json!({"hostname":"mbp"})),
            )
            .unwrap();
        assert_eq!(record.worker_id, "forge-worker-1");

        let persisted = controller.store().load().unwrap().unwrap();
        assert_eq!(persisted.session_id, "forge-worker-session-1");

        let context = controller.worker_context().unwrap().unwrap();
        assert_eq!(context.worker_id, "forge-worker-1");
        assert_eq!(context.worker_state, "attached");

        let heartbeat = controller
            .heartbeat("busy", Some("forge-run-1"), Some(json!({"load":"active"})))
            .unwrap();
        assert_eq!(heartbeat.worker_state, "busy");
    }

    #[test]
    fn unauthorized_worker_context_clears_local_session() {
        let server = FakeOpenAiServer::from_handler(|request: FakeHttpRequest| {
            match request.path.as_str() {
                "/worker/v1/attach" => FakeHttpResponse::json_ok(json!({
                    "worker": {
                        "id": "forge-worker-1",
                        "org_id": "org-1",
                        "project_id": "project-1",
                        "runtime_kind": "probe",
                        "environment_class": "linux-dev",
                        "state": "attached"
                    },
                    "session_id": "forge-worker-session-1",
                    "session_token": "session-token-1",
                    "expires_at": "2026-04-14T18:00:00Z"
                })),
                "/worker/v1/me" => FakeHttpResponse::json_status(
                    401,
                    json!({ "error": { "code": "invalid_worker_auth" } }),
                ),
                other => panic!("unexpected path {other}"),
            }
        });
        let temp = tempdir().expect("temp dir");
        let controller = ForgeWorkerAuthController::new(temp.path(), server.base_url()).unwrap();

        controller
            .attach_worker("forge-worker-1", "bootstrap-token", None)
            .unwrap();
        let error = controller
            .worker_context()
            .expect_err("expected revocation");
        assert!(matches!(error, ForgeWorkerError::WorkerSessionRevoked));
        assert!(controller.store().load().unwrap().is_none());
    }
}
