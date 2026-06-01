use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use probe_protocol::managed_runtime::{
    ManagedApprovalResolutionRequest, ManagedApprovalResolutionResponse,
    ManagedChildSessionHookRequest, ManagedChildSessionHookResponse, ManagedRuntimeActor,
    ManagedRuntimeArtifactRef, ManagedRuntimeCorrelation, ManagedRuntimeErrorPayload,
    ManagedRuntimeEvent, ManagedRuntimeEventPayload, ManagedRuntimeEventType,
    ManagedRuntimeHeartbeatRequest, ManagedRuntimeHeartbeatResponse, ManagedRuntimeSessionStatus,
    ManagedRuntimeSessionStatusProjection, ManagedRuntimeSource, ManagedSessionControlRequest,
    ManagedSessionControlResponse, ManagedSessionRef, ManagedSessionReplayRequest,
    ManagedSessionReplayResponse, ManagedSessionResumeRequest, ManagedSessionResumeResponse,
    ManagedSessionStartRequest, ManagedSessionStartResponse, PROBE_MANAGED_RUNTIME_SCHEMA_VERSION,
    managed_runtime_transcript_ref,
};
use probe_protocol::session::{SessionBackendTarget, SessionId, TimestampMs};
use sha2::{Digest, Sha256};

use crate::session_store::{FilesystemSessionStore, NewSession, SessionStoreError};

const SESSIONS_DIR: &str = "sessions";
const MANAGED_RUNTIME_EVENTS_FILE: &str = "managed_runtime_events.jsonl";

#[derive(Debug)]
pub enum ManagedRuntimeError {
    Io(std::io::Error),
    Json(serde_json::Error),
    SessionStore(SessionStoreError),
    InvalidSchemaVersion { expected: String, actual: String },
}

impl Display for ManagedRuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::SessionStore(error) => write!(f, "{error}"),
            Self::InvalidSchemaVersion { expected, actual } => {
                write!(
                    f,
                    "invalid managed runtime schema version `{actual}`; expected `{expected}`"
                )
            }
        }
    }
}

impl std::error::Error for ManagedRuntimeError {}

impl From<std::io::Error> for ManagedRuntimeError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ManagedRuntimeError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<SessionStoreError> for ManagedRuntimeError {
    fn from(value: SessionStoreError) -> Self {
        Self::SessionStore(value)
    }
}

#[derive(Clone, Debug)]
pub struct ManagedRuntimeController {
    session_store: FilesystemSessionStore,
}

#[derive(Clone, Debug)]
pub struct ManagedRuntimeEventDraft {
    pub event_type: ManagedRuntimeEventType,
    pub status: ManagedRuntimeSessionStatus,
    pub actor: ManagedRuntimeActor,
    pub source: ManagedRuntimeSource,
    pub session: ManagedSessionRef,
    pub correlation: ManagedRuntimeCorrelation,
    pub artifact_refs: Vec<ManagedRuntimeArtifactRef>,
    pub payload: ManagedRuntimeEventPayload,
}

impl ManagedRuntimeController {
    #[must_use]
    pub fn new(session_store: FilesystemSessionStore) -> Self {
        Self { session_store }
    }

    #[must_use]
    pub fn session_store(&self) -> &FilesystemSessionStore {
        &self.session_store
    }

