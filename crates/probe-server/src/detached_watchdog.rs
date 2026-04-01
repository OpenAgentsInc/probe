use probe_protocol::runtime::{QueuedTurnStatus, SessionTurnControlRecord};
use probe_protocol::session::TimestampMs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DetachedTurnWatchdogPolicy {
    pub poll_interval_ms: u64,
    pub stall_timeout_ms: u64,
    pub execution_timeout_ms: u64,
}

impl Default for DetachedTurnWatchdogPolicy {
    fn default() -> Self {
        Self {
            poll_interval_ms: 500,
            stall_timeout_ms: 30_000,
            execution_timeout_ms: 300_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetachedTurnWatchdogTrigger {
    ProgressStalled {
        last_progress_at_ms: TimestampMs,
        stall_timeout_ms: u64,
    },
    ExecutionTimedOut {
        timeout_at_ms: TimestampMs,
        execution_timeout_ms: u64,
    },
}

pub fn evaluate_detached_turn_watchdog(
    record: &SessionTurnControlRecord,
    now_ms: TimestampMs,
    policy: DetachedTurnWatchdogPolicy,
) -> Option<DetachedTurnWatchdogTrigger> {
    if record.status != QueuedTurnStatus::Running || record.awaiting_approval {
        return None;
    }

    if let Some(timeout_at_ms) = record.execution_timeout_at_ms {
        if now_ms >= timeout_at_ms {
            return Some(DetachedTurnWatchdogTrigger::ExecutionTimedOut {
                timeout_at_ms,
                execution_timeout_ms: policy.execution_timeout_ms,
            });
        }
    }

    let last_progress_at_ms = record
        .last_progress_at_ms
        .or(record.started_at_ms)
        .unwrap_or(record.requested_at_ms);
    if now_ms >= last_progress_at_ms.saturating_add(policy.stall_timeout_ms) {
        return Some(DetachedTurnWatchdogTrigger::ProgressStalled {
            last_progress_at_ms,
            stall_timeout_ms: policy.stall_timeout_ms,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use probe_protocol::runtime::{
        QueuedTurnStatus, SessionTurnControlRecord, TurnAuthor, TurnSubmissionKind,
    };
    use probe_protocol::session::SessionId;

    use super::{
        DetachedTurnWatchdogPolicy, DetachedTurnWatchdogTrigger, evaluate_detached_turn_watchdog,
    };

    fn sample_record() -> SessionTurnControlRecord {
        SessionTurnControlRecord {
            turn_id: String::from("turn-0"),
            session_id: SessionId::new("sess-watchdog"),
            submission_kind: TurnSubmissionKind::Continue,
            status: QueuedTurnStatus::Running,
            prompt: String::from("run a long task"),
            author: TurnAuthor {
                client_name: String::from("probe-test"),
                client_version: None,
                display_name: None,
            },
            requested_at_ms: 100,
            started_at_ms: Some(120),
            finished_at_ms: None,
            queue_position: Some(0),
            awaiting_approval: false,
            failure_message: None,
            cancellation_reason: None,
            last_progress_at_ms: Some(150),
            execution_timeout_at_ms: Some(400),
        }
    }

    #[test]
    fn approval_paused_turns_are_exempt_from_watchdog() {
        let mut record = sample_record();
        record.awaiting_approval = true;
        assert_eq!(
            evaluate_detached_turn_watchdog(&record, 10_000, DetachedTurnWatchdogPolicy::default(),),
            None
        );
    }

    #[test]
    fn progress_stall_trigger_fires_before_total_execution_timeout() {
        let record = sample_record();
        let trigger = evaluate_detached_turn_watchdog(
            &record,
            200,
            DetachedTurnWatchdogPolicy {
                poll_interval_ms: 25,
                stall_timeout_ms: 40,
                execution_timeout_ms: 1_000,
            },
        );
        assert_eq!(
            trigger,
            Some(DetachedTurnWatchdogTrigger::ProgressStalled {
                last_progress_at_ms: 150,
                stall_timeout_ms: 40,
            })
        );
    }

    #[test]
    fn total_execution_timeout_fires_when_progress_keeps_moving() {
        let mut record = sample_record();
        record.last_progress_at_ms = Some(390);
        let trigger = evaluate_detached_turn_watchdog(
            &record,
            401,
            DetachedTurnWatchdogPolicy {
                poll_interval_ms: 25,
                stall_timeout_ms: 500,
                execution_timeout_ms: 280,
            },
        );
        assert_eq!(
            trigger,
            Some(DetachedTurnWatchdogTrigger::ExecutionTimedOut {
                timeout_at_ms: 400,
                execution_timeout_ms: 280,
            })
        );
    }

    #[test]
    fn completed_turns_are_not_reconsidered() {
        let mut record = sample_record();
        record.status = QueuedTurnStatus::Completed;
        assert_eq!(
            evaluate_detached_turn_watchdog(&record, 10_000, DetachedTurnWatchdogPolicy::default(),),
            None
        );
    }
}
