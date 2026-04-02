use std::fmt::{Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use probe_protocol::session::{
    BackendTurnReceipt, ItemId, PendingToolApproval, SessionBackendTarget, SessionChildLink,
    SessionHarnessProfile, SessionId, SessionIndex, SessionMetadata, SessionParentLink,
    SessionRuntimeOwner, SessionState, SessionTurn, SessionWorkspaceState, TimestampMs,
    ToolApprovalResolution, ToolExecutionRecord, TranscriptEvent, TranscriptItem,
    TranscriptItemKind, TurnId, TurnObservability,
};
use serde::{Deserialize, Serialize};

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

const INDEX_FILE: &str = "index.json";
const SESSIONS_DIR: &str = "sessions";
const METADATA_FILE: &str = "metadata.json";
const TRANSCRIPT_FILE: &str = "transcript.jsonl";
const APPROVALS_FILE: &str = "approvals.json";
const SUMMARY_FILE: &str = "summary.json";

#[derive(Debug)]
pub enum SessionStoreError {
    Io(std::io::Error),
    Json(serde_json::Error),
    NotFound(String),
    Conflict(String),
}

impl Display for SessionStoreError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::NotFound(id) => write!(f, "session not found: {id}"),
            Self::Conflict(message) => write!(f, "conflict: {message}"),
        }
    }
}

impl std::error::Error for SessionStoreError {}

impl From<std::io::Error> for SessionStoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for SessionStoreError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Clone, Debug)]
pub struct NewItem {
    pub kind: TranscriptItemKind,
    pub text: String,
    pub name: Option<String>,
    pub tool_call_id: Option<String>,
    pub arguments: Option<serde_json::Value>,
    pub tool_execution: Option<ToolExecutionRecord>,
}

impl NewItem {
    #[must_use]
    pub fn new(kind: TranscriptItemKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            name: None,
            tool_call_id: None,
            arguments: None,
            tool_execution: None,
        }
    }

    #[must_use]
    pub fn tool_call(
        name: impl Into<String>,
        tool_call_id: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            kind: TranscriptItemKind::ToolCall,
            text: serde_json::to_string(&arguments)
                .unwrap_or_else(|_| String::from("{\"error\":\"tool arguments encode failed\"}")),
            name: Some(name.into()),
            tool_call_id: Some(tool_call_id.into()),
            arguments: Some(arguments),
            tool_execution: None,
        }
    }

    #[must_use]
    pub fn tool_result(
        name: impl Into<String>,
        tool_call_id: impl Into<String>,
        text: impl Into<String>,
        tool_execution: ToolExecutionRecord,
    ) -> Self {
        Self {
            kind: TranscriptItemKind::ToolResult,
            text: text.into(),
            name: Some(name.into()),
            tool_call_id: Some(tool_call_id.into()),
            arguments: None,
            tool_execution: Some(tool_execution),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NewSession {
    pub title: String,
    pub cwd: PathBuf,
    pub system_prompt: Option<String>,
    pub harness_profile: Option<SessionHarnessProfile>,
    pub backend: Option<SessionBackendTarget>,
    pub runtime_owner: Option<SessionRuntimeOwner>,
    pub workspace_state: Option<SessionWorkspaceState>,
    pub parent_link: Option<SessionParentLink>,
}

impl NewSession {
    #[must_use]
    pub fn new(title: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            title: title.into(),
            cwd: cwd.into(),
            system_prompt: None,
            harness_profile: None,
            backend: None,
            runtime_owner: None,
            workspace_state: None,
            parent_link: None,
        }
    }

    #[must_use]
    pub fn with_system_prompt(mut self, system_prompt: Option<String>) -> Self {
        self.system_prompt = system_prompt;
        self
    }

    #[must_use]
    pub fn with_harness_profile(mut self, harness_profile: Option<SessionHarnessProfile>) -> Self {
        self.harness_profile = harness_profile;
        self
    }

    #[must_use]
    pub fn with_backend(mut self, backend: SessionBackendTarget) -> Self {
        self.backend = Some(backend);
        self
    }

    #[must_use]
    pub fn with_runtime_owner(mut self, runtime_owner: Option<SessionRuntimeOwner>) -> Self {
        self.runtime_owner = runtime_owner;
        self
    }

    #[must_use]
    pub fn with_workspace_state(mut self, workspace_state: Option<SessionWorkspaceState>) -> Self {
        self.workspace_state = workspace_state;
        self
    }

    #[must_use]
    pub fn with_parent_link(mut self, parent_link: Option<SessionParentLink>) -> Self {
        self.parent_link = parent_link;
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: SessionId,
    pub title: String,
    pub cwd: PathBuf,
    pub system_prompt: Option<String>,
    pub harness_profile: Option<SessionHarnessProfile>,
    pub backend: Option<SessionBackendTarget>,
    pub runtime_owner: Option<SessionRuntimeOwner>,
    pub workspace_state: Option<SessionWorkspaceState>,
    pub parent_link: Option<SessionParentLink>,
    pub child_links: Vec<SessionChildLink>,
    pub transcript: Vec<TranscriptItem>,
}

#[derive(Clone, Debug)]
pub struct FilesystemSessionStore {
    root: PathBuf,
}

impl FilesystemSessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn create_session(&self, session: NewSession) -> Result<SessionId, SessionStoreError> {
        let session_id = SESSION_COUNTER.fetch_add(1, Ordering::SeqCst);
        let session_dir = self.root.join(SESSIONS_DIR).join(session_id.to_string());
        fs::create_dir_all(&session_dir)?;

        let metadata = SessionMetadata {
            title: session.title,
            cwd: session.cwd,
            system_prompt: session.system_prompt,
            harness_profile: session.harness_profile,
            backend: session.backend,
            runtime_owner: session.runtime_owner,
            workspace_state: session.workspace_state,
            parent_link: session.parent_link,
        };

        let metadata_file = session_dir.join(METADATA_FILE);
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(&metadata_file)?;
        serde_json::to_writer_pretty(&mut file, &metadata)?;

        let transcript_file = session_dir.join(TRANSCRIPT_FILE);
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(&transcript_file)?;

        let summary_file = session_dir.join(SUMMARY_FILE);
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(&summary_file)?;

        Ok(SessionId(session_id))
    }

    pub fn get_session_summary(&self, session_id: SessionId) -> Result<SessionSummary, SessionStoreError> {
        let session_dir = self.root.join(SESSIONS_DIR).join(session_id.to_string());
        let summary_file = session_dir.join(SUMMARY_FILE);
        let file = File::open(&summary_file)?;
        let reader = BufReader::new(file);
        let summary: SessionSummary = serde_json::from_reader(reader)?;
        Ok(summary)
    }

    pub fn update_session_summary(&self, session_id: SessionId, summary: SessionSummary) -> Result<(), SessionStoreError> {
        let session_dir = self.root.join(SESSIONS_DIR).join(session_id.to_string());
        let summary_file = session_dir.join(SUMMARY_FILE);
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&summary_file)?;
        serde_json::to_writer_pretty(&mut file, &summary)?;
        Ok(())
    }
}