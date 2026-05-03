use std::fmt::{Display, Formatter};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use probe_protocol::backend::BackendProfile;
use probe_protocol::managed_environment::{
    ManagedEnvironmentConstraints, ManagedEnvironmentHostClass, ManagedEnvironmentProviderKind,
};
use probe_protocol::managed_runtime::{
    ManagedRuntimeActor, ManagedRuntimeArtifactKind, ManagedRuntimeArtifactRef,
    ManagedRuntimeCorrelation, ManagedRuntimeErrorPayload, ManagedRuntimeEventPayload,
    ManagedRuntimeEventType, ManagedRuntimeSessionStatus, ManagedRuntimeSource, ManagedSessionRef,
    managed_runtime_transcript_ref,
};
use probe_protocol::session::{SessionHarnessProfile, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::managed_runtime::{
    ManagedRuntimeController, ManagedRuntimeError, ManagedRuntimeEventDraft,
};
use crate::runtime::{
    PlainTextExecRequest, ProbeRuntime, RuntimeError, RuntimeEvent, RuntimeEventSink,
};
use crate::tools::ToolLoopConfig;

pub const PROBE_MANAGED_CLOUD_RUN_JOB_SCHEMA_VERSION: &str = "probe.managed_cloud_run_job.v1";
pub const PROBE_MANAGED_CLOUD_RUN_ASSIGNMENT_SCHEMA_VERSION: &str =
    "probe.managed_cloud_run_job.assignment.v1";

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedCloudRunEnvironmentAllocation {
    pub managed_environment_id: String,
    pub provider: ManagedEnvironmentProviderKind,
    pub host_class: ManagedEnvironmentHostClass,
    pub environment_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<ManagedEnvironmentConstraints>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedCloudRunJobAssignmentClaims {
    pub schema_version: String,
    pub assignment_id: String,
    pub idempotency_key: String,
    pub managed_session_id: String,
    pub managed_run_id: String,
    pub callback_url: String,
    pub goal_prompt: String,
    pub environment: ManagedCloudRunEnvironmentAllocation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_gcs_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

impl ManagedCloudRunJobAssignmentClaims {
    #[must_use]
    pub fn new(
        assignment_id: impl Into<String>,
        idempotency_key: impl Into<String>,
        managed_session_id: impl Into<String>,
        managed_run_id: impl Into<String>,
        callback_url: impl Into<String>,
        goal_prompt: impl Into<String>,
        managed_environment_id: impl Into<String>,
        environment_class: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: String::from(PROBE_MANAGED_CLOUD_RUN_ASSIGNMENT_SCHEMA_VERSION),
            assignment_id: assignment_id.into(),
            idempotency_key: idempotency_key.into(),
            managed_session_id: managed_session_id.into(),
            managed_run_id: managed_run_id.into(),
            callback_url: callback_url.into(),
            goal_prompt: goal_prompt.into(),
            environment: ManagedCloudRunEnvironmentAllocation {
                managed_environment_id: managed_environment_id.into(),
                provider: ManagedEnvironmentProviderKind::GoogleCloud,
                host_class: ManagedEnvironmentHostClass::CloudRunJob,
                environment_class: environment_class.into(),
                constraints: None,
            },
            title: None,
            profile: None,
            model: None,
            cwd: None,
            artifact_gcs_prefix: None,
            issued_at_ms: None,
            expires_at_ms: None,
            metadata: Map::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedCloudRunJobIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_index: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_attempt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logs_url: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedCloudRunJobStatus {
    Started,
    Completed,
    Failed,
    DuplicateSkipped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedCloudRunJobState {
    pub schema_version: String,
    pub assignment_id: String,
    pub idempotency_key: String,
    pub status: ManagedCloudRunJobStatus,
    pub cloud_run: ManagedCloudRunJobIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_event_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<ManagedRuntimeArtifactRef>,
    pub started_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManagedCloudRunJobOutcome {
    Completed(ManagedCloudRunJobState),
    Failed(ManagedCloudRunJobState),
    DuplicateSkipped(ManagedCloudRunJobState),
}

impl ManagedCloudRunJobOutcome {
    #[must_use]
    pub fn state(&self) -> &ManagedCloudRunJobState {
        match self {
            Self::Completed(state) | Self::Failed(state) | Self::DuplicateSkipped(state) => state,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ManagedCloudRunJobRunRequest {
    pub assignment_token: String,
    pub signing_secret: String,
    pub callback_bearer_token: Option<String>,
    pub profile: BackendProfile,
    pub default_cwd: PathBuf,
    pub artifact_dir: PathBuf,
    pub cloud_run: ManagedCloudRunJobIdentity,
    pub system_prompt: Option<String>,
    pub harness_profile: Option<SessionHarnessProfile>,
    pub tool_loop: Option<ToolLoopConfig>,
    pub dry_run: bool,
}

#[derive(Clone, Debug)]
pub struct ManagedCloudRunJobRunner {
    runtime: ProbeRuntime,
    managed_runtime: ManagedRuntimeController,
}

impl ManagedCloudRunJobRunner {
    #[must_use]
    pub fn new(runtime: ProbeRuntime, managed_runtime: ManagedRuntimeController) -> Self {
        Self {
            runtime,
            managed_runtime,
        }
    }

    pub fn run_once(
        &self,
        request: ManagedCloudRunJobRunRequest,
    ) -> Result<ManagedCloudRunJobOutcome, ManagedCloudRunJobError> {
        let claims = verify_assignment_token(
            request.assignment_token.as_str(),
            request.signing_secret.as_str(),
            now_ms(),
        )?;
        validate_cloud_run_allocation(&claims)?;

        fs::create_dir_all(request.artifact_dir.as_path()).map_err(ManagedCloudRunJobError::Io)?;
        let state_path = state_path(&request.artifact_dir, claims.idempotency_key.as_str());
        if let Some(existing) = read_state_if_present(state_path.as_path())? {
            let mut duplicate = existing;
            duplicate.status = ManagedCloudRunJobStatus::DuplicateSkipped;
            return Ok(ManagedCloudRunJobOutcome::DuplicateSkipped(duplicate));
        }

        let started_at_ms = now_ms();
        let mut state = ManagedCloudRunJobState {
            schema_version: String::from(PROBE_MANAGED_CLOUD_RUN_JOB_SCHEMA_VERSION),
            assignment_id: claims.assignment_id.clone(),
            idempotency_key: claims.idempotency_key.clone(),
            status: ManagedCloudRunJobStatus::Started,
            cloud_run: request.cloud_run.clone(),
            probe_session_id: None,
            terminal_event_sequence: None,
            artifact_refs: Vec::new(),
            started_at_ms,
            finished_at_ms: None,
            error: None,
        };
        write_state(state_path.as_path(), &state)?;
        post_callback(
            &claims,
            request.callback_bearer_token.as_deref(),
            "job.started",
            &state,
            None,
        )?;

        let execution = if request.dry_run {
            let session = self
                .runtime
                .session_store()
                .create_session(
                    claims.title.clone().unwrap_or_else(|| {
                        format!("Managed Cloud Run: {}", claims.assignment_id.as_str())
                    }),
                    claims
                        .cwd
                        .clone()
                        .unwrap_or_else(|| request.default_cwd.clone()),
                )
                .map_err(|error| ManagedCloudRunJobError::Runtime {
                    session_id: None,
                    error: error.into(),
                });
            session.map(|session| ManagedCloudRunExecutionResult {
                session_id: session.id,
                assistant_text: String::from("dry-run completed without model execution"),
            })
        } else {
            self.execute_assignment(&claims, &request)
        };

        match execution {
            Ok(result) => {
                state.probe_session_id = Some(result.session_id.as_str().to_string());
                let artifacts = write_evidence_artifact(
                    request.artifact_dir.as_path(),
                    &claims,
                    &state,
                    Some(result.assistant_text.as_str()),
                    None,
                )?;
                state.artifact_refs = artifacts.clone();
                let terminal_event = self.record_terminal_event(
                    &claims,
                    &request,
                    result.session_id,
                    ManagedRuntimeSessionStatus::Completed,
                    Some("Cloud Run Job managed assignment completed"),
                    artifacts,
                    None,
                )?;
                state.status = ManagedCloudRunJobStatus::Completed;
                state.terminal_event_sequence = Some(terminal_event.sequence);
                state.finished_at_ms = Some(now_ms());
                write_state(state_path.as_path(), &state)?;
                post_callback(
                    &claims,
                    request.callback_bearer_token.as_deref(),
                    "job.completed",
                    &state,
                    None,
                )?;
                Ok(ManagedCloudRunJobOutcome::Completed(state))
            }
            Err(error) => {
                let error_text = error.to_string();
                let session_id = error.session_id();
                state.probe_session_id = session_id.as_ref().map(|id| id.as_str().to_string());
                let artifacts = write_evidence_artifact(
                    request.artifact_dir.as_path(),
                    &claims,
                    &state,
                    None,
                    Some(error_text.as_str()),
                )?;
                state.artifact_refs = artifacts.clone();
                if let Some(session_id) = session_id {
                    let terminal_event = self.record_terminal_event(
                        &claims,
                        &request,
                        session_id,
                        ManagedRuntimeSessionStatus::Failed,
                        Some(error_text.as_str()),
                        artifacts,
                        Some(error_text.as_str()),
                    )?;
                    state.terminal_event_sequence = Some(terminal_event.sequence);
                }
                state.status = ManagedCloudRunJobStatus::Failed;
                state.finished_at_ms = Some(now_ms());
                state.error = Some(error_text.clone());
                write_state(state_path.as_path(), &state)?;
                post_callback(
                    &claims,
                    request.callback_bearer_token.as_deref(),
                    "job.failed",
                    &state,
                    Some(error_text.as_str()),
                )?;
                Ok(ManagedCloudRunJobOutcome::Failed(state))
            }
        }
    }

    fn execute_assignment(
        &self,
        claims: &ManagedCloudRunJobAssignmentClaims,
        request: &ManagedCloudRunJobRunRequest,
    ) -> Result<ManagedCloudRunExecutionResult, ManagedCloudRunJobError> {
        let captured_session_id = Arc::new(Mutex::new(None::<SessionId>));
        let sink_session_id = Arc::clone(&captured_session_id);
        let event_sink: Arc<dyn RuntimeEventSink> = Arc::new(move |event: RuntimeEvent| {
            if let RuntimeEvent::TurnStarted { session_id, .. } = event {
                *sink_session_id
                    .lock()
                    .expect("managed cloud run session capture mutex") = Some(session_id);
            }
        });

        let outcome = self
            .runtime
            .exec_plain_text_with_events(
                PlainTextExecRequest {
                    profile: request.profile.clone(),
                    prompt: claims.goal_prompt.clone(),
                    title: claims.title.clone().or_else(|| {
                        Some(format!(
                            "Managed Cloud Run: {}",
                            claims.assignment_id.as_str()
                        ))
                    }),
                    cwd: claims
                        .cwd
                        .clone()
                        .unwrap_or_else(|| request.default_cwd.clone()),
                    system_prompt: request.system_prompt.clone(),
                    harness_profile: request.harness_profile.clone(),
                    tool_loop: request.tool_loop.clone(),
                },
                event_sink,
            )
            .map_err(|error| ManagedCloudRunJobError::Runtime {
                session_id: captured_session_id
                    .lock()
                    .expect("managed cloud run session capture mutex")
                    .clone(),
                error,
            })?;

        Ok(ManagedCloudRunExecutionResult {
            session_id: outcome.session.id,
            assistant_text: outcome.assistant_text,
        })
    }

    fn record_terminal_event(
        &self,
        claims: &ManagedCloudRunJobAssignmentClaims,
        request: &ManagedCloudRunJobRunRequest,
        session_id: SessionId,
        status: ManagedRuntimeSessionStatus,
        reason: Option<&str>,
        artifact_refs: Vec<ManagedRuntimeArtifactRef>,
        error: Option<&str>,
    ) -> Result<probe_protocol::managed_runtime::ManagedRuntimeEvent, ManagedCloudRunJobError> {
        let actor = managed_actor(&request.cloud_run);
        let correlation = managed_correlation(claims);
        let session = ManagedSessionRef {
            probe_session_id: session_id.clone(),
            managed_session_id: Some(claims.managed_session_id.clone()),
            parent_probe_session_id: None,
            child_probe_session_id: None,
        };

        if self
            .managed_runtime
            .replay_events(
                probe_protocol::managed_runtime::ManagedSessionReplayRequest {
                    schema_version: String::from(
                        probe_protocol::managed_runtime::PROBE_MANAGED_RUNTIME_SCHEMA_VERSION,
                    ),
                    request_id: format!("{}:replay-before-terminal", claims.assignment_id),
                    session_ref: session.clone(),
                    after_sequence: 0,
                    limit: Some(1),
                },
            )?
            .events
            .is_empty()
        {
            self.managed_runtime
                .append_event(ManagedRuntimeEventDraft {
                    event_type: ManagedRuntimeEventType::SessionStarted,
                    status: ManagedRuntimeSessionStatus::Running,
                    actor: actor.clone(),
                    source: ManagedRuntimeSource {
                        kind: String::from("cloud_run_job"),
                        id: request.cloud_run.execution_name.clone(),
                        label: Some(String::from("Cloud Run Job")),
                    },
                    session: session.clone(),
                    correlation: correlation.clone(),
                    artifact_refs: vec![managed_runtime_transcript_ref(&session_id)],
                    payload: ManagedRuntimeEventPayload::SessionLifecycle {
                        title: claims
                            .title
                            .clone()
                            .unwrap_or_else(|| claims.assignment_id.clone()),
                        cwd: claims
                            .cwd
                            .clone()
                            .unwrap_or_else(|| request.default_cwd.clone()),
                        backend_profile: request.profile.name.clone(),
                        model: request.profile.model.clone(),
                        environment_constraints: claims.environment.constraints.clone(),
                    },
                })?;
        }

        let mut refs = artifact_refs;
        refs.push(managed_runtime_transcript_ref(&session_id));
        self.managed_runtime
            .append_event(ManagedRuntimeEventDraft {
                event_type: if status == ManagedRuntimeSessionStatus::Completed {
                    ManagedRuntimeEventType::SessionCompleted
                } else {
                    ManagedRuntimeEventType::SessionFailed
                },
                status,
                actor,
                source: ManagedRuntimeSource {
                    kind: String::from("cloud_run_job"),
                    id: request.cloud_run.execution_name.clone(),
                    label: Some(String::from("Cloud Run Job terminal status")),
                },
                session,
                correlation,
                artifact_refs: refs,
                payload: if let Some(error) = error {
                    ManagedRuntimeEventPayload::Error {
                        error: ManagedRuntimeErrorPayload {
                            code: String::from("managed_cloud_run_job_failed"),
                            message: error.to_string(),
                            retryable: false,
                            details: Map::new(),
                        },
                    }
                } else {
                    ManagedRuntimeEventPayload::Terminal {
                        status,
                        reason: reason.map(str::to_string),
                    }
                },
            })
            .map_err(ManagedCloudRunJobError::ManagedRuntime)
    }
}

#[derive(Clone, Debug)]
struct ManagedCloudRunExecutionResult {
    session_id: SessionId,
    assistant_text: String,
}

#[derive(Debug)]
pub enum ManagedCloudRunJobError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Token(String),
    InvalidAssignment(String),
    Runtime {
        session_id: Option<SessionId>,
        error: RuntimeError,
    },
    ManagedRuntime(ManagedRuntimeError),
    Callback(reqwest::Error),
    CallbackStatus {
        status: u16,
        body: String,
    },
}

impl ManagedCloudRunJobError {
    #[must_use]
    pub fn session_id(&self) -> Option<SessionId> {
        match self {
            Self::Runtime { session_id, .. } => session_id.clone(),
            _ => None,
        }
    }
}

impl Display for ManagedCloudRunJobError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::Token(message) | Self::InvalidAssignment(message) => f.write_str(message),
            Self::Runtime { error, .. } => write!(f, "runtime error: {error}"),
            Self::ManagedRuntime(error) => write!(f, "managed runtime error: {error}"),
            Self::Callback(error) => write!(f, "callback error: {error}"),
            Self::CallbackStatus { status, body } => {
                write!(f, "callback returned HTTP {status}: {body}")
            }
        }
    }
}

impl std::error::Error for ManagedCloudRunJobError {}

impl From<std::io::Error> for ManagedCloudRunJobError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ManagedCloudRunJobError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<ManagedRuntimeError> for ManagedCloudRunJobError {
    fn from(value: ManagedRuntimeError) -> Self {
        Self::ManagedRuntime(value)
    }
}

#[must_use]
pub fn state_path(artifact_dir: &Path, idempotency_key: &str) -> PathBuf {
    artifact_dir
        .join("state")
        .join(format!("{}.json", short_digest(idempotency_key.as_bytes())))
}

pub fn sign_assignment_token(
    claims: &ManagedCloudRunJobAssignmentClaims,
    signing_secret: &str,
) -> Result<String, ManagedCloudRunJobError> {
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims)?);
    let signature = hmac_signature(payload.as_bytes(), signing_secret)?;
    Ok(format!("{payload}.{signature}"))
}

pub fn verify_assignment_token(
    token: &str,
    signing_secret: &str,
    now_ms: u64,
) -> Result<ManagedCloudRunJobAssignmentClaims, ManagedCloudRunJobError> {
    let (payload, signature) = token.split_once('.').ok_or_else(|| {
        ManagedCloudRunJobError::Token(String::from("assignment token is malformed"))
    })?;
    let expected = hmac_signature(payload.as_bytes(), signing_secret)?;
    if !constant_time_eq(signature.as_bytes(), expected.as_bytes()) {
        return Err(ManagedCloudRunJobError::Token(String::from(
            "assignment token signature is invalid",
        )));
    }
    let claims: ManagedCloudRunJobAssignmentClaims = serde_json::from_slice(
        URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|error| ManagedCloudRunJobError::Token(error.to_string()))?
            .as_slice(),
    )?;
    if claims.schema_version != PROBE_MANAGED_CLOUD_RUN_ASSIGNMENT_SCHEMA_VERSION {
        return Err(ManagedCloudRunJobError::InvalidAssignment(format!(
            "unsupported assignment schema version `{}`",
            claims.schema_version
        )));
    }
    if let Some(expires_at_ms) = claims.expires_at_ms
        && expires_at_ms <= now_ms
    {
        return Err(ManagedCloudRunJobError::Token(String::from(
            "assignment token is expired",
        )));
    }
    validate_cloud_run_allocation(&claims)?;
    Ok(claims)
}

fn hmac_signature(payload: &[u8], signing_secret: &str) -> Result<String, ManagedCloudRunJobError> {
    let mut mac = HmacSha256::new_from_slice(signing_secret.as_bytes())
        .map_err(|error| ManagedCloudRunJobError::Token(error.to_string()))?;
    mac.update(payload);
    Ok(hex::encode(mac.finalize().into_bytes()))
}

fn validate_cloud_run_allocation(
    claims: &ManagedCloudRunJobAssignmentClaims,
) -> Result<(), ManagedCloudRunJobError> {
    if claims.environment.provider != ManagedEnvironmentProviderKind::GoogleCloud {
        return Err(ManagedCloudRunJobError::InvalidAssignment(String::from(
            "Cloud Run Job runner only accepts google_cloud assignments",
        )));
    }
    if claims.environment.host_class != ManagedEnvironmentHostClass::CloudRunJob {
        return Err(ManagedCloudRunJobError::InvalidAssignment(String::from(
            "Cloud Run Job runner only accepts cloud_run_job host allocations",
        )));
    }
    if let Some(constraints) = claims.environment.constraints.as_ref() {
        if !constraints.allowed_providers.is_empty()
            && !constraints
                .allowed_providers
                .contains(&ManagedEnvironmentProviderKind::GoogleCloud)
        {
            return Err(ManagedCloudRunJobError::InvalidAssignment(String::from(
                "assignment constraints do not allow google_cloud",
            )));
        }
        if !constraints.allowed_host_classes.is_empty()
            && !constraints
                .allowed_host_classes
                .contains(&ManagedEnvironmentHostClass::CloudRunJob)
        {
            return Err(ManagedCloudRunJobError::InvalidAssignment(String::from(
                "assignment constraints do not allow cloud_run_job",
            )));
        }
    }
    Ok(())
}

fn read_state_if_present(
    path: &Path,
) -> Result<Option<ManagedCloudRunJobState>, ManagedCloudRunJobError> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
}

fn write_state(
    path: &Path,
    state: &ManagedCloudRunJobState,
) -> Result<(), ManagedCloudRunJobError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = path.with_extension(format!("json.tmp-{}", std::process::id()));
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(temp_path.as_path())?;
        serde_json::to_writer_pretty(&mut file, state)?;
        file.write_all(b"\n")?;
        file.flush()?;
    }
    fs::rename(temp_path, path)?;
    Ok(())
}

