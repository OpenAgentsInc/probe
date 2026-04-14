use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use probe_protocol::backend::BackendProfile;
use serde_json::{Value, json};

use crate::forge_worker::{ForgeAssignedRunRecord, ForgeWorkerAuthController, ForgeWorkerError};
use crate::runtime::{
    PlainTextExecOutcome, PlainTextExecRequest, ProbeRuntime, RuntimeError, RuntimeEvent,
    RuntimeEventSink,
};
use crate::tools::ToolLoopConfig;

#[derive(Clone, Debug)]
pub struct ForgeAssignedRunExecutionRequest {
    pub profile: BackendProfile,
    pub default_cwd: PathBuf,
    pub system_prompt: Option<String>,
    pub tool_loop: Option<ToolLoopConfig>,
}

#[derive(Clone, Debug)]
pub enum ForgeAssignedRunExecutionOutcome {
    Idle,
    ExistingActiveRun { assignment: ForgeAssignedRunRecord },
    Executed(ForgeAssignedRunExecutionResult),
}

#[derive(Clone, Debug)]
pub struct ForgeAssignedRunExecutionResult {
    pub assignment: ForgeAssignedRunRecord,
    pub probe_session_id: Option<String>,
    pub final_run_state: String,
    pub assistant_text: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub enum ForgeAssignedRunExecutionError {
    Forge(ForgeWorkerError),
    Runtime(RuntimeError),
    Reporting(String),
}

impl std::fmt::Display for ForgeAssignedRunExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Forge(error) => write!(f, "{error}"),
            Self::Runtime(error) => write!(f, "{error}"),
            Self::Reporting(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ForgeAssignedRunExecutionError {}

impl From<ForgeWorkerError> for ForgeAssignedRunExecutionError {
    fn from(value: ForgeWorkerError) -> Self {
        Self::Forge(value)
    }
}

impl From<RuntimeError> for ForgeAssignedRunExecutionError {
    fn from(value: RuntimeError) -> Self {
        Self::Runtime(value)
    }
}

#[derive(Clone, Debug)]
pub struct ForgeAssignedRunExecutor {
    forge: ForgeWorkerAuthController,
    runtime: ProbeRuntime,
}

impl ForgeAssignedRunExecutor {
    pub fn new(forge: ForgeWorkerAuthController, runtime: ProbeRuntime) -> Self {
        Self { forge, runtime }
    }

    pub fn run_once(
        &self,
        request: ForgeAssignedRunExecutionRequest,
    ) -> Result<ForgeAssignedRunExecutionOutcome, ForgeAssignedRunExecutionError> {
        let assignment = match self.forge.current_run()? {
            Some(assignment) => {
                if assignment.run.runtime_session_id.is_some() || assignment.run.state == "running"
                {
                    return Ok(ForgeAssignedRunExecutionOutcome::ExistingActiveRun { assignment });
                }
                assignment
            }
            None => match self.forge.claim_next_run()? {
                Some(assignment) => assignment,
                None => return Ok(ForgeAssignedRunExecutionOutcome::Idle),
            },
        };

        self.forge.heartbeat(
            "busy",
            Some(assignment.run.id.as_str()),
            Some(json!({
                "phase": "starting",
                "forge_run_id": assignment.run.id,
            })),
        )?;

        let reporter_state = Arc::new(Mutex::new(ForgeEventReporterState::default()));
        let event_sink: Arc<dyn RuntimeEventSink> = Arc::new(ForgeEventReporter {
            forge: self.forge.clone(),
            run_id: assignment.run.id.clone(),
            state: Arc::clone(&reporter_state),
        });

        let exec_result = self.runtime.exec_plain_text_with_events(
            PlainTextExecRequest {
                profile: request.profile,
                prompt: build_assignment_prompt(&assignment),
                title: Some(format!("Forge: {}", assignment.work_order.title)),
                cwd: execution_cwd(&assignment, &request.default_cwd),
                system_prompt: request.system_prompt,
                harness_profile: None,
                tool_loop: request.tool_loop,
            },
            event_sink,
        );

        let reporter = reporter_state
            .lock()
            .expect("forge event reporter mutex")
            .clone();

        if let Some(error) = reporter.reporting_error {
            return Err(ForgeAssignedRunExecutionError::Reporting(error));
        }

        match exec_result {
            Ok(outcome) => {
                let runtime_session_id = reporter
                    .runtime_session_id
                    .clone()
                    .unwrap_or_else(|| outcome.session.id.as_str().to_string());
                let final_state = self.forge.record_run_event(
                    assignment.run.id.as_str(),
                    "run.ready_for_verification",
                    Some(runtime_session_id.as_str()),
                    ready_for_verification_summary(&outcome),
                )?;
                self.forge
                    .heartbeat("attached", None, Some(json!({"phase":"idle"})))?;
                Ok(ForgeAssignedRunExecutionOutcome::Executed(
                    ForgeAssignedRunExecutionResult {
                        assignment,
                        probe_session_id: Some(runtime_session_id),
                        final_run_state: final_state.run_state,
                        assistant_text: Some(outcome.assistant_text),
                        error: None,
                    },
                ))
            }
            Err(error) => {
                let runtime_session_id = reporter.runtime_session_id.clone();
                let final_state = self.forge.record_run_event(
                    assignment.run.id.as_str(),
                    "run.failed",
                    runtime_session_id.as_deref(),
                    json!({
                        "error": error.to_string(),
                    }),
                )?;
                self.forge.heartbeat(
                    "attached",
                    None,
                    Some(json!({"phase":"idle_after_error"})),
                )?;
                Ok(ForgeAssignedRunExecutionOutcome::Executed(
                    ForgeAssignedRunExecutionResult {
                        assignment,
                        probe_session_id: runtime_session_id,
                        final_run_state: final_state.run_state,
                        assistant_text: None,
                        error: Some(error.to_string()),
                    },
                ))
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
struct ForgeEventReporterState {
    runtime_session_id: Option<String>,
    started_reported: bool,
    progress_reported: bool,
    reporting_error: Option<String>,
}

struct ForgeEventReporter {
    forge: ForgeWorkerAuthController,
    run_id: String,
    state: Arc<Mutex<ForgeEventReporterState>>,
}

impl RuntimeEventSink for ForgeEventReporter {
    fn emit(&self, event: RuntimeEvent) {
        let mut state = self.state.lock().expect("forge event reporter mutex");
        if state.reporting_error.is_some() {
            return;
        }

        match event {
            RuntimeEvent::TurnStarted {
                session_id,
                profile_name,
                ..
            } => {
                state.runtime_session_id = Some(session_id.as_str().to_string());
                if !state.started_reported {
                    if let Err(error) = self.forge.record_run_event(
                        self.run_id.as_str(),
                        "run.started",
                        Some(session_id.as_str()),
                        json!({
                            "phase": "turn_started",
                            "profile_name": profile_name,
                        }),
                    ) {
                        state.reporting_error = Some(error.to_string());
                        return;
                    }
                    state.started_reported = true;
                }
            }
            RuntimeEvent::ModelRequestStarted {
                round_trip,
                backend_kind,
                ..
            } => {
                if let Err(error) = self.forge.heartbeat(
                    "busy",
                    Some(self.run_id.as_str()),
                    Some(json!({
                        "phase": "model_request_started",
                        "round_trip": round_trip,
                    })),
                ) {
                    state.reporting_error = Some(error.to_string());
                    return;
                }

                if !state.progress_reported {
                    if let Err(error) = self.forge.record_run_event(
                        self.run_id.as_str(),
                        "run.progress",
                        state.runtime_session_id.as_deref(),
                        json!({
                            "phase": "model_request_started",
                            "round_trip": round_trip,
                            "backend_kind": format!("{backend_kind:?}"),
                        }),
                    ) {
                        state.reporting_error = Some(error.to_string());
                        return;
                    }
                    state.progress_reported = true;
                }
            }
            RuntimeEvent::ToolExecutionStarted {
                call_id, tool_name, ..
            } => {
                if let Err(error) = self.forge.heartbeat(
                    "busy",
                    Some(self.run_id.as_str()),
                    Some(json!({
                        "phase": "tool_execution_started",
                        "call_id": call_id,
                        "tool_name": tool_name,
                    })),
                ) {
                    state.reporting_error = Some(error.to_string());
                }
            }
            RuntimeEvent::ToolExecutionCompleted { tool, .. }
            | RuntimeEvent::ToolRefused { tool, .. }
            | RuntimeEvent::ToolPaused { tool, .. } => {
                if let Err(error) = self.forge.heartbeat(
                    "busy",
                    Some(self.run_id.as_str()),
                    Some(json!({
                        "phase": "tool_result",
                        "tool_name": tool.name,
                    })),
                ) {
                    state.reporting_error = Some(error.to_string());
                }
            }
            _ => {}
        }
    }
}

fn build_assignment_prompt(assignment: &ForgeAssignedRunRecord) -> String {
    let requested_outputs = serde_json::to_string_pretty(&assignment.work_order.requested_outputs)
        .unwrap_or_else(|_| assignment.work_order.requested_outputs.to_string());
    let verification_policy =
        serde_json::to_string_pretty(&assignment.work_order.verification_policy)
            .unwrap_or_else(|_| assignment.work_order.verification_policy.to_string());

    format!(
        "You are executing a Forge-assigned software work order.\n\nTitle: {}\nRepository: {}\nBase ref: {}\nWorkspace environment: {}\nRequested outputs:\n{}\n\nVerification policy:\n{}\n\nCarry out the requested software work in the checked-out repository and leave the session ready for Forge verification.",
        assignment.work_order.title,
        assignment
            .workspace
            .repository_id
            .as_deref()
            .or(assignment.work_order.repository_id.as_deref())
            .unwrap_or("unknown"),
        assignment
            .workspace
            .base_ref
            .as_deref()
            .or(assignment.work_order.base_ref.as_deref())
            .unwrap_or("unknown"),
        assignment
            .workspace
            .environment_class
            .as_deref()
            .unwrap_or("default"),
        requested_outputs,
        verification_policy,
    )
}

fn execution_cwd(assignment: &ForgeAssignedRunRecord, default_cwd: &PathBuf) -> PathBuf {
    assignment
        .workspace
        .worktree_ref
        .as_ref()
        .map(PathBuf::from)
        .filter(|candidate| candidate.exists())
        .unwrap_or_else(|| default_cwd.clone())
}

fn ready_for_verification_summary(outcome: &PlainTextExecOutcome) -> Value {
    json!({
        "assistant_text": outcome.assistant_text,
        "response_id": outcome.response_id,
        "response_model": outcome.response_model,
        "executed_tool_calls": outcome.executed_tool_calls,
        "tool_results": outcome.tool_results.len(),
        "usage": outcome.usage.as_ref().map(|usage| json!({
            "prompt_tokens": usage.prompt_tokens,
            "completion_tokens": usage.completion_tokens,
            "total_tokens": usage.total_tokens,
        })),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use probe_test_support::{FakeHttpRequest, FakeHttpResponse, FakeOpenAiServer};
    use tempfile::tempdir;

    use super::{
        ForgeAssignedRunExecutionOutcome, ForgeAssignedRunExecutionRequest,
        ForgeAssignedRunExecutor,
    };
    use crate::forge_worker::ForgeWorkerAuthController;
    use crate::runtime::ProbeRuntime;
    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use serde_json::json;

    #[test]
    fn forge_assigned_run_executes_and_reports_lifecycle_events() {
        let forge_requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let forge_requests_thread = Arc::clone(&forge_requests);
        let forge = FakeOpenAiServer::from_handler(move |request: FakeHttpRequest| {
            forge_requests_thread
                .lock()
                .expect("forge request lock")
                .push(format!("{} {}", request.method, request.path));

            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/worker/v1/attach") => FakeHttpResponse::json_ok(json!({
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
                ("GET", "/worker/v1/runs/current") => FakeHttpResponse::json_ok(json!({
                    "request_id": "req-current",
                    "assignment": null
                })),
                ("POST", "/worker/v1/runs/claim-next") => FakeHttpResponse::json_ok(json!({
                    "request_id": "req-claim",
                    "assignment": {
                        "run": {
                            "id": "forge-run-1",
                            "work_order_id": "forge-work-1",
                            "state": "starting",
                            "version": 2,
                            "workspace_id": "forge-workspace-1",
                            "controller_lease_id": "forge-lease-1",
                            "assigned_worker_id": "forge-worker-1",
                            "active_worker_session_id": "forge-worker-session-1",
                            "runtime": {
                                "kind": "probe",
                                "session_id": null,
                                "summary": {}
                            },
                            "started_at": null,
                            "finished_at": null
                        },
                        "work_order": {
                            "id": "forge-work-1",
                            "org_id": "org-1",
                            "project_id": "project-1",
                            "title": "Implement Forge run loop",
                            "state": "leased",
                            "version": 2,
                            "repository_id": "repo-1",
                            "base_ref": "main",
                            "verification_policy": { "required_checks": ["tests"] },
                            "requested_outputs": ["patch", "verification_report"]
                        },
                        "workspace": {
                            "id": "forge-workspace-1",
                            "state": "ready",
                            "version": 2,
                            "repository_id": "repo-1",
                            "base_ref": "main",
                            "worktree_ref": null,
                            "environment_class": "linux-dev",
                            "mounted_pack_ids": [],
                            "secret_scope_ref": null,
                            "retention_policy": "retain_until_delivery",
                            "status_metadata": {}
                        },
                        "controller_lease": {
                            "id": "forge-lease-1",
                            "state": "active",
                            "version": 1,
                            "holder_actor_id": "controller-1",
                            "holder_kind": "worker",
                            "expires_at": null
                        },
                        "worker": {
                            "id": "forge-worker-1",
                            "display_name": "Forge worker",
                            "runtime_kind": "probe",
                            "environment_class": "linux-dev",
                            "state": "busy",
                            "last_seen_at": null
                        },
                        "active_recovery": {
                            "id": "forge-run-recovery-1",
                            "worker_id": "forge-worker-1",
                            "worker_session_id": "forge-worker-session-1",
                            "attempt_number": 1,
                            "status": "active",
                            "summary": {},
                            "started_at": "2026-04-14T18:00:00Z",
                            "ended_at": null,
                            "updated_at": "2026-04-14T18:00:00Z"
                        }
                    }
                })),
                ("POST", "/worker/v1/heartbeat") => {
                    let body: serde_json::Value =
                        serde_json::from_str(request.body.as_str()).expect("heartbeat body");
                    let state = body["state"].as_str().unwrap_or("attached");
                    FakeHttpResponse::json_ok(json!({
                        "request_id": "req-heartbeat",
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
                            "state": state
                        }
                    }))
                }
                ("POST", path) if path.starts_with("/worker/v1/runs/forge-run-1/events") => {
                    let body: serde_json::Value =
                        serde_json::from_str(request.body.as_str()).expect("event body");
                    let event_type = body["event_type"].as_str().unwrap_or("run.progress");
                    let run_state = match event_type {
                        "run.started" | "run.progress" => "running",
                        "run.ready_for_verification" => "verifying",
                        "run.failed" => "failed",
                        _ => "running",
                    };
                    let work_order_state = match event_type {
                        "run.ready_for_verification" => "verification_pending",
                        "run.failed" => "failed",
                        _ => "running",
                    };
                    let recovery_status = match event_type {
                        "run.ready_for_verification" => "completed",
                        "run.failed" => "failed",
                        _ => "active",
                    };
                    FakeHttpResponse::json_ok(json!({
                        "run": {
                            "id": "forge-run-1",
                            "work_order_id": "forge-work-1",
                            "state": run_state,
                            "version": 3,
                            "workspace_id": "forge-workspace-1",
                            "controller_lease_id": "forge-lease-1",
                            "assigned_worker_id": "forge-worker-1",
                            "active_worker_session_id": if event_type == "run.ready_for_verification" { serde_json::Value::Null } else { json!("forge-worker-session-1") },
                            "runtime": {
                                "kind": "probe",
                                "session_id": body["runtime_session_id"],
                                "summary": {}
                            },
                            "started_at": "2026-04-14T18:00:01Z",
                            "finished_at": if event_type == "run.ready_for_verification" { json!("2026-04-14T18:00:02Z") } else { serde_json::Value::Null }
                        },
                        "work_order": {
                            "id": "forge-work-1",
                            "org_id": "org-1",
                            "project_id": "project-1",
                            "title": "Implement Forge run loop",
                            "state": work_order_state,
                            "version": 3,
                            "repository_id": "repo-1",
                            "base_ref": "main",
                            "verification_policy": { "required_checks": ["tests"] },
                            "requested_outputs": ["patch", "verification_report"]
                        },
                        "workspace": {
                            "id": "forge-workspace-1",
                            "state": "ready",
                            "version": 2,
                            "repository_id": "repo-1",
                            "base_ref": "main",
                            "worktree_ref": null,
                            "environment_class": "linux-dev",
                            "mounted_pack_ids": [],
                            "secret_scope_ref": null,
                            "retention_policy": "retain_until_delivery",
                            "status_metadata": {}
                        },
                        "controller_lease": null,
                        "worker": {
                            "id": "forge-worker-1",
                            "display_name": "Forge worker",
                            "runtime_kind": "probe",
                            "environment_class": "linux-dev",
                            "state": if event_type == "run.ready_for_verification" { "attached" } else { "busy" },
                            "last_seen_at": "2026-04-14T18:00:01Z"
                        },
                        "recent_events": [{ "event_type": event_type }],
                        "recovery_history": [{
                            "id": "forge-run-recovery-1",
                            "worker_id": "forge-worker-1",
                            "worker_session_id": "forge-worker-session-1",
                            "attempt_number": 1,
                            "status": recovery_status,
                            "summary": {},
                            "started_at": "2026-04-14T18:00:00Z",
                            "ended_at": if event_type == "run.ready_for_verification" { json!("2026-04-14T18:00:02Z") } else { serde_json::Value::Null },
                            "updated_at": "2026-04-14T18:00:02Z"
                        }]
                    }))
                }
                other => panic!("unexpected forge request {other:?}"),
            }
        });

        let provider = FakeOpenAiServer::from_json_responses(vec![json!({
            "id": "chatcmpl_forge_run",
            "model": "qwen3.5-2b-q8_0-registry.gguf",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "forge run complete" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 8,
                "completion_tokens": 4,
                "total_tokens": 12
            }
        })]);

        let temp = tempdir().expect("temp dir");
        let controller = ForgeWorkerAuthController::new(temp.path(), forge.base_url()).unwrap();
        controller
            .attach_worker("forge-worker-1", "bootstrap-token", None)
            .unwrap();

        let runtime = ProbeRuntime::new(temp.path());
        let executor = ForgeAssignedRunExecutor::new(controller, runtime);
        let profile = BackendProfile {
            name: String::from("forge-test-profile"),
            kind: BackendKind::OpenAiChatCompletions,
            base_url: String::from(provider.base_url()),
            model: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            reasoning_level: None,
            api_key_env: String::from("PROBE_OPENAI_API_KEY"),
            timeout_secs: 15,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
            control_plane: None,
            psionic_mesh: None,
        };

        let outcome = executor
            .run_once(ForgeAssignedRunExecutionRequest {
                profile,
                default_cwd: temp.path().to_path_buf(),
                system_prompt: None,
                tool_loop: None,
            })
            .unwrap();

        match outcome {
            ForgeAssignedRunExecutionOutcome::Executed(result) => {
                assert_eq!(result.final_run_state, "verifying");
                assert_eq!(result.assistant_text.as_deref(), Some("forge run complete"));
                assert!(result.error.is_none());
                assert!(result.probe_session_id.is_some());
            }
            other => panic!("unexpected outcome {other:?}"),
        }

        let recorded = forge_requests.lock().expect("forge requests lock").clone();
        assert!(
            recorded
                .iter()
                .any(|request| request == "POST /worker/v1/runs/claim-next")
        );
        assert!(
            recorded
                .iter()
                .any(|request| request == "POST /worker/v1/runs/forge-run-1/events")
        );
        assert!(
            recorded
                .iter()
                .any(|request| request == "POST /worker/v1/heartbeat")
        );
    }
}