    pub fn start_session(
        &self,
        request: ManagedSessionStartRequest,
    ) -> Result<ManagedSessionStartResponse, ManagedRuntimeError> {
        validate_schema_version(request.schema_version.as_str())?;
        let ManagedSessionStartRequest {
            request_id,
            actor,
            mut correlation,
            title,
            cwd,
            profile,
            system_prompt,
            harness_profile,
            signature_context,
            workspace_state,
            mounted_refs,
            initial_prompt,
            environment_constraints,
            ..
        } = request;

        correlation.request_id = correlation.request_id.or_else(|| Some(request_id.clone()));
        let session = self.session_store.create_session_with(
            NewSession::new(
                title.unwrap_or_else(|| {
                    initial_prompt
                        .as_deref()
                        .map(default_managed_session_title)
                        .unwrap_or_else(|| String::from("Managed Probe Session"))
                }),
                cwd.clone(),
            )
            .with_system_prompt(system_prompt)
            .with_harness_profile(harness_profile)
            .with_signature_context(signature_context.clone())
            .with_backend(SessionBackendTarget::from_profile(&profile))
            .with_workspace_state(workspace_state)
            .with_mounted_refs(mounted_refs),
        )?;
        let session_ref = ManagedSessionRef {
            probe_session_id: session.id.clone(),
            managed_session_id: correlation.managed_session_id.clone(),
            parent_probe_session_id: correlation.parent_probe_session_id.clone(),
            child_probe_session_id: correlation.child_probe_session_id.clone(),
        };
        let transcript_ref = managed_runtime_transcript_ref(&session.id);
        let mut events = Vec::new();
        events.push(self.append_event(ManagedRuntimeEventDraft {
            event_type: ManagedRuntimeEventType::SessionCreated,
            status: ManagedRuntimeSessionStatus::Created,
            actor: actor.clone(),
            source: ManagedRuntimeSource {
                kind: String::from("runtime"),
                id: Some(session.id.as_str().to_string()),
                label: Some(String::from("Probe session store")),
            },
            session: session_ref.clone(),
            correlation: correlation.clone(),
            artifact_refs: vec![transcript_ref.clone()],
            payload: ManagedRuntimeEventPayload::SessionLifecycle {
                title: session.title.clone(),
                cwd: session.cwd.clone(),
                backend_profile: profile.name.clone(),
                model: profile.model.clone(),
                environment_constraints: environment_constraints.clone(),
            },
        })?);
        events.push(self.append_event(ManagedRuntimeEventDraft {
            event_type: ManagedRuntimeEventType::SessionStarted,
            status: ManagedRuntimeSessionStatus::Running,
            actor: actor.clone(),
            source: ManagedRuntimeSource {
                kind: String::from("runtime"),
                id: Some(session.id.as_str().to_string()),
                label: Some(String::from("Probe managed runtime")),
            },
            session: session_ref.clone(),
            correlation: correlation.clone(),
            artifact_refs: vec![transcript_ref.clone()],
            payload: ManagedRuntimeEventPayload::SessionLifecycle {
                title: session.title.clone(),
                cwd: session.cwd.clone(),
                backend_profile: profile.name,
                model: profile.model,
                environment_constraints,
            },
        })?);
        if let Some(signature_context) = signature_context {
            events.push(
                self.append_event(ManagedRuntimeEventDraft {
                    event_type: ManagedRuntimeEventType::SignatureContextSelected,
                    status: ManagedRuntimeSessionStatus::Running,
                    actor: actor.clone(),
                    source: ManagedRuntimeSource {
                        kind: String::from("signature_selector"),
                        id: signature_context
                            .selection_decision
                            .as_ref()
                            .map(|decision| decision.decision_id.clone()),
                        label: Some(String::from("Probe signature context")),
                    },
                    session: session_ref.clone(),
                    correlation: correlation.clone(),
                    artifact_refs: vec![transcript_ref.clone()],
                    payload: ManagedRuntimeEventPayload::SignatureContext { signature_context },
                })?,
            );
        }
        if let Some(prompt) = initial_prompt {
            events.push(self.append_event(ManagedRuntimeEventDraft {
                event_type: ManagedRuntimeEventType::TurnStarted,
                status: ManagedRuntimeSessionStatus::Running,
                actor,
                source: ManagedRuntimeSource {
                    kind: String::from("turn"),
                    id: Some(format!("turn-{}", session.next_turn_index)),
                    label: Some(String::from("initial managed turn")),
                },
                session: session_ref.clone(),
                correlation,
                artifact_refs: vec![transcript_ref.clone()],
                payload: ManagedRuntimeEventPayload::TurnLifecycle {
                    probe_turn_id: format!("turn-{}", session.next_turn_index),
                    prompt_sha256: Some(sha256_hex(prompt.as_bytes())),
                    prompt: Some(prompt),
                },
            })?);
        }

        let newest_sequence = events.last().map(|event| event.sequence).unwrap_or(0);
        Ok(ManagedSessionStartResponse {
            schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
            request_id,
            session_ref,
            status: events
                .last()
                .map(|event| event.status)
                .unwrap_or(ManagedRuntimeSessionStatus::Created),
            transcript_ref,
            replay_after_sequence: 0,
            next_sequence: newest_sequence + 1,
            events,
        })
    }

