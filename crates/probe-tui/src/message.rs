use std::path::PathBuf;

use probe_core::runtime::{RuntimeEvent, StreamedToolCallDelta};
use probe_core::tools::ToolLoopConfig;
use probe_decisions::{GithubIssueSelectionDecision, SelectedGithubIssue};
use probe_protocol::backend::{BackendKind, BackendProfile};
use probe_protocol::session::{PendingToolApproval, SessionHarnessProfile, ToolApprovalResolution};

use crate::transcript::{ActiveTurn, TranscriptEntry};

#[derive(Debug, Clone)]
pub enum BackgroundTaskRequest {
    AppleFmSetup {
        profile: BackendProfile,
    },
    AttachProbeRuntimeSession {
        session_id: String,
        config: ProbeRuntimeTurnConfig,
    },
    ProbeRuntimeTurn {
        prompt: String,
        config: ProbeRuntimeTurnConfig,
    },
    ClearProbeRuntimeContext {
        config: ProbeRuntimeTurnConfig,
    },
    SelectGithubIssue {
        priority: String,
        cwd: PathBuf,
    },
    ResolvePendingToolApproval {
        session_id: String,
        call_id: String,
        resolution: ToolApprovalResolution,
        config: ProbeRuntimeTurnConfig,
    },
}

impl BackgroundTaskRequest {
    #[must_use]
    pub fn apple_fm_setup(profile: BackendProfile) -> Self {
        Self::AppleFmSetup { profile }
    }

    #[must_use]
    pub fn attach_probe_runtime_session(
        session_id: impl Into<String>,
        config: ProbeRuntimeTurnConfig,
    ) -> Self {
        Self::AttachProbeRuntimeSession {
            session_id: session_id.into(),
            config,
        }
    }

    #[must_use]
    pub fn probe_runtime_turn(prompt: impl Into<String>, config: ProbeRuntimeTurnConfig) -> Self {
        Self::ProbeRuntimeTurn {
            prompt: prompt.into(),
            config,
        }
    }

    #[must_use]
    pub fn clear_probe_runtime_context(config: ProbeRuntimeTurnConfig) -> Self {
        Self::ClearProbeRuntimeContext { config }
    }

    #[must_use]
    pub fn select_github_issue(priority: impl Into<String>, cwd: PathBuf) -> Self {
        Self::SelectGithubIssue {
            priority: priority.into(),
            cwd,
        }
    }

    #[must_use]
    pub fn resolve_pending_tool_approval(
        session_id: impl Into<String>,
        call_id: impl Into<String>,
        resolution: ToolApprovalResolution,
        config: ProbeRuntimeTurnConfig,
    ) -> Self {
        Self::ResolvePendingToolApproval {
            session_id: session_id.into(),
            call_id: call_id.into(),
            resolution,
            config,
        }
    }

    #[must_use]
    pub fn setup_backend(&self) -> Option<AppleFmBackendSummary> {
        match self {
            Self::AppleFmSetup { profile } => Some(AppleFmBackendSummary::from_profile(profile)),
            Self::AttachProbeRuntimeSession { .. }
            | Self::ProbeRuntimeTurn { .. }
            | Self::ClearProbeRuntimeContext { .. }
            | Self::SelectGithubIssue { .. }
            | Self::ResolvePendingToolApproval { .. } => None,
        }
    }

