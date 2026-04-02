use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use probe_protocol::runtime::{
    DetachedSessionRecoveryState, DetachedSessionStatus, DetachedSessionSummary,
};
use probe_protocol::session::{SessionId, SessionMetadata, TimestampMs};
use serde::{Deserialize, Serialize};

const DETACHED_REGISTRY_FILE: &str = "detached-sessions.json";
const DETACHED_REGISTRY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug)]
pub(crate) enum DetachedRegistryError {
    Io(io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for DetachedRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
        }
    }
}

impl std::error::Error for DetachedRegistryError {}

impl From<io::Error> for DetachedRegistryError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for DetachedRegistryError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Clone)]
pub(crate) struct DetachedSessionRegistry {
    path: PathBuf,
}

impl DetachedSessionRegistry {
    pub(crate) fn new(probe_home: &Path) -> Self {
        Self {
            path: probe_home.join("daemon").join(DETACHED_REGISTRY_FILE),
        }
    }

    pub(crate) fn tracked_session_ids(&self) -> Result<Vec<SessionId>, DetachedRegistryError> {
        Ok(self
            .read_state()?
            .sessions
            .into_iter()
            .map(|summary| summary.session_id)
            .collect())
    }

    pub(crate) fn list(&self) -> Result<Vec<DetachedSessionSummary>, DetachedRegistryError> {
        let mut sessions = self.read_state()?.sessions;
        sessions.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
        Ok(sessions)
    }

    pub(crate) fn read(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<DetachedSessionSummary>, DetachedRegistryError> {
        Ok(self
            .read_state()?
            .sessions
            .into_iter()
            .find(|summary| summary.session_id == *session_id))
    }

    pub(crate) fn remove(&self, session_id: &SessionId) -> Result<(), DetachedRegistryError> {
        let mut state = self.read_state()?;
        let previous_len = state.sessions.len();
        state
            .sessions
            .retain(|summary| summary.session_id != *session_id);
        if state.sessions.len() != previous_len {
            self.write_state(&state)?;
        }
        Ok(())
    }

    pub(crate) fn upsert(
        &self,
        summary: DetachedSessionSummary,
    ) -> Result<(), DetachedRegistryError> {
        let mut state = self.read_state()?;
        if let Some(existing) = state
            .sessions
            .iter_mut()
            .find(|existing| existing.session_id == summary.session_id)
        {
            *existing = summary;
        } else {
            state.sessions.push(summary);
        }
        state
            .sessions
            .sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
        self.write_state(&state)
    }

    pub(crate) fn register_session(
        &self,
        metadata: &SessionMetadata,
        now_ms: TimestampMs,
    ) -> Result<DetachedSessionSummary, DetachedRegistryError> {
        let existing = self.read(&metadata.id)?;
        let summary = DetachedSessionSummary {
            session_id: metadata.id.clone(),
            title: metadata.title.clone(),
            cwd: metadata.cwd.clone(),
            status: existing
                .as_ref()
                .map(|value| value.status)
                .unwrap_or(DetachedSessionStatus::Idle),
            runtime_owner: metadata.runtime_owner.clone(),
            workspace_state: metadata.workspace_state.clone(),
            active_turn_id: existing
                .as_ref()
                .and_then(|value| value.active_turn_id.clone()),
            queued_turn_count: existing
                .as_ref()
                .map(|value| value.queued_turn_count)
                .unwrap_or(0),
            pending_approval_count: existing
                .as_ref()
                .map(|value| value.pending_approval_count)
                .unwrap_or(0),
            last_terminal_turn_id: existing
                .as_ref()
                .and_then(|value| value.last_terminal_turn_id.clone()),
            last_terminal_status: existing
                .as_ref()
                .and_then(|value| value.last_terminal_status),
            registered_at_ms: existing
                .as_ref()
                .map(|value| value.registered_at_ms)
                .unwrap_or(now_ms),
            updated_at_ms: now_ms,
            recovery_state: existing
                .as_ref()
                .map(|value| value.recovery_state)
                .unwrap_or(DetachedSessionRecoveryState::Clean),
            recovery_note: existing
                .as_ref()
                .and_then(|value| value.recovery_note.clone()),
        };
        self.upsert(summary.clone())?;
        Ok(summary)
    }

    fn read_state(&self) -> Result<DetachedRegistryState, DetachedRegistryError> {
        if !self.path.exists() {
            return Ok(DetachedRegistryState::default());
        }
        let file = File::open(&self.path)?;
        Ok(serde_json::from_reader(file)?)
    }

    fn write_state(&self, state: &DetachedRegistryState) -> Result<(), DetachedRegistryError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp_path = self.path.with_file_name(format!(
            "{}.tmp-{}-{}",
            self.path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("detached-sessions.json"),
            std::process::id(),
            now_ms(),
        ));
        {
            let mut file = File::create(&temp_path)?;
            serde_json::to_writer_pretty(&mut file, state)?;
            file.flush()?;
        }
        fs::rename(temp_path, &self.path)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct DetachedRegistryState {
    #[serde(default = "schema_version")]
    schema_version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sessions: Vec<DetachedSessionSummary>,
}

fn schema_version() -> u32 {
    DETACHED_REGISTRY_SCHEMA_VERSION
}

fn now_ms() -> TimestampMs {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_millis() as u64
}
