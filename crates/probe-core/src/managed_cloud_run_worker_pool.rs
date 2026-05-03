use std::fmt::{Display, Formatter};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use probe_protocol::backend::BackendProfile;
use probe_protocol::managed_environment::{
    ManagedEnvironmentCapabilities, ManagedEnvironmentConstraints, ManagedEnvironmentHostClass,
    ManagedEnvironmentProviderKind,
};
use probe_protocol::managed_runtime::{
    ManagedRuntimeActor, ManagedRuntimeArtifactKind, ManagedRuntimeArtifactRef,
    ManagedRuntimeCorrelation, ManagedRuntimeErrorPayload, ManagedRuntimeEventPayload,
    ManagedRuntimeEventType, ManagedRuntimeSessionStatus, ManagedRuntimeSource, ManagedSessionRef,
    managed_runtime_transcript_ref,
};
use probe_protocol::session::{SessionHarnessProfile, SessionId};
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::managed_runtime::{
    ManagedRuntimeController, ManagedRuntimeError, ManagedRuntimeEventDraft,
};
use crate::runtime::{
    PlainTextExecRequest, ProbeRuntime, RuntimeError, RuntimeEvent, RuntimeEventSink,
};
use crate::tools::ToolLoopConfig;

pub const PROBE_MANAGED_CLOUD_RUN_WORKER_POOL_SCHEMA_VERSION: &str =
    "probe.managed_cloud_run_worker_pool.v1";
pub const PROBE_MANAGED_CLOUD_RUN_WORKER_POOL_ASSIGNMENT_SCHEMA_VERSION: &str =
    "probe.managed_cloud_run_worker_pool.assignment.v1";

const ATTACH_PATH: &str = "/api/admin/managed-agents/v1/runtime/workers/attach";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedCloudRunWorkerPoolIdentity {
    pub worker_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_pool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_pool_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logs_url: Option<String>,
}

impl ManagedCloudRunWorkerPoolIdentity {
    #[must_use]
    pub fn from_env(worker_id: Option<String>) -> Self {
        let worker_pool = env_nonempty("CLOUD_RUN_WORKER_POOL");
        let worker_pool_revision = env_nonempty("CLOUD_RUN_WORKER_POOL_REVISION");
        let instance_id = env_nonempty("HOSTNAME");
        let worker_id = worker_id
            .or_else(|| env_nonempty("PROBE_MANAGED_WORKER_ID"))
            .or_else(|| {
                worker_pool_revision
                    .as_ref()
                    .zip(instance_id.as_ref())
                    .map(|(revision, instance)| format!("{revision}:{instance}"))
            })
            .or_else(|| instance_id.clone())
            .unwrap_or_else(|| format!("probe-managed-worker-{}", std::process::id()));

        Self {
            worker_id,
            worker_pool,
            worker_pool_revision,
            instance_id,
            region: env_nonempty("PROBE_MANAGED_GCP_REGION"),
            logs_url: env_nonempty("PROBE_MANAGED_CLOUD_RUN_LOGS_URL"),
        }
    }