fn write_evidence_artifact(
    artifact_dir: &Path,
    claims: &ManagedCloudRunJobAssignmentClaims,
    state: &ManagedCloudRunJobState,
    assistant_text: Option<&str>,
    error: Option<&str>,
) -> Result<Vec<ManagedRuntimeArtifactRef>, ManagedCloudRunJobError> {
    fs::create_dir_all(artifact_dir)?;
    let evidence = json!({
        "schemaVersion": PROBE_MANAGED_CLOUD_RUN_JOB_SCHEMA_VERSION,
        "assignmentId": claims.assignment_id,
        "managedSessionId": claims.managed_session_id,
        "managedRunId": claims.managed_run_id,
        "managedEnvironmentId": claims.environment.managed_environment_id,
        "cloudRun": state.cloud_run,
        "status": state.status,
        "probeSessionId": state.probe_session_id,
        "assistantText": assistant_text,
        "error": error,
        "finishedAtMs": now_ms(),
    });
    let path = artifact_dir.join("evidence.json");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path.as_path())?;
    serde_json::to_writer_pretty(&mut file, &evidence)?;
    file.write_all(b"\n")?;
    file.flush()?;

    let local_ref = format!("file://{}", path.display());
    let resource_ref = claims
        .artifact_gcs_prefix
        .as_ref()
        .map(|prefix| format!("{}/evidence.json", prefix.trim_end_matches('/')))
        .unwrap_or(local_ref);
    Ok(vec![ManagedRuntimeArtifactRef {
        kind: ManagedRuntimeArtifactKind::VerificationPack,
        resource_ref,
        stable_digest: Some(short_digest(serde_json::to_vec(&evidence)?.as_slice())),
        label: Some(String::from("Managed Cloud Run Job evidence")),
        updated_at_ms: Some(now_ms()),
    }])
}

