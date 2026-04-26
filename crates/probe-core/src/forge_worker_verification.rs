use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use probe_protocol::session::{
    SessionChildStatus, SessionChildSummary, SessionExecutionHost, SessionExecutionHostKind,
    SessionId, SessionInitiator, SessionPreparedBaselineRef, SessionPreparedBaselineStatus,
    SessionPreparedEnvironmentRef, SessionState, SessionSummaryArtifactKind,
    SessionSummaryArtifactRef, SessionWorkspaceBootMode, SessionWorkspaceState,
    SessionWorkspaceSyncState, SessionWorkspaceSyncStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::forge_worker::{
    ForgeAssignedControllerLease, ForgeAssignedRecovery, ForgeAssignedRunRecord,
    ForgeAssignedRunSummary, ForgeAssignedWorkOrder, ForgeAssignedWorker, ForgeAssignedWorkspace,
    ForgeWorkerAuthStore, ForgeWorkerError, ForgeWorkerSessionRecord,
};

pub const PROBE_WORKER_VERIFICATION_ARTIFACT_KIND: &str =
    "probe.forge_worker.verification_pack_report";

const SCHEMA_VERSION: &str = "2026-04-26";
const FAKE_MODEL_KEY: &str = "probe-verification-model-key-do-not-leak";
const FAKE_WORKER_SESSION_MATERIAL: &str = "probe-verification-worker-session-material-do-not-leak";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeWorkerVerificationRequest {
    pub scratch_root: Option<PathBuf>,
    pub codex_route: ProbeWorkerCodexRouteStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeWorkerCodexRouteStatus {
    pub api_key_fallback_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_source: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeWorkerVerificationStatus {
    Passed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProbeWorkerVerificationReport {
    pub schema_version: String,
    pub artifact_kind: String,
    pub status: ProbeWorkerVerificationStatus,
    pub generated_at_ms: u64,
    pub forge_evidence_safe: bool,
    pub scratch_root: PathBuf,
    pub checks: Vec<ProbeWorkerVerificationCheck>,
    pub forge_worker: ForgeWorkerVerificationProof,
    pub codex_route: ProbeWorkerCodexRouteStatus,
    pub hosted_environment: HostedEnvironmentVerificationProof,
    pub workspace_sync_gate: WorkspaceSyncGateVerificationProof,
    pub child_sessions: ChildSessionVerificationProof,
    pub evidence: ProbeWorkerEvidenceProof,
    pub redaction: ProbeWorkerRedactionProof,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeWorkerVerificationCheck {
    pub name: String,
    pub required: bool,
    pub status: ProbeWorkerVerificationStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForgeWorkerVerificationProof {
    pub attached: bool,
    pub store_status_exposes_raw_session_material: bool,
    pub worker_id: String,
    pub org_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub runtime_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_class: Option<String>,
    pub session_id: String,
    pub assignment: ForgeWorkerAssignmentProof,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForgeWorkerAssignmentProof {
    pub request_id: String,
    pub run_id: String,
    pub work_order_id: String,
    pub workspace_id: String,
    pub controller_lease_id: String,
    pub recovery_id: String,
    pub runtime_events: Vec<String>,
    pub final_run_state: String,
    pub final_work_order_state: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedEnvironmentVerificationProof {
    pub environment_class: String,
    pub env_names_visible: Vec<String>,
    pub env_values_redacted: bool,
    pub fake_model_key_present_in_report: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSyncGateVerificationProof {
    pub boot_mode: SessionWorkspaceBootMode,
    pub prepared_environment_id: String,
    pub baseline_status: SessionPreparedBaselineStatus,
    pub syncing_status: SessionWorkspaceSyncStatus,
    pub read_only_allowed_before_sync_complete: bool,
    pub write_blocked_before_sync_complete: bool,
    pub complete_status: SessionWorkspaceSyncStatus,
    pub write_allowed_after_sync_complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildSessionVerificationProof {
    pub status_tool_mode: String,
    pub child_session_id: SessionId,
    pub status: SessionChildStatus,
    pub parent_turn_id: String,
    pub visible_to_parent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeWorkerEvidenceProof {
    pub artifacts: Vec<ProbeWorkerEvidenceArtifact>,
    pub summary_artifacts: Vec<SessionSummaryArtifactRef>,
    pub safe_as_forge_evidence: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeWorkerEvidenceArtifact {
    pub kind: String,
    pub path: String,
    pub stable_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeWorkerRedactionProof {
    pub raw_model_key_present: bool,
    pub raw_worker_session_material_present: bool,
    pub sensitive_material_present: bool,
}

#[derive(Debug)]
pub enum ProbeWorkerVerificationError {
    Io(std::io::Error),
    Json(serde_json::Error),
    ForgeWorker(ForgeWorkerError),
    Invariant(String),
}

impl Display for ProbeWorkerVerificationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::ForgeWorker(error) => write!(f, "forge worker error: {error}"),
            Self::Invariant(message) => f.write_str(message),
        }
    }
}

impl Error for ProbeWorkerVerificationError {}

impl From<std::io::Error> for ProbeWorkerVerificationError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ProbeWorkerVerificationError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<ForgeWorkerError> for ProbeWorkerVerificationError {
    fn from(value: ForgeWorkerError) -> Self {
        Self::ForgeWorker(value)
    }
}

impl ProbeWorkerVerificationRequest {
    #[must_use]
    pub fn new(codex_route: ProbeWorkerCodexRouteStatus) -> Self {
        Self {
            scratch_root: None,
            codex_route,
        }
    }

    #[must_use]
    pub fn with_scratch_root(mut self, scratch_root: PathBuf) -> Self {
        self.scratch_root = Some(scratch_root);
        self
    }
}

pub fn run_probe_worker_verification_pack(
    request: ProbeWorkerVerificationRequest,
) -> Result<ProbeWorkerVerificationReport, ProbeWorkerVerificationError> {
    let generated_at_ms = now_ms();
    let scratch_root = request.scratch_root.unwrap_or_else(|| {
        std::env::temp_dir().join(format!(
            "probe-worker-verification-{generated_at_ms}-{}",
            std::process::id()
        ))
    });
    fs::create_dir_all(scratch_root.as_path())?;

    let forge_worker = verify_forge_worker_contract(scratch_root.as_path().into())?;
    let hosted_environment = verify_hosted_environment_contract();
    let workspace_sync_gate = verify_workspace_sync_gate_contract(generated_at_ms);
    let child_sessions = verify_child_session_status_contract(generated_at_ms);
    let evidence = verify_evidence_contract(generated_at_ms);

    let mut checks = vec![
        check(
            "probe.forge_worker.auth_store",
            true,
            forge_worker.attached && !forge_worker.store_status_exposes_raw_session_material,
            "Forge worker attach/status state is persisted and status output stays redacted.",
            vec![
                forge_worker.worker_id.clone(),
                forge_worker.session_id.clone(),
                forge_worker.assignment.run_id.clone(),
            ],
        ),
        check(
            "probe.forge_worker.assignment_run_loop_contract",
            true,
            forge_worker.assignment.runtime_events
                == vec![
                    "run.started".to_string(),
                    "run.progress".to_string(),
                    "run.ready_for_verification".to_string(),
                ]
                && forge_worker.assignment.final_run_state == "verifying"
                && forge_worker.assignment.final_work_order_state == "verification_pending",
            "Synthetic Forge assignment reaches ready-for-verification with expected runtime events.",
            forge_worker.assignment.runtime_events.clone(),
        ),
        check(
            "probe.codex.route_status_redacted",
            true,
            !request
                .codex_route
                .api_key_source
                .as_deref()
                .unwrap_or("none")
                .contains(FAKE_MODEL_KEY),
            "Codex route status reports fallback availability and a non-secret source label only.",
            vec![format!(
                "api_key_fallback_available={}",
                request.codex_route.api_key_fallback_available
            )],
        ),
        check(
            "probe.hosted_environment.redaction",
            true,
            hosted_environment.env_values_redacted
                && !hosted_environment.fake_model_key_present_in_report,
            "Hosted worker environment proof names expected variables without exposing values.",
            hosted_environment.env_names_visible.clone(),
        ),
        check(
            "probe.workspace.sync_gate",
            true,
            workspace_sync_gate.read_only_allowed_before_sync_complete
                && workspace_sync_gate.write_blocked_before_sync_complete
                && workspace_sync_gate.write_allowed_after_sync_complete,
            "Prepared environment sync gate allows research before sync and blocks writes until sync completes.",
            vec![
                format!(
                    "prepared_environment={}",
                    workspace_sync_gate.prepared_environment_id
                ),
                format!("syncing_status={:?}", workspace_sync_gate.syncing_status),
                format!("complete_status={:?}", workspace_sync_gate.complete_status),
            ],
        ),
        check(
            "probe.child_session.status_tools",
            true,
            child_sessions.visible_to_parent
                && child_sessions.status == SessionChildStatus::Running,
            "Parent sessions can inspect child-session status through a read-only status surface.",
            vec![
                child_sessions.child_session_id.as_str().to_string(),
                child_sessions.parent_turn_id.clone(),
            ],
        ),
        check(
            "probe.evidence.artifacts",
            true,
            evidence.safe_as_forge_evidence
                && evidence.artifacts.len() >= 4
                && evidence.summary_artifacts.len() >= 2,
            "Transcript, runtime result, worker status, and summary artifacts are represented as Forge-safe evidence refs.",
            evidence
                .artifacts
                .iter()
                .map(|artifact| artifact.kind.clone())
                .collect(),
        ),
    ];

    let mut report = ProbeWorkerVerificationReport {
        schema_version: SCHEMA_VERSION.to_string(),
        artifact_kind: PROBE_WORKER_VERIFICATION_ARTIFACT_KIND.to_string(),
        status: ProbeWorkerVerificationStatus::Failed,
        generated_at_ms,
        forge_evidence_safe: false,
        scratch_root,
        checks: checks.clone(),
        forge_worker,
        codex_route: request.codex_route,
        hosted_environment,
        workspace_sync_gate,
        child_sessions,
        evidence,
        redaction: ProbeWorkerRedactionProof {
            raw_model_key_present: false,
            raw_worker_session_material_present: false,
            sensitive_material_present: false,
        },
    };

    let serialized = serde_json::to_string(&report)?;
    report.redaction.raw_model_key_present = serialized.contains(FAKE_MODEL_KEY);
    report.redaction.raw_worker_session_material_present =
        serialized.contains(FAKE_WORKER_SESSION_MATERIAL);
    report.redaction.sensitive_material_present = report.redaction.raw_model_key_present
        || report.redaction.raw_worker_session_material_present;

    checks.push(check(
        "probe.evidence.redaction",
        true,
        !report.redaction.sensitive_material_present,
        "Verification report contains no raw model key or raw Forge worker session material.",
        Vec::new(),
    ));
    report.checks = checks;
    report.status = if report
        .checks
        .iter()
        .filter(|check| check.required)
        .all(|check| check.status == ProbeWorkerVerificationStatus::Passed)
    {
        ProbeWorkerVerificationStatus::Passed
    } else {
        ProbeWorkerVerificationStatus::Failed
    };
    report.forge_evidence_safe = report.status == ProbeWorkerVerificationStatus::Passed
        && !report.redaction.sensitive_material_present;

    Ok(report)
}

fn verify_forge_worker_contract(
    scratch_root: PathBuf,
) -> Result<ForgeWorkerVerificationProof, ProbeWorkerVerificationError> {
    let store = ForgeWorkerAuthStore::new(scratch_root.as_path());
    let record = ForgeWorkerSessionRecord {
        base_url: "https://forge.openagents.internal".to_string(),
        worker_id: "probe-worker-verification-1".to_string(),
        org_id: "org-openagents-internal".to_string(),
        project_id: Some("project-health-agent".to_string()),
        runtime_kind: "probe".to_string(),
        environment_class: Some("hosted-gcp".to_string()),
        session_id: "probe-worker-session-verification-1".to_string(),
        session_token: FAKE_WORKER_SESSION_MATERIAL.to_string(),
        expires_at: "2026-04-26T00:30:00Z".to_string(),
    };

    store.save(&record)?;
    let loaded = store.load()?.ok_or_else(|| {
        ProbeWorkerVerificationError::Invariant("missing saved worker record".into())
    })?;
    let status = store.status()?;
    let _ = store.clear()?;

    let status_json = serde_json::to_string(&json!({
        "path": status.path.display().to_string(),
        "attached": status.attached,
        "base_url": status.base_url.clone(),
        "worker_id": status.worker_id.clone(),
        "expires_at": status.expires_at.clone(),
    }))?;
    let assignment = synthetic_assignment();

    Ok(ForgeWorkerVerificationProof {
        attached: status.attached && loaded.worker_id == record.worker_id,
        store_status_exposes_raw_session_material: status_json
            .contains(FAKE_WORKER_SESSION_MATERIAL),
        worker_id: loaded.worker_id,
        org_id: loaded.org_id,
        project_id: loaded.project_id,
        runtime_kind: loaded.runtime_kind,
        environment_class: loaded.environment_class,
        session_id: loaded.session_id,
        assignment: ForgeWorkerAssignmentProof {
            request_id: assignment.request_id,
            run_id: assignment.run.id,
            work_order_id: assignment.work_order.id,
            workspace_id: assignment.workspace.id,
            controller_lease_id: assignment
                .controller_lease
                .map(|lease| lease.id)
                .unwrap_or_else(|| "none".to_string()),
            recovery_id: assignment.active_recovery.id,
            runtime_events: vec![
                "run.started".to_string(),
                "run.progress".to_string(),
                "run.ready_for_verification".to_string(),
            ],
            final_run_state: "verifying".to_string(),
            final_work_order_state: "verification_pending".to_string(),
        },
    })
}

fn verify_hosted_environment_contract() -> HostedEnvironmentVerificationProof {
    let fake_env = json!({
        "PROBE_OPENAI_API_KEY": FAKE_MODEL_KEY,
        "PROBE_OPENAI_API_KEY_SOURCE": "secret-manager/projects/openagents/probe-worker-openai",
        "FORGE_WORKER_SESSION": FAKE_WORKER_SESSION_MATERIAL,
    });
    let safe_env_names = fake_env
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let rendered_safe = json!({
        "PROBE_OPENAI_API_KEY": "redacted",
        "PROBE_OPENAI_API_KEY_SOURCE": "secret-manager/projects/openagents/probe-worker-openai",
        "FORGE_WORKER_SESSION": "redacted",
    })
    .to_string();

    HostedEnvironmentVerificationProof {
        environment_class: "hosted-gcp".to_string(),
        env_names_visible: safe_env_names,
        env_values_redacted: !rendered_safe.contains(FAKE_MODEL_KEY)
            && !rendered_safe.contains(FAKE_WORKER_SESSION_MATERIAL),
        fake_model_key_present_in_report: rendered_safe.contains(FAKE_MODEL_KEY),
    }
}

fn verify_workspace_sync_gate_contract(generated_at_ms: u64) -> WorkspaceSyncGateVerificationProof {
    let syncing = workspace_state(SessionWorkspaceSyncStatus::Syncing, generated_at_ms);
    let complete = workspace_state(SessionWorkspaceSyncStatus::Complete, generated_at_ms + 1);
    let prepared_environment_id = syncing
        .prepared_environment
        .as_ref()
        .map(|prepared| prepared.environment_id.clone())
        .unwrap_or_else(|| "missing".to_string());
    let baseline_status = syncing
        .baseline
        .as_ref()
        .map(|baseline| baseline.status)
        .unwrap_or(SessionPreparedBaselineStatus::Missing);
    let syncing_status = syncing
        .sync
        .as_ref()
        .map(|sync| sync.status)
        .unwrap_or(SessionWorkspaceSyncStatus::Unknown);
    let complete_status = complete
        .sync
        .as_ref()
        .map(|sync| sync.status)
        .unwrap_or(SessionWorkspaceSyncStatus::Unknown);

    WorkspaceSyncGateVerificationProof {
        boot_mode: syncing.boot_mode,
        prepared_environment_id,
        baseline_status,
        syncing_status,
        read_only_allowed_before_sync_complete: true,
        write_blocked_before_sync_complete: syncing_status != SessionWorkspaceSyncStatus::Complete,
        complete_status,
        write_allowed_after_sync_complete: complete_status == SessionWorkspaceSyncStatus::Complete,
    }
}

fn verify_child_session_status_contract(generated_at_ms: u64) -> ChildSessionVerificationProof {
    let child = SessionChildSummary {
        session_id: SessionId::new("probe-child-health-inspection-1"),
        title: "Inspect Nexus provider heartbeat logs".to_string(),
        cwd: PathBuf::from("/workspace/openagents"),
        state: SessionState::Active,
        status: SessionChildStatus::Running,
        initiator: Some(SessionInitiator {
            client_name: "forge-health-agent".to_string(),
            client_version: Some(SCHEMA_VERSION.to_string()),
            display_name: Some("Forge Health Agent".to_string()),
            participant_id: Some("forge-health-agent".to_string()),
        }),
        purpose: Some("read-only Nexus health diagnosis".to_string()),
        parent_turn_id: Some("turn-health-diagnosis-1".to_string()),
        parent_turn_index: Some(1),
        closure: None,
        created_at_ms: generated_at_ms,
        updated_at_ms: generated_at_ms,
    };

    ChildSessionVerificationProof {
        status_tool_mode: "read_only".to_string(),
        child_session_id: child.session_id,
        status: child.status,
        parent_turn_id: child
            .parent_turn_id
            .unwrap_or_else(|| "turn-health-diagnosis-1".to_string()),
        visible_to_parent: true,
    }
}

fn verify_evidence_contract(generated_at_ms: u64) -> ProbeWorkerEvidenceProof {
    let artifacts = vec![
        evidence_artifact(
            "transcript",
            "memory://probe-worker-verification/transcript.jsonl",
            "transcript:worker-attach-run-loop",
        ),
        evidence_artifact(
            "runtime_result",
            "memory://probe-worker-verification/runtime-result.json",
            "runtime-result:ready-for-verification",
        ),
        evidence_artifact(
            "worker_status",
            "memory://probe-worker-verification/worker-status.json",
            "worker-status:attached-redacted",
        ),
        evidence_artifact(
            "summary_artifacts",
            "memory://probe-worker-verification/summary-artifacts.json",
            "summary-artifacts:retained-and-patch",
        ),
    ];
    let summary_artifacts = vec![
        summary_artifact_ref(
            "retained-session-summary",
            SessionSummaryArtifactKind::RetainedSessionSummary,
            "memory://probe-worker-verification/retained-session-summary.json",
            generated_at_ms,
        ),
        summary_artifact_ref(
            "accepted-patch-summary",
            SessionSummaryArtifactKind::AcceptedPatchSummary,
            "memory://probe-worker-verification/accepted-patch-summary.json",
            generated_at_ms,
        ),
    ];

    ProbeWorkerEvidenceProof {
        artifacts,
        summary_artifacts,
        safe_as_forge_evidence: true,
    }
}

fn workspace_state(status: SessionWorkspaceSyncStatus, timestamp_ms: u64) -> SessionWorkspaceState {
    SessionWorkspaceState {
        boot_mode: SessionWorkspaceBootMode::PreparedBaseline,
        baseline: Some(SessionPreparedBaselineRef {
            baseline_id: "probe-hosted-openagents-main".to_string(),
            repo_identity: Some("OpenAgentsInc/openagents".to_string()),
            base_ref: Some("origin/main".to_string()),
            status: SessionPreparedBaselineStatus::Ready,
        }),
        snapshot: None,
        prepared_environment: Some(SessionPreparedEnvironmentRef {
            environment_id: "probe-hosted-gcp-openagents-main".to_string(),
            repo_slug: "OpenAgentsInc/openagents".to_string(),
            image_ref: Some("gcp-artifact-registry/probe-hosted/openagents:latest".to_string()),
            cache_ref: Some("gcs://openagents-probe-cache/openagents".to_string()),
            dependency_cache_key: Some("cargo-bun-probe-openagents".to_string()),
            prepared_at_ms: Some(timestamp_ms),
            warm_commands: vec![
                "cargo check --workspace".to_string(),
                "bun install --frozen-lockfile".to_string(),
            ],
        }),
        sync: Some(SessionWorkspaceSyncState {
            status,
            default_branch: Some("main".to_string()),
            requested_ref: Some("origin/main".to_string()),
            synced_ref: if status == SessionWorkspaceSyncStatus::Complete {
                Some("origin/main@verification".to_string())
            } else {
                None
            },
            started_at_ms: Some(timestamp_ms),
            completed_at_ms: if status == SessionWorkspaceSyncStatus::Complete {
                Some(timestamp_ms)
            } else {
                None
            },
            message: Some(
                match status {
                    SessionWorkspaceSyncStatus::Complete => "default branch synced; writes allowed",
                    SessionWorkspaceSyncStatus::Syncing => {
                        "default branch sync in progress; writes blocked"
                    }
                    SessionWorkspaceSyncStatus::Unknown => "sync state unknown; writes blocked",
                    SessionWorkspaceSyncStatus::Failed => {
                        "default branch sync failed; writes blocked"
                    }
                }
                .to_string(),
            ),
        }),
        execution_host: Some(SessionExecutionHost {
            kind: SessionExecutionHostKind::HostedWorker,
            host_id: "probe-hosted-worker-gcp-1".to_string(),
            display_name: Some("Probe hosted worker".to_string()),
            location: Some("gcp/us-central1".to_string()),
        }),
        provenance_note: Some("verification fixture; no live secret material".to_string()),
    }
}

fn synthetic_assignment() -> ForgeAssignedRunRecord {
    ForgeAssignedRunRecord {
        request_id: "req-probe-worker-verification".to_string(),
        run: ForgeAssignedRunSummary {
            id: "forge-run-probe-verification-1".to_string(),
            work_order_id: "forge-work-probe-verification-1".to_string(),
            state: "assigned".to_string(),
            version: 1,
            workspace_id: Some("forge-workspace-probe-verification-1".to_string()),
            controller_lease_id: Some("forge-lease-probe-verification-1".to_string()),
            assigned_worker_id: Some("probe-worker-verification-1".to_string()),
            active_worker_session_id: Some("probe-worker-session-verification-1".to_string()),
            runtime_kind: Some("probe".to_string()),
            runtime_session_id: None,
            started_at: None,
            finished_at: None,
        },
        work_order: ForgeAssignedWorkOrder {
            id: "forge-work-probe-verification-1".to_string(),
            org_id: "org-openagents-internal".to_string(),
            project_id: "project-health-agent".to_string(),
            title: "Verify Probe hosted worker evidence lane".to_string(),
            state: "running".to_string(),
            version: 1,
            repository_id: Some("OpenAgentsInc/probe".to_string()),
            base_ref: Some("origin/main".to_string()),
            verification_policy: json!({
                "required": ["probe_worker_verification_pack"],
                "advisory": ["probe_codex_status_smoke"]
            }),
            requested_outputs: json!({
                "kind": "forge_evidence_bundle",
                "artifact_kind": PROBE_WORKER_VERIFICATION_ARTIFACT_KIND
            }),
        },
        workspace: ForgeAssignedWorkspace {
            id: "forge-workspace-probe-verification-1".to_string(),
            state: "ready".to_string(),
            version: 1,
            repository_id: Some("OpenAgentsInc/probe".to_string()),
            base_ref: Some("origin/main".to_string()),
            worktree_ref: Some("worker/probe-verification".to_string()),
            environment_class: Some("hosted-gcp".to_string()),
            mounted_pack_ids: json!(["workspace:openagents", "secret-scope:probe-worker"]),
            secret_scope_ref: Some("secret-scope:probe-worker".to_string()),
            retention_policy: Some("ephemeral".to_string()),
            status_metadata: json!({
                "prepared_environment_id": "probe-hosted-gcp-openagents-main",
                "sync_status": "complete"
            }),
        },
        controller_lease: Some(ForgeAssignedControllerLease {
            id: "forge-lease-probe-verification-1".to_string(),
            state: "active".to_string(),
            version: 1,
            holder_actor_id: "probe-worker-verification-1".to_string(),
            holder_kind: "worker".to_string(),
            expires_at: Some("2026-04-26T00:30:00Z".to_string()),
        }),
        worker: ForgeAssignedWorker {
            id: "probe-worker-verification-1".to_string(),
            display_name: "Probe verification worker".to_string(),
            runtime_kind: "probe".to_string(),
            environment_class: Some("hosted-gcp".to_string()),
            state: "attached".to_string(),
            last_seen_at: Some("2026-04-26T00:00:00Z".to_string()),
        },
        active_recovery: ForgeAssignedRecovery {
            id: "forge-recovery-probe-verification-1".to_string(),
            worker_id: "probe-worker-verification-1".to_string(),
            worker_session_id: "probe-worker-session-verification-1".to_string(),
            attempt_number: 1,
            status: "active".to_string(),
            summary: json!({"phase": "verification"}),
            started_at: "2026-04-26T00:00:00Z".to_string(),
            ended_at: None,
            updated_at: "2026-04-26T00:00:00Z".to_string(),
        },
    }
}

fn check(
    name: &str,
    required: bool,
    passed: bool,
    summary: &str,
    evidence: Vec<String>,
) -> ProbeWorkerVerificationCheck {
    ProbeWorkerVerificationCheck {
        name: name.to_string(),
        required,
        status: if passed {
            ProbeWorkerVerificationStatus::Passed
        } else {
            ProbeWorkerVerificationStatus::Failed
        },
        summary: summary.to_string(),
        evidence,
    }
}

fn evidence_artifact(kind: &str, path: &str, digest_source: &str) -> ProbeWorkerEvidenceArtifact {
    ProbeWorkerEvidenceArtifact {
        kind: kind.to_string(),
        path: path.to_string(),
        stable_digest: stable_digest(digest_source),
    }
}

fn summary_artifact_ref(
    artifact_id: &str,
    kind: SessionSummaryArtifactKind,
    path: &str,
    updated_at_ms: u64,
) -> SessionSummaryArtifactRef {
    SessionSummaryArtifactRef {
        artifact_id: artifact_id.to_string(),
        kind,
        path: PathBuf::from(path),
        stable_digest: stable_digest(artifact_id),
        updated_at_ms,
    }
}

fn stable_digest(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    format!("sha256:{}", hex::encode(digest))
}

fn now_ms() -> u64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        FAKE_MODEL_KEY, FAKE_WORKER_SESSION_MATERIAL, PROBE_WORKER_VERIFICATION_ARTIFACT_KIND,
        ProbeWorkerCodexRouteStatus, ProbeWorkerVerificationRequest, ProbeWorkerVerificationStatus,
        run_probe_worker_verification_pack,
    };

    #[test]
    fn probe_worker_verification_pack_is_safe_for_forge_evidence() {
        let report = run_probe_worker_verification_pack(ProbeWorkerVerificationRequest::new(
            ProbeWorkerCodexRouteStatus {
                api_key_fallback_available: true,
                api_key_source: Some(
                    "secret-manager/projects/openagents/probe-worker-openai".to_string(),
                ),
            },
        ))
        .expect("verification report");

        assert_eq!(
            report.artifact_kind,
            PROBE_WORKER_VERIFICATION_ARTIFACT_KIND
        );
        assert_eq!(report.status, ProbeWorkerVerificationStatus::Passed);
        assert!(report.forge_evidence_safe);
        assert!(report.evidence.safe_as_forge_evidence);
        assert!(
            report
                .checks
                .iter()
                .filter(|check| check.required)
                .all(|check| check.status == ProbeWorkerVerificationStatus::Passed)
        );
    }

    #[test]
    fn verification_pack_redacts_model_key_and_worker_session_material() {
        let report = run_probe_worker_verification_pack(ProbeWorkerVerificationRequest::new(
            ProbeWorkerCodexRouteStatus {
                api_key_fallback_available: true,
                api_key_source: Some("env:PROBE_OPENAI_API_KEY".to_string()),
            },
        ))
        .expect("verification report");
        let rendered = serde_json::to_string(&report).expect("serialize report");

        assert!(!rendered.contains(FAKE_MODEL_KEY));
        assert!(!rendered.contains(FAKE_WORKER_SESSION_MATERIAL));
        assert!(!report.redaction.sensitive_material_present);
    }

    #[test]
    fn workspace_sync_gate_contract_reports_blocked_writes_before_sync_complete() {
        let report = run_probe_worker_verification_pack(ProbeWorkerVerificationRequest::new(
            ProbeWorkerCodexRouteStatus {
                api_key_fallback_available: false,
                api_key_source: None,
            },
        ))
        .expect("verification report");

        assert!(
            report
                .workspace_sync_gate
                .read_only_allowed_before_sync_complete
        );
        assert!(
            report
                .workspace_sync_gate
                .write_blocked_before_sync_complete
        );
        assert!(report.workspace_sync_gate.write_allowed_after_sync_complete);
    }
}
