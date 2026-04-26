use std::path::PathBuf;

use probe_protocol::session::{
    SessionChildLink, SessionChildStatus, SessionChildSummary, SessionId, SessionInitiator,
    SessionParentLink, SessionState, SessionSummaryArtifactRef, TimestampMs, TranscriptItemKind,
};
use serde::{Deserialize, Serialize};

use crate::session_store::{FilesystemSessionStore, NewItem, NewSession, SessionStoreError};
use crate::session_summary_artifacts::{
    SessionSummaryArtifactError, refresh_session_summary_artifacts,
};

const DEFAULT_MAX_CHILD_SESSIONS: usize = 3;
const DEFAULT_MAX_PROMPT_BYTES: usize = 16 * 1024;
const DEFAULT_MAX_CHILD_TIMEOUT_SECS: u64 = 900;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeChildSessionMode {
    Research,
    PatchAttempt,
    ProductionRecovery,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeChildSessionPolicy {
    pub max_child_sessions: usize,
    pub max_prompt_bytes: usize,
    pub max_child_timeout_secs: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_repositories: Vec<String>,
    pub allow_patch_attempts: bool,
    pub allow_production_recovery_actions: bool,
}

impl Default for ForgeChildSessionPolicy {
    fn default() -> Self {
        Self {
            max_child_sessions: DEFAULT_MAX_CHILD_SESSIONS,
            max_prompt_bytes: DEFAULT_MAX_PROMPT_BYTES,
            max_child_timeout_secs: DEFAULT_MAX_CHILD_TIMEOUT_SECS,
            allowed_repositories: Vec::new(),
            allow_patch_attempts: false,
            allow_production_recovery_actions: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeChildSessionSpawnRequest {
    pub parent_session_id: SessionId,
    pub title: String,
    pub cwd: PathBuf,
    pub prompt: String,
    pub purpose: String,
    pub repo_slug: String,
    pub mode: ForgeChildSessionMode,
    pub requested_timeout_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_turn_index: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeChildSessionSpawnResult {
    pub parent_session_id: SessionId,
    pub child: SessionChildSummary,
    pub mode: ForgeChildSessionMode,
    pub read_only: bool,
    pub patch_attempt_authorized: bool,
    pub production_recovery_authorized: bool,
    pub artifacts: Vec<ForgeChildSessionArtifactRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeChildSessionStatusReport {
    pub parent_session_id: SessionId,
    pub child: SessionChildSummary,
    pub artifacts: Vec<ForgeChildSessionArtifactRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeParentSynthesisArtifact {
    pub kind: String,
    pub parent_session_id: SessionId,
    pub child_sessions: Vec<ForgeChildSessionStatusReport>,
    pub direct_production_recovery_allowed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeChildSessionArtifactRef {
    pub kind: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<TimestampMs>,
}

#[derive(Debug)]
pub enum ForgeChildSessionError {
    SessionStore(SessionStoreError),
    SummaryArtifact(SessionSummaryArtifactError),
    Policy(String),
}

impl std::fmt::Display for ForgeChildSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionStore(error) => write!(f, "{error}"),
            Self::SummaryArtifact(error) => write!(f, "{error}"),
            Self::Policy(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ForgeChildSessionError {}

impl From<SessionStoreError> for ForgeChildSessionError {
    fn from(value: SessionStoreError) -> Self {
        Self::SessionStore(value)
    }
}

impl From<SessionSummaryArtifactError> for ForgeChildSessionError {
    fn from(value: SessionSummaryArtifactError) -> Self {
        Self::SummaryArtifact(value)
    }
}

#[derive(Clone, Debug)]
pub struct ForgeChildSessionController<'a> {
    store: &'a FilesystemSessionStore,
}

impl<'a> ForgeChildSessionController<'a> {
    #[must_use]
    pub fn new(store: &'a FilesystemSessionStore) -> Self {
        Self { store }
    }

    pub fn spawn_child_session(
        &self,
        policy: &ForgeChildSessionPolicy,
        request: ForgeChildSessionSpawnRequest,
    ) -> Result<ForgeChildSessionSpawnResult, ForgeChildSessionError> {
        let parent = self.store.read_metadata(&request.parent_session_id)?;
        enforce_child_policy(policy, &request, parent.child_links.len())?;

        let read_only = request.mode == ForgeChildSessionMode::Research;
        let patch_attempt_authorized =
            request.mode == ForgeChildSessionMode::PatchAttempt && policy.allow_patch_attempts;
        let production_recovery_authorized = request.mode
            == ForgeChildSessionMode::ProductionRecovery
            && policy.allow_production_recovery_actions;
        let system_prompt = child_system_prompt(&request, read_only, patch_attempt_authorized);
        let child = self.store.create_session_with(
            NewSession::new(request.title.clone(), request.cwd.clone())
                .with_system_prompt(Some(system_prompt))
                .with_parent_link(Some(SessionParentLink {
                    session_id: parent.id.clone(),
                    turn_id: request.parent_turn_id.clone(),
                    turn_index: request.parent_turn_index,
                    initiator: Some(SessionInitiator {
                        client_name: "forge-child-session-tool".to_string(),
                        client_version: Some("2026-04-26".to_string()),
                        display_name: Some("Forge child session tool".to_string()),
                        participant_id: Some("forge-child-session-tool".to_string()),
                    }),
                    purpose: Some(request.purpose.clone()),
                })),
        )?;
        self.store.append_child_link(
            &parent.id,
            SessionChildLink {
                session_id: child.id.clone(),
                added_at_ms: child.created_at_ms,
            },
        )?;
        self.store.append_turn(
            &child.id,
            &[NewItem::new(
                TranscriptItemKind::UserMessage,
                child_spawn_prompt(&request, read_only, patch_attempt_authorized),
            )],
        )?;
        let artifacts = self.child_artifacts(&child.id)?;
        let summary = child_summary_from_metadata(
            child,
            SessionChildStatus::Idle,
            request.purpose,
            request.parent_turn_id,
            request.parent_turn_index,
        );

        Ok(ForgeChildSessionSpawnResult {
            parent_session_id: parent.id,
            child: summary,
            mode: request.mode,
            read_only,
            patch_attempt_authorized,
            production_recovery_authorized,
            artifacts,
        })
    }

    pub fn read_child_status(
        &self,
        parent_session_id: &SessionId,
        child_session_id: &SessionId,
    ) -> Result<ForgeChildSessionStatusReport, ForgeChildSessionError> {
        let parent = self.store.read_metadata(parent_session_id)?;
        if !parent
            .child_links
            .iter()
            .any(|child| &child.session_id == child_session_id)
        {
            return Err(ForgeChildSessionError::Policy(format!(
                "child session {} is not linked to parent {}",
                child_session_id.as_str(),
                parent_session_id.as_str()
            )));
        }
        let child = self.store.read_metadata(child_session_id)?;
        let artifacts = self.child_artifacts(child_session_id)?;
        let parent_link = child.parent_link.clone();
        Ok(ForgeChildSessionStatusReport {
            parent_session_id: parent.id,
            child: child_summary_from_metadata(
                child,
                SessionChildStatus::Idle,
                parent_link
                    .as_ref()
                    .and_then(|link| link.purpose.clone())
                    .unwrap_or_else(|| "forge child session".to_string()),
                parent_link.as_ref().and_then(|link| link.turn_id.clone()),
                parent_link.as_ref().and_then(|link| link.turn_index),
            ),
            artifacts,
        })
    }

    pub fn parent_synthesis_artifact(
        &self,
        parent_session_id: &SessionId,
    ) -> Result<ForgeParentSynthesisArtifact, ForgeChildSessionError> {
        let parent = self.store.read_metadata(parent_session_id)?;
        let mut child_sessions = Vec::new();
        for link in parent.child_links {
            child_sessions.push(self.read_child_status(&parent.id, &link.session_id)?);
        }

        Ok(ForgeParentSynthesisArtifact {
            kind: "probe.forge_worker.child_session_synthesis".to_string(),
            parent_session_id: parent.id,
            child_sessions,
            direct_production_recovery_allowed: false,
        })
    }

    fn child_artifacts(
        &self,
        child_session_id: &SessionId,
    ) -> Result<Vec<ForgeChildSessionArtifactRef>, ForgeChildSessionError> {
        let child = self.store.read_metadata(child_session_id)?;
        let transcript = self.store.read_transcript(child_session_id)?;
        let summary_artifacts = refresh_session_summary_artifacts(
            self.store,
            &child,
            transcript.as_slice(),
            None,
            None,
        )?;
        let mut artifacts = vec![ForgeChildSessionArtifactRef {
            kind: "probe.transcript".to_string(),
            path: child.transcript_path.clone(),
            stable_digest: None,
            updated_at_ms: Some(child.updated_at_ms),
        }];
        artifacts.extend(summary_artifacts.into_iter().map(summary_artifact_ref));
        Ok(artifacts)
    }
}

fn enforce_child_policy(
    policy: &ForgeChildSessionPolicy,
    request: &ForgeChildSessionSpawnRequest,
    existing_child_count: usize,
) -> Result<(), ForgeChildSessionError> {
    if existing_child_count >= policy.max_child_sessions {
        return Err(ForgeChildSessionError::Policy(format!(
            "Forge child-session policy allows at most {} child sessions",
            policy.max_child_sessions
        )));
    }
    if request.prompt.len() > policy.max_prompt_bytes {
        return Err(ForgeChildSessionError::Policy(format!(
            "child prompt exceeds Forge policy budget of {} bytes",
            policy.max_prompt_bytes
        )));
    }
    if request.requested_timeout_secs > policy.max_child_timeout_secs {
        return Err(ForgeChildSessionError::Policy(format!(
            "child timeout {}s exceeds Forge policy budget of {}s",
            request.requested_timeout_secs, policy.max_child_timeout_secs
        )));
    }
    if !policy.allowed_repositories.is_empty()
        && !policy
            .allowed_repositories
            .iter()
            .any(|repo| repo == &request.repo_slug)
    {
        return Err(ForgeChildSessionError::Policy(format!(
            "repository {} is not allowed by Forge child-session policy",
            request.repo_slug
        )));
    }
    if request.mode == ForgeChildSessionMode::PatchAttempt && !policy.allow_patch_attempts {
        return Err(ForgeChildSessionError::Policy(
            "Forge child-session policy does not authorize patch attempts".to_string(),
        ));
    }
    if request.mode == ForgeChildSessionMode::ProductionRecovery
        && !policy.allow_production_recovery_actions
    {
        return Err(ForgeChildSessionError::Policy(
            "child sessions cannot execute production recovery actions without explicit Forge policy authorization".to_string(),
        ));
    }
    Ok(())
}

fn child_system_prompt(
    request: &ForgeChildSessionSpawnRequest,
    read_only: bool,
    patch_attempt_authorized: bool,
) -> String {
    let authority = if read_only {
        "read-only research only"
    } else if patch_attempt_authorized {
        "bounded patch attempt authorized by Forge policy"
    } else {
        "no mutation authority"
    };
    format!(
        "You are a bounded child Probe session for a Forge Run.\nAuthority: {authority}.\nRepository: {}.\nNever execute production recovery actions directly. Return artifacts for parent synthesis.",
        request.repo_slug
    )
}

fn child_spawn_prompt(
    request: &ForgeChildSessionSpawnRequest,
    read_only: bool,
    patch_attempt_authorized: bool,
) -> String {
    format!(
        "Purpose: {}\nMode: {:?}\nRead only: {}\nPatch attempt authorized: {}\nTimeout seconds: {}\n\n{}",
        request.purpose,
        request.mode,
        read_only,
        patch_attempt_authorized,
        request.requested_timeout_secs,
        request.prompt
    )
}

fn child_summary_from_metadata(
    metadata: probe_protocol::session::SessionMetadata,
    status: SessionChildStatus,
    purpose: String,
    parent_turn_id: Option<String>,
    parent_turn_index: Option<u64>,
) -> SessionChildSummary {
    SessionChildSummary {
        session_id: metadata.id,
        title: metadata.title,
        cwd: metadata.cwd,
        state: SessionState::Active,
        status,
        initiator: metadata
            .parent_link
            .as_ref()
            .and_then(|parent| parent.initiator.clone()),
        purpose: Some(purpose),
        parent_turn_id,
        parent_turn_index,
        closure: None,
        created_at_ms: metadata.created_at_ms,
        updated_at_ms: metadata.updated_at_ms,
    }
}

fn summary_artifact_ref(
    reference: probe_protocol::session::SessionSummaryArtifact,
) -> ForgeChildSessionArtifactRef {
    let reference: SessionSummaryArtifactRef = reference.artifact_ref().clone();
    ForgeChildSessionArtifactRef {
        kind: format!("{:?}", reference.kind),
        path: reference.path,
        stable_digest: Some(reference.stable_digest),
        updated_at_ms: Some(reference.updated_at_ms),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{
        ForgeChildSessionController, ForgeChildSessionMode, ForgeChildSessionPolicy,
        ForgeChildSessionSpawnRequest,
    };
    use crate::session_store::FilesystemSessionStore;

    #[test]
    fn forge_child_session_defaults_to_read_only_research_and_returns_artifacts() {
        let temp = tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let parent = store
            .create_session("parent health diagnosis", temp.path())
            .expect("parent session");
        let controller = ForgeChildSessionController::new(&store);
        let policy = ForgeChildSessionPolicy {
            allowed_repositories: vec!["OpenAgentsInc/openagents".to_string()],
            ..ForgeChildSessionPolicy::default()
        };

        let spawned = controller
            .spawn_child_session(
                &policy,
                ForgeChildSessionSpawnRequest {
                    parent_session_id: parent.id.clone(),
                    title: "Research Nexus Cloudflare 1033".to_string(),
                    cwd: temp.path().to_path_buf(),
                    prompt: "Read health evidence and summarize public-edge failure.".to_string(),
                    purpose: "read-only Nexus edge research".to_string(),
                    repo_slug: "OpenAgentsInc/openagents".to_string(),
                    mode: ForgeChildSessionMode::Research,
                    requested_timeout_secs: 120,
                    parent_turn_id: Some("turn-1".to_string()),
                    parent_turn_index: Some(1),
                },
            )
            .expect("spawn child");

        assert_eq!(spawned.parent_session_id, parent.id);
        assert!(spawned.read_only);
        assert!(!spawned.patch_attempt_authorized);
        assert!(!spawned.production_recovery_authorized);
        assert!(
            spawned
                .artifacts
                .iter()
                .any(|artifact| artifact.kind == "probe.transcript")
        );

        let status = controller
            .read_child_status(&spawned.parent_session_id, &spawned.child.session_id)
            .expect("read child status");
        assert_eq!(status.child.session_id, spawned.child.session_id);
        assert!(
            status
                .artifacts
                .iter()
                .any(|artifact| artifact.kind == "probe.transcript")
        );

        let synthesis = controller
            .parent_synthesis_artifact(&spawned.parent_session_id)
            .expect("parent synthesis");
        assert_eq!(synthesis.child_sessions.len(), 1);
        assert!(!synthesis.direct_production_recovery_allowed);
    }

    #[test]
    fn forge_child_session_enforces_budgets_repo_and_patch_policy() {
        let temp = tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let parent = store
            .create_session("parent health diagnosis", temp.path())
            .expect("parent session");
        let controller = ForgeChildSessionController::new(&store);
        let policy = ForgeChildSessionPolicy {
            max_child_sessions: 1,
            max_prompt_bytes: 8,
            max_child_timeout_secs: 30,
            allowed_repositories: vec!["OpenAgentsInc/openagents".to_string()],
            allow_patch_attempts: false,
            allow_production_recovery_actions: false,
        };

        let too_large = controller.spawn_child_session(
            &policy,
            spawn_request(
                &parent.id,
                "OpenAgentsInc/openagents",
                "this prompt is too large",
            ),
        );
        assert!(too_large.is_err());

        let wrong_repo = controller.spawn_child_session(
            &ForgeChildSessionPolicy {
                max_prompt_bytes: 128,
                ..policy.clone()
            },
            spawn_request(&parent.id, "OpenAgentsInc/treasury", "research"),
        );
        assert!(wrong_repo.is_err());

        let patch = controller.spawn_child_session(
            &ForgeChildSessionPolicy {
                max_prompt_bytes: 128,
                ..policy.clone()
            },
            ForgeChildSessionSpawnRequest {
                mode: ForgeChildSessionMode::PatchAttempt,
                ..spawn_request(&parent.id, "OpenAgentsInc/openagents", "patch")
            },
        );
        assert!(patch.is_err());

        let production_recovery = controller.spawn_child_session(
            &ForgeChildSessionPolicy {
                max_prompt_bytes: 128,
                allow_patch_attempts: true,
                ..policy
            },
            ForgeChildSessionSpawnRequest {
                mode: ForgeChildSessionMode::ProductionRecovery,
                ..spawn_request(&parent.id, "OpenAgentsInc/openagents", "recover")
            },
        );
        assert!(production_recovery.is_err());
    }

    #[test]
    fn forge_child_session_allows_patch_attempt_when_policy_authorizes_it() {
        let temp = tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let parent = store
            .create_session("parent health diagnosis", temp.path())
            .expect("parent session");
        let controller = ForgeChildSessionController::new(&store);
        let policy = ForgeChildSessionPolicy {
            max_prompt_bytes: 128,
            allowed_repositories: vec!["OpenAgentsInc/openagents".to_string()],
            allow_patch_attempts: true,
            ..ForgeChildSessionPolicy::default()
        };

        let spawned = controller
            .spawn_child_session(
                &policy,
                ForgeChildSessionSpawnRequest {
                    mode: ForgeChildSessionMode::PatchAttempt,
                    ..spawn_request(&parent.id, "OpenAgentsInc/openagents", "patch")
                },
            )
            .expect("spawn patch child");
        assert!(!spawned.read_only);
        assert!(spawned.patch_attempt_authorized);
        assert!(!spawned.production_recovery_authorized);
    }

    fn spawn_request(
        parent_session_id: &probe_protocol::session::SessionId,
        repo_slug: &str,
        prompt: &str,
    ) -> ForgeChildSessionSpawnRequest {
        ForgeChildSessionSpawnRequest {
            parent_session_id: parent_session_id.clone(),
            title: "child".to_string(),
            cwd: std::env::temp_dir(),
            prompt: prompt.to_string(),
            purpose: "test child".to_string(),
            repo_slug: repo_slug.to_string(),
            mode: ForgeChildSessionMode::Research,
            requested_timeout_secs: 10,
            parent_turn_id: None,
            parent_turn_index: None,
        }
    }
}
