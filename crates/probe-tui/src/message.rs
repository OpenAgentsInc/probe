use std::path::PathBuf;

use probe_core::tools::ToolLoopConfig;
use probe_protocol::backend::BackendProfile;
use probe_protocol::session::SessionHarnessProfile;

use crate::transcript::{ActiveTurn, TranscriptEntry};

#[derive(Debug, Clone)]
pub enum BackgroundTaskRequest {
    AppleFmSetup { profile: BackendProfile },
    ProbeRuntimeTurn {
        prompt: String,
        config: ProbeRuntimeTurnConfig,
    },
}

impl BackgroundTaskRequest {
    #[must_use]
    pub fn apple_fm_setup(profile: BackendProfile) -> Self {
        Self::AppleFmSetup { profile }
    }

    #[must_use]
    pub fn probe_runtime_turn(prompt: impl Into<String>, config: ProbeRuntimeTurnConfig) -> Self {
        Self::ProbeRuntimeTurn {
            prompt: prompt.into(),
            config,
        }
    }

    #[must_use]
    pub fn setup_backend(&self) -> Option<AppleFmBackendSummary> {
        match self {
            Self::AppleFmSetup { profile } => Some(AppleFmBackendSummary::from_profile(profile)),
            Self::ProbeRuntimeTurn { .. } => None,
        }
    }

    #[must_use]
    pub const fn title(&self) -> &'static str {
        match self {
            Self::AppleFmSetup { .. } => "Apple FM setup demo",
            Self::ProbeRuntimeTurn { .. } => "Probe runtime turn",
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
