use probe_protocol::backend::BackendProfile;

use crate::transcript::{ActiveTurn, TranscriptEntry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackgroundTaskRequest {
    AppleFmSetup { profile: BackendProfile },
    TranscriptDemoReply { prompt: String },
}

impl BackgroundTaskRequest {
    #[must_use]
    pub fn apple_fm_setup(profile: BackendProfile) -> Self {
        Self::AppleFmSetup { profile }
    }

    #[must_use]
    pub fn transcript_demo_reply(prompt: impl Into<String>) -> Self {
        Self::TranscriptDemoReply {
            prompt: prompt.into(),
        }
    }

    #[must_use]
    pub fn setup_backend(&self) -> Option<AppleFmBackendSummary> {
        match self {
            Self::AppleFmSetup { profile } => Some(AppleFmBackendSummary::from_profile(profile)),
            Self::TranscriptDemoReply { .. } => None,
        }
    }

    #[must_use]
    pub fn prompt(&self) -> Option<&str> {
        match self {
            Self::AppleFmSetup { .. } => None,
            Self::TranscriptDemoReply { prompt } => Some(prompt.as_str()),
        }
    }

    #[must_use]
    pub fn profile(&self) -> Option<&BackendProfile> {
        match self {
            Self::AppleFmSetup { profile } => Some(profile),
            Self::TranscriptDemoReply { .. } => None,
        }
    }

    #[must_use]
    pub const fn title(&self) -> &'static str {
        match self {
            Self::AppleFmSetup { .. } => "Apple FM setup demo",
            Self::TranscriptDemoReply { .. } => "transcript demo reply",
        }
    }
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
    TranscriptEntryCommitted {
        entry: TranscriptEntry,
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