    pub fn resume_session(
        &self,
        request: ManagedSessionResumeRequest,
    ) -> Result<ManagedSessionResumeResponse, ManagedRuntimeError> {
        validate_schema_version(request.schema_version.as_str())?;
        let projection = self.reconstruct_status(&request.session_ref)?;
        let replayed_events = self.replay_events_for_session(
            &request.session_ref.probe_session_id,
            request.after_sequence,
            None,
        )?;
        Ok(ManagedSessionResumeResponse {
            schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
            request_id: request.request_id,
            transcript_ref: managed_runtime_transcript_ref(&request.session_ref.probe_session_id),
            projection,
            replayed_events,
            snapshot_ref: request.include_snapshot.then(|| ManagedRuntimeArtifactRef {
                kind: probe_protocol::managed_runtime::ManagedRuntimeArtifactKind::Other,
                resource_ref: format!(
                    "probe://sessions/{}/snapshot",
                    request.session_ref.probe_session_id.as_str()
                ),
                stable_digest: None,
                label: Some(String::from("Probe managed runtime session snapshot")),
                updated_at_ms: Some(now_ms()),
            }),
        })
    }

    pub fn replay_events(
        &self,
        request: ManagedSessionReplayRequest,
    ) -> Result<ManagedSessionReplayResponse, ManagedRuntimeError> {
        validate_schema_version(request.schema_version.as_str())?;
        let events = self.replay_events_for_session(
            &request.session_ref.probe_session_id,
            request.after_sequence,
            request.limit,
        )?;
        Ok(ManagedSessionReplayResponse {
            schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
            request_id: request.request_id,
            session_ref: request.session_ref,
            newest_sequence: events.last().map(|event| event.sequence),
            events,
        })
    }

    pub fn interrupt_session(
        &self,
        request: ManagedSessionControlRequest,
    ) -> Result<ManagedSessionControlResponse, ManagedRuntimeError> {
        validate_schema_version(request.schema_version.as_str())?;
        let event = self.append_terminal_or_status_control_event(
            request.session_ref.clone(),
            request.actor,
            request.correlation,
            request.reason,
            ManagedRuntimeEventType::SessionInterrupted,
            ManagedRuntimeSessionStatus::Interrupted,
        )?;
        let projection = self.reconstruct_status(&request.session_ref)?;
        Ok(ManagedSessionControlResponse {
            schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
            request_id: request.request_id,
            projection,
            event,
        })
    }

    pub fn cancel_session(
        &self,
        request: ManagedSessionControlRequest,
    ) -> Result<ManagedSessionControlResponse, ManagedRuntimeError> {
        validate_schema_version(request.schema_version.as_str())?;
        let event = self.append_terminal_or_status_control_event(
            request.session_ref.clone(),
            request.actor,
            request.correlation,
            request.reason,
            ManagedRuntimeEventType::SessionCancelled,
            ManagedRuntimeSessionStatus::Cancelled,
        )?;
        let projection = self.reconstruct_status(&request.session_ref)?;
        Ok(ManagedSessionControlResponse {
            schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
            request_id: request.request_id,
            projection,
            event,
        })
    }

    pub fn resolve_approval(
        &self,
        request: ManagedApprovalResolutionRequest,
    ) -> Result<ManagedApprovalResolutionResponse, ManagedRuntimeError> {
        validate_schema_version(request.schema_version.as_str())?;
        let resolution = request.resolution;
        let event = self.append_event(ManagedRuntimeEventDraft {
            event_type: ManagedRuntimeEventType::ApprovalResolved,
            status: ManagedRuntimeSessionStatus::Running,
            actor: request.actor,
            source: ManagedRuntimeSource {
                kind: String::from("approval"),
                id: Some(request.approval_id.clone()),
                label: Some(String::from("managed runtime approval")),
            },
            session: request.session_ref.clone(),
            correlation: request.correlation,
            artifact_refs: vec![managed_runtime_transcript_ref(
                &request.session_ref.probe_session_id,
            )],
            payload: ManagedRuntimeEventPayload::Approval {
                approval: probe_protocol::managed_runtime::ManagedRuntimeApproval {
                    approval_id: request.approval_id,
                    call_id: request.call_id,
                    tool_name: request.tool_name.unwrap_or_else(|| String::from("unknown")),
                    status: String::from("resolved"),
                    risk_class: None,
                    resolution: Some(resolution),
                    reason: None,
                    pending_tool_approval: None,
                },
            },
        })?;
        let projection = self.reconstruct_status(&request.session_ref)?;
        Ok(ManagedApprovalResolutionResponse {
            schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
            request_id: request.request_id,
            projection,
            event,
        })
    }