    #[must_use]
    pub const fn title(&self) -> &'static str {
        match self {
            Self::AppleFmSetup { .. } => "Apple FM setup check",
            Self::AttachProbeRuntimeSession { .. } => "Probe runtime attach",
            Self::ProbeRuntimeTurn { .. } => "Probe runtime turn",
            Self::ClearProbeRuntimeContext { .. } => "Probe runtime context reset",
            Self::SelectGithubIssue { .. } => "GitHub issue selection",
            Self::ResolvePendingToolApproval { .. } => "pending approval decision",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProbeRuntimeTurnConfig {
    pub probe_home: Option<PathBuf>,
    pub cwd: PathBuf,
    pub profile: BackendProfile,
    pub system_prompt: Option<String>,
    pub harness_profile: Option<SessionHarnessProfile>,
    pub tool_loop: Option<ToolLoopConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppleFmBackendSummary {
    pub profile_name: String,
    pub base_url: String,
    pub model_id: String,
}

impl AppleFmBackendSummary {
    #[must_use]
    pub fn from_profile(profile: &BackendProfile) -> Self {
        Self {
            profile_name: profile.name.clone(),
            base_url: profile.base_url.clone(),
            model_id: profile.model.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppleFmAvailabilitySummary {
    pub ready: bool,
    pub unavailable_reason: Option<String>,
    pub availability_message: Option<String>,
    pub version: Option<String>,
    pub platform: Option<String>,
    pub apple_silicon_required: Option<bool>,
    pub apple_intelligence_required: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppleFmUsageSummary {
    pub prompt_tokens: Option<u64>,
    pub prompt_truth: Option<String>,
    pub completion_tokens: Option<u64>,
    pub completion_truth: Option<String>,
    pub total_tokens: Option<u64>,
    pub total_truth: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppleFmCallRecord {
    pub title: String,
    pub prompt: String,
    pub response_text: String,
    pub response_id: String,
    pub response_model: String,
    pub usage: AppleFmUsageSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppleFmFailureSummary {
    pub stage: String,
    pub detail: String,
    pub reason_code: Option<String>,
    pub retryable: Option<bool>,
    pub failure_reason: Option<String>,
    pub recovery_suggestion: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMessage {
    AssistantStreamStarted {
        session_id: String,
        round_trip: usize,
        response_id: String,
        response_model: String,
    },
    AssistantFirstChunkObserved {
        session_id: String,
        round_trip: usize,
        milliseconds: u64,
    },
    AssistantDeltaAppended {
        session_id: String,
        round_trip: usize,
        delta: String,
    },
    AssistantSnapshotUpdated {
        session_id: String,
        round_trip: usize,
        snapshot: String,
    },
    AssistantToolCallDeltaUpdated {
        session_id: String,
        round_trip: usize,
        deltas: Vec<StreamedToolCallDelta>,
    },
    AssistantStreamFinished {
        session_id: String,
        round_trip: usize,
        response_id: String,
        response_model: String,
        finish_reason: Option<String>,
    },
    AssistantStreamFailed {
        session_id: String,
        round_trip: usize,
        backend_kind: BackendKind,
        error: String,
    },
    TranscriptActiveTurnSet {
        turn: ActiveTurn,
    },
    TranscriptEntriesCommitted {
        entries: Vec<TranscriptEntry>,
    },
    TranscriptEntryCommitted {
        entry: TranscriptEntry,
    },
    ProbeRuntimeSessionReady {
        session_id: String,
        profile_name: String,
        model_id: String,
        cwd: String,
    },
    ProbeRuntimeEvent {
        event: RuntimeEvent,
    },
    GithubIssueSelectionResolved {
        priority: String,
        decision: GithubIssueSelectionDecision,
    },
    GithubIssueSelectionFailed {
        priority: String,
        error: String,
        selected_issue: Option<SelectedGithubIssue>,
    },
    PendingToolApprovalsUpdated {
        session_id: String,
        approvals: Vec<PendingToolApproval>,
    },
    AppleFmSetupStarted {
        backend: AppleFmBackendSummary,
    },
    AppleFmAvailabilityReady {
        backend: AppleFmBackendSummary,
        availability: AppleFmAvailabilitySummary,
    },
    AppleFmAvailabilityUnavailable {
        backend: AppleFmBackendSummary,
        availability: AppleFmAvailabilitySummary,
    },
    AppleFmCallStarted {
        backend: AppleFmBackendSummary,
        index: usize,
        total_calls: usize,
        title: String,
        prompt: String,
    },
    AppleFmCallCompleted {
        backend: AppleFmBackendSummary,
        index: usize,
        total_calls: usize,
        call: AppleFmCallRecord,
    },
    AppleFmSetupCompleted {
        backend: AppleFmBackendSummary,
        total_calls: usize,
    },
    AppleFmSetupFailed {
        backend: AppleFmBackendSummary,
        failure: AppleFmFailureSummary,
    },
}
