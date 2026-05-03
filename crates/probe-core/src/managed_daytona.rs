use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use probe_protocol::managed_environment::{
    ManagedEnvironmentCapabilities, ManagedEnvironmentConstraints, ManagedEnvironmentHostClass,
    ManagedEnvironmentProviderKind, ManagedEnvironmentPublicMetadata,
    ManagedEnvironmentResourceLimits, ManagedEnvironmentRuntimeRef,
    ManagedEnvironmentRuntimeRefKind, is_secret_like_key,
};
use probe_protocol::managed_runtime::{
    ManagedRuntimeActor, ManagedRuntimeArtifactKind, ManagedRuntimeArtifactRef,
    ManagedRuntimeCorrelation, ManagedRuntimeErrorPayload, ManagedRuntimeEventPayload,
    ManagedRuntimeEventType, ManagedRuntimeSessionStatus, ManagedRuntimeSource, ManagedSessionRef,
};
use probe_protocol::session::SessionId;
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::managed_runtime::{
    ManagedRuntimeController, ManagedRuntimeError, ManagedRuntimeEventDraft,
};

pub const PROBE_MANAGED_DAYTONA_SCHEMA_VERSION: &str = "probe.managed_daytona.v1";
pub const PROBE_MANAGED_DAYTONA_ASSIGNMENT_SCHEMA_VERSION: &str =
    "probe.managed_daytona.assignment.v1";

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedDaytonaEnvironmentAllocation {
    pub managed_environment_id: String,
    pub provider: ManagedEnvironmentProviderKind,
    pub host_class: ManagedEnvironmentHostClass,
    pub environment_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<ManagedEnvironmentConstraints>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedDaytonaAssignmentClaims {
    pub schema_version: String,
    pub assignment_id: String,
    pub idempotency_key: String,
    pub managed_session_id: String,
    pub managed_run_id: String,
    pub callback_url: String,
    pub goal_prompt: String,
    pub environment: ManagedDaytonaEnvironmentAllocation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_gcs_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

impl ManagedDaytonaAssignmentClaims {
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
            schema_version: String::from(PROBE_MANAGED_DAYTONA_ASSIGNMENT_SCHEMA_VERSION),
            assignment_id: assignment_id.into(),
            idempotency_key: idempotency_key.into(),
            managed_session_id: managed_session_id.into(),
            managed_run_id: managed_run_id.into(),
            callback_url: callback_url.into(),
            goal_prompt: goal_prompt.into(),
            environment: ManagedDaytonaEnvironmentAllocation {
                managed_environment_id: managed_environment_id.into(),
                provider: ManagedEnvironmentProviderKind::Daytona,
                host_class: ManagedEnvironmentHostClass::DaytonaWorkspace,
                environment_class: environment_class.into(),
                constraints: None,
            },
            title: None,
            profile: None,
            model: None,
            cwd: None,
            snapshot: None,
            target: None,
            sandbox_id: None,
            sandbox_name: None,
            bootstrap_command: None,
            artifact_gcs_prefix: None,
            issued_at_ms: None,
            expires_at_ms: None,
            metadata: Map::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ManagedDaytonaConfig {
    pub base_url: String,
    pub toolbox_base_url: String,
    pub api_key: String,
    pub organization_id: Option<String>,
    pub request_timeout_secs: u64,
}

impl ManagedDaytonaConfig {
    #[must_use]
    pub fn from_env() -> Option<Self> {
        Some(Self {
            base_url: env_nonempty("DAYTONA_BASE_URL")
                .or_else(|| env_nonempty("DAYTONA_API_URL"))
                .unwrap_or_else(|| String::from("https://app.daytona.io/api")),
            toolbox_base_url: env_nonempty("DAYTONA_TOOLBOX_BASE_URL")
                .unwrap_or_else(|| String::from("https://proxy.app.daytona.io/toolbox")),
            api_key: env_nonempty("DAYTONA_API_KEY")?,
            organization_id: env_nonempty("DAYTONA_ORGANIZATION_ID"),
            request_timeout_secs: env_nonempty("DAYTONA_REQUEST_TIMEOUT_SECS")
                .and_then(|value| value.parse().ok())
                .unwrap_or(30),
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedDaytonaSnapshotTemplate {
    pub worker_id: String,
    pub managed_environment_id: String,
    pub environment_class: String,
    pub snapshot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default)]
    pub resource_limits: ManagedEnvironmentResourceLimits,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backend_profiles: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default)]
    pub public_metadata: ManagedEnvironmentPublicMetadata,
}

impl ManagedDaytonaSnapshotTemplate {
    #[must_use]
    pub fn capabilities(&self) -> ManagedEnvironmentCapabilities {
        let mut capabilities = ManagedEnvironmentCapabilities::daytona_workspace(
            self.worker_id.clone(),
            self.environment_class.clone(),
        );
        capabilities.resource_limits = self.resource_limits.clone();
        capabilities.backend_profiles = self.backend_profiles.clone();
        capabilities.snapshot_ref = Some(ManagedEnvironmentRuntimeRef {
            kind: ManagedEnvironmentRuntimeRefKind::WorkspaceSnapshot,
            resource_ref: format!("daytona://snapshots/{}", self.snapshot),
            stable_digest: None,
            label: Some(String::from("Daytona snapshot")),
        });
        for label in &self.labels {
            if !capabilities.labels.iter().any(|existing| existing == label) {
                capabilities.labels.push(label.clone());
            }
        }
        let mut metadata = self.public_metadata.clone();
        metadata.insert("managedEnvironmentId", json!(self.managed_environment_id));
        metadata.insert("snapshot", json!(self.snapshot));
        if let Some(target) = self.target.as_ref() {
            metadata.insert("target", json!(target));
        }
        capabilities.public_metadata = metadata;
        capabilities
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedDaytonaAllocation {
    pub schema_version: String,
    pub allocation_id: String,
    pub sandbox_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub managed_environment_id: String,
    pub managed_session_id: String,
    pub managed_run_id: String,
    pub capabilities: ManagedEnvironmentCapabilities,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_refs: Vec<ManagedRuntimeArtifactRef>,
    pub allocated_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedDaytonaStatus {
    Started,
    Completed,
    Failed,
    DuplicateSkipped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedDaytonaState {
    pub schema_version: String,
    pub assignment_id: String,
    pub idempotency_key: String,
    pub status: ManagedDaytonaStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allocation: Option<ManagedDaytonaAllocation>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_error: Option<ManagedDaytonaProviderError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManagedDaytonaOutcome {
    Completed(ManagedDaytonaState),
    Failed(ManagedDaytonaState),
    DuplicateSkipped(ManagedDaytonaState),
}

impl ManagedDaytonaOutcome {
    #[must_use]
    pub fn state(&self) -> &ManagedDaytonaState {
        match self {
            Self::Completed(state) | Self::Failed(state) | Self::DuplicateSkipped(state) => state,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedDaytonaProviderErrorCode {
    NotConfigured,
    Unauthorized,
    Forbidden,
    SandboxNotFound,
    Timeout,
    ApiError,
    InvalidResponse,
    Network,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedDaytonaProviderError {
    pub code: ManagedDaytonaProviderErrorCode,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct ManagedDaytonaRunRequest {
    pub assignment_token: String,
    pub signing_secret: String,
    pub callback_bearer_token: Option<String>,
    pub artifact_dir: PathBuf,
    pub config: ManagedDaytonaConfig,
    pub default_snapshot: Option<String>,
    pub default_target: Option<String>,
    pub wait_timeout_ms: u64,
    pub delete_sandbox_on_finish: bool,
    pub dry_run: bool,
}

#[derive(Clone, Debug)]
pub struct ManagedDaytonaRunner {
    managed_runtime: ManagedRuntimeController,
}

impl ManagedDaytonaRunner {
    #[must_use]
    pub fn new(managed_runtime: ManagedRuntimeController) -> Self {
        Self { managed_runtime }
    }

    pub fn run_once(
        &self,
        request: ManagedDaytonaRunRequest,
    ) -> Result<ManagedDaytonaOutcome, ManagedDaytonaError> {
        let claims = verify_assignment_token(
            request.assignment_token.as_str(),
            request.signing_secret.as_str(),
            now_ms(),
        )?;
        validate_daytona_allocation(&claims)?;
        fs::create_dir_all(request.artifact_dir.as_path()).map_err(ManagedDaytonaError::Io)?;

        let state_path = state_path(&request.artifact_dir, claims.idempotency_key.as_str());
        if let Some(existing) = read_state_if_present(state_path.as_path())? {
            let mut duplicate = existing;
            duplicate.status = ManagedDaytonaStatus::DuplicateSkipped;
            post_callback(
                &claims,
                request.callback_bearer_token.as_deref(),
                "daytona.duplicate_skipped",
                &duplicate,
                None,
            )?;
            return Ok(ManagedDaytonaOutcome::DuplicateSkipped(duplicate));
        }

        let mut state = ManagedDaytonaState {
            schema_version: String::from(PROBE_MANAGED_DAYTONA_SCHEMA_VERSION),
            assignment_id: claims.assignment_id.clone(),
            idempotency_key: claims.idempotency_key.clone(),
            status: ManagedDaytonaStatus::Started,
            allocation: None,
            probe_session_id: None,
            terminal_event_sequence: None,
            artifact_refs: Vec::new(),
            started_at_ms: now_ms(),
            finished_at_ms: None,
            error: None,
            provider_error: None,
        };
        write_state(state_path.as_path(), &state)?;

        let adapter = ManagedDaytonaProviderAdapter::new(request.config.clone())?;
        let allocation = match adapter.allocate(&claims, &request) {
            Ok(allocation) => allocation,
            Err(error) => {
                return self.finish_failed(
                    &claims,
                    &request,
                    state,
                    None,
                    error,
                    state_path.as_path(),
                );
            }
        };
        state.allocation = Some(allocation.clone());
        write_state(state_path.as_path(), &state)?;
        post_callback(
            &claims,
            request.callback_bearer_token.as_deref(),
            "daytona.started",
            &state,
            None,
        )?;

        let execution = if request.dry_run {
            Ok(ManagedDaytonaExecutionResult {
                session_id: synthetic_session_id(&claims, &allocation),
                command: String::from("dry-run"),
                exit_code: 0,
                output: String::from("dry-run completed without Daytona command execution"),
            })
        } else {
            let command = claims
                .bootstrap_command
                .clone()
                .unwrap_or_else(|| default_probe_bootstrap_command(&claims));
            adapter.execute_probe_bootstrap(&allocation, &claims, command)
        };

        match execution {
            Ok(execution) if execution.exit_code == 0 => {
                state.probe_session_id = Some(execution.session_id.as_str().to_string());
                let artifacts = write_evidence_artifact(
                    request.artifact_dir.as_path(),
                    &claims,
                    &state,
                    &execution,
                    None,
                )?;
                state.artifact_refs = artifacts.clone();
                let terminal_event = self.record_terminal_event(
                    &claims,
                    &state,
                    execution.session_id,
                    ManagedRuntimeSessionStatus::Completed,
                    Some("Daytona managed assignment completed"),
                    artifacts,
                    None,
                )?;
                state.status = ManagedDaytonaStatus::Completed;
                state.terminal_event_sequence = Some(terminal_event.sequence);
                state.finished_at_ms = Some(now_ms());
                write_state(state_path.as_path(), &state)?;
                post_callback(
                    &claims,
                    request.callback_bearer_token.as_deref(),
                    "daytona.completed",
                    &state,
                    None,
                )?;
                if request.delete_sandbox_on_finish && claims.sandbox_id.is_none() {
                    let _ = adapter.delete_sandbox(allocation.sandbox_id.as_str());
                }
                Ok(ManagedDaytonaOutcome::Completed(state))
            }
            Ok(execution) => self.finish_failed(
                &claims,
                &request,
                state,
                Some(execution),
                ManagedDaytonaError::Provider(ManagedDaytonaProviderError {
                    code: ManagedDaytonaProviderErrorCode::ApiError,
                    message: String::from("Daytona bootstrap command exited non-zero"),
                    retryable: false,
                    status: None,
                }),
                state_path.as_path(),
            ),
            Err(error) => {
                self.finish_failed(&claims, &request, state, None, error, state_path.as_path())
            }
        }
    }

    fn finish_failed(
        &self,
        claims: &ManagedDaytonaAssignmentClaims,
        request: &ManagedDaytonaRunRequest,
        mut state: ManagedDaytonaState,
        execution: Option<ManagedDaytonaExecutionResult>,
        error: ManagedDaytonaError,
        state_path: &Path,
    ) -> Result<ManagedDaytonaOutcome, ManagedDaytonaError> {
        let error_text = error.to_string();
        state.provider_error = error.provider_error().cloned();
        let session_id = execution
            .as_ref()
            .map(|execution| execution.session_id.clone())
            .or_else(|| {
                state
                    .allocation
                    .as_ref()
                    .map(|allocation| synthetic_session_id(claims, allocation))
            });
        state.probe_session_id = session_id.as_ref().map(|id| id.as_str().to_string());
        let fallback_execution =
            state
                .allocation
                .as_ref()
                .map(|allocation| ManagedDaytonaExecutionResult {
                    session_id: synthetic_session_id(claims, allocation),
                    command: String::from("daytona allocation"),
                    exit_code: 1,
                    output: String::new(),
                });
        let artifacts = write_evidence_artifact(
            request.artifact_dir.as_path(),
            claims,
            &state,
            execution
                .as_ref()
                .or(fallback_execution.as_ref())
                .unwrap_or(&ManagedDaytonaExecutionResult {
                    session_id: SessionId::new(format!("daytona:{}", claims.assignment_id)),
                    command: String::from("daytona assignment"),
                    exit_code: 1,
                    output: String::new(),
                }),
            Some(error_text.as_str()),
        )?;
        state.artifact_refs = artifacts.clone();
        if let Some(session_id) = session_id {
            let terminal_event = self.record_terminal_event(
                claims,
                &state,
                session_id,
                ManagedRuntimeSessionStatus::Failed,
                Some(error_text.as_str()),
                artifacts,
                Some(error_text.as_str()),
            )?;
            state.terminal_event_sequence = Some(terminal_event.sequence);
        }
        state.status = ManagedDaytonaStatus::Failed;
        state.finished_at_ms = Some(now_ms());
        state.error = Some(error_text.clone());
        write_state(state_path, &state)?;
        post_callback(
            claims,
            request.callback_bearer_token.as_deref(),
            "daytona.failed",
            &state,
            Some(error_text.as_str()),
        )?;
        Ok(ManagedDaytonaOutcome::Failed(state))
    }

    fn record_terminal_event(
        &self,
        claims: &ManagedDaytonaAssignmentClaims,
        state: &ManagedDaytonaState,
        session_id: SessionId,
        status: ManagedRuntimeSessionStatus,
        reason: Option<&str>,
        artifact_refs: Vec<ManagedRuntimeArtifactRef>,
        error: Option<&str>,
    ) -> Result<probe_protocol::managed_runtime::ManagedRuntimeEvent, ManagedDaytonaError> {
        let allocation = state.allocation.as_ref();
        let sandbox_id = allocation
            .map(|allocation| allocation.sandbox_id.clone())
            .unwrap_or_else(|| String::from("unallocated"));
        let actor = ManagedRuntimeActor {
            kind: String::from("daytona_workspace"),
            id: Some(sandbox_id.clone()),
            label: allocation
                .and_then(|allocation| allocation.snapshot.clone())
                .or_else(|| Some(String::from("Daytona workspace"))),
        };
        let correlation = ManagedRuntimeCorrelation {
            request_id: Some(claims.assignment_id.clone()),
            workspace: Some(String::from("openagents.com")),
            managed_environment_id: Some(claims.environment.managed_environment_id.clone()),
            managed_session_id: Some(claims.managed_session_id.clone()),
            managed_run_id: Some(claims.managed_run_id.clone()),
            ..ManagedRuntimeCorrelation::default()
        };
        let session = ManagedSessionRef {
            probe_session_id: session_id.clone(),
            managed_session_id: Some(claims.managed_session_id.clone()),
            parent_probe_session_id: None,
            child_probe_session_id: None,
        };
        let source = ManagedRuntimeSource {
            kind: String::from("daytona_workspace"),
            id: Some(sandbox_id.clone()),
            label: Some(String::from("Daytona managed provider")),
        };

        if self
            .managed_runtime
            .replay_events(
                probe_protocol::managed_runtime::ManagedSessionReplayRequest {
                    schema_version: String::from(
                        probe_protocol::managed_runtime::PROBE_MANAGED_RUNTIME_SCHEMA_VERSION,
                    ),
                    request_id: format!("{}:daytona-replay-before-terminal", claims.assignment_id),
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
                    source: source.clone(),
                    session: session.clone(),
                    correlation: correlation.clone(),
                    artifact_refs: daytona_transcript_refs(&session_id, allocation),
                    payload: ManagedRuntimeEventPayload::SessionLifecycle {
                        title: claims
                            .title
                            .clone()
                            .unwrap_or_else(|| claims.assignment_id.clone()),
                        cwd: claims
                            .cwd
                            .clone()
                            .unwrap_or_else(|| PathBuf::from("/workspace")),
                        backend_profile: claims
                            .profile
                            .clone()
                            .unwrap_or_else(|| String::from("daytona-bootstrap")),
                        model: claims
                            .model
                            .clone()
                            .unwrap_or_else(|| String::from("daytona-bootstrap")),
                        environment_constraints: claims.environment.constraints.clone(),
                    },
                })?;
        }

        let mut refs = artifact_refs;
        refs.extend(daytona_transcript_refs(&session_id, allocation));
        self.managed_runtime
            .append_event(ManagedRuntimeEventDraft {
                event_type: if status == ManagedRuntimeSessionStatus::Completed {
                    ManagedRuntimeEventType::SessionCompleted
                } else {
                    ManagedRuntimeEventType::SessionFailed
                },
                status,
                actor,
                source,
                session,
                correlation,
                artifact_refs: refs,
                payload: if let Some(error) = error {
                    ManagedRuntimeEventPayload::Error {
                        error: ManagedRuntimeErrorPayload {
                            code: String::from("managed_daytona_failed"),
                            message: error.to_string(),
                            retryable: state
                                .provider_error
                                .as_ref()
                                .is_some_and(|provider_error| provider_error.retryable),
                            details: state
                                .provider_error
                                .as_ref()
                                .and_then(|provider_error| {
                                    serde_json::to_value(provider_error).ok()
                                })
                                .and_then(|value| value.as_object().cloned())
                                .unwrap_or_default(),
                        },
                    }
                } else {
                    ManagedRuntimeEventPayload::Terminal {
                        status,
                        reason: reason.map(str::to_string),
                    }
                },
            })
            .map_err(ManagedDaytonaError::ManagedRuntime)
    }
}

#[derive(Clone, Debug)]
pub struct ManagedDaytonaProviderAdapter {
    client: Client,
    config: ManagedDaytonaConfig,
}

impl ManagedDaytonaProviderAdapter {
    pub fn new(config: ManagedDaytonaConfig) -> Result<Self, ManagedDaytonaError> {
        if config.api_key.trim().is_empty() {
            return Err(ManagedDaytonaError::Provider(ManagedDaytonaProviderError {
                code: ManagedDaytonaProviderErrorCode::NotConfigured,
                message: String::from("DAYTONA_API_KEY is required for Daytona provider access"),
                retryable: false,
                status: None,
            }));
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .build()
            .map_err(ManagedDaytonaError::Http)?;
        Ok(Self { client, config })
    }

    #[must_use]
    pub fn capabilities_for_snapshot(
        template: ManagedDaytonaSnapshotTemplate,
    ) -> ManagedEnvironmentCapabilities {
        template.capabilities()
    }

    pub fn allocate(
        &self,
        claims: &ManagedDaytonaAssignmentClaims,
        request: &ManagedDaytonaRunRequest,
    ) -> Result<ManagedDaytonaAllocation, ManagedDaytonaError> {
        let sandbox = if let Some(sandbox_id) = claims.sandbox_id.as_ref() {
            self.get_sandbox(sandbox_id)?
        } else {
            let sandbox = self.create_sandbox(claims, request)?;
            if request.wait_timeout_ms > 0 {
                self.wait_until_started(sandbox.id.as_str(), request.wait_timeout_ms)?
            } else {
                sandbox
            }
        };
        Ok(allocation_from_sandbox(claims, &sandbox, request))
    }

    pub fn execute_probe_bootstrap(
        &self,
        allocation: &ManagedDaytonaAllocation,
        claims: &ManagedDaytonaAssignmentClaims,
        command: String,
    ) -> Result<ManagedDaytonaExecutionResult, ManagedDaytonaError> {
        let response = self.execute_command(
            allocation.sandbox_id.as_str(),
            &DaytonaExecuteRequest {
                command: command.clone(),
                cwd: claims
                    .cwd
                    .as_ref()
                    .map(|cwd| cwd.to_string_lossy().to_string()),
                timeout: None,
            },
        )?;
        let session_id = parse_probe_session_id(response.result.as_str())
            .unwrap_or_else(|| synthetic_session_id(claims, allocation));
        Ok(ManagedDaytonaExecutionResult {
            session_id,
            command,
            exit_code: response.exit_code,
            output: response.result,
        })
    }

    pub fn get_sandbox(&self, sandbox_id: &str) -> Result<DaytonaSandbox, ManagedDaytonaError> {
        self.get_json(
            api_url(
                self.config.base_url.as_str(),
                format!("/sandbox/{sandbox_id}").as_str(),
            )
            .as_str(),
            "get Daytona sandbox",
        )
    }

    pub fn delete_sandbox(&self, sandbox_id: &str) -> Result<(), ManagedDaytonaError> {
        let response = self
            .client
            .delete(api_url(
                self.config.base_url.as_str(),
                format!("/sandbox/{sandbox_id}").as_str(),
            ))
            .headers(self.auth_headers())
            .send()
            .map_err(ManagedDaytonaError::Http)?;
        decode_empty_response(response, "delete Daytona sandbox")
    }

    fn create_sandbox(
        &self,
        claims: &ManagedDaytonaAssignmentClaims,
        request: &ManagedDaytonaRunRequest,
    ) -> Result<DaytonaSandbox, ManagedDaytonaError> {
        let snapshot = claims
            .snapshot
            .clone()
            .or_else(|| request.default_snapshot.clone());
        let target = claims
            .target
            .clone()
            .or_else(|| request.default_target.clone());
        let create = DaytonaCreateSandboxRequest {
            snapshot,
            name: claims.sandbox_name.clone().or_else(|| {
                Some(format!(
                    "probe-{}",
                    short_digest(claims.assignment_id.as_bytes())
                ))
            }),
            target,
            labels: daytona_labels(claims),
            public: Some(false),
            auto_stop_interval: None,
            auto_archive_interval: None,
            resources: daytona_resources(claims.environment.constraints.as_ref()),
        };
        self.post_json(
            api_url(self.config.base_url.as_str(), "/sandbox").as_str(),
            &create,
            "create Daytona sandbox",
        )
    }

    fn wait_until_started(
        &self,
        sandbox_id: &str,
        timeout_ms: u64,
    ) -> Result<DaytonaSandbox, ManagedDaytonaError> {
        let start = Instant::now();
        let timeout = Duration::from_millis(timeout_ms);
        loop {
            let sandbox = self.get_sandbox(sandbox_id)?;
            if is_daytona_started(sandbox.state.as_deref()) {
                return Ok(sandbox);
            }
            if is_daytona_terminal_error(sandbox.state.as_deref()) {
                return Err(ManagedDaytonaError::Provider(ManagedDaytonaProviderError {
                    code: ManagedDaytonaProviderErrorCode::ApiError,
                    message: sandbox.error_reason.unwrap_or_else(|| {
                        format!(
                            "Daytona sandbox `{sandbox_id}` reached state `{}`",
                            sandbox.state.unwrap_or_else(|| String::from("unknown"))
                        )
                    }),
                    retryable: false,
                    status: None,
                }));
            }
            if start.elapsed() >= timeout {
                return Err(ManagedDaytonaError::Provider(ManagedDaytonaProviderError {
                    code: ManagedDaytonaProviderErrorCode::Timeout,
                    message: format!("timed out waiting for Daytona sandbox `{sandbox_id}`"),
                    retryable: true,
                    status: None,
                }));
            }
            thread::sleep(Duration::from_millis(1_000));
        }
    }

    fn execute_command(
        &self,
        sandbox_id: &str,
        request: &DaytonaExecuteRequest,
    ) -> Result<DaytonaExecuteResponse, ManagedDaytonaError> {
        self.post_json(
            api_url(
                self.config.toolbox_base_url.as_str(),
                format!("/{sandbox_id}/process/execute").as_str(),
            )
            .as_str(),
            request,
            "execute Daytona command",
        )
    }

    fn get_json<T: DeserializeOwned>(
        &self,
        url: &str,
        operation: &'static str,
    ) -> Result<T, ManagedDaytonaError> {
        let response = self
            .client
            .get(url)
            .headers(self.auth_headers())
            .send()
            .map_err(ManagedDaytonaError::Http)?;
        decode_json_response(response, operation)
    }

    fn post_json<T: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        body: &T,
        operation: &'static str,
    ) -> Result<R, ManagedDaytonaError> {
        let response = self
            .client
            .post(url)
            .headers(self.auth_headers())
            .json(body)
            .send()
            .map_err(ManagedDaytonaError::Http)?;
        decode_json_response(response, operation)
    }

    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        let auth = format!("Bearer {}", self.config.api_key);
        if let Ok(value) = reqwest::header::HeaderValue::from_str(auth.as_str()) {
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        if let Some(organization_id) = self.config.organization_id.as_ref()
            && let Ok(value) = reqwest::header::HeaderValue::from_str(organization_id)
        {
            headers.insert("x-daytona-organization-id", value);
        }
        headers
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaytonaSandbox {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolbox_proxy_url: Option<String>,
    #[serde(default, deserialize_with = "deserialize_labels")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaytonaCreateSandboxRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_stop_interval: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_archive_interval: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<DaytonaResources>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaytonaResources {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaytonaExecuteRequest {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaytonaExecuteResponse {
    #[serde(default, alias = "code")]
    pub exit_code: i32,
    #[serde(default)]
    pub result: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedDaytonaExecutionResult {
    pub session_id: SessionId,
    pub command: String,
    pub exit_code: i32,
    pub output: String,
}

#[derive(Debug)]
pub enum ManagedDaytonaError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Http(reqwest::Error),
    Token(String),
    InvalidAssignment(String),
    Provider(ManagedDaytonaProviderError),
    ManagedRuntime(ManagedRuntimeError),
    Callback(reqwest::Error),
    CallbackStatus { status: u16, body: String },
}

impl ManagedDaytonaError {
    #[must_use]
    pub fn provider_error(&self) -> Option<&ManagedDaytonaProviderError> {
        match self {
            Self::Provider(error) => Some(error),
            _ => None,
        }
    }
}

impl Display for ManagedDaytonaError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::Http(error) => write!(f, "http error: {error}"),
            Self::Token(message) | Self::InvalidAssignment(message) => f.write_str(message),
            Self::Provider(error) => {
                write!(
                    f,
                    "daytona provider error {:?}: {}",
                    error.code, error.message
                )
            }
            Self::ManagedRuntime(error) => write!(f, "managed runtime error: {error}"),
            Self::Callback(error) => write!(f, "callback error: {error}"),
            Self::CallbackStatus { status, body } => {
                write!(f, "callback returned HTTP {status}: {body}")
            }
        }
    }
}

impl std::error::Error for ManagedDaytonaError {}

impl From<std::io::Error> for ManagedDaytonaError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ManagedDaytonaError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<ManagedRuntimeError> for ManagedDaytonaError {
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
    claims: &ManagedDaytonaAssignmentClaims,
    signing_secret: &str,
) -> Result<String, ManagedDaytonaError> {
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims)?);
    let signature = hmac_signature(payload.as_bytes(), signing_secret)?;
    Ok(format!("{payload}.{signature}"))
}

pub fn verify_assignment_token(
    token: &str,
    signing_secret: &str,
    now_ms: u64,
) -> Result<ManagedDaytonaAssignmentClaims, ManagedDaytonaError> {
    let (payload, signature) = token
        .split_once('.')
        .ok_or_else(|| ManagedDaytonaError::Token(String::from("assignment token is malformed")))?;
    let expected = hmac_signature(payload.as_bytes(), signing_secret)?;
    if !constant_time_eq(signature.as_bytes(), expected.as_bytes()) {
        return Err(ManagedDaytonaError::Token(String::from(
            "assignment token signature is invalid",
        )));
    }
    let claims: ManagedDaytonaAssignmentClaims = serde_json::from_slice(
        URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|error| ManagedDaytonaError::Token(error.to_string()))?
            .as_slice(),
    )?;
    if claims.schema_version != PROBE_MANAGED_DAYTONA_ASSIGNMENT_SCHEMA_VERSION {
        return Err(ManagedDaytonaError::InvalidAssignment(format!(
            "unsupported Daytona assignment schema version `{}`",
            claims.schema_version
        )));
    }
    if let Some(expires_at_ms) = claims.expires_at_ms
        && expires_at_ms <= now_ms
    {
        return Err(ManagedDaytonaError::Token(String::from(
            "assignment token is expired",
        )));
    }
    validate_daytona_allocation(&claims)?;
    Ok(claims)
}

pub fn validate_daytona_allocation(
    claims: &ManagedDaytonaAssignmentClaims,
) -> Result<(), ManagedDaytonaError> {
    if claims.environment.provider != ManagedEnvironmentProviderKind::Daytona {
        return Err(ManagedDaytonaError::InvalidAssignment(String::from(
            "Daytona runner only accepts daytona assignments",
        )));
    }
    if claims.environment.host_class != ManagedEnvironmentHostClass::DaytonaWorkspace {
        return Err(ManagedDaytonaError::InvalidAssignment(String::from(
            "Daytona runner only accepts daytona_workspace host allocations",
        )));
    }
    if let Some(constraints) = claims.environment.constraints.as_ref() {
        if !constraints.allowed_providers.is_empty()
            && !constraints
                .allowed_providers
                .contains(&ManagedEnvironmentProviderKind::Daytona)
        {
            return Err(ManagedDaytonaError::InvalidAssignment(String::from(
                "assignment constraints do not allow daytona",
            )));
        }
        if !constraints.allowed_host_classes.is_empty()
            && !constraints
                .allowed_host_classes
                .contains(&ManagedEnvironmentHostClass::DaytonaWorkspace)
        {
            return Err(ManagedDaytonaError::InvalidAssignment(String::from(
                "assignment constraints do not allow daytona_workspace",
            )));
        }
    }
    Ok(())
}

#[must_use]
pub fn default_probe_bootstrap_command(claims: &ManagedDaytonaAssignmentClaims) -> String {
    let profile = claims
        .profile
        .clone()
        .unwrap_or_else(|| String::from("openai-codex-subscription"));
    let cwd = claims
        .cwd
        .as_ref()
        .map(|cwd| cwd.to_string_lossy().to_string())
        .unwrap_or_else(|| String::from("/workspace"));
    let title = claims
        .title
        .clone()
        .unwrap_or_else(|| format!("Managed Daytona: {}", claims.assignment_id));
    format!(
        "probe exec --profile {} --cwd {} --title {} -- {} 2>&1",
        shell_quote(profile.as_str()),
        shell_quote(cwd.as_str()),
        shell_quote(title.as_str()),
        shell_quote(claims.goal_prompt.as_str())
    )
}

fn allocation_from_sandbox(
    claims: &ManagedDaytonaAssignmentClaims,
    sandbox: &DaytonaSandbox,
    request: &ManagedDaytonaRunRequest,
) -> ManagedDaytonaAllocation {
    let snapshot = sandbox
        .snapshot
        .clone()
        .or_else(|| claims.snapshot.clone())
        .or_else(|| request.default_snapshot.clone());
    let target = sandbox
        .target
        .clone()
        .or_else(|| claims.target.clone())
        .or_else(|| request.default_target.clone());
    let worker_id = format!("daytona:{}", sandbox.id);
    let mut template = ManagedDaytonaSnapshotTemplate {
        worker_id,
        managed_environment_id: claims.environment.managed_environment_id.clone(),
        environment_class: claims.environment.environment_class.clone(),
        snapshot: snapshot
            .clone()
            .unwrap_or_else(|| String::from("daytona-default")),
        target: target.clone(),
        resource_limits: ManagedEnvironmentResourceLimits {
            cpu_millicores: sandbox.cpu.map(|cpu| cpu.saturating_mul(1_000)),
            memory_mib: sandbox
                .memory
                .map(|memory_gib| memory_gib.saturating_mul(1_024)),
            disk_mib: sandbox.disk.map(|disk_gib| disk_gib.saturating_mul(1_024)),
            gpu_count: sandbox.gpu,
        },
        backend_profiles: claims.profile.clone().into_iter().collect(),
        labels: vec![String::from("supplemental")],
        public_metadata: ManagedEnvironmentPublicMetadata::default(),
    };
    template
        .public_metadata
        .insert("sandboxId", json!(sandbox.id.as_str()));
    template.public_metadata.insert(
        "sandboxState",
        json!(
            sandbox
                .state
                .clone()
                .unwrap_or_else(|| String::from("unknown"))
        ),
    );
    let capabilities = ManagedDaytonaProviderAdapter::capabilities_for_snapshot(template);
    ManagedDaytonaAllocation {
        schema_version: String::from(PROBE_MANAGED_DAYTONA_SCHEMA_VERSION),
        allocation_id: format!("daytona:{}:{}", sandbox.id, claims.assignment_id),
        sandbox_id: sandbox.id.clone(),
        sandbox_name: sandbox.name.clone(),
        sandbox_state: sandbox.state.clone(),
        snapshot,
        target,
        managed_environment_id: claims.environment.managed_environment_id.clone(),
        managed_session_id: claims.managed_session_id.clone(),
        managed_run_id: claims.managed_run_id.clone(),
        capabilities,
        resource_refs: vec![ManagedRuntimeArtifactRef {
            kind: ManagedRuntimeArtifactKind::WorkspaceSnapshot,
            resource_ref: format!("daytona://sandboxes/{}", sandbox.id),
            stable_digest: None,
            label: Some(String::from("Daytona sandbox")),
            updated_at_ms: Some(now_ms()),
        }],
        allocated_at_ms: now_ms(),
    }
}

fn write_evidence_artifact(
    artifact_dir: &Path,
    claims: &ManagedDaytonaAssignmentClaims,
    state: &ManagedDaytonaState,
    execution: &ManagedDaytonaExecutionResult,
    error: Option<&str>,
) -> Result<Vec<ManagedRuntimeArtifactRef>, ManagedDaytonaError> {
    fs::create_dir_all(artifact_dir)?;
    let evidence = json!({
        "schemaVersion": PROBE_MANAGED_DAYTONA_SCHEMA_VERSION,
        "assignmentId": claims.assignment_id,
        "managedSessionId": claims.managed_session_id,
        "managedRunId": claims.managed_run_id,
        "managedEnvironmentId": claims.environment.managed_environment_id,
        "allocation": state.allocation,
        "status": state.status,
        "probeSessionId": state.probe_session_id,
        "command": execution.command,
        "exitCode": execution.exit_code,
        "output": truncate_for_evidence(execution.output.as_str()),
        "error": error,
        "providerError": state.provider_error,
        "finishedAtMs": now_ms(),
    });
    let path = artifact_dir.join("daytona-evidence.json");
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
        .map(|prefix| format!("{}/daytona-evidence.json", prefix.trim_end_matches('/')))
        .unwrap_or(local_ref);
    Ok(vec![ManagedRuntimeArtifactRef {
        kind: ManagedRuntimeArtifactKind::VerificationPack,
        resource_ref,
        stable_digest: Some(short_digest(serde_json::to_vec(&evidence)?.as_slice())),
        label: Some(String::from("Managed Daytona evidence")),
        updated_at_ms: Some(now_ms()),
    }])
}

fn post_callback(
    claims: &ManagedDaytonaAssignmentClaims,
    callback_bearer_token: Option<&str>,
    event_type: &str,
    state: &ManagedDaytonaState,
    error: Option<&str>,
) -> Result<(), ManagedDaytonaError> {
    if claims.callback_url.trim().is_empty() {
        return Ok(());
    }
    let payload = json!({
        "schemaVersion": PROBE_MANAGED_DAYTONA_SCHEMA_VERSION,
        "eventType": event_type,
        "assignmentId": claims.assignment_id,
        "managedSessionId": claims.managed_session_id,
        "managedRunId": claims.managed_run_id,
        "idempotencyKey": claims.idempotency_key,
        "status": state.status,
        "allocation": state.allocation,
        "probeSessionId": state.probe_session_id,
        "terminalEventSequence": state.terminal_event_sequence,
        "artifactRefs": state.artifact_refs,
        "error": error,
        "providerError": state.provider_error,
        "occurredAtMs": now_ms(),
    });
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(ManagedDaytonaError::Callback)?;
    let mut request = client.post(claims.callback_url.as_str()).json(&payload);
    if let Some(token) = callback_bearer_token {
        request = request.bearer_auth(token);
    }
    let response = request.send().map_err(ManagedDaytonaError::Callback)?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().unwrap_or_default();
    Err(ManagedDaytonaError::CallbackStatus {
        status: status.as_u16(),
        body,
    })
}

fn decode_json_response<T: DeserializeOwned>(
    response: Response,
    operation: &'static str,
) -> Result<T, ManagedDaytonaError> {
    let status = response.status();
    let body = response.text().map_err(ManagedDaytonaError::Http)?;
    if !status.is_success() {
        return Err(ManagedDaytonaError::Provider(provider_error_from_status(
            status, body, operation,
        )));
    }
    if body.trim().is_empty() {
        return serde_json::from_value(Value::Object(Map::new()))
            .map_err(ManagedDaytonaError::Json);
    }
    serde_json::from_str(body.as_str()).map_err(|error| {
        ManagedDaytonaError::Provider(ManagedDaytonaProviderError {
            code: ManagedDaytonaProviderErrorCode::InvalidResponse,
            message: format!("{operation} returned invalid JSON: {error}"),
            retryable: false,
            status: Some(status.as_u16()),
        })
    })
}

fn decode_empty_response(
    response: Response,
    operation: &'static str,
) -> Result<(), ManagedDaytonaError> {
    let status = response.status();
    if status.is_success() || status == StatusCode::NO_CONTENT {
        return Ok(());
    }
    let body = response.text().unwrap_or_default();
    Err(ManagedDaytonaError::Provider(provider_error_from_status(
        status, body, operation,
    )))
}

fn provider_error_from_status(
    status: StatusCode,
    body: String,
    operation: &'static str,
) -> ManagedDaytonaProviderError {
    let code = match status {
        StatusCode::UNAUTHORIZED => ManagedDaytonaProviderErrorCode::Unauthorized,
        StatusCode::FORBIDDEN => ManagedDaytonaProviderErrorCode::Forbidden,
        StatusCode::NOT_FOUND => ManagedDaytonaProviderErrorCode::SandboxNotFound,
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => {
            ManagedDaytonaProviderErrorCode::Timeout
        }
        _ => ManagedDaytonaProviderErrorCode::ApiError,
    };
    ManagedDaytonaProviderError {
        code,
        message: format!("{operation} returned HTTP {}: {}", status.as_u16(), body),
        retryable: status.is_server_error()
            || matches!(
                status,
                StatusCode::REQUEST_TIMEOUT
                    | StatusCode::TOO_MANY_REQUESTS
                    | StatusCode::GATEWAY_TIMEOUT
            ),
        status: Some(status.as_u16()),
    }
}

fn daytona_labels(claims: &ManagedDaytonaAssignmentClaims) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    labels.insert(
        String::from("managed_session_id"),
        claims.managed_session_id.clone(),
    );
    labels.insert(
        String::from("managed_run_id"),
        claims.managed_run_id.clone(),
    );
    labels.insert(String::from("assignment_id"), claims.assignment_id.clone());
    labels.insert(
        String::from("managed_environment_id"),
        claims.environment.managed_environment_id.clone(),
    );
    for (key, value) in &claims.metadata {
        if is_secret_like_key(key) {
            continue;
        }
        if let Some(value) = value.as_str()
            && value.len() <= 512
        {
            labels.insert(key.clone(), value.to_string());
        }
    }
    labels
}

fn daytona_resources(
    constraints: Option<&ManagedEnvironmentConstraints>,
) -> Option<DaytonaResources> {
    let constraints = constraints?;
    if constraints.min_resources == ManagedEnvironmentResourceLimits::default() {
        return None;
    }
    Some(DaytonaResources {
        cpu: constraints
            .min_resources
            .cpu_millicores
            .map(|millicores| millicores.div_ceil(1_000)),
        memory: constraints
            .min_resources
            .memory_mib
            .map(|mib| mib.div_ceil(1_024)),
        disk: constraints
            .min_resources
            .disk_mib
            .map(|mib| mib.div_ceil(1_024)),
        gpu: constraints.min_resources.gpu_count,
    })
}

fn read_state_if_present(path: &Path) -> Result<Option<ManagedDaytonaState>, ManagedDaytonaError> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
}

fn write_state(path: &Path, state: &ManagedDaytonaState) -> Result<(), ManagedDaytonaError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = path.with_extension(format!("json.tmp-{}", std::process::id()));
    {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(temp_path.as_path())?;
        serde_json::to_writer_pretty(&mut file, state)?;
        file.write_all(b"\n")?;
        file.flush()?;
    }
    fs::rename(temp_path, path)?;
    Ok(())
}

fn hmac_signature(payload: &[u8], signing_secret: &str) -> Result<String, ManagedDaytonaError> {
    let mut mac = HmacSha256::new_from_slice(signing_secret.as_bytes())
        .map_err(|error| ManagedDaytonaError::Token(error.to_string()))?;
    mac.update(payload);
    Ok(hex::encode(mac.finalize().into_bytes()))
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

fn parse_probe_session_id(output: &str) -> Option<SessionId> {
    let value = output
        .find("session=")
        .map(|index| &output[index + "session=".len()..])?;
    let value = value
        .split_whitespace()
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(SessionId::new(value.to_string()))
}

fn synthetic_session_id(
    claims: &ManagedDaytonaAssignmentClaims,
    allocation: &ManagedDaytonaAllocation,
) -> SessionId {
    SessionId::new(format!(
        "daytona:{}:{}",
        allocation.sandbox_id, claims.assignment_id
    ))
}

fn daytona_transcript_refs(
    session_id: &SessionId,
    allocation: Option<&ManagedDaytonaAllocation>,
) -> Vec<ManagedRuntimeArtifactRef> {
    allocation
        .map(|allocation| ManagedRuntimeArtifactRef {
            kind: ManagedRuntimeArtifactKind::Transcript,
            resource_ref: format!(
                "daytona://sandboxes/{}/probe-sessions/{}/transcript",
                allocation.sandbox_id,
                session_id.as_str()
            ),
            stable_digest: None,
            label: Some(String::from("Daytona Probe transcript")),
            updated_at_ms: Some(now_ms()),
        })
        .into_iter()
        .collect()
}

fn api_url(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn is_daytona_started(state: Option<&str>) -> bool {
    state.is_some_and(|state| {
        let state = state.to_ascii_lowercase();
        state == "started" || state == "running"
    })
}

fn is_daytona_terminal_error(state: Option<&str>) -> bool {
    state.is_some_and(|state| {
        let state = state.to_ascii_lowercase();
        state == "error" || state == "build_failed" || state == "failed"
    })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn truncate_for_evidence(value: &str) -> String {
    const MAX: usize = 20_000;
    if value.len() <= MAX {
        return value.to_string();
    }
    let boundary = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= MAX)
        .last()
        .unwrap_or(0);
    format!("{}...[truncated]", &value[..boundary])
}

fn short_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    hex::encode(&digest[..8])
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_millis() as u64
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn deserialize_labels<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<BTreeMap<String, String>>::deserialize(deserializer)?;
    Ok(value.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use probe_protocol::managed_environment::{
        ManagedEnvironmentConstraints, ManagedEnvironmentHostClass, ManagedEnvironmentProviderKind,
        ManagedEnvironmentResourceLimits,
    };
    use probe_test_support::{FakeHttpResponse, FakeOpenAiServer};
    use serde_json::json;
    use tempfile::tempdir;

    use crate::managed_runtime::ManagedRuntimeController;
    use crate::session_store::FilesystemSessionStore;

    use super::{
        ManagedDaytonaAssignmentClaims, ManagedDaytonaConfig, ManagedDaytonaOutcome,
        ManagedDaytonaRunRequest, ManagedDaytonaRunner, sign_assignment_token,
        verify_assignment_token,
    };

    #[test]
    fn assignment_token_round_trips_and_rejects_wrong_provider() {
        let claims = test_claims("https://openagents.test/callback");
        let token = sign_assignment_token(&claims, "secret").expect("token");
        let decoded = verify_assignment_token(&token, "secret", 1).expect("decode token");
        assert_eq!(decoded.assignment_id, "assignment-1");

        let mut wrong_provider = claims;
        wrong_provider.environment.provider = ManagedEnvironmentProviderKind::GoogleCloud;
        let token = sign_assignment_token(&wrong_provider, "secret").expect("token");
        let error = verify_assignment_token(&token, "secret", 1).expect_err("reject provider");
        assert!(error.to_string().contains("daytona assignments"));
    }

    #[test]
    fn runner_creates_sandbox_executes_bootstrap_and_reports_callbacks() {
        let callback_bodies = Arc::new(Mutex::new(Vec::<String>::new()));
        let callback_bodies_server = Arc::clone(&callback_bodies);
        let daytona = FakeOpenAiServer::from_handler(move |request| {
            if request.path == "/v1/sandbox" && request.method == "POST" {
                assert!(request.body.contains("managed_session_id"));
                assert!(!request.body.contains("secret-token"));
                return FakeHttpResponse::json_ok(json!({
                    "id": "sandbox-1",
                    "name": "probe-assignment",
                    "state": "started",
                    "snapshot": "probe-managed-agent",
                    "target": "us",
                    "cpu": 2,
                    "memory": 4,
                    "disk": 20
                }));
            }
            if request.path == "/v1/sandbox/sandbox-1" && request.method == "GET" {
                return FakeHttpResponse::json_ok(json!({
                    "id": "sandbox-1",
                    "state": "started",
                    "snapshot": "probe-managed-agent",
                    "target": "us",
                    "cpu": 2,
                    "memory": 4,
                    "disk": 20
                }));
            }
            if request.path == "/v1/toolbox/sandbox-1/process/execute" {
                assert!(request.body.contains("probe exec"));
                return FakeHttpResponse::json_ok(json!({
                    "exitCode": 0,
                    "result": "managed daytona complete\\nsession=session-daytona-1 profile=openai-codex-subscription"
                }));
            }
            if request.path == "/v1/callback" {
                callback_bodies_server
                    .lock()
                    .expect("callback bodies lock")
                    .push(request.body);
                return FakeHttpResponse::json_ok(json!({"accepted": true}));
            }
            FakeHttpResponse::json_status(404, json!({"error": request.path}))
        });
        let temp = tempdir().expect("temp dir");
        let claims = test_claims(format!("{}/callback", daytona.base_url()).as_str());
        let token = sign_assignment_token(&claims, "secret").expect("token");
        let runner = ManagedDaytonaRunner::new(ManagedRuntimeController::new(
            FilesystemSessionStore::new(temp.path()),
        ));

        let outcome = runner
            .run_once(ManagedDaytonaRunRequest {
                assignment_token: token,
                signing_secret: String::from("secret"),
                callback_bearer_token: None,
                artifact_dir: temp.path().join("artifacts"),
                config: test_config(daytona.base_url()),
                default_snapshot: Some(String::from("probe-managed-agent")),
                default_target: Some(String::from("us")),
                wait_timeout_ms: 1,
                delete_sandbox_on_finish: false,
                dry_run: false,
            })
            .expect("run daytona");

        let ManagedDaytonaOutcome::Completed(state) = outcome else {
            panic!("expected completed");
        };
        assert_eq!(state.probe_session_id.as_deref(), Some("session-daytona-1"));
        assert_eq!(
            state
                .allocation
                .as_ref()
                .map(|allocation| allocation.sandbox_id.as_str()),
            Some("sandbox-1")
        );
        let callbacks = callback_bodies.lock().expect("callback bodies lock");
        assert_eq!(callbacks.len(), 2);
        assert!(callbacks[0].contains("daytona.started"));
        assert!(callbacks[1].contains("daytona.completed"));
        let evidence = std::fs::read_to_string(temp.path().join("artifacts/daytona-evidence.json"))
            .expect("evidence");
        assert!(evidence.contains("sandbox-1"));
        assert!(!evidence.contains("secret-token"));
    }

    #[test]
    fn provider_errors_are_normalized_and_do_not_persist_credentials() {
        let daytona = FakeOpenAiServer::from_handler(move |request| {
            if request.path == "/v1/sandbox" {
                return FakeHttpResponse::json_status(401, json!({"message":"bad key"}));
            }
            if request.path == "/v1/callback" {
                return FakeHttpResponse::json_ok(json!({"accepted": true}));
            }
            FakeHttpResponse::json_status(404, json!({"error": request.path}))
        });
        let temp = tempdir().expect("temp dir");
        let claims = test_claims(format!("{}/callback", daytona.base_url()).as_str());
        let token = sign_assignment_token(&claims, "secret").expect("token");
        let runner = ManagedDaytonaRunner::new(ManagedRuntimeController::new(
            FilesystemSessionStore::new(temp.path()),
        ));

        let outcome = runner
            .run_once(ManagedDaytonaRunRequest {
                assignment_token: token,
                signing_secret: String::from("secret"),
                callback_bearer_token: None,
                artifact_dir: temp.path().join("artifacts"),
                config: test_config(daytona.base_url()),
                default_snapshot: Some(String::from("probe-managed-agent")),
                default_target: Some(String::from("us")),
                wait_timeout_ms: 1,
                delete_sandbox_on_finish: false,
                dry_run: false,
            })
            .expect("failed outcome should be recorded");

        let ManagedDaytonaOutcome::Failed(state) = outcome else {
            panic!("expected failed");
        };
        let provider_error = state.provider_error.expect("provider error");
        assert_eq!(
            provider_error.code,
            super::ManagedDaytonaProviderErrorCode::Unauthorized
        );
        let state_json = std::fs::read_to_string(super::state_path(
            temp.path().join("artifacts").as_path(),
            "assignment-1:lease-1",
        ))
        .expect("state");
        assert!(!state_json.contains("secret-token"));
        let evidence = std::fs::read_to_string(temp.path().join("artifacts/daytona-evidence.json"))
            .expect("evidence");
        assert!(!evidence.contains("secret-token"));
    }

    fn test_claims(callback_url: &str) -> ManagedDaytonaAssignmentClaims {
        let mut claims = ManagedDaytonaAssignmentClaims::new(
            "assignment-1",
            "assignment-1:lease-1",
            "managed-session-1",
            "managed-run-1",
            callback_url,
            "Return exactly: managed daytona complete",
            "environment-1",
            "daytona-coding",
        );
        claims.profile = Some(String::from("openai-codex-subscription"));
        claims.cwd = Some(std::path::PathBuf::from("/workspace"));
        claims.metadata.insert(
            String::from("safe_label"),
            serde_json::Value::String(String::from("visible")),
        );
        claims.metadata.insert(
            String::from("api_token"),
            serde_json::Value::String(String::from("secret-token")),
        );
        claims.environment.constraints = Some(ManagedEnvironmentConstraints {
            allowed_providers: vec![ManagedEnvironmentProviderKind::Daytona],
            allowed_host_classes: vec![ManagedEnvironmentHostClass::DaytonaWorkspace],
            min_resources: ManagedEnvironmentResourceLimits {
                cpu_millicores: Some(2_000),
                memory_mib: Some(4_096),
                disk_mib: Some(20_480),
                gpu_count: None,
            },
            ..ManagedEnvironmentConstraints::empty()
        });
        claims
    }

    fn test_config(base_url: &str) -> ManagedDaytonaConfig {
        ManagedDaytonaConfig {
            base_url: base_url.to_string(),
            toolbox_base_url: format!("{}/toolbox", base_url.trim_end_matches('/')),
            api_key: String::from("secret-token"),
            organization_id: None,
            request_timeout_secs: 5,
        }
    }
}