    pub fn heartbeat(
        &self,
        request: ManagedRuntimeHeartbeatRequest,
    ) -> Result<ManagedRuntimeHeartbeatResponse, ManagedRuntimeError> {
        validate_schema_version(request.schema_version.as_str())?;
        let mut events = Vec::new();
        for projection in request.sessions {
            let session_ref = projection.session_ref.clone();
            events.push(self.append_event(ManagedRuntimeEventDraft {
                event_type: ManagedRuntimeEventType::Heartbeat,
                status: projection.status,
                actor: ManagedRuntimeActor {
                    kind: String::from("probe_worker"),
                    id: Some(request.worker_id.clone()),
                    label: None,
                },
                source: ManagedRuntimeSource {
                    kind: String::from("heartbeat"),
                    id: Some(request.worker_id.clone()),
                    label: None,
                },
                session: session_ref,
                correlation: ManagedRuntimeCorrelation::default(),
                artifact_refs: Vec::new(),
                payload: ManagedRuntimeEventPayload::Heartbeat { projection },
            })?);
        }
        Ok(ManagedRuntimeHeartbeatResponse {
            schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
            request_id: request.request_id,
            accepted: true,
            events,
        })
    }

    pub fn record_child_session(
        &self,
        request: ManagedChildSessionHookRequest,
    ) -> Result<ManagedChildSessionHookResponse, ManagedRuntimeError> {
        validate_schema_version(request.schema_version.as_str())?;
        let event = self.append_event(ManagedRuntimeEventDraft {
            event_type: ManagedRuntimeEventType::ChildSessionSpawned,
            status: request.status,
            actor: request.actor,
            source: ManagedRuntimeSource {
                kind: String::from("child_session"),
                id: Some(request.child.probe_session_id.as_str().to_string()),
                label: None,
            },
            session: request.parent.clone(),
            correlation: request.correlation,
            artifact_refs: Vec::new(),
            payload: ManagedRuntimeEventPayload::ChildSession {
                child: request.child,
                purpose: request.purpose,
                status: request.status,
            },
        })?;
        let projection = self.reconstruct_status(&request.parent)?;
        Ok(ManagedChildSessionHookResponse {
            schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
            request_id: request.request_id,
            projection,
            event,
        })
    }

