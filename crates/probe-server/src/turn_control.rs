use std::fs::{self, File};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use probe_core::runtime::ProbeRuntime;
use probe_core::session_store::SessionStoreError;
use probe_protocol::backend::BackendProfile;
use probe_protocol::runtime::{
    InspectSessionTurnsResponse, QueuedTurnStatus, SessionTurnControlRecord, ToolLoopRecipe,
    TurnAuthor, TurnRequest, TurnSubmissionKind,
};
use probe_protocol::session::{SessionId, TimestampMs};
use serde::{Deserialize, Serialize};

const TURN_CONTROL_FILE: &str = "turn-control.json";
const TURN_CONTROL_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredTurnControlRecord {
    pub record: SessionTurnControlRecord,
    pub profile: BackendProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_loop: Option<ToolLoopRecipe>,
}

impl StoredTurnControlRecord {
    pub(crate) fn to_turn_request(&self) -> TurnRequest {
        TurnRequest {
            session_id: self.record.session_id.clone(),
            profile: self.profile.clone(),
            prompt: self.record.prompt.clone(),
            author: Some(self.record.author.clone()),
            tool_loop: self.tool_loop.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SessionTurnControlState {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub next_turn_id: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<StoredTurnControlRecord>,
}

impl Default for SessionTurnControlState {
    fn default() -> Self {
        Self {
            schema_version: TURN_CONTROL_SCHEMA_VERSION,
            next_turn_id: 0,
            turns: Vec::new(),
        }
    }
}

impl SessionTurnControlState {
    pub(crate) fn load(
        runtime: &ProbeRuntime,
        session_id: &SessionId,
    ) -> Result<Self, SessionStoreError> {
        let path = control_path(runtime, session_id)?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let file = File::open(path)?;
        Ok(serde_json::from_reader(file)?)
    }

    pub(crate) fn save(
        &self,
        runtime: &ProbeRuntime,
        session_id: &SessionId,
    ) -> Result<(), SessionStoreError> {
        let path = control_path(runtime, session_id)?;
        write_json_pretty_atomic(path.as_path(), self)?;
        Ok(())
    }

    pub(crate) fn recover_orphaned_running_turns(
        &mut self,
        approvals_pending: bool,
        now_ms: TimestampMs,
    ) -> bool {
        let mut mutated = false;
        let mut kept_resumable_approval_turn = false;
        for turn in &mut self.turns {
            if turn.record.status != QueuedTurnStatus::Running {
                continue;
            }
            if turn.record.awaiting_approval && approvals_pending && !kept_resumable_approval_turn {
                kept_resumable_approval_turn = true;
                continue;
            }
            turn.record.status = QueuedTurnStatus::Failed;
            turn.record.awaiting_approval = false;
            turn.record.finished_at_ms = Some(now_ms);
            turn.record.failure_message = Some(if turn.record.awaiting_approval {
                String::from(
                    "probe-server restarted before this approval-paused turn could be resumed",
                )
            } else {
                String::from("probe-server restarted before this running turn completed")
            });
            turn.record.last_progress_at_ms = Some(now_ms);
            mutated = true;
        }
        mutated
    }

    pub(crate) fn push_turn(
        &mut self,
        session_id: &SessionId,
        submission_kind: TurnSubmissionKind,
        status: QueuedTurnStatus,
        request: &TurnRequest,
        requested_at_ms: TimestampMs,
        started_at_ms: Option<TimestampMs>,
        execution_timeout_ms: Option<u64>,
    ) -> SessionTurnControlRecord {
        let last_progress_at_ms = started_at_ms;
        let record = SessionTurnControlRecord {
            turn_id: format!("turn-{}", self.next_turn_id),
            session_id: session_id.clone(),
            submission_kind,
            status,
            prompt: request.prompt.clone(),
            author: request.author.clone().unwrap_or_else(default_turn_author),
            requested_at_ms,
            started_at_ms,
            finished_at_ms: None,
            queue_position: None,
            awaiting_approval: false,
            failure_message: None,
            cancellation_reason: None,
            last_progress_at_ms,
            execution_timeout_at_ms: started_at_ms.zip(execution_timeout_ms).map(
                |(started_at_ms, execution_timeout_ms)| {
                    started_at_ms.saturating_add(execution_timeout_ms)
                },
            ),
        };
        self.next_turn_id += 1;
        self.turns.push(StoredTurnControlRecord {
            record: record.clone(),
            profile: request.profile.clone(),
            tool_loop: request.tool_loop.clone(),
        });
        record
    }

    pub(crate) fn active_turn(&self) -> Option<&StoredTurnControlRecord> {
        self.turns
            .iter()
            .find(|turn| turn.record.status == QueuedTurnStatus::Running)
    }

    pub(crate) fn active_turn_mut(&mut self) -> Option<&mut StoredTurnControlRecord> {
        self.turns
            .iter_mut()
            .find(|turn| turn.record.status == QueuedTurnStatus::Running)
    }

    pub(crate) fn queued_turn_mut(
        &mut self,
        turn_id: &str,
    ) -> Option<&mut StoredTurnControlRecord> {
        self.turns.iter_mut().find(|turn| {
            turn.record.turn_id == turn_id && turn.record.status == QueuedTurnStatus::Queued
        })
    }

    pub(crate) fn turn_by_id_mut(&mut self, turn_id: &str) -> Option<&mut StoredTurnControlRecord> {
        self.turns
            .iter_mut()
            .find(|turn| turn.record.turn_id == turn_id)
    }

    pub(crate) fn mark_turn_running(
        &mut self,
        turn_id: &str,
        started_at_ms: TimestampMs,
        execution_timeout_ms: u64,
    ) -> bool {
        let Some(turn) = self.turn_by_id_mut(turn_id) else {
            return false;
        };
        turn.record.status = QueuedTurnStatus::Running;
        turn.record.started_at_ms = Some(started_at_ms);
        turn.record.finished_at_ms = None;
        turn.record.awaiting_approval = false;
        turn.record.failure_message = None;
        turn.record.cancellation_reason = None;
        turn.record.last_progress_at_ms = Some(started_at_ms);
        turn.record.execution_timeout_at_ms =
            Some(started_at_ms.saturating_add(execution_timeout_ms));
        true
    }

    pub(crate) fn record_runtime_progress(
        &mut self,
        turn_id: &str,
        progress_at_ms: TimestampMs,
    ) -> bool {
        let Some(turn) = self.turn_by_id_mut(turn_id) else {
            return false;
        };
        if turn.record.status != QueuedTurnStatus::Running || turn.record.awaiting_approval {
            return false;
        }
        turn.record.last_progress_at_ms = Some(progress_at_ms);
        true
    }

    pub(crate) fn queued_turn_count(&self) -> usize {
        self.turns
            .iter()
            .filter(|turn| turn.record.status == QueuedTurnStatus::Queued)
            .count()
    }

    pub(crate) fn unfinished_turn_count(&self) -> usize {
        self.turns
            .iter()
            .filter(|turn| {
                matches!(
                    turn.record.status,
                    QueuedTurnStatus::Queued | QueuedTurnStatus::Running
                )
            })
            .count()
    }

    pub(crate) fn next_queued_turn(&self) -> Option<StoredTurnControlRecord> {
        self.turns
            .iter()
            .find(|turn| turn.record.status == QueuedTurnStatus::Queued)
            .cloned()
    }

    pub(crate) fn view_for(&self, turn_id: &str) -> Option<SessionTurnControlRecord> {
        let active_turn_id = self.active_turn().map(|turn| turn.record.turn_id.clone());
        let queued_ids = self
            .turns
            .iter()
            .filter(|turn| turn.record.status == QueuedTurnStatus::Queued)
            .map(|turn| turn.record.turn_id.clone())
            .collect::<Vec<_>>();
        self.turns.iter().find_map(|turn| {
            (turn.record.turn_id == turn_id).then(|| {
                decorate_queue_position(&turn.record, active_turn_id.as_deref(), &queued_ids)
            })
        })
    }

    pub(crate) fn inspect_view(&self, session_id: &SessionId) -> InspectSessionTurnsResponse {
        let active_turn_id = self.active_turn().map(|turn| turn.record.turn_id.clone());
        let queued_ids = self
            .turns
            .iter()
            .filter(|turn| turn.record.status == QueuedTurnStatus::Queued)
            .map(|turn| turn.record.turn_id.clone())
            .collect::<Vec<_>>();

        let active_turn = self.active_turn().map(|turn| {
            decorate_queue_position(&turn.record, active_turn_id.as_deref(), &queued_ids)
        });
        let queued_turns = self
            .turns
            .iter()
            .filter(|turn| turn.record.status == QueuedTurnStatus::Queued)
            .map(|turn| {
                decorate_queue_position(&turn.record, active_turn_id.as_deref(), &queued_ids)
            })
            .collect::<Vec<_>>();
        let recent_turns = self
            .turns
            .iter()
            .rev()
            .filter(|turn| {
                matches!(
                    turn.record.status,
                    QueuedTurnStatus::Completed
                        | QueuedTurnStatus::Failed
                        | QueuedTurnStatus::Cancelled
                        | QueuedTurnStatus::TimedOut
                )
            })
            .map(|turn| {
                decorate_queue_position(&turn.record, active_turn_id.as_deref(), &queued_ids)
            })
            .collect::<Vec<_>>();

        InspectSessionTurnsResponse {
            session_id: session_id.clone(),
            active_turn,
            queued_turns,
            recent_turns,
        }
    }
}

pub(crate) fn default_turn_author() -> TurnAuthor {
    TurnAuthor {
        client_name: String::from("unknown"),
        client_version: None,
        display_name: None,
    }
}

pub(crate) fn now_ms() -> TimestampMs {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_millis() as u64
}

fn schema_version() -> u32 {
    TURN_CONTROL_SCHEMA_VERSION
}

fn control_path(
    runtime: &ProbeRuntime,
    session_id: &SessionId,
) -> Result<PathBuf, SessionStoreError> {
    let metadata = runtime.session_store().read_metadata(session_id)?;
    metadata
        .transcript_path
        .parent()
        .map(|path| path.join(TURN_CONTROL_FILE))
        .ok_or_else(|| {
            SessionStoreError::Conflict(format!(
                "session {} transcript path has no parent directory",
                session_id.as_str()
            ))
        })
}

fn write_json_pretty_atomic<T: Serialize>(
    path: &std::path::Path,
    value: &T,
) -> Result<(), SessionStoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("turn-control.json");
    let temp_path = path.with_file_name(format!(
        "{file_name}.tmp-{}-{}",
        std::process::id(),
        now_ms()
    ));
    {
        let mut file = File::create(&temp_path)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        std::io::Write::flush(&mut file)?;
    }
    fs::rename(temp_path, path)?;
    Ok(())
}

fn decorate_queue_position(
    record: &SessionTurnControlRecord,
    active_turn_id: Option<&str>,
    queued_ids: &[String],
) -> SessionTurnControlRecord {
    let mut record = record.clone();
    record.queue_position = match record.status {
        QueuedTurnStatus::Running => Some(0),
        QueuedTurnStatus::Queued => queued_ids
            .iter()
            .position(|turn_id| turn_id == &record.turn_id)
            .map(|index| {
                if active_turn_id.is_some() {
                    index + 1
                } else {
                    index
                }
            }),
        QueuedTurnStatus::Completed
        | QueuedTurnStatus::Failed
        | QueuedTurnStatus::Cancelled
        | QueuedTurnStatus::TimedOut => None,
    };
    record
}