fn post_callback(
    claims: &ManagedCloudRunJobAssignmentClaims,
    callback_bearer_token: Option<&str>,
    event_type: &str,
    state: &ManagedCloudRunJobState,
    error: Option<&str>,
) -> Result<(), ManagedCloudRunJobError> {
    if claims.callback_url.trim().is_empty() {
        return Ok(());
    }
    let payload = json!({
        "schemaVersion": PROBE_MANAGED_CLOUD_RUN_JOB_SCHEMA_VERSION,
        "eventType": event_type,
        "assignmentId": claims.assignment_id,
        "managedSessionId": claims.managed_session_id,
        "managedRunId": claims.managed_run_id,
        "idempotencyKey": claims.idempotency_key,
        "cloudRun": state.cloud_run,
        "status": state.status,
        "probeSessionId": state.probe_session_id,
        "terminalEventSequence": state.terminal_event_sequence,
        "artifactRefs": state.artifact_refs,
        "error": error,
        "occurredAtMs": now_ms(),
    });
    let client = reqwest::blocking::Client::new();
    let mut request = client.post(claims.callback_url.as_str()).json(&payload);
    if let Some(token) = callback_bearer_token {
        request = request.bearer_auth(token);
    }
    let response = request.send().map_err(ManagedCloudRunJobError::Callback)?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().unwrap_or_default();
        return Err(ManagedCloudRunJobError::CallbackStatus { status, body });
    }
    Ok(())
}