    pub fn append_event(
        &self,
        draft: ManagedRuntimeEventDraft,
    ) -> Result<ManagedRuntimeEvent, ManagedRuntimeError> {
        let path = self.event_log_path(&draft.session.probe_session_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let sequence = self
            .read_events_from_path(path.as_path())?
            .last()
            .map(|event| event.sequence + 1)
            .unwrap_or(1);
        let event = ManagedRuntimeEvent::new(
            sequence,
            now_ms(),
            draft.event_type,
            draft.status,
            draft.actor,
            draft.source,
            draft.session,
            draft.correlation,
            draft.payload,
        )
        .with_artifact_refs(draft.artifact_refs);
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        serde_json::to_writer(&mut file, &event)?;
        file.write_all(b"\n")?;
        file.flush()?;
        Ok(event)
    }

    pub fn record_approval_requested(
        &self,
        session: ManagedSessionRef,
        actor: ManagedRuntimeActor,
        correlation: ManagedRuntimeCorrelation,
        approval: probe_protocol::managed_runtime::ManagedRuntimeApproval,
    ) -> Result<ManagedRuntimeEvent, ManagedRuntimeError> {
        self.append_event(ManagedRuntimeEventDraft {
            event_type: ManagedRuntimeEventType::ApprovalRequested,
            status: ManagedRuntimeSessionStatus::ApprovalPaused,
            actor,
            source: ManagedRuntimeSource {
                kind: String::from("approval"),
                id: Some(approval.approval_id.clone()),
                label: Some(approval.tool_name.clone()),
            },
            artifact_refs: vec![managed_runtime_transcript_ref(&session.probe_session_id)],
            session,
            correlation,
            payload: ManagedRuntimeEventPayload::Approval { approval },
        })
    }

    pub fn record_failure(
        &self,
        session: ManagedSessionRef,
        actor: ManagedRuntimeActor,
        correlation: ManagedRuntimeCorrelation,
        error: ManagedRuntimeErrorPayload,
    ) -> Result<ManagedRuntimeEvent, ManagedRuntimeError> {
        self.append_event(ManagedRuntimeEventDraft {
            event_type: ManagedRuntimeEventType::SessionFailed,
            status: ManagedRuntimeSessionStatus::Failed,
            actor,
            source: ManagedRuntimeSource {
                kind: String::from("runtime"),
                id: Some(session.probe_session_id.as_str().to_string()),
                label: Some(String::from("managed runtime failure")),
            },
            artifact_refs: vec![managed_runtime_transcript_ref(&session.probe_session_id)],
            session,
            correlation,
            payload: ManagedRuntimeEventPayload::Error { error },
        })
    }

    pub fn reconstruct_status(
        &self,
        session_ref: &ManagedSessionRef,
    ) -> Result<ManagedRuntimeSessionStatusProjection, ManagedRuntimeError> {
        let _ = self
            .session_store
            .read_metadata(&session_ref.probe_session_id)?;
        let events = self.replay_events_for_session(&session_ref.probe_session_id, 0, None)?;
        let mut status = ManagedRuntimeSessionStatus::Created;
        let mut last_sequence = None;
        let mut last_event_at_ms = None;
        let mut approval_states = BTreeMap::<String, bool>::new();
        let mut active_probe_turn_id = None;
        let mut message = None;
        for event in events {
            status = event.status;
            last_sequence = Some(event.sequence);
            last_event_at_ms = Some(event.occurred_at_ms);
            match event.payload {
                ManagedRuntimeEventPayload::Approval { approval } => {
                    approval_states.insert(
                        approval.approval_id,
                        approval.resolution.is_none() && approval.status == "pending",
                    );
                }
                ManagedRuntimeEventPayload::TurnLifecycle { probe_turn_id, .. } => {
                    active_probe_turn_id = Some(probe_turn_id);
                }
                ManagedRuntimeEventPayload::Terminal { reason, .. } => {
                    message = reason;
                }
                ManagedRuntimeEventPayload::Error { error } => {
                    message = Some(error.message);
                }
                ManagedRuntimeEventPayload::Status {
                    message: status_message,
                } => {
                    message = status_message;
                }
                _ => {}
            }
        }
        Ok(ManagedRuntimeSessionStatusProjection {
            session_ref: session_ref.clone(),
            status,
            last_sequence,
            last_event_at_ms,
            active_probe_turn_id,
            pending_approval_count: approval_states
                .values()
                .filter(|is_pending| **is_pending)
                .count(),
            message,
        })
    }

    fn append_terminal_or_status_control_event(
        &self,
        session_ref: ManagedSessionRef,
        actor: ManagedRuntimeActor,
        correlation: ManagedRuntimeCorrelation,
        reason: Option<String>,
        event_type: ManagedRuntimeEventType,
        status: ManagedRuntimeSessionStatus,
    ) -> Result<ManagedRuntimeEvent, ManagedRuntimeError> {
        self.append_event(ManagedRuntimeEventDraft {
            event_type,
            status,
            actor,
            source: ManagedRuntimeSource {
                kind: String::from("control"),
                id: Some(session_ref.probe_session_id.as_str().to_string()),
                label: None,
            },
            artifact_refs: vec![managed_runtime_transcript_ref(
                &session_ref.probe_session_id,
            )],
            session: session_ref,
            correlation,
            payload: ManagedRuntimeEventPayload::Terminal { status, reason },
        })
    }

    fn replay_events_for_session(
        &self,
        session_id: &SessionId,
        after_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<ManagedRuntimeEvent>, ManagedRuntimeError> {
        let mut events = self
            .read_events_from_path(self.event_log_path(session_id).as_path())?
            .into_iter()
            .filter(|event| event.sequence > after_sequence)
            .collect::<Vec<_>>();
        if let Some(limit) = limit {
            events.truncate(limit);
        }
        Ok(events)
    }

    fn read_events_from_path(
        &self,
        path: &Path,
    ) -> Result<Vec<ManagedRuntimeEvent>, ManagedRuntimeError> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            events.push(serde_json::from_str(line.as_str())?);
        }
        Ok(events)
    }

    fn event_log_path(&self, session_id: &SessionId) -> PathBuf {
        self.session_store
            .root()
            .join(SESSIONS_DIR)
            .join(session_id.as_str())
            .join(MANAGED_RUNTIME_EVENTS_FILE)
    }
}