    #[must_use]
    pub fn deploy_revision(&self) -> Option<String> {
        self.worker_pool_revision.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedCloudRunWorkerPoolEnvironmentAllocation {
    pub managed_environment_id: String,
    pub provider: ManagedEnvironmentProviderKind,
    pub host_class: ManagedEnvironmentHostClass,
    pub environment_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<ManagedEnvironmentConstraints>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedCloudRunWorkerPoolAssignment {
    #[serde(default = "assignment_schema_version")]
    pub schema_version: String,
    pub assignment_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
    pub idempotency_key: String,
    pub managed_session_id: String,
    pub managed_run_id: String,
    pub goal_prompt: String,
    pub environment: ManagedCloudRunWorkerPoolEnvironmentAllocation,
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
    pub lease_expires_at_ms: Option<u64>,
    #[serde(default)]
    pub cancel_requested: bool,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

impl ManagedCloudRunWorkerPoolAssignment {
    #[must_use]
    pub fn new(
        assignment_id: impl Into<String>,
        idempotency_key: impl Into<String>,
        managed_session_id: impl Into<String>,
        managed_run_id: impl Into<String>,
        goal_prompt: impl Into<String>,
        managed_environment_id: impl Into<String>,
        environment_class: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: assignment_schema_version(),
            assignment_id: assignment_id.into(),
            lease_id: None,
            idempotency_key: idempotency_key.into(),
            managed_session_id: managed_session_id.into(),
            managed_run_id: managed_run_id.into(),
            goal_prompt: goal_prompt.into(),
            environment: ManagedCloudRunWorkerPoolEnvironmentAllocation {
                managed_environment_id: managed_environment_id.into(),
                provider: ManagedEnvironmentProviderKind::GoogleCloud,
                host_class: ManagedEnvironmentHostClass::CloudRunWorkerPool,
                environment_class: environment_class.into(),
                constraints: None,
            },
            title: None,
            profile: None,
            model: None,
            cwd: None,
            artifact_gcs_prefix: None,
            lease_expires_at_ms: None,
            cancel_requested: false,
            metadata: Map::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedCloudRunWorkerPoolState {
    Starting,
    Idle,
    Claiming,
    Running,
    Draining,
    Stopped,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedCloudRunWorkerPoolAssignmentStatus {
    Started,
    Completed,
    Failed,
    Cancelled,
    DuplicateSkipped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedCloudRunWorkerPoolAssignmentState {
    pub schema_version: String,
    pub assignment_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
    pub idempotency_key: String,
    pub status: ManagedCloudRunWorkerPoolAssignmentStatus,
    pub worker: ManagedCloudRunWorkerPoolIdentity,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedCloudRunWorkerPoolRunReport {
    pub schema_version: String,
    pub worker_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_pool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_pool_revision: Option<String>,
    pub state: ManagedCloudRunWorkerPoolState,
    pub iterations: usize,
    pub claimed_assignments: usize,
    pub completed_assignments: usize,
    pub failed_assignments: usize,
    pub cancelled_assignments: usize,
    pub duplicate_assignments: usize,
    pub idle_cycles: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_assignment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_assignment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat_at_ms: Option<u64>,
}

impl ManagedCloudRunWorkerPoolRunReport {
    #[must_use]
    pub fn new(identity: &ManagedCloudRunWorkerPoolIdentity) -> Self {
        Self {
            schema_version: String::from(PROBE_MANAGED_CLOUD_RUN_WORKER_POOL_SCHEMA_VERSION),
            worker_id: identity.worker_id.clone(),
            worker_pool: identity.worker_pool.clone(),
            worker_pool_revision: identity.worker_pool_revision.clone(),
            state: ManagedCloudRunWorkerPoolState::Starting,
            iterations: 0,
            claimed_assignments: 0,
            completed_assignments: 0,
            failed_assignments: 0,
            cancelled_assignments: 0,
            duplicate_assignments: 0,
            idle_cycles: 0,
            current_assignment_id: None,
            last_assignment_id: None,
            stopped_reason: None,
            last_heartbeat_at_ms: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ManagedCloudRunWorkerPoolRunRequest {
    pub controller_base_url: String,
    pub bearer_token: String,
    pub profile: BackendProfile,
    pub default_cwd: PathBuf,
    pub artifact_dir: PathBuf,
    pub identity: ManagedCloudRunWorkerPoolIdentity,
    pub capabilities: ManagedEnvironmentCapabilities,
    pub system_prompt: Option<String>,
    pub harness_profile: Option<SessionHarnessProfile>,
    pub tool_loop: Option<ToolLoopConfig>,
    pub poll_interval_ms: u64,
    pub max_iterations: Option<usize>,
    pub exit_on_idle: bool,
    pub dry_run: bool,
    pub shutdown: ManagedCloudRunWorkerPoolShutdown,
}

#[derive(Clone, Debug, Default)]
pub struct ManagedCloudRunWorkerPoolShutdown {
    flag: Option<Arc<AtomicBool>>,
    file: Option<PathBuf>,
}

impl ManagedCloudRunWorkerPoolShutdown {
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn flag(flag: Arc<AtomicBool>) -> Self {
        Self {
            flag: Some(flag),
            file: None,
        }
    }

    #[must_use]
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            flag: None,
            file: Some(path.into()),
        }
    }

    #[must_use]
    pub fn flag_and_file(flag: Arc<AtomicBool>, path: impl Into<PathBuf>) -> Self {
        Self {
            flag: Some(flag),
            file: Some(path.into()),
        }
    }

    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.flag
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
            || self.file.as_ref().is_some_and(|path| path.exists())
    }

    pub fn request(&self) {
        if let Some(flag) = self.flag.as_ref() {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

#[derive(Clone, Debug)]
pub struct ManagedCloudRunWorkerPoolRunner {
    runtime: ProbeRuntime,
    managed_runtime: ManagedRuntimeController,
    client: Client,
}

impl ManagedCloudRunWorkerPoolRunner {
    pub fn new(
        runtime: ProbeRuntime,
        managed_runtime: ManagedRuntimeController,
    ) -> Result<Self, ManagedCloudRunWorkerPoolError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(ManagedCloudRunWorkerPoolError::Http)?;
        Ok(Self {
            runtime,
            managed_runtime,
            client,
        })
    }

    pub fn run_loop(
        &self,
        request: ManagedCloudRunWorkerPoolRunRequest,
    ) -> Result<ManagedCloudRunWorkerPoolRunReport, ManagedCloudRunWorkerPoolError> {
        validate_worker_pool_capabilities(&request.capabilities)?;
        fs::create_dir_all(request.artifact_dir.as_path())
            .map_err(ManagedCloudRunWorkerPoolError::Io)?;

        let mut report = ManagedCloudRunWorkerPoolRunReport::new(&request.identity);
        let attach = self.attach(&request)?;
        let _attach_accepted = attach.accepted;
        if attach.shutdown_requested {
            request.shutdown.request();
        }
        let mut poll_interval_ms = attach.poll_interval_ms.unwrap_or(request.poll_interval_ms);
        self.heartbeat(
            &request,
            ManagedCloudRunWorkerPoolState::Starting,
            None,
            &mut report,
            None,
        )?;

        loop {
            if request.shutdown.is_requested() {
                report.state = ManagedCloudRunWorkerPoolState::Draining;
                report.stopped_reason = Some(String::from("shutdown_requested"));
                self.heartbeat(
                    &request,
                    ManagedCloudRunWorkerPoolState::Draining,
                    None,
                    &mut report,
                    Some(json!({"phase": "shutdown_requested_before_claim"})),
                )?;
                break;
            }

            if let Some(max_iterations) = request.max_iterations
                && report.iterations >= max_iterations
            {
                report.stopped_reason = Some(String::from("max_iterations_reached"));
                break;
            }

            report.iterations += 1;
            let heartbeat = self.heartbeat(
                &request,
                ManagedCloudRunWorkerPoolState::Idle,
                None,
                &mut report,
                Some(json!({"phase": "idle"})),
            )?;
            let _heartbeat_accepted = heartbeat.accepted;
            if let Some(next_poll_interval_ms) = heartbeat.poll_interval_ms {
                poll_interval_ms = next_poll_interval_ms;
            }
            if heartbeat.shutdown_requested {
                request.shutdown.request();
                continue;
            }

            let claim = self.claim_next(&request, &mut report)?;
            if claim.shutdown_requested {
                request.shutdown.request();
            }
            let Some(assignment) = claim.assignment else {
                report.idle_cycles += 1;
                report.state = ManagedCloudRunWorkerPoolState::Idle;
                if request.exit_on_idle {
                    report.stopped_reason = Some(String::from("idle"));
                    break;
                }
                sleep_poll_interval(claim.poll_interval_ms.unwrap_or(poll_interval_ms));
                continue;
            };

            report.claimed_assignments += 1;
            report.current_assignment_id = Some(assignment.assignment_id.clone());
            report.last_assignment_id = Some(assignment.assignment_id.clone());
            let outcome = self.execute_assignment(&request, assignment)?;
            match outcome.status {
                ManagedCloudRunWorkerPoolAssignmentStatus::Completed => {
                    report.completed_assignments += 1;
                }
                ManagedCloudRunWorkerPoolAssignmentStatus::Failed => {
                    report.failed_assignments += 1;
                }
                ManagedCloudRunWorkerPoolAssignmentStatus::Cancelled => {
                    report.cancelled_assignments += 1;
                }
                ManagedCloudRunWorkerPoolAssignmentStatus::DuplicateSkipped => {
                    report.duplicate_assignments += 1;
                }
                ManagedCloudRunWorkerPoolAssignmentStatus::Started => {}
            }
            report.current_assignment_id = None;

            if request.shutdown.is_requested() {
                report.state = ManagedCloudRunWorkerPoolState::Draining;
                report.stopped_reason = Some(String::from("shutdown_requested"));
                self.heartbeat(
                    &request,
                    ManagedCloudRunWorkerPoolState::Draining,
                    None,
                    &mut report,
                    Some(json!({"phase": "shutdown_requested_after_assignment"})),
                )?;
                break;
            }
        }

        report.state = ManagedCloudRunWorkerPoolState::Stopped;
        let stopped_reason = report.stopped_reason.clone();
        self.heartbeat(
            &request,
            ManagedCloudRunWorkerPoolState::Stopped,
            None,
            &mut report,
            Some(json!({"phase": "stopped", "reason": stopped_reason})),
        )?;
        Ok(report)
    }

    fn execute_assignment(
        &self,
        request: &ManagedCloudRunWorkerPoolRunRequest,
        assignment: ManagedCloudRunWorkerPoolAssignment,
    ) -> Result<ManagedCloudRunWorkerPoolAssignmentState, ManagedCloudRunWorkerPoolError> {
        validate_assignment(&assignment)?;
        let state_path = assignment_state_path(
            request.artifact_dir.as_path(),
            assignment.idempotency_key.as_str(),
        );
        if let Some(mut state) = read_assignment_state_if_present(state_path.as_path())? {
            state.status = ManagedCloudRunWorkerPoolAssignmentStatus::DuplicateSkipped;
            self.record_assignment_event(
                request,
                &assignment,
                "assignment.duplicate_skipped",
                &state,
                None,
            )?;
            return Ok(state);
        }

        let mut state = ManagedCloudRunWorkerPoolAssignmentState {
            schema_version: String::from(PROBE_MANAGED_CLOUD_RUN_WORKER_POOL_SCHEMA_VERSION),
            assignment_id: assignment.assignment_id.clone(),
            lease_id: assignment.lease_id.clone(),
            idempotency_key: assignment.idempotency_key.clone(),
            status: ManagedCloudRunWorkerPoolAssignmentStatus::Started,
            worker: request.identity.clone(),
            probe_session_id: None,
            terminal_event_sequence: None,
            artifact_refs: Vec::new(),
            started_at_ms: now_ms(),
            finished_at_ms: None,
            error: None,
        };
        write_assignment_state(state_path.as_path(), &state)?;

        let mut transient_report = ManagedCloudRunWorkerPoolRunReport::new(&request.identity);
        let heartbeat = self.heartbeat(
            request,
            ManagedCloudRunWorkerPoolState::Running,
            Some(assignment.assignment_id.as_str()),
            &mut transient_report,
            Some(json!({
                "phase": "assignment_started",
                "assignmentId": assignment.assignment_id,
                "leaseId": assignment.lease_id,
            })),
        )?;
        let _heartbeat_accepted = heartbeat.accepted;
        if assignment.cancel_requested || heartbeat.cancel_current_assignment {
            state.status = ManagedCloudRunWorkerPoolAssignmentStatus::Cancelled;
            state.finished_at_ms = Some(now_ms());
            state.error = Some(String::from("assignment cancelled before runtime start"));
            write_assignment_state(state_path.as_path(), &state)?;
            self.record_assignment_event(
                request,
                &assignment,
                "assignment.cancelled",
                &state,
                state.error.as_deref(),
            )?;
            return Ok(state);
        }

        let execution = if request.dry_run {
            self.runtime
                .session_store()
                .create_session(
                    assignment
                        .title
                        .clone()
                        .unwrap_or_else(|| format!("Managed Worker: {}", assignment.assignment_id)),
                    assignment
                        .cwd
                        .clone()
                        .unwrap_or_else(|| request.default_cwd.clone()),
                )
                .map(|session| ManagedCloudRunWorkerPoolExecutionResult {
                    session_id: session.id,
                    assistant_text: String::from("dry-run completed without model execution"),
                })
                .map_err(|error| ManagedCloudRunWorkerPoolError::Runtime {
                    session_id: None,
                    error: error.into(),
                })
        } else {
            self.execute_probe_session(&assignment, request)
        };

        match execution {
            Ok(result) => {
                state.probe_session_id = Some(result.session_id.as_str().to_string());
                let artifacts = write_evidence_artifact(
                    request.artifact_dir.as_path(),
                    &assignment,
                    &state,
                    Some(result.assistant_text.as_str()),
                    None,
                )?;
                state.artifact_refs = artifacts.clone();
                let terminal_event = self.record_terminal_event(
                    &assignment,
                    request,
                    result.session_id,
                    ManagedRuntimeSessionStatus::Completed,
                    Some("Cloud Run Worker Pool assignment completed"),
                    artifacts,
                    None,
                )?;
                state.status = ManagedCloudRunWorkerPoolAssignmentStatus::Completed;
                state.terminal_event_sequence = Some(terminal_event.sequence);
                state.finished_at_ms = Some(now_ms());
                write_assignment_state(state_path.as_path(), &state)?;
                self.record_assignment_event(
                    request,
                    &assignment,
                    "assignment.completed",
                    &state,
                    None,
                )?;
                Ok(state)
            }
            Err(error) => {
                let error_text = error.to_string();
                let session_id = error.session_id();
                state.probe_session_id = session_id.as_ref().map(|id| id.as_str().to_string());
                let artifacts = write_evidence_artifact(
                    request.artifact_dir.as_path(),
                    &assignment,
                    &state,
                    None,
                    Some(error_text.as_str()),
                )?;
                state.artifact_refs = artifacts.clone();
                if let Some(session_id) = session_id {
                    let terminal_event = self.record_terminal_event(
                        &assignment,
                        request,
                        session_id,
                        ManagedRuntimeSessionStatus::Failed,
                        Some(error_text.as_str()),
                        artifacts,
                        Some(error_text.as_str()),
                    )?;
                    state.terminal_event_sequence = Some(terminal_event.sequence);
                }
                state.status = ManagedCloudRunWorkerPoolAssignmentStatus::Failed;
                state.finished_at_ms = Some(now_ms());
                state.error = Some(error_text.clone());
                write_assignment_state(state_path.as_path(), &state)?;
                self.record_assignment_event(
                    request,
                    &assignment,
                    "assignment.failed",
                    &state,
                    Some(error_text.as_str()),
                )?;
                Ok(state)
            }
        }
    }

    fn execute_probe_session(
        &self,
        assignment: &ManagedCloudRunWorkerPoolAssignment,
        request: &ManagedCloudRunWorkerPoolRunRequest,
    ) -> Result<ManagedCloudRunWorkerPoolExecutionResult, ManagedCloudRunWorkerPoolError> {
        let captured_session_id = Arc::new(Mutex::new(None::<SessionId>));
        let sink_session_id = Arc::clone(&captured_session_id);
        let event_sink: Arc<dyn RuntimeEventSink> = Arc::new(move |event: RuntimeEvent| {
            if let RuntimeEvent::TurnStarted { session_id, .. } = event {
                *sink_session_id
                    .lock()
                    .expect("managed worker pool session capture mutex") = Some(session_id);
            }
        });

        let outcome = self
            .runtime
            .exec_plain_text_with_events(
                PlainTextExecRequest {
                    profile: request.profile.clone(),
                    prompt: assignment.goal_prompt.clone(),
                    title: assignment.title.clone().or_else(|| {
                        Some(format!(
                            "Managed Worker: {}",
                            assignment.assignment_id.as_str()
                        ))
                    }),
                    cwd: assignment
                        .cwd
                        .clone()
                        .unwrap_or_else(|| request.default_cwd.clone()),
                    system_prompt: request.system_prompt.clone(),
                    harness_profile: request.harness_profile.clone(),
                    tool_loop: request.tool_loop.clone(),
                },
                event_sink,
            )
            .map_err(|error| ManagedCloudRunWorkerPoolError::Runtime {
                session_id: captured_session_id
                    .lock()
                    .expect("managed worker pool session capture mutex")
                    .clone(),
                error,
            })?;

        Ok(ManagedCloudRunWorkerPoolExecutionResult {
            session_id: outcome.session.id,
            assistant_text: outcome.assistant_text,
        })
    }

    fn attach(
        &self,
        request: &ManagedCloudRunWorkerPoolRunRequest,
    ) -> Result<WorkerPoolAttachResponse, ManagedCloudRunWorkerPoolError> {
        self.post_json(
            request,
            ATTACH_PATH,
            &json!({
                "schemaVersion": PROBE_MANAGED_CLOUD_RUN_WORKER_POOL_SCHEMA_VERSION,
                "worker": request.identity,
                "deployRevision": request.identity.deploy_revision(),
                "capabilities": request.capabilities,
                "attachedAtMs": now_ms(),
            }),
            "attach worker",
        )
    }

    fn heartbeat(
        &self,
        request: &ManagedCloudRunWorkerPoolRunRequest,
        state: ManagedCloudRunWorkerPoolState,
        assignment_id: Option<&str>,
        report: &mut ManagedCloudRunWorkerPoolRunReport,
        metadata: Option<Value>,
    ) -> Result<WorkerPoolHeartbeatResponse, ManagedCloudRunWorkerPoolError> {
        report.state = state;
        report.current_assignment_id = assignment_id.map(str::to_string);
        report.last_heartbeat_at_ms = Some(now_ms());
        let path = format!(
            "/api/admin/managed-agents/v1/runtime/workers/{}/heartbeat",
            request.identity.worker_id
        );
        self.post_json(
            request,
            path.as_str(),
            &json!({
                "schemaVersion": PROBE_MANAGED_CLOUD_RUN_WORKER_POOL_SCHEMA_VERSION,
                "workerId": request.identity.worker_id,
                "state": state,
                "currentAssignmentId": assignment_id,
                "worker": request.identity,
                "deployRevision": request.identity.deploy_revision(),
                "capabilities": request.capabilities,
                "report": report,
                "metadata": metadata.unwrap_or_else(|| json!({})),
                "heartbeatAtMs": report.last_heartbeat_at_ms,
            }),
            "heartbeat worker",
        )
    }

    fn claim_next(
        &self,
        request: &ManagedCloudRunWorkerPoolRunRequest,
        report: &mut ManagedCloudRunWorkerPoolRunReport,
    ) -> Result<WorkerPoolClaimResponse, ManagedCloudRunWorkerPoolError> {
        report.state = ManagedCloudRunWorkerPoolState::Claiming;
        let path = format!(
            "/api/admin/managed-agents/v1/runtime/workers/{}/assignments/claim-next",
            request.identity.worker_id
        );
        self.post_json(
            request,
            path.as_str(),
            &json!({
                "schemaVersion": PROBE_MANAGED_CLOUD_RUN_WORKER_POOL_SCHEMA_VERSION,
                "workerId": request.identity.worker_id,
                "worker": request.identity,
                "deployRevision": request.identity.deploy_revision(),
                "capabilities": request.capabilities,
                "claimedAtMs": now_ms(),
            }),
            "claim next assignment",
        )
    }

    fn record_assignment_event(
        &self,
        request: &ManagedCloudRunWorkerPoolRunRequest,
        assignment: &ManagedCloudRunWorkerPoolAssignment,
        event_type: &str,
        state: &ManagedCloudRunWorkerPoolAssignmentState,
        error: Option<&str>,
    ) -> Result<(), ManagedCloudRunWorkerPoolError> {
        let path = format!(
            "/api/admin/managed-agents/v1/runtime/workers/{}/assignments/{}/events",
            request.identity.worker_id, assignment.assignment_id
        );
        let response: WorkerPoolAssignmentEventResponse = self.post_json(
            request,
            path.as_str(),
            &json!({
                "schemaVersion": PROBE_MANAGED_CLOUD_RUN_WORKER_POOL_SCHEMA_VERSION,
                "eventType": event_type,
                "workerId": request.identity.worker_id,
                "worker": request.identity,
                "deployRevision": request.identity.deploy_revision(),
                "assignmentId": assignment.assignment_id,
                "leaseId": assignment.lease_id,
                "managedSessionId": assignment.managed_session_id,
                "managedRunId": assignment.managed_run_id,
                "idempotencyKey": assignment.idempotency_key,
                "status": state.status,
                "probeSessionId": state.probe_session_id,
                "terminalEventSequence": state.terminal_event_sequence,
                "artifactRefs": state.artifact_refs,
                "error": error,
                "occurredAtMs": now_ms(),
            }),
            "record assignment event",
        )?;
        let _event_accepted = response.accepted;
        Ok(())
    }

    fn post_json<T: Serialize, R: DeserializeOwned>(
        &self,
        request: &ManagedCloudRunWorkerPoolRunRequest,
        path: &str,
        body: &T,
        operation: &'static str,
    ) -> Result<R, ManagedCloudRunWorkerPoolError> {
        let url = api_url(request.controller_base_url.as_str(), path);
        let response = self
            .client
            .post(url)
            .bearer_auth(request.bearer_token.as_str())
            .header("accept", "application/json")
            .header("x-probe-worker-id", request.identity.worker_id.as_str())
            .json(body)
            .send()
            .map_err(ManagedCloudRunWorkerPoolError::Http)?;
        decode_json_response(response, operation)
    }

    fn record_terminal_event(
        &self,
        assignment: &ManagedCloudRunWorkerPoolAssignment,
        request: &ManagedCloudRunWorkerPoolRunRequest,
        session_id: SessionId,
        status: ManagedRuntimeSessionStatus,
        reason: Option<&str>,
        artifact_refs: Vec<ManagedRuntimeArtifactRef>,
        error: Option<&str>,
    ) -> Result<probe_protocol::managed_runtime::ManagedRuntimeEvent, ManagedCloudRunWorkerPoolError>
    {
        let actor = ManagedRuntimeActor {
            kind: String::from("cloud_run_worker_pool"),
            id: Some(request.identity.worker_id.clone()),
            label: request.identity.worker_pool.clone(),
        };
        let correlation = ManagedRuntimeCorrelation {
            request_id: Some(assignment.assignment_id.clone()),
            workspace: Some(String::from("openagents.com")),
            managed_environment_id: Some(assignment.environment.managed_environment_id.clone()),
            managed_session_id: Some(assignment.managed_session_id.clone()),
            managed_run_id: Some(assignment.managed_run_id.clone()),
            ..ManagedRuntimeCorrelation::default()
        };
        let session = ManagedSessionRef {
            probe_session_id: session_id.clone(),
            managed_session_id: Some(assignment.managed_session_id.clone()),
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
                    request_id: format!("{}:replay-before-terminal", assignment.assignment_id),
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
                        kind: String::from("cloud_run_worker_pool"),
                        id: Some(request.identity.worker_id.clone()),
                        label: request.identity.worker_pool.clone(),
                    },
                    session: session.clone(),
                    correlation: correlation.clone(),
                    artifact_refs: vec![managed_runtime_transcript_ref(&session_id)],
                    payload: ManagedRuntimeEventPayload::SessionLifecycle {
                        title: assignment
                            .title
                            .clone()
                            .unwrap_or_else(|| assignment.assignment_id.clone()),
                        cwd: assignment
                            .cwd
                            .clone()
                            .unwrap_or_else(|| request.default_cwd.clone()),
                        backend_profile: request.profile.name.clone(),
                        model: request.profile.model.clone(),
                        environment_constraints: assignment.environment.constraints.clone(),
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
                    kind: String::from("cloud_run_worker_pool"),
                    id: Some(request.identity.worker_id.clone()),
                    label: Some(String::from("Cloud Run Worker Pool terminal status")),
                },
                session,
                correlation,
                artifact_refs: refs,
                payload: if let Some(error) = error {
                    ManagedRuntimeEventPayload::Error {
                        error: ManagedRuntimeErrorPayload {
                            code: String::from("managed_cloud_run_worker_pool_failed"),
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
            .map_err(ManagedCloudRunWorkerPoolError::ManagedRuntime)
    }
}

#[derive(Clone, Debug)]
struct ManagedCloudRunWorkerPoolExecutionResult {
    session_id: SessionId,
    assistant_text: String,
}

#[derive(Debug)]
pub enum ManagedCloudRunWorkerPoolError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Http(reqwest::Error),
    HttpStatus {
        operation: &'static str,
        status: u16,
        body: String,
    },
    InvalidAssignment(String),
    Runtime {
        session_id: Option<SessionId>,
        error: RuntimeError,
    },
    ManagedRuntime(ManagedRuntimeError),
}

impl ManagedCloudRunWorkerPoolError {
    #[must_use]
    pub fn session_id(&self) -> Option<SessionId> {
        match self {
            Self::Runtime { session_id, .. } => session_id.clone(),
            _ => None,
        }
    }
}

impl Display for ManagedCloudRunWorkerPoolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::Http(error) => write!(f, "http error: {error}"),
            Self::HttpStatus {
                operation,
                status,
                body,
            } => write!(f, "{operation} returned HTTP {status}: {body}"),
            Self::InvalidAssignment(message) => f.write_str(message),
            Self::Runtime { error, .. } => write!(f, "runtime error: {error}"),
            Self::ManagedRuntime(error) => write!(f, "managed runtime error: {error}"),
        }
    }
}

impl std::error::Error for ManagedCloudRunWorkerPoolError {}

impl From<std::io::Error> for ManagedCloudRunWorkerPoolError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ManagedCloudRunWorkerPoolError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<ManagedRuntimeError> for ManagedCloudRunWorkerPoolError {
    fn from(value: ManagedRuntimeError) -> Self {
        Self::ManagedRuntime(value)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerPoolAttachResponse {
    #[serde(default)]
    accepted: bool,
    #[serde(default)]
    poll_interval_ms: Option<u64>,
    #[serde(default)]
    shutdown_requested: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerPoolHeartbeatResponse {
    #[serde(default)]
    accepted: bool,
    #[serde(default)]
    poll_interval_ms: Option<u64>,
    #[serde(default)]
    shutdown_requested: bool,
    #[serde(default)]
    cancel_current_assignment: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerPoolClaimResponse {
    #[serde(default)]
    assignment: Option<ManagedCloudRunWorkerPoolAssignment>,
    #[serde(default)]
    poll_interval_ms: Option<u64>,
    #[serde(default)]
    shutdown_requested: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerPoolAssignmentEventResponse {
    #[serde(default)]
    accepted: bool,
}

fn decode_json_response<T: DeserializeOwned>(
    response: Response,
    operation: &'static str,
) -> Result<T, ManagedCloudRunWorkerPoolError> {
    let status = response.status();
    if status == StatusCode::NO_CONTENT {
        return serde_json::from_value(Value::Object(Map::new()))
            .map_err(ManagedCloudRunWorkerPoolError::Json);
    }
    let body = response
        .text()
        .map_err(ManagedCloudRunWorkerPoolError::Http)?;
    if !status.is_success() {
        return Err(ManagedCloudRunWorkerPoolError::HttpStatus {
            operation,
            status: status.as_u16(),
            body,
        });
    }
    if body.trim().is_empty() {
        return serde_json::from_value(Value::Object(Map::new()))
            .map_err(ManagedCloudRunWorkerPoolError::Json);
    }
    serde_json::from_str(body.as_str()).map_err(ManagedCloudRunWorkerPoolError::Json)
}

fn validate_worker_pool_capabilities(
    capabilities: &ManagedEnvironmentCapabilities,
) -> Result<(), ManagedCloudRunWorkerPoolError> {
    if capabilities.provider != ManagedEnvironmentProviderKind::GoogleCloud {
        return Err(ManagedCloudRunWorkerPoolError::InvalidAssignment(
            "Cloud Run Worker Pool runner requires google_cloud provider capabilities".to_string(),
        ));
    }
    if capabilities.host_class != ManagedEnvironmentHostClass::CloudRunWorkerPool {
        return Err(ManagedCloudRunWorkerPoolError::InvalidAssignment(
            "Cloud Run Worker Pool runner requires cloud_run_worker_pool host class".to_string(),
        ));
    }
    Ok(())
}

fn validate_assignment(
    assignment: &ManagedCloudRunWorkerPoolAssignment,
) -> Result<(), ManagedCloudRunWorkerPoolError> {
    if assignment.schema_version != PROBE_MANAGED_CLOUD_RUN_WORKER_POOL_ASSIGNMENT_SCHEMA_VERSION {
        return Err(ManagedCloudRunWorkerPoolError::InvalidAssignment(format!(
            "unsupported worker-pool assignment schema version `{}`",
            assignment.schema_version
        )));
    }
    if assignment.environment.provider != ManagedEnvironmentProviderKind::GoogleCloud {
        return Err(ManagedCloudRunWorkerPoolError::InvalidAssignment(
            "worker-pool assignment must target google_cloud".to_string(),
        ));
    }
    if assignment.environment.host_class != ManagedEnvironmentHostClass::CloudRunWorkerPool {
        return Err(ManagedCloudRunWorkerPoolError::InvalidAssignment(
            "worker-pool assignment must target cloud_run_worker_pool".to_string(),
        ));
    }
    if let Some(constraints) = assignment.environment.constraints.as_ref() {
        if !constraints.allowed_providers.is_empty()
            && !constraints
                .allowed_providers
                .contains(&ManagedEnvironmentProviderKind::GoogleCloud)
        {
            return Err(ManagedCloudRunWorkerPoolError::InvalidAssignment(
                "assignment constraints do not allow google_cloud".to_string(),
            ));
        }
        if !constraints.allowed_host_classes.is_empty()
            && !constraints
                .allowed_host_classes
                .contains(&ManagedEnvironmentHostClass::CloudRunWorkerPool)
        {
            return Err(ManagedCloudRunWorkerPoolError::InvalidAssignment(
                "assignment constraints do not allow cloud_run_worker_pool".to_string(),
            ));
        }
    }
    Ok(())
}

fn write_evidence_artifact(
    artifact_dir: &Path,
    assignment: &ManagedCloudRunWorkerPoolAssignment,
    state: &ManagedCloudRunWorkerPoolAssignmentState,
    assistant_text: Option<&str>,
    error: Option<&str>,
) -> Result<Vec<ManagedRuntimeArtifactRef>, ManagedCloudRunWorkerPoolError> {
    let evidence_dir = artifact_dir.join("evidence");
    fs::create_dir_all(evidence_dir.as_path())?;
    let evidence = json!({
        "schemaVersion": PROBE_MANAGED_CLOUD_RUN_WORKER_POOL_SCHEMA_VERSION,
        "assignmentId": assignment.assignment_id,
        "leaseId": assignment.lease_id,
        "managedSessionId": assignment.managed_session_id,
        "managedRunId": assignment.managed_run_id,
        "managedEnvironmentId": assignment.environment.managed_environment_id,
        "worker": state.worker,
        "status": state.status,
        "probeSessionId": state.probe_session_id,
        "assistantText": assistant_text,
        "error": error,
        "finishedAtMs": now_ms(),
    });
    let file_name = format!(
        "{}.json",
        short_digest(format!("{}:{}", assignment.assignment_id, state.started_at_ms).as_bytes())
    );
    let path = evidence_dir.join(file_name.as_str());
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path.as_path())?;
    serde_json::to_writer_pretty(&mut file, &evidence)?;
    file.write_all(b"\n")?;
    file.flush()?;

    let local_ref = format!("file://{}", path.display());
    let resource_ref = assignment
        .artifact_gcs_prefix
        .as_ref()
        .map(|prefix| format!("{}/evidence/{file_name}", prefix.trim_end_matches('/')))
        .unwrap_or(local_ref);
    Ok(vec![ManagedRuntimeArtifactRef {
        kind: ManagedRuntimeArtifactKind::VerificationPack,
        resource_ref,
        stable_digest: Some(short_digest(serde_json::to_vec(&evidence)?.as_slice())),
        label: Some(String::from("Managed Cloud Run Worker Pool evidence")),
        updated_at_ms: Some(now_ms()),
    }])
}

#[must_use]
pub fn assignment_state_path(artifact_dir: &Path, idempotency_key: &str) -> PathBuf {
    artifact_dir
        .join("state")
        .join(format!("{}.json", short_digest(idempotency_key.as_bytes())))
}

fn read_assignment_state_if_present(
    path: &Path,
) -> Result<Option<ManagedCloudRunWorkerPoolAssignmentState>, ManagedCloudRunWorkerPoolError> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
}

fn write_assignment_state(
    path: &Path,
    state: &ManagedCloudRunWorkerPoolAssignmentState,
) -> Result<(), ManagedCloudRunWorkerPoolError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = path.with_file_name(format!(
        "{}.tmp-{}",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("assignment-state.json"),
        std::process::id()
    ));
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

fn assignment_schema_version() -> String {
    String::from(PROBE_MANAGED_CLOUD_RUN_WORKER_POOL_ASSIGNMENT_SCHEMA_VERSION)
}

fn api_url(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn sleep_poll_interval(poll_interval_ms: u64) {
    if poll_interval_ms > 0 {
        std::thread::sleep(Duration::from_millis(poll_interval_ms));
    }
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

#[allow(dead_code)]
fn deserialize_empty_object<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(|value| value.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use probe_protocol::managed_environment::ManagedEnvironmentCapabilities;
    use probe_test_support::{FakeHttpResponse, FakeOpenAiServer};
    use serde_json::json;
    use tempfile::tempdir;

    use crate::managed_runtime::ManagedRuntimeController;
    use crate::runtime::ProbeRuntime;
    use crate::session_store::FilesystemSessionStore;

    use super::{
        ManagedCloudRunWorkerPoolAssignment, ManagedCloudRunWorkerPoolIdentity,
        ManagedCloudRunWorkerPoolRunRequest, ManagedCloudRunWorkerPoolRunner,
        ManagedCloudRunWorkerPoolShutdown,
    };

    #[test]
    fn worker_pool_claims_completes_and_reports_assignment_events() {
        let _api_key = ScopedEnvVar::set("PROBE_MANAGED_WORKER_POOL_TEST_KEY", "probe-test-key");
        let provider = FakeOpenAiServer::from_json_responses(vec![json!({
            "id": "chatcmpl_managed_worker_pool",
            "model": "qwen3.5-2b-q8_0-registry.gguf",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "managed worker pool complete"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 4, "completion_tokens": 4, "total_tokens": 8}
        })]);
        let claim_count = std::sync::Arc::new(AtomicUsize::new(0));
        let claim_count_server = std::sync::Arc::clone(&claim_count);
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let events_server = std::sync::Arc::clone(&events);
        let laravel = FakeOpenAiServer::from_handler(move |request| {
            if request.path.ends_with("/runtime/workers/attach") {
                return FakeHttpResponse::json_ok(json!({"accepted": true}));
            }
            if request.path.ends_with("/heartbeat") {
                return FakeHttpResponse::json_ok(json!({"accepted": true}));
            }
            if request.path.ends_with("/assignments/claim-next") {
                let count = claim_count_server.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    return FakeHttpResponse::json_ok(json!({
                        "assignment": test_assignment(),
                        "pollIntervalMs": 0
                    }));
                }
                return FakeHttpResponse::json_ok(json!({"assignment": null, "pollIntervalMs": 0}));
            }
            if request.path.ends_with("/assignments/assignment-1/events") {
                events_server
                    .lock()
                    .expect("events lock")
                    .push(request.body);
                return FakeHttpResponse::json_ok(json!({"accepted": true}));
            }
            FakeHttpResponse::json_status(404, json!({"error":"unexpected path"}))
        });
        let temp = tempdir().expect("temp dir");
        let runner = ManagedCloudRunWorkerPoolRunner::new(
            ProbeRuntime::new(temp.path()),
            ManagedRuntimeController::new(FilesystemSessionStore::new(temp.path())),
        )
        .expect("runner");

        let report = runner
            .run_loop(test_run_request(
                temp.path(),
                base_without_v1(laravel.base_url()),
                test_profile(provider.base_url()),
                ManagedCloudRunWorkerPoolShutdown::none(),
                Some(2),
            ))
            .expect("worker pool run");

        assert_eq!(report.claimed_assignments, 1);
        assert_eq!(report.completed_assignments, 1);
        assert_eq!(report.idle_cycles, 1);
        assert_eq!(provider.recorded_requests().len(), 1);
        let events = events.lock().expect("events lock");
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("assignment.completed"));
        assert!(temp.path().join("artifacts/evidence").exists());
    }

    #[test]
    fn worker_pool_drains_after_in_flight_shutdown_without_claiming_again() {
        let _api_key = ScopedEnvVar::set("PROBE_MANAGED_WORKER_POOL_TEST_KEY", "probe-test-key");
        let shutdown = std::sync::Arc::new(AtomicBool::new(false));
        let shutdown_provider = std::sync::Arc::clone(&shutdown);
        let provider = FakeOpenAiServer::from_handler(move |_request| {
            shutdown_provider.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(25));
            FakeHttpResponse::json_ok(json!({
                "id": "chatcmpl_managed_worker_pool_shutdown",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "drain complete"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 4, "completion_tokens": 2, "total_tokens": 6}
            }))
        });
        let claim_count = std::sync::Arc::new(AtomicUsize::new(0));
        let claim_count_server = std::sync::Arc::clone(&claim_count);
        let heartbeat_bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let heartbeat_bodies_server = std::sync::Arc::clone(&heartbeat_bodies);
        let laravel = FakeOpenAiServer::from_handler(move |request| {
            if request.path.ends_with("/runtime/workers/attach") {
                return FakeHttpResponse::json_ok(json!({"accepted": true}));
            }
            if request.path.ends_with("/heartbeat") {
                heartbeat_bodies_server
                    .lock()
                    .expect("heartbeat lock")
                    .push(request.body);
                return FakeHttpResponse::json_ok(json!({"accepted": true}));
            }
            if request.path.ends_with("/assignments/claim-next") {
                claim_count_server.fetch_add(1, Ordering::SeqCst);
                return FakeHttpResponse::json_ok(json!({
                    "assignment": test_assignment(),
                    "pollIntervalMs": 0
                }));
            }
            if request.path.ends_with("/assignments/assignment-1/events") {
                return FakeHttpResponse::json_ok(json!({"accepted": true}));
            }
            FakeHttpResponse::json_status(404, json!({"error":"unexpected path"}))
        });
        let temp = tempdir().expect("temp dir");
        let runner = ManagedCloudRunWorkerPoolRunner::new(
            ProbeRuntime::new(temp.path()),
            ManagedRuntimeController::new(FilesystemSessionStore::new(temp.path())),
        )
        .expect("runner");

        let report = runner
            .run_loop(test_run_request(
                temp.path(),
                base_without_v1(laravel.base_url()),
                test_profile(provider.base_url()),
                ManagedCloudRunWorkerPoolShutdown::flag(shutdown),
                Some(10),
            ))
            .expect("worker pool run");

        assert_eq!(claim_count.load(Ordering::SeqCst), 1);
        assert_eq!(report.completed_assignments, 1);
        assert_eq!(report.stopped_reason.as_deref(), Some("shutdown_requested"));
        assert!(
            heartbeat_bodies
                .lock()
                .expect("heartbeat lock")
                .iter()
                .any(|body| body.contains("\"state\":\"draining\""))
        );
    }

    fn test_run_request(
        temp: &std::path::Path,
        controller_base_url: String,
        profile: BackendProfile,
        shutdown: ManagedCloudRunWorkerPoolShutdown,
        max_iterations: Option<usize>,
    ) -> ManagedCloudRunWorkerPoolRunRequest {
        ManagedCloudRunWorkerPoolRunRequest {
            controller_base_url,
            bearer_token: String::from("worker-token"),
            profile,
            default_cwd: temp.to_path_buf(),
            artifact_dir: temp.join("artifacts"),
            identity: ManagedCloudRunWorkerPoolIdentity {
                worker_id: String::from("worker-1"),
                worker_pool: Some(String::from("probe-managed-workers")),
                worker_pool_revision: Some(String::from("probe-managed-workers-0001")),
                instance_id: Some(String::from("instance-1")),
                region: Some(String::from("us-central1")),
                logs_url: Some(String::from(
                    "https://console.cloud.google.com/run/workerpools",
                )),
            },
            capabilities: ManagedEnvironmentCapabilities::gcp_cloud_run_worker_pool(
                "worker-1",
                "gcp-coding-standard",
            ),
            system_prompt: None,
            harness_profile: None,
            tool_loop: None,
            poll_interval_ms: 0,
            max_iterations,
            exit_on_idle: true,
            dry_run: false,
            shutdown,
        }
    }

    fn test_assignment() -> ManagedCloudRunWorkerPoolAssignment {
        let mut assignment = ManagedCloudRunWorkerPoolAssignment::new(
            "assignment-1",
            "assignment-1:lease-1",
            "managed-session-1",
            "managed-run-1",
            "Return exactly: managed worker pool complete",
            "managed-environment-1",
            "gcp-coding-standard",
        );
        assignment.lease_id = Some(String::from("lease-1"));
        assignment.title = Some(String::from("Managed Worker Pool test"));
        assignment
    }

    fn test_profile(base_url: &str) -> BackendProfile {
        BackendProfile {
            name: String::from("test-profile"),
            kind: BackendKind::OpenAiChatCompletions,
            base_url: base_url.to_string(),
            model: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            reasoning_level: None,
            service_tier: None,
            api_key_env: String::from("PROBE_MANAGED_WORKER_POOL_TEST_KEY"),
            timeout_secs: 15,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
            control_plane: None,
            psionic_mesh: None,
        }
    }

    fn base_without_v1(base_url: &str) -> String {
        base_url.trim_end_matches("/v1").to_string()
    }

    struct ScopedEnvVar {
        key: String,
    }

    impl ScopedEnvVar {
        fn set(key: &str, value: &str) -> Self {
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
            unsafe {
                std::env::remove_var(&self.key);
            }
        }
    }
}