fn managed_actor(cloud_run: &ManagedCloudRunJobIdentity) -> ManagedRuntimeActor {
    ManagedRuntimeActor {
        kind: String::from("cloud_run_job"),
        id: cloud_run
            .execution_name
            .clone()
            .or_else(|| cloud_run.job_name.clone()),
        label: Some(String::from("Google Cloud Run Job")),
    }
}

fn managed_correlation(claims: &ManagedCloudRunJobAssignmentClaims) -> ManagedRuntimeCorrelation {
    ManagedRuntimeCorrelation {
        request_id: Some(claims.assignment_id.clone()),
        workspace: Some(String::from("openagents.com")),
        managed_environment_id: Some(claims.environment.managed_environment_id.clone()),
        managed_session_id: Some(claims.managed_session_id.clone()),
        managed_run_id: Some(claims.managed_run_id.clone()),
        ..ManagedRuntimeCorrelation::default()
    }
}

fn short_digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))[..16].to_string()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right.iter())
        .fold(0_u8, |acc, (left, right)| acc | (left ^ right))
        == 0
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use probe_protocol::managed_runtime::{
        ManagedSessionRef, ManagedSessionReplayRequest, PROBE_MANAGED_RUNTIME_SCHEMA_VERSION,
    };
    use probe_test_support::FakeOpenAiServer;
    use serde_json::json;
    use tempfile::tempdir;

    use crate::managed_runtime::ManagedRuntimeController;
    use crate::runtime::ProbeRuntime;
    use crate::session_store::FilesystemSessionStore;

    use super::{
        ManagedCloudRunJobAssignmentClaims, ManagedCloudRunJobIdentity, ManagedCloudRunJobOutcome,
        ManagedCloudRunJobRunRequest, ManagedCloudRunJobRunner, sign_assignment_token,
        verify_assignment_token,
    };

    #[test]
    fn assignment_token_round_trips_and_rejects_wrong_provider() {
        let mut claims = test_claims("http://127.0.0.1:1/callback");
        claims.expires_at_ms = Some(u64::MAX);
        let token = sign_assignment_token(&claims, "secret").expect("sign token");
        let decoded = verify_assignment_token(&token, "secret", 1).expect("verify token");
        assert_eq!(decoded.assignment_id, "assignment-1");

        claims.environment.provider =
            probe_protocol::managed_environment::ManagedEnvironmentProviderKind::Daytona;
        let token = sign_assignment_token(&claims, "secret").expect("sign token");
        let error = verify_assignment_token(&token, "secret", 1).expect_err("reject provider");
        assert!(error.to_string().contains("google_cloud"));
    }

    #[test]
    fn runner_executes_once_and_skips_duplicate_idempotency_key() {
        let _api_key = ScopedEnvVar::set("PROBE_MANAGED_CLOUD_RUN_TEST_KEY", "probe-test-key");
        let provider = FakeOpenAiServer::from_json_responses(vec![json!({
            "id": "chatcmpl_managed_cloud_run",
            "model": "qwen3.5-2b-q8_0-registry.gguf",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "managed job complete"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 4, "completion_tokens": 3, "total_tokens": 7}
        })]);
        let callback = FakeOpenAiServer::from_handler(|_request| {
            probe_test_support::FakeHttpResponse::json_ok(json!({"ok": true}))
        });
        let temp = tempdir().expect("temp dir");
        let claims = test_claims(callback.base_url());
        let token = sign_assignment_token(&claims, "secret").expect("sign token");
        let runtime = ProbeRuntime::new(temp.path());
        let managed_runtime =
            ManagedRuntimeController::new(FilesystemSessionStore::new(temp.path()));
        let runner = ManagedCloudRunJobRunner::new(runtime, managed_runtime.clone());

        let first = runner
            .run_once(ManagedCloudRunJobRunRequest {
                assignment_token: token.clone(),
                signing_secret: String::from("secret"),
                callback_bearer_token: Some(String::from("callback-token")),
                profile: test_profile(provider.base_url()),
                default_cwd: temp.path().to_path_buf(),
                artifact_dir: temp.path().join("artifacts"),
                cloud_run: test_cloud_run_identity(),
                system_prompt: None,
                harness_profile: None,
                tool_loop: None,
                dry_run: false,
            })
            .expect("managed job should run");
        let ManagedCloudRunJobOutcome::Completed(state) = first else {
            panic!("expected completion");
        };
        let session_id = state
            .probe_session_id
            .as_ref()
            .expect("probe session id should be retained");
        let replay = managed_runtime
            .replay_events(ManagedSessionReplayRequest {
                schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
                request_id: String::from("replay"),
                session_ref: ManagedSessionRef {
                    probe_session_id: probe_protocol::session::SessionId::new(session_id),
                    managed_session_id: Some(String::from("managed-session-1")),
                    parent_probe_session_id: None,
                    child_probe_session_id: None,
                },
                after_sequence: 0,
                limit: None,
            })
            .expect("managed events should replay");
        assert_eq!(replay.events.len(), 2);
        assert!(temp.path().join("artifacts/evidence.json").exists());

        let duplicate = runner
            .run_once(ManagedCloudRunJobRunRequest {
                assignment_token: token,
                signing_secret: String::from("secret"),
                callback_bearer_token: Some(String::from("callback-token")),
                profile: test_profile(provider.base_url()),
                default_cwd: temp.path().to_path_buf(),
                artifact_dir: temp.path().join("artifacts"),
                cloud_run: test_cloud_run_identity(),
                system_prompt: None,
                harness_profile: None,
                tool_loop: None,
                dry_run: false,
            })
            .expect("duplicate should be safe");
        assert!(matches!(
            duplicate,
            ManagedCloudRunJobOutcome::DuplicateSkipped(_)
        ));
        assert_eq!(provider.recorded_requests().len(), 1);
    }

    fn test_claims(callback_url: &str) -> ManagedCloudRunJobAssignmentClaims {
        let mut claims = ManagedCloudRunJobAssignmentClaims::new(
            "assignment-1",
            "assignment-1:attempt-1",
            "managed-session-1",
            "managed-run-1",
            callback_url,
            "Return exactly: managed job complete",
            "managed-environment-1",
            "gcp-cloud-run-job",
        );
        claims.title = Some(String::from("Managed Cloud Run test"));
        claims.profile = Some(String::from("test-profile"));
        claims
    }

    fn test_cloud_run_identity() -> ManagedCloudRunJobIdentity {
        ManagedCloudRunJobIdentity {
            job_name: Some(String::from("probe-managed-job")),
            execution_name: Some(String::from("probe-managed-job-abcd")),
            task_index: Some(String::from("0")),
            task_attempt: Some(String::from("0")),
            logs_url: Some(String::from(
                "https://console.cloud.google.com/run/jobs/details",
            )),
        }
    }

    fn test_profile(base_url: &str) -> BackendProfile {
        BackendProfile {
            name: String::from("test-profile"),
            kind: BackendKind::OpenAiChatCompletions,
            base_url: base_url.to_string(),
            model: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            reasoning_level: None,
            service_tier: None,
            api_key_env: String::from("PROBE_MANAGED_CLOUD_RUN_TEST_KEY"),
            timeout_secs: 15,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
            control_plane: None,
            psionic_mesh: None,
        }
    }

    struct ScopedEnvVar {
        key: String,
    }

    impl ScopedEnvVar {
        fn set(key: &str, value: &str) -> Self {
            // SAFETY: this test uses a unique process-wide env key and removes
            // it when the guard drops.
            unsafe {
                std::env::set_var(key, value);
            }
            Self {
                key: key.to_string(),
            }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            // SAFETY: this removes only the unique key created by the guard.
            unsafe {
                std::env::remove_var(&self.key);
            }
        }
    }
}
