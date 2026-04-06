use std::fmt::{Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use probe_protocol::session::{
    BackendTurnReceipt, ItemId, PendingToolApproval, SessionBackendTarget, SessionChildLink,
    SessionHarnessProfile, SessionId, SessionIndex, SessionMetadata, SessionMountRef,
    SessionParentLink, SessionRuntimeOwner, SessionState, SessionTurn, SessionWorkspaceState,
    TimestampMs, ToolApprovalResolution, ToolExecutionRecord, TranscriptEvent, TranscriptItem,
    TranscriptItemKind, TurnId, TurnObservability,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

const INDEX_FILE: &str = "index.json";
const SESSIONS_DIR: &str = "sessions";
const METADATA_FILE: &str = "metadata.json";
const TRANSCRIPT_FILE: &str = "transcript.jsonl";
const APPROVALS_FILE: &str = "approvals.json";
const ARTIFACTS_DIR: &str = "artifacts";

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
    pub mounted_refs: Vec<SessionMountRef>,
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
            mounted_refs: Vec::new(),
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
    pub fn with_mounted_refs(mut self, mounted_refs: Vec<SessionMountRef>) -> Self {
        self.mounted_refs = mounted_refs;
        self
    }

    #[must_use]
    pub fn with_parent_link(mut self, parent_link: Option<SessionParentLink>) -> Self {
        self.parent_link = parent_link;
        self
    }
}

#[derive(Clone, Debug)]
pub struct FilesystemSessionStore {
    root: PathBuf,
}

impl FilesystemSessionStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    pub fn create_session(
        &self,
        title: impl Into<String>,
        cwd: impl Into<PathBuf>,
    ) -> Result<SessionMetadata, SessionStoreError> {
        self.create_session_with(NewSession::new(title, cwd))
    }

    pub fn create_session_with(
        &self,
        session: NewSession,
    ) -> Result<SessionMetadata, SessionStoreError> {
        self.ensure_layout()?;
        let created_at_ms = now_ms();
        let session_id = SessionId::new(format!(
            "sess_{}_{}_{}",
            created_at_ms,
            std::process::id(),
            SESSION_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let session_dir = self.session_dir(&session_id);
        fs::create_dir_all(&session_dir)?;

        let transcript_path = session_dir.join(TRANSCRIPT_FILE);
        File::create(&transcript_path)?;
        let approvals_path = session_dir.join(APPROVALS_FILE);
        write_json_pretty_atomic(&approvals_path, &Vec::<PendingToolApproval>::new())?;

        let metadata = SessionMetadata {
            id: session_id,
            title: session.title,
            cwd: session.cwd,
            system_prompt: session.system_prompt,
            harness_profile: session.harness_profile,
            created_at_ms,
            updated_at_ms: created_at_ms,
            state: SessionState::Active,
            next_turn_index: 0,
            backend: session.backend,
            runtime_owner: session.runtime_owner,
            workspace_state: session.workspace_state,
            hosted_receipts: None,
            mounted_refs: session.mounted_refs,
            participants: Vec::new(),
            controller_lease: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            transcript_path,
            parent_link: session.parent_link,
            child_links: Vec::new(),
        };
        self.write_metadata(&metadata)?;
        self.upsert_index(metadata.clone())?;
        Ok(metadata)
    }

    pub fn append_turn(
        &self,
        session_id: &SessionId,
        items: &[NewItem],
    ) -> Result<SessionTurn, SessionStoreError> {
        self.append_turn_with_details(session_id, items, None, None)
    }

    pub fn append_turn_with_observability(
        &self,
        session_id: &SessionId,
        items: &[NewItem],
        observability: Option<TurnObservability>,
    ) -> Result<SessionTurn, SessionStoreError> {
        self.append_turn_with_details(session_id, items, observability, None)
    }

    pub fn append_turn_with_details(
        &self,
        session_id: &SessionId,
        items: &[NewItem],
        observability: Option<TurnObservability>,
        backend_receipt: Option<BackendTurnReceipt>,
    ) -> Result<SessionTurn, SessionStoreError> {
        let mut metadata = self.read_metadata(session_id)?;
        let turn_id = TurnId(metadata.next_turn_index);
        let started_at_ms = now_ms();
        let transcript_items = items
            .iter()
            .enumerate()
            .map(|(index, item)| TranscriptItem {
                id: ItemId::new(format!("item-{}-{index}", turn_id.0)),
                turn_id,
                sequence: index as u32,
                kind: item.kind,
                text: item.text.clone(),
                name: item.name.clone(),
                tool_call_id: item.tool_call_id.clone(),
                arguments: item.arguments.clone(),
                tool_execution: item.tool_execution.clone(),
            })
            .collect::<Vec<_>>();
        let turn = SessionTurn {
            id: turn_id,
            index: metadata.next_turn_index,
            started_at_ms,
            completed_at_ms: Some(now_ms()),
            observability,
            backend_receipt,
            items: transcript_items,
        };
        let event = TranscriptEvent {
            session_id: session_id.clone(),
            turn: turn.clone(),
        };
        self.append_event(&metadata.transcript_path, &event)?;
        metadata.next_turn_index += 1;
        metadata.updated_at_ms = now_ms();
        self.write_metadata(&metadata)?;
        self.upsert_index(metadata)?;
        Ok(turn)
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionMetadata>, SessionStoreError> {
        let mut sessions = self.read_index()?.sessions;
        sessions.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
        Ok(sessions)
    }

    pub fn read_metadata(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionMetadata, SessionStoreError> {
        let path = self.session_dir(session_id).join(METADATA_FILE);
        if !path.exists() {
            return Err(SessionStoreError::NotFound(session_id.as_str().to_string()));
        }
        let file = File::open(path)?;
        Ok(serde_json::from_reader(file)?)
    }

    pub fn replace_metadata(
        &self,
        metadata: SessionMetadata,
    ) -> Result<SessionMetadata, SessionStoreError> {
        self.write_metadata(&metadata)?;
        self.upsert_index(metadata.clone())?;
        Ok(metadata)
    }

    pub fn append_child_link(
        &self,
        parent_session_id: &SessionId,
        child_link: SessionChildLink,
    ) -> Result<SessionMetadata, SessionStoreError> {
        let mut metadata = self.read_metadata(parent_session_id)?;
        if metadata
            .child_links
            .iter()
            .any(|existing| existing.session_id == child_link.session_id)
        {
            return Ok(metadata);
        }
        metadata.child_links.push(child_link);
        metadata.updated_at_ms = now_ms();
        self.replace_metadata(metadata)
    }

    pub fn read_transcript(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<TranscriptEvent>, SessionStoreError> {
        let metadata = self.read_metadata(session_id)?;
        let file = File::open(metadata.transcript_path)?;
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

    pub fn read_tool_approvals(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<PendingToolApproval>, SessionStoreError> {
        let path = self.session_dir(session_id).join(APPROVALS_FILE);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(path)?;
        Ok(serde_json::from_reader(file)?)
    }

    pub fn read_pending_tool_approvals(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<PendingToolApproval>, SessionStoreError> {
        Ok(self
            .read_tool_approvals(session_id)?
            .into_iter()
            .filter(PendingToolApproval::is_pending)
            .collect())
    }

    pub fn append_pending_tool_approvals(
        &self,
        session_id: &SessionId,
        new_approvals: &[PendingToolApproval],
    ) -> Result<(), SessionStoreError> {
        if new_approvals.is_empty() {
            return Ok(());
        }

        let mut approvals = self.read_tool_approvals(session_id)?;
        for approval in new_approvals {
            if approvals
                .iter()
                .any(|existing| existing.tool_call_id == approval.tool_call_id)
            {
                return Err(SessionStoreError::Conflict(format!(
                    "session {} already tracks approval state for call {}",
                    session_id.as_str(),
                    approval.tool_call_id
                )));
            }
            approvals.push(approval.clone());
        }
        self.write_tool_approvals(session_id, approvals.as_slice())
    }

    pub fn resolve_pending_tool_approval(
        &self,
        session_id: &SessionId,
        call_id: &str,
        resolution: ToolApprovalResolution,
    ) -> Result<PendingToolApproval, SessionStoreError> {
        let mut approvals = self.read_tool_approvals(session_id)?;
        let Some(index) = approvals
            .iter()
            .position(|approval| approval.tool_call_id == call_id)
        else {
            return Err(SessionStoreError::NotFound(format!(
                "{}:{}",
                session_id.as_str(),
                call_id
            )));
        };
        if !approvals[index].is_pending() {
            return Err(SessionStoreError::Conflict(format!(
                "approval for session {} call {} was already resolved",
                session_id.as_str(),
                call_id
            )));
        }
        approvals[index].resolution = Some(resolution);
        approvals[index].resolved_at_ms = Some(now_ms());
        let resolved = approvals[index].clone();
        self.write_tool_approvals(session_id, approvals.as_slice())?;
        Ok(resolved)
    }

    fn ensure_layout(&self) -> Result<(), SessionStoreError> {
        fs::create_dir_all(self.root.join(SESSIONS_DIR))?;
        if !self.index_path().exists() {
            write_json_pretty_atomic(self.index_path().as_path(), &SessionIndex::default())?;
        }
        Ok(())
    }

    fn index_path(&self) -> PathBuf {
        self.root.join(INDEX_FILE)
    }

    fn session_dir(&self, session_id: &SessionId) -> PathBuf {
        self.root.join(SESSIONS_DIR).join(session_id.as_str())
    }

    pub(crate) fn session_artifact_path(&self, session_id: &SessionId, file_name: &str) -> PathBuf {
        self.session_dir(session_id)
            .join(ARTIFACTS_DIR)
            .join(file_name)
    }

    pub(crate) fn write_session_artifact_json<T: Serialize + ?Sized>(
        &self,
        session_id: &SessionId,
        file_name: &str,
        value: &T,
    ) -> Result<PathBuf, SessionStoreError> {
        let path = self.session_artifact_path(session_id, file_name);
        write_json_pretty_atomic(path.as_path(), value)?;
        Ok(path)
    }

    pub(crate) fn read_session_artifact_json<T: DeserializeOwned>(
        &self,
        session_id: &SessionId,
        file_name: &str,
    ) -> Result<Option<T>, SessionStoreError> {
        let path = self.session_artifact_path(session_id, file_name);
        if !path.exists() {
            return Ok(None);
        }
        let file = File::open(path)?;
        Ok(Some(serde_json::from_reader(file)?))
    }

    pub(crate) fn remove_session_artifact(
        &self,
        session_id: &SessionId,
        file_name: &str,
    ) -> Result<(), SessionStoreError> {
        let path = self.session_artifact_path(session_id, file_name);
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(SessionStoreError::Io(error)),
        }
    }

    fn write_tool_approvals(
        &self,
        session_id: &SessionId,
        approvals: &[PendingToolApproval],
    ) -> Result<(), SessionStoreError> {
        let path = self.session_dir(session_id).join(APPROVALS_FILE);
        write_json_pretty_atomic(path.as_path(), approvals)?;
        Ok(())
    }

    fn write_metadata(&self, metadata: &SessionMetadata) -> Result<(), SessionStoreError> {
        let path = self.session_dir(&metadata.id).join(METADATA_FILE);
        write_json_pretty_atomic(path.as_path(), metadata)?;
        Ok(())
    }

    fn append_event(
        &self,
        transcript_path: &Path,
        event: &TranscriptEvent,
    ) -> Result<(), SessionStoreError> {
        let mut file = OpenOptions::new().append(true).open(transcript_path)?;
        serde_json::to_writer(&mut file, event)?;
        file.write_all(b"\n")?;
        Ok(())
    }

    fn read_index(&self) -> Result<SessionIndex, SessionStoreError> {
        self.ensure_layout()?;
        let file = File::open(self.index_path())?;
        Ok(serde_json::from_reader(file)?)
    }

    fn upsert_index(&self, metadata: SessionMetadata) -> Result<(), SessionStoreError> {
        let mut index = self.read_index()?;
        if let Some(existing) = index
            .sessions
            .iter_mut()
            .find(|session| session.id.as_str() == metadata.id.as_str())
        {
            *existing = metadata;
        } else {
            index.sessions.push(metadata);
        }
        write_json_pretty_atomic(self.index_path().as_path(), &index)?;
        Ok(())
    }
}

fn write_json_pretty_atomic<T: Serialize + ?Sized>(
    path: &Path,
    value: &T,
) -> Result<(), SessionStoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = temp_json_path(path);
    {
        let mut file = File::create(&temp_path)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.flush()?;
    }
    fs::rename(temp_path, path)?;
    Ok(())
}

fn temp_json_path(path: &Path) -> PathBuf {
    let suffix = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("probe-json");
    path.with_file_name(format!("{file_name}.tmp-{}-{suffix}", std::process::id()))
}

fn now_ms() -> TimestampMs {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::{FilesystemSessionStore, NewItem, NewSession};
    use probe_protocol::session::{
        BackendTranscriptReceipt, BackendTurnReceipt, CacheSignal, PendingToolApproval,
        SessionBackendTarget, SessionHarnessProfile, ToolApprovalResolution, ToolApprovalState,
        ToolExecutionRecord, ToolPolicyDecision, ToolRiskClass, TranscriptItemKind,
        TurnObservability, UsageMeasurement, UsageTruth,
    };

    #[test]
    fn create_session_persists_metadata_and_index() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());

        let metadata = store
            .create_session_with(NewSession::new("bootstrap", temp.path()).with_backend(
                SessionBackendTarget {
                    profile_name: String::from("psionic-qwen35-2b-q8-registry"),
                    base_url: String::from("http://127.0.0.1:8080/v1"),
                    model: String::from("qwen3.5-2b-q8_0-registry.gguf"),
                },
            ))
            .expect("create session");

        let listed = store.list_sessions().expect("list sessions");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], metadata);
        assert_eq!(
            listed[0]
                .backend
                .as_ref()
                .expect("backend should be recorded")
                .profile_name,
            "psionic-qwen35-2b-q8-registry"
        );
    }

    #[test]
    fn create_session_persists_harness_profile() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());

        let metadata = store
            .create_session_with(
                NewSession::new("harness", temp.path()).with_harness_profile(Some(
                    SessionHarnessProfile {
                        name: String::from("coding_bootstrap_default"),
                        version: String::from("v1"),
                    },
                )),
            )
            .expect("create session");

        assert_eq!(
            metadata
                .harness_profile
                .as_ref()
                .expect("harness profile should persist")
                .name,
            "coding_bootstrap_default"
        );
    }

    #[test]
    fn append_turn_writes_append_only_transcript_and_updates_index() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let metadata = store
            .create_session("turns", temp.path())
            .expect("create session");

        let turn = store
            .append_turn(
                &metadata.id,
                &[
                    NewItem::new(TranscriptItemKind::UserMessage, "hello"),
                    NewItem::new(TranscriptItemKind::AssistantMessage, "hi"),
                ],
            )
            .expect("append turn");

        assert_eq!(turn.index, 0);
        assert_eq!(turn.items.len(), 2);

        let transcript = store
            .read_transcript(&metadata.id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].turn.items[0].text, "hello");

        let listed = store.list_sessions().expect("list sessions");
        assert_eq!(listed[0].next_turn_index, 1);
    }

    #[test]
    fn append_turn_persists_tool_execution_record() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let metadata = store
            .create_session("tool-result", temp.path())
            .expect("create session");

        store
            .append_turn(
                &metadata.id,
                &[NewItem::tool_result(
                    "shell",
                    "call_1",
                    "{\"ok\":true}",
                    ToolExecutionRecord {
                        risk_class: ToolRiskClass::ShellReadOnly,
                        policy_decision: ToolPolicyDecision::AutoAllow,
                        approval_state: ToolApprovalState::NotRequired,
                        command: Some(String::from("pwd")),
                        exit_code: Some(0),
                        timed_out: Some(false),
                        truncated: Some(false),
                        bytes_returned: Some(4),
                        files_touched: Vec::new(),
                        files_changed: Vec::new(),
                        reason: None,
                    },
                )],
            )
            .expect("append turn");

        let transcript = store
            .read_transcript(&metadata.id)
            .expect("read transcript");
        let item = &transcript[0].turn.items[0];
        assert_eq!(item.kind, TranscriptItemKind::ToolResult);
        assert_eq!(
            item.tool_execution
                .as_ref()
                .expect("tool execution record should persist")
                .command
                .as_deref(),
            Some("pwd")
        );
    }

    #[test]
    fn append_turn_with_observability_persists_metrics() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let metadata = store
            .create_session("metrics", temp.path())
            .expect("create session");

        store
            .append_turn_with_observability(
                &metadata.id,
                &[NewItem::new(
                    TranscriptItemKind::AssistantMessage,
                    "metrics",
                )],
                Some(TurnObservability {
                    wallclock_ms: 42,
                    model_output_ms: Some(42),
                    prompt_tokens: Some(12),
                    prompt_tokens_detail: Some(UsageMeasurement {
                        value: 12,
                        truth: UsageTruth::Estimated,
                    }),
                    completion_tokens: Some(6),
                    completion_tokens_detail: Some(UsageMeasurement {
                        value: 6,
                        truth: UsageTruth::Exact,
                    }),
                    total_tokens: Some(18),
                    total_tokens_detail: Some(UsageMeasurement {
                        value: 18,
                        truth: UsageTruth::Estimated,
                    }),
                    completion_tokens_per_second_x1000: Some(142_857),
                    cache_signal: CacheSignal::ColdStart,
                }),
            )
            .expect("append turn");

        let transcript = store
            .read_transcript(&metadata.id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 1);
        let observability = transcript[0]
            .turn
            .observability
            .as_ref()
            .expect("observability should persist");
        assert_eq!(observability.wallclock_ms, 42);
        assert_eq!(observability.model_output_ms, Some(42));
        assert_eq!(
            observability
                .prompt_tokens_detail
                .as_ref()
                .expect("prompt token detail should persist")
                .truth,
            UsageTruth::Estimated
        );
        assert_eq!(
            observability.completion_tokens_per_second_x1000,
            Some(142_857)
        );
        assert!(matches!(observability.cache_signal, CacheSignal::ColdStart));
    }

    #[test]
    fn append_turn_with_details_persists_backend_receipt() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let metadata = store
            .create_session("backend-receipt", temp.path())
            .expect("create session");

        store
            .append_turn_with_details(
                &metadata.id,
                &[NewItem::new(
                    TranscriptItemKind::AssistantMessage,
                    "receipt",
                )],
                None,
                Some(BackendTurnReceipt {
                    failure: None,
                    availability: None,
                    transcript: Some(BackendTranscriptReceipt {
                        format: String::from("foundation_models.transcript.v1"),
                        payload: String::from("{\"version\":1}"),
                    }),
                }),
            )
            .expect("append turn");

        let transcript = store
            .read_transcript(&metadata.id)
            .expect("read transcript");
        let receipt = transcript[0]
            .turn
            .backend_receipt
            .as_ref()
            .expect("backend receipt should persist");
        assert_eq!(
            receipt
                .transcript
                .as_ref()
                .expect("transcript receipt should persist")
                .format,
            "foundation_models.transcript.v1"
        );
    }

    #[test]
    fn pending_tool_approvals_persist_and_filter_pending_entries() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let metadata = store
            .create_session("approvals", temp.path())
            .expect("create session");

        let approvals = vec![
            PendingToolApproval {
                session_id: metadata.id.clone(),
                tool_call_id: String::from("call_patch_1"),
                tool_name: String::from("apply_patch"),
                arguments: serde_json::json!({ "path": "hello.txt" }),
                risk_class: ToolRiskClass::Write,
                reason: Some(String::from("write approval required")),
                tool_call_turn_index: 0,
                paused_result_turn_index: 1,
                requested_at_ms: 1,
                resolved_at_ms: None,
                resolution: None,
            },
            PendingToolApproval {
                session_id: metadata.id.clone(),
                tool_call_id: String::from("call_shell_1"),
                tool_name: String::from("shell"),
                arguments: serde_json::json!({ "command": "curl https://example.com" }),
                risk_class: ToolRiskClass::Network,
                reason: Some(String::from("network approval required")),
                tool_call_turn_index: 0,
                paused_result_turn_index: 1,
                requested_at_ms: 2,
                resolved_at_ms: Some(3),
                resolution: Some(ToolApprovalResolution::Rejected),
            },
        ];

        store
            .append_pending_tool_approvals(&metadata.id, approvals.as_slice())
            .expect("append approvals");

        let all = store
            .read_tool_approvals(&metadata.id)
            .expect("read all approvals");
        assert_eq!(all, approvals);

        let pending = store
            .read_pending_tool_approvals(&metadata.id)
            .expect("read pending approvals");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].tool_call_id, "call_patch_1");
    }

    #[test]
    fn resolve_pending_tool_approval_marks_record_resolved() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = FilesystemSessionStore::new(temp.path());
        let metadata = store
            .create_session("resolve-approval", temp.path())
            .expect("create session");

        store
            .append_pending_tool_approvals(
                &metadata.id,
                &[PendingToolApproval {
                    session_id: metadata.id.clone(),
                    tool_call_id: String::from("call_patch_1"),
                    tool_name: String::from("apply_patch"),
                    arguments: serde_json::json!({ "path": "hello.txt" }),
                    risk_class: ToolRiskClass::Write,
                    reason: Some(String::from("write approval required")),
                    tool_call_turn_index: 0,
                    paused_result_turn_index: 1,
                    requested_at_ms: 1,
                    resolved_at_ms: None,
                    resolution: None,
                }],
            )
            .expect("append pending approval");

        let resolved = store
            .resolve_pending_tool_approval(
                &metadata.id,
                "call_patch_1",
                ToolApprovalResolution::Approved,
            )
            .expect("resolve approval");

        assert_eq!(resolved.resolution, Some(ToolApprovalResolution::Approved));
        assert!(resolved.resolved_at_ms.is_some());
        assert!(
            store
                .read_pending_tool_approvals(&metadata.id)
                .expect("read pending approvals")
                .is_empty()
        );
    }
}