fn validate_schema_version(schema_version: &str) -> Result<(), ManagedRuntimeError> {
    if schema_version == PROBE_MANAGED_RUNTIME_SCHEMA_VERSION {
        return Ok(());
    }
    Err(ManagedRuntimeError::InvalidSchemaVersion {
        expected: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
        actual: String::from(schema_version),
    })
}

fn default_managed_session_title(prompt: &str) -> String {
    let collapsed = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return String::from("Managed Probe Session");
    }
    let truncated = collapsed.chars().take(72).collect::<String>();
    if collapsed.chars().count() > 72 {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn now_ms() -> TimestampMs {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_millis() as TimestampMs
}

#[cfg(test)]
mod tests {
    use super::{ManagedRuntimeController, ManagedRuntimeEventDraft};
    use crate::backend_profiles::named_backend_profile;
    use crate::session_store::FilesystemSessionStore;
    use probe_protocol::managed_runtime::{
        ManagedApprovalResolutionRequest, ManagedRuntimeActor, ManagedRuntimeApproval,
        ManagedRuntimeCorrelation, ManagedRuntimeErrorPayload, ManagedRuntimeEventPayload,
        ManagedRuntimeEventType, ManagedRuntimeSessionStatus, ManagedRuntimeSource,
        ManagedSessionControlRequest, ManagedSessionReplayRequest, ManagedSessionResumeRequest,
        ManagedSessionStartRequest, PROBE_MANAGED_RUNTIME_SCHEMA_VERSION,
    };
    use probe_protocol::session::ToolApprovalResolution;
    use probe_protocol::signature_context::{
        PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION, SessionSignatureContext, SignatureAdoptionState,
        SignaturePack, SignaturePackEntry, SignatureRef, SignatureSelectionDecision,
        SignatureSelectionScore, SignatureSelectorMode,
    };

    #[test]
    fn managed_session_start_persists_replayable_events() {
        let temp = tempfile::tempdir().expect("temp dir");
        let controller = ManagedRuntimeController::new(FilesystemSessionStore::new(temp.path()));
        let response = controller
            .start_session(start_request(
                temp.path(),
                "req-start",
                Some("inspect repo state"),
            ))
            .expect("start managed session");

        assert_eq!(response.status, ManagedRuntimeSessionStatus::Running);
        assert_eq!(response.events.len(), 3);
        assert_eq!(response.events[0].sequence, 1);
        assert_eq!(response.events[1].sequence, 2);
        assert_eq!(response.events[2].sequence, 3);

        let restarted = ManagedRuntimeController::new(FilesystemSessionStore::new(temp.path()));
        let projection = restarted
            .reconstruct_status(&response.session_ref)
            .expect("reconstruct status");
        assert_eq!(projection.status, ManagedRuntimeSessionStatus::Running);
        assert_eq!(projection.last_sequence, Some(3));

        let replay = restarted
            .replay_events(ManagedSessionReplayRequest {
                schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
                request_id: String::from("req-replay"),
                session_ref: response.session_ref,
                after_sequence: 1,
                limit: None,
            })
            .expect("replay events");
        assert_eq!(replay.events.len(), 2);
        assert_eq!(
            replay.events[0].event_type,
            ManagedRuntimeEventType::SessionStarted
        );
    }

    #[test]
    fn managed_session_start_persists_signature_context_before_initial_turn() {
        let temp = tempfile::tempdir().expect("temp dir");
        let controller = ManagedRuntimeController::new(FilesystemSessionStore::new(temp.path()));
        let mut request = start_request(temp.path(), "req-start", Some("inspect service task"));
        request.signature_context = Some(signature_context());

        let response = controller
            .start_session(request)
            .expect("start managed session with signature context");

        assert_eq!(response.events.len(), 4);
        assert_eq!(
            response.events[2].event_type,
            ManagedRuntimeEventType::SignatureContextSelected
        );
        assert_eq!(
            response.events[3].event_type,
            ManagedRuntimeEventType::TurnStarted
        );
        let metadata = controller
            .session_store()
            .read_metadata(&response.session_ref.probe_session_id)
            .expect("read metadata");
        let stored_context = metadata
            .signature_context
            .expect("signature context stored in metadata");
        assert_eq!(stored_context.signature_pack.entries.len(), 1);
        assert_eq!(
            stored_context.signature_pack.entries[0].signature.id,
            "coding.service_readiness"
        );
    }

    #[test]
    fn managed_approval_pause_and_resolution_reconstructs_status() {
        let temp = tempfile::tempdir().expect("temp dir");
        let controller = ManagedRuntimeController::new(FilesystemSessionStore::new(temp.path()));
        let response = controller
            .start_session(start_request(temp.path(), "req-start", Some("edit file")))
            .expect("start managed session");
        let session_ref = response.session_ref;

        controller
            .record_approval_requested(
                session_ref.clone(),
                actor(),
                correlation(),
                ManagedRuntimeApproval {
                    approval_id: String::from("approval-1"),
                    call_id: String::from("call-1"),
                    tool_name: String::from("patch"),
                    status: String::from("pending"),
                    risk_class: None,
                    resolution: None,
                    reason: Some(String::from("write requires approval")),
                    pending_tool_approval: None,
                },
            )
            .expect("record approval request");
        let paused = controller
            .reconstruct_status(&session_ref)
            .expect("reconstruct paused status");
        assert_eq!(paused.status, ManagedRuntimeSessionStatus::ApprovalPaused);
        assert_eq!(paused.pending_approval_count, 1);

        let resolved = controller
            .resolve_approval(ManagedApprovalResolutionRequest {
                schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
                request_id: String::from("req-resolve"),
                idempotency_key: String::from("approval-1:approve"),
                actor: actor(),
                session_ref: session_ref.clone(),
                correlation: correlation(),
                approval_id: String::from("approval-1"),
                call_id: String::from("call-1"),
                tool_name: Some(String::from("patch")),
                resolution: ToolApprovalResolution::Approved,
                author: None,
            })
            .expect("resolve approval");
        assert_eq!(
            resolved.projection.status,
            ManagedRuntimeSessionStatus::Running
        );
        assert_eq!(resolved.projection.pending_approval_count, 0);
    }

    #[test]
    fn managed_cancel_and_failure_are_terminal_after_restart() {
        let temp = tempfile::tempdir().expect("temp dir");
        let controller = ManagedRuntimeController::new(FilesystemSessionStore::new(temp.path()));
        let cancelled = controller
            .start_session(start_request(temp.path(), "req-start-cancel", None))
            .expect("start managed session")
            .session_ref;
        let cancel = controller
            .cancel_session(ManagedSessionControlRequest {
                schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
                request_id: String::from("req-cancel"),
                idempotency_key: String::from("cancel-1"),
                actor: actor(),
                session_ref: cancelled.clone(),
                correlation: correlation(),
                reason: Some(String::from("admin cancelled")),
                cancel_queued_turns: true,
            })
            .expect("cancel session");
        assert_eq!(
            cancel.projection.status,
            ManagedRuntimeSessionStatus::Cancelled
        );
        assert!(cancel.projection.status.is_terminal());

        let failed = controller
            .start_session(start_request(temp.path(), "req-start-fail", None))
            .expect("start managed session")
            .session_ref;
        controller
            .record_failure(
                failed.clone(),
                actor(),
                correlation(),
                ManagedRuntimeErrorPayload {
                    code: String::from("backend.unavailable"),
                    message: String::from("selected backend is unavailable"),
                    retryable: true,
                    details: serde_json::Map::new(),
                },
            )
            .expect("record failure");

        let restarted = ManagedRuntimeController::new(FilesystemSessionStore::new(temp.path()));
        assert_eq!(
            restarted
                .reconstruct_status(&cancelled)
                .expect("cancelled status")
                .status,
            ManagedRuntimeSessionStatus::Cancelled
        );
        let failed_projection = restarted
            .reconstruct_status(&failed)
            .expect("failed status");
        assert_eq!(
            failed_projection.status,
            ManagedRuntimeSessionStatus::Failed
        );
        assert_eq!(
            failed_projection.message.as_deref(),
            Some("selected backend is unavailable")
        );
    }

    #[test]
    fn managed_resume_returns_replay_window() {
        let temp = tempfile::tempdir().expect("temp dir");
        let controller = ManagedRuntimeController::new(FilesystemSessionStore::new(temp.path()));
        let response = controller
            .start_session(start_request(
                temp.path(),
                "req-start",
                Some("continue work"),
            ))
            .expect("start managed session");
        controller
            .append_event(ManagedRuntimeEventDraft {
                event_type: ManagedRuntimeEventType::TextDelta,
                status: ManagedRuntimeSessionStatus::Running,
                actor: actor(),
                source: ManagedRuntimeSource {
                    kind: String::from("model"),
                    id: Some(String::from("response-1")),
                    label: None,
                },
                session: response.session_ref.clone(),
                correlation: correlation(),
                artifact_refs: Vec::new(),
                payload: ManagedRuntimeEventPayload::TextDelta {
                    delta: String::from("hello"),
                },
            })
            .expect("append text delta");

        let resume = controller
            .resume_session(ManagedSessionResumeRequest {
                schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
                request_id: String::from("req-resume"),
                actor: actor(),
                session_ref: response.session_ref,
                correlation: correlation(),
                after_sequence: 3,
                include_snapshot: true,
            })
            .expect("resume managed session");
        assert_eq!(resume.replayed_events.len(), 1);
        assert_eq!(
            resume.replayed_events[0].event_type,
            ManagedRuntimeEventType::TextDelta
        );
        assert!(resume.snapshot_ref.is_some());
    }

    fn start_request(
        cwd: &std::path::Path,
        request_id: &str,
        prompt: Option<&str>,
    ) -> ManagedSessionStartRequest {
        ManagedSessionStartRequest {
            schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
            request_id: String::from(request_id),
            idempotency_key: format!("{request_id}:start"),
            actor: actor(),
            correlation: correlation(),
            title: Some(String::from("managed test")),
            cwd: cwd.to_path_buf(),
            profile: named_backend_profile("openai-codex-subscription").expect("profile"),
            system_prompt: Some(String::from("You are Probe.")),
            harness_profile: None,
            signature_context: None,
            workspace_state: None,
            mounted_refs: Vec::new(),
            initial_prompt: prompt.map(String::from),
            tool_loop: None,
            environment_constraints: None,
            metadata: serde_json::Map::new(),
        }
    }

    fn signature_context() -> SessionSignatureContext {
        let signature = SignatureRef {
            id: String::from("coding.service_readiness"),
            version: String::from("candidate"),
            adoption_state: SignatureAdoptionState::Candidate,
            source_ref: Some(String::from("vortex://signatureTools/service-readiness")),
        };
        SessionSignatureContext::new(SignaturePack {
            schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
            pack_id: Some(String::from("pack-1")),
            selected_by: Some(String::from("probe-selector")),
            selected_at_ms: Some(1_777_777_777_000),
            max_signature_count: Some(4),
            entries: vec![SignaturePackEntry {
                signature: signature.clone(),
                task_classes: vec![String::from("service_readiness")],
                benchmark_families: vec![String::from("terminal-bench")],
                required_evidence: Vec::new(),
                recommended_tools: Vec::new(),
                forbidden_tools: Vec::new(),
                failure_fingerprints: vec![String::from("port_not_ready")],
                fixture_refs: vec![String::from("tb2:pypi-server")],
                rendered_description: None,
            }],
        })
        .with_selection_decision(SignatureSelectionDecision {
            schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
            decision_id: String::from("decision-1"),
            selector_mode: SignatureSelectorMode::Hybrid,
            task_envelope_digest: Some(String::from("sha256:task-envelope")),
            selected_signatures: vec![SignatureSelectionScore {
                signature,
                rank: 1,
                score_bps: 9_100,
                reason_code: Some(String::from("matched_failure_fingerprint")),
            }],
            runner_up_signatures: Vec::new(),
            recommended_harness_profile: Some(String::from("coding_bootstrap_codex@v1")),
            recommended_tool_set: Some(String::from("coding_bootstrap")),
            recommended_tool_choice: Some(String::from("auto")),
            forbidden_tools: Vec::new(),
            fallback_reason_code: None,
        })
    }

    fn actor() -> ManagedRuntimeActor {
        ManagedRuntimeActor {
            kind: String::from("laravel_admin"),
            id: Some(String::from("user-1")),
            label: Some(String::from("Admin")),
        }
    }

    fn correlation() -> ManagedRuntimeCorrelation {
        ManagedRuntimeCorrelation {
            request_id: Some(String::from("request-1")),
            workspace: Some(String::from("openagents.com")),
            managed_agent_id: Some(String::from("agent-1")),
            managed_session_id: Some(String::from("managed-session-1")),
            ..ManagedRuntimeCorrelation::default()
        }
    }
}
