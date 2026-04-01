use std::collections::BTreeMap;

use probe_protocol::session::SessionId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceBootMode {
    Fresh,
    RestorePreparedBaseline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceStartPreference {
    FreshOnly,
    PreferPreparedBaseline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceFailureKind {
    LaunchFailed,
    StartupTimedOut,
    ConnectTimedOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceLaunchReason {
    InitialStart,
    RestartAfterFailure,
    RestartAfterTimeout,
    RetryAfterCircuitOpen,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum WorkspaceLifecycleState {
    #[default]
    Idle,
    Starting {
        boot_mode: WorkspaceBootMode,
        started_at_ms: u64,
        consecutive_failures: u32,
    },
    Ready {
        boot_mode: WorkspaceBootMode,
        ready_at_ms: u64,
    },
    Failed {
        boot_mode: WorkspaceBootMode,
        failed_at_ms: u64,
        consecutive_failures: u32,
        failure_kind: WorkspaceFailureKind,
    },
    CircuitOpen {
        boot_mode: WorkspaceBootMode,
        opened_at_ms: u64,
        retry_at_ms: u64,
        consecutive_failures: u32,
        failure_kind: WorkspaceFailureKind,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspaceLifecyclePolicy {
    pub startup_timeout_ms: u64,
    pub circuit_breaker_threshold: u32,
    pub circuit_breaker_cooldown_ms: u64,
}

impl Default for WorkspaceLifecyclePolicy {
    fn default() -> Self {
        Self {
            startup_timeout_ms: 30_000,
            circuit_breaker_threshold: 3,
            circuit_breaker_cooldown_ms: 60_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspaceLifecycleRequest {
    pub now_ms: u64,
    pub prepared_baseline_available: bool,
    pub start_preference: WorkspaceStartPreference,
    pub policy: WorkspaceLifecyclePolicy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceLifecycleDecision {
    ReuseReady,
    WaitForStarting {
        remaining_startup_ms: u64,
    },
    StartWorkspace {
        boot_mode: WorkspaceBootMode,
        reason: WorkspaceLaunchReason,
    },
    DenyUntilCircuitCloses {
        retry_at_ms: u64,
        consecutive_failures: u32,
        failure_kind: WorkspaceFailureKind,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceLifecyclePlan {
    pub decision: WorkspaceLifecycleDecision,
    pub next_state: WorkspaceLifecycleState,
}

#[derive(Clone, Debug, Default)]
pub struct WorkspaceLifecycleRegistry {
    states: BTreeMap<String, WorkspaceLifecycleState>,
}

impl WorkspaceLifecycleRegistry {
    #[must_use]
    pub fn state_for(&self, session_id: &SessionId) -> WorkspaceLifecycleState {
        self.states
            .get(session_id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    pub fn plan(
        &mut self,
        session_id: &SessionId,
        request: WorkspaceLifecycleRequest,
    ) -> WorkspaceLifecyclePlan {
        let current = self.state_for(session_id);
        let plan = plan_workspace_lifecycle(current, request);
        self.states
            .insert(session_id.as_str().to_string(), plan.next_state.clone());
        plan
    }

    pub fn record_ready(&mut self, session_id: &SessionId, now_ms: u64) -> WorkspaceLifecycleState {
        let updated = record_workspace_ready(self.state_for(session_id), now_ms);
        self.states
            .insert(session_id.as_str().to_string(), updated.clone());
        updated
    }

    pub fn record_failure(
        &mut self,
        session_id: &SessionId,
        now_ms: u64,
        failure_kind: WorkspaceFailureKind,
        policy: WorkspaceLifecyclePolicy,
    ) -> WorkspaceLifecycleState {
        let updated =
            record_workspace_failure(self.state_for(session_id), now_ms, failure_kind, policy);
        self.states
            .insert(session_id.as_str().to_string(), updated.clone());
        updated
    }
}

#[must_use]
pub fn plan_workspace_lifecycle(
    state: WorkspaceLifecycleState,
    request: WorkspaceLifecycleRequest,
) -> WorkspaceLifecyclePlan {
    match state {
        WorkspaceLifecycleState::Idle => start_plan(
            choose_boot_mode(
                request.start_preference,
                request.prepared_baseline_available,
            ),
            WorkspaceLaunchReason::InitialStart,
            0,
            request.now_ms,
        ),
        WorkspaceLifecycleState::Ready { .. } => WorkspaceLifecyclePlan {
            decision: WorkspaceLifecycleDecision::ReuseReady,
            next_state: state,
        },
        WorkspaceLifecycleState::Starting {
            boot_mode,
            started_at_ms,
            consecutive_failures,
        } => {
            let elapsed_ms = request.now_ms.saturating_sub(started_at_ms);
            if elapsed_ms < request.policy.startup_timeout_ms {
                WorkspaceLifecyclePlan {
                    decision: WorkspaceLifecycleDecision::WaitForStarting {
                        remaining_startup_ms: request.policy.startup_timeout_ms - elapsed_ms,
                    },
                    next_state: WorkspaceLifecycleState::Starting {
                        boot_mode,
                        started_at_ms,
                        consecutive_failures,
                    },
                }
            } else {
                restart_after_failure(
                    boot_mode,
                    consecutive_failures + 1,
                    WorkspaceFailureKind::StartupTimedOut,
                    request,
                )
            }
        }
        WorkspaceLifecycleState::Failed {
            boot_mode,
            consecutive_failures,
            failure_kind,
            ..
        } => restart_after_failure(boot_mode, consecutive_failures, failure_kind, request),
        WorkspaceLifecycleState::CircuitOpen {
            boot_mode,
            opened_at_ms,
            retry_at_ms,
            consecutive_failures,
            failure_kind,
            ..
        } => {
            if request.now_ms < retry_at_ms {
                WorkspaceLifecyclePlan {
                    decision: WorkspaceLifecycleDecision::DenyUntilCircuitCloses {
                        retry_at_ms,
                        consecutive_failures,
                        failure_kind,
                    },
                    next_state: WorkspaceLifecycleState::CircuitOpen {
                        boot_mode,
                        opened_at_ms,
                        retry_at_ms,
                        consecutive_failures,
                        failure_kind,
                    },
                }
            } else {
                start_plan(
                    boot_mode,
                    WorkspaceLaunchReason::RetryAfterCircuitOpen,
                    consecutive_failures,
                    request.now_ms,
                )
            }
        }
    }
}

#[must_use]
pub fn record_workspace_ready(
    state: WorkspaceLifecycleState,
    now_ms: u64,
) -> WorkspaceLifecycleState {
    let boot_mode = match state {
        WorkspaceLifecycleState::Starting { boot_mode, .. }
        | WorkspaceLifecycleState::Ready { boot_mode, .. }
        | WorkspaceLifecycleState::Failed { boot_mode, .. }
        | WorkspaceLifecycleState::CircuitOpen { boot_mode, .. } => boot_mode,
        WorkspaceLifecycleState::Idle => WorkspaceBootMode::Fresh,
    };
    WorkspaceLifecycleState::Ready {
        boot_mode,
        ready_at_ms: now_ms,
    }
}

#[must_use]
pub fn record_workspace_failure(
    state: WorkspaceLifecycleState,
    now_ms: u64,
    failure_kind: WorkspaceFailureKind,
    policy: WorkspaceLifecyclePolicy,
) -> WorkspaceLifecycleState {
    let (boot_mode, consecutive_failures) = match state {
        WorkspaceLifecycleState::Starting {
            boot_mode,
            consecutive_failures,
            ..
        } => (boot_mode, consecutive_failures + 1),
        WorkspaceLifecycleState::Ready { boot_mode, .. } => (boot_mode, 1),
        WorkspaceLifecycleState::Failed {
            boot_mode,
            consecutive_failures,
            ..
        }
        | WorkspaceLifecycleState::CircuitOpen {
            boot_mode,
            consecutive_failures,
            ..
        } => (boot_mode, consecutive_failures + 1),
        WorkspaceLifecycleState::Idle => (WorkspaceBootMode::Fresh, 1),
    };
    if consecutive_failures >= policy.circuit_breaker_threshold {
        WorkspaceLifecycleState::CircuitOpen {
            boot_mode,
            opened_at_ms: now_ms,
            retry_at_ms: now_ms + policy.circuit_breaker_cooldown_ms,
            consecutive_failures,
            failure_kind,
        }
    } else {
        WorkspaceLifecycleState::Failed {
            boot_mode,
            failed_at_ms: now_ms,
            consecutive_failures,
            failure_kind,
        }
    }
}

fn restart_after_failure(
    previous_boot_mode: WorkspaceBootMode,
    consecutive_failures: u32,
    failure_kind: WorkspaceFailureKind,
    request: WorkspaceLifecycleRequest,
) -> WorkspaceLifecyclePlan {
    if consecutive_failures >= request.policy.circuit_breaker_threshold {
        let retry_at_ms = request.now_ms + request.policy.circuit_breaker_cooldown_ms;
        WorkspaceLifecyclePlan {
            decision: WorkspaceLifecycleDecision::DenyUntilCircuitCloses {
                retry_at_ms,
                consecutive_failures,
                failure_kind,
            },
            next_state: WorkspaceLifecycleState::CircuitOpen {
                boot_mode: previous_boot_mode,
                opened_at_ms: request.now_ms,
                retry_at_ms,
                consecutive_failures,
                failure_kind,
            },
        }
    } else {
        let boot_mode = choose_boot_mode(
            request.start_preference,
            request.prepared_baseline_available,
        );
        let reason = match failure_kind {
            WorkspaceFailureKind::StartupTimedOut | WorkspaceFailureKind::ConnectTimedOut => {
                WorkspaceLaunchReason::RestartAfterTimeout
            }
            WorkspaceFailureKind::LaunchFailed => WorkspaceLaunchReason::RestartAfterFailure,
        };
        start_plan(boot_mode, reason, consecutive_failures, request.now_ms)
    }
}

fn start_plan(
    boot_mode: WorkspaceBootMode,
    reason: WorkspaceLaunchReason,
    consecutive_failures: u32,
    now_ms: u64,
) -> WorkspaceLifecyclePlan {
    WorkspaceLifecyclePlan {
        decision: WorkspaceLifecycleDecision::StartWorkspace { boot_mode, reason },
        next_state: WorkspaceLifecycleState::Starting {
            boot_mode,
            started_at_ms: now_ms,
            consecutive_failures,
        },
    }
}

const fn choose_boot_mode(
    start_preference: WorkspaceStartPreference,
    prepared_baseline_available: bool,
) -> WorkspaceBootMode {
    match (start_preference, prepared_baseline_available) {
        (WorkspaceStartPreference::PreferPreparedBaseline, true) => {
            WorkspaceBootMode::RestorePreparedBaseline
        }
        (WorkspaceStartPreference::PreferPreparedBaseline, false)
        | (WorkspaceStartPreference::FreshOnly, _) => WorkspaceBootMode::Fresh,
    }
}

#[cfg(test)]
mod tests {
    use probe_protocol::session::SessionId;

    use super::{
        WorkspaceBootMode, WorkspaceFailureKind, WorkspaceLaunchReason, WorkspaceLifecycleDecision,
        WorkspaceLifecyclePolicy, WorkspaceLifecycleRegistry, WorkspaceLifecycleRequest,
        WorkspaceLifecycleState, WorkspaceStartPreference, plan_workspace_lifecycle,
        record_workspace_failure, record_workspace_ready,
    };

    #[test]
    fn idle_workspace_starts_fresh_by_default() {
        let plan = plan_workspace_lifecycle(
            WorkspaceLifecycleState::Idle,
            WorkspaceLifecycleRequest {
                now_ms: 10,
                prepared_baseline_available: false,
                start_preference: WorkspaceStartPreference::FreshOnly,
                policy: WorkspaceLifecyclePolicy::default(),
            },
        );
        assert_eq!(
            plan.decision,
            WorkspaceLifecycleDecision::StartWorkspace {
                boot_mode: WorkspaceBootMode::Fresh,
                reason: WorkspaceLaunchReason::InitialStart,
            }
        );
        assert_eq!(
            plan.next_state,
            WorkspaceLifecycleState::Starting {
                boot_mode: WorkspaceBootMode::Fresh,
                started_at_ms: 10,
                consecutive_failures: 0,
            }
        );
    }

    #[test]
    fn prepared_baseline_is_preferred_when_available() {
        let plan = plan_workspace_lifecycle(
            WorkspaceLifecycleState::Idle,
            WorkspaceLifecycleRequest {
                now_ms: 22,
                prepared_baseline_available: true,
                start_preference: WorkspaceStartPreference::PreferPreparedBaseline,
                policy: WorkspaceLifecyclePolicy::default(),
            },
        );
        assert_eq!(
            plan.decision,
            WorkspaceLifecycleDecision::StartWorkspace {
                boot_mode: WorkspaceBootMode::RestorePreparedBaseline,
                reason: WorkspaceLaunchReason::InitialStart,
            }
        );
    }

    #[test]
    fn ready_workspace_is_reused() {
        let plan = plan_workspace_lifecycle(
            WorkspaceLifecycleState::Ready {
                boot_mode: WorkspaceBootMode::Fresh,
                ready_at_ms: 90,
            },
            WorkspaceLifecycleRequest {
                now_ms: 100,
                prepared_baseline_available: false,
                start_preference: WorkspaceStartPreference::FreshOnly,
                policy: WorkspaceLifecyclePolicy::default(),
            },
        );
        assert_eq!(plan.decision, WorkspaceLifecycleDecision::ReuseReady);
    }

    #[test]
    fn starting_workspace_waits_until_timeout() {
        let plan = plan_workspace_lifecycle(
            WorkspaceLifecycleState::Starting {
                boot_mode: WorkspaceBootMode::Fresh,
                started_at_ms: 1_000,
                consecutive_failures: 1,
            },
            WorkspaceLifecycleRequest {
                now_ms: 1_400,
                prepared_baseline_available: false,
                start_preference: WorkspaceStartPreference::FreshOnly,
                policy: WorkspaceLifecyclePolicy {
                    startup_timeout_ms: 1_000,
                    ..WorkspaceLifecyclePolicy::default()
                },
            },
        );
        assert_eq!(
            plan.decision,
            WorkspaceLifecycleDecision::WaitForStarting {
                remaining_startup_ms: 600,
            }
        );
    }

    #[test]
    fn timed_out_start_retries_before_circuit_breaker_threshold() {
        let plan = plan_workspace_lifecycle(
            WorkspaceLifecycleState::Starting {
                boot_mode: WorkspaceBootMode::Fresh,
                started_at_ms: 0,
                consecutive_failures: 1,
            },
            WorkspaceLifecycleRequest {
                now_ms: 1_500,
                prepared_baseline_available: false,
                start_preference: WorkspaceStartPreference::FreshOnly,
                policy: WorkspaceLifecyclePolicy {
                    startup_timeout_ms: 1_000,
                    circuit_breaker_threshold: 3,
                    circuit_breaker_cooldown_ms: 30_000,
                },
            },
        );
        assert_eq!(
            plan.decision,
            WorkspaceLifecycleDecision::StartWorkspace {
                boot_mode: WorkspaceBootMode::Fresh,
                reason: WorkspaceLaunchReason::RestartAfterTimeout,
            }
        );
        assert_eq!(
            plan.next_state,
            WorkspaceLifecycleState::Starting {
                boot_mode: WorkspaceBootMode::Fresh,
                started_at_ms: 1_500,
                consecutive_failures: 2,
            }
        );
    }

    #[test]
    fn repeated_failures_open_a_circuit_breaker() {
        let policy = WorkspaceLifecyclePolicy {
            startup_timeout_ms: 1_000,
            circuit_breaker_threshold: 3,
            circuit_breaker_cooldown_ms: 5_000,
        };
        let plan = plan_workspace_lifecycle(
            WorkspaceLifecycleState::Starting {
                boot_mode: WorkspaceBootMode::Fresh,
                started_at_ms: 0,
                consecutive_failures: 2,
            },
            WorkspaceLifecycleRequest {
                now_ms: 2_000,
                prepared_baseline_available: false,
                start_preference: WorkspaceStartPreference::FreshOnly,
                policy,
            },
        );
        assert_eq!(
            plan.decision,
            WorkspaceLifecycleDecision::DenyUntilCircuitCloses {
                retry_at_ms: 7_000,
                consecutive_failures: 3,
                failure_kind: WorkspaceFailureKind::StartupTimedOut,
            }
        );
    }

    #[test]
    fn record_failure_opens_circuit_at_threshold() {
        let updated = record_workspace_failure(
            WorkspaceLifecycleState::Failed {
                boot_mode: WorkspaceBootMode::Fresh,
                failed_at_ms: 10,
                consecutive_failures: 2,
                failure_kind: WorkspaceFailureKind::LaunchFailed,
            },
            20,
            WorkspaceFailureKind::ConnectTimedOut,
            WorkspaceLifecyclePolicy {
                startup_timeout_ms: 1_000,
                circuit_breaker_threshold: 3,
                circuit_breaker_cooldown_ms: 4_000,
            },
        );
        assert_eq!(
            updated,
            WorkspaceLifecycleState::CircuitOpen {
                boot_mode: WorkspaceBootMode::Fresh,
                opened_at_ms: 20,
                retry_at_ms: 4_020,
                consecutive_failures: 3,
                failure_kind: WorkspaceFailureKind::ConnectTimedOut,
            }
        );
    }

    #[test]
    fn record_ready_converts_starting_state_into_ready() {
        let updated = record_workspace_ready(
            WorkspaceLifecycleState::Starting {
                boot_mode: WorkspaceBootMode::RestorePreparedBaseline,
                started_at_ms: 10,
                consecutive_failures: 1,
            },
            25,
        );
        assert_eq!(
            updated,
            WorkspaceLifecycleState::Ready {
                boot_mode: WorkspaceBootMode::RestorePreparedBaseline,
                ready_at_ms: 25,
            }
        );
    }

    #[test]
    fn registry_persists_per_session_lifecycle_state() {
        let mut registry = WorkspaceLifecycleRegistry::default();
        let session = SessionId::new("sess_registry");
        let request = WorkspaceLifecycleRequest {
            now_ms: 100,
            prepared_baseline_available: false,
            start_preference: WorkspaceStartPreference::FreshOnly,
            policy: WorkspaceLifecyclePolicy::default(),
        };

        let plan = registry.plan(&session, request);
        assert!(matches!(
            plan.decision,
            WorkspaceLifecycleDecision::StartWorkspace { .. }
        ));
        assert!(matches!(
            registry.state_for(&session),
            WorkspaceLifecycleState::Starting { .. }
        ));

        let ready = registry.record_ready(&session, 150);
        assert!(matches!(ready, WorkspaceLifecycleState::Ready { .. }));
    }
}
