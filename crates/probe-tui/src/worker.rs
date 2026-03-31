use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use probe_core::provider::{
    PlainTextMessage, PlainTextProviderResponse, ProviderError, ProviderUsageTruth,
    complete_plain_text,
};
use probe_provider_apple_fm::{AppleFmProviderClient, AppleFmProviderConfig, AppleFmProviderError};
use psionic_apple_fm::AppleFmSystemLanguageModelAvailability;

use crate::message::{
    AppMessage, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
    AppleFmFailureSummary, AppleFmUsageSummary, BackgroundTaskRequest,
};

const APPLE_FM_SETUP_SYSTEM_PROMPT: &str = "You are Probe's Apple Foundation Models setup check. Keep responses short and follow explicit formatting requests exactly.";
const APPLE_FM_SETUP_PROMPTS: [(&str, &str); 3] = [
    ("Sanity Check", "Reply with exactly READY."),
    (
        "Runtime Boundary",
        "In one sentence, summarize what Probe owns.",
    ),
    (
        "Next Step",
        "In one short sentence, say what this terminal UI should prove next.",
    ),
];

enum WorkerCommand {
    Run(BackgroundTaskRequest),
    Shutdown,
}

#[derive(Debug)]
pub struct BackgroundWorker {
    command_tx: Sender<WorkerCommand>,
    message_rx: Receiver<AppMessage>,
    join_handle: Option<JoinHandle<()>>,
}

impl BackgroundWorker {
    #[must_use]
    pub fn new() -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (message_tx, message_rx) = mpsc::channel();
        let join_handle = thread::Builder::new()
            .name(String::from("probe-tui-worker"))
            .spawn(move || worker_loop(command_rx, message_tx))
            .expect("probe tui worker thread should spawn");
        Self {
            command_tx,
            message_rx,
            join_handle: Some(join_handle),
        }
    }

    pub fn submit(&self, request: BackgroundTaskRequest) -> Result<(), String> {
        self.command_tx
            .send(WorkerCommand::Run(request))
            .map_err(|_| String::from("background worker is unavailable"))
    }

    pub fn try_recv(&self) -> Result<Option<AppMessage>, String> {
        match self.message_rx.try_recv() {
            Ok(message) => Ok(Some(message)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                Err(String::from("background worker disconnected unexpectedly"))
            }
        }
    }
}

impl Default for BackgroundWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BackgroundWorker {
    fn drop(&mut self) {
        let _ = self.command_tx.send(WorkerCommand::Shutdown);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

fn worker_loop(command_rx: Receiver<WorkerCommand>, message_tx: Sender<AppMessage>) {
    while let Ok(command) = command_rx.recv() {
        match command {
            WorkerCommand::Run(request) => run_request(request, &message_tx),
            WorkerCommand::Shutdown => break,
        }
    }
}

fn run_request(request: BackgroundTaskRequest, message_tx: &Sender<AppMessage>) {
    match request {
        BackgroundTaskRequest::AppleFmSetup { profile } => run_apple_fm_setup(profile, message_tx),
    }
}

fn run_apple_fm_setup(
    profile: probe_protocol::backend::BackendProfile,
    message_tx: &Sender<AppMessage>,
) {
    let backend = AppleFmBackendSummary::from_profile(&profile);
    if message_tx
        .send(AppMessage::AppleFmSetupStarted {
            backend: backend.clone(),
        })
        .is_err()
    {
        return;
    }

    let provider = match AppleFmProviderClient::new(AppleFmProviderConfig::from_backend_profile(
        &profile,
    )) {
        Ok(provider) => provider,
        Err(error) => {
            let _ = message_tx.send(AppMessage::AppleFmSetupFailed {
                backend,
                failure: failure_from_availability_error("provider_init", &error),
            });
            return;
        }
    };

    let availability = match provider.system_model_availability() {
        Ok(availability) => availability,
        Err(error) => {
            let _ = message_tx.send(AppMessage::AppleFmSetupFailed {
                backend,
                failure: failure_from_availability_error("availability_check", &error),
            });
            return;
        }
    };
    let availability_summary = availability_summary_from_bridge(&availability);
    if !availability.is_ready() {
        let _ = message_tx.send(AppMessage::AppleFmAvailabilityUnavailable {
            backend,
            availability: availability_summary,
        });
        return;
    }
    if message_tx
        .send(AppMessage::AppleFmAvailabilityReady {
            backend: backend.clone(),
            availability: availability_summary,
        })
        .is_err()
    {
        return;
    }

    let total_calls = APPLE_FM_SETUP_PROMPTS.len();
    for (index, (title, prompt)) in APPLE_FM_SETUP_PROMPTS.iter().enumerate() {
        let call_index = index + 1;
        if message_tx
            .send(AppMessage::AppleFmCallStarted {
                backend: backend.clone(),
                index: call_index,
                total_calls,
                title: (*title).to_string(),
                prompt: (*prompt).to_string(),
            })
            .is_err()
        {
            return;
        }

        let response = match complete_plain_text(
            &profile,
            vec![
                PlainTextMessage::system(APPLE_FM_SETUP_SYSTEM_PROMPT),
                PlainTextMessage::user(*prompt),
            ],
        ) {
            Ok(response) => response,
            Err(error) => {
                let _ = message_tx.send(AppMessage::AppleFmSetupFailed {
                    backend,
                    failure: failure_from_provider_error(
                        format!("call_{call_index}_{}", title.to_lowercase().replace(' ', "_")),
                        &error,
                    ),
                });
                return;
            }
        };

        if message_tx
            .send(AppMessage::AppleFmCallCompleted {
                backend: backend.clone(),
                index: call_index,
                total_calls,
                call: call_record_from_response(title, prompt, response),
            })
            .is_err()
        {
            return;
        }
    }

    let _ = message_tx.send(AppMessage::AppleFmSetupCompleted {
        backend,
        total_calls,
    });
}

fn availability_summary_from_bridge(
    availability: &AppleFmSystemLanguageModelAvailability,
) -> AppleFmAvailabilitySummary {
    AppleFmAvailabilitySummary {
        ready: availability.is_ready(),
        unavailable_reason: availability
            .unavailable_reason
            .map(|reason| reason.label().to_string()),
        availability_message: availability.availability_message.clone(),
        version: availability.version.clone(),
        platform: availability.platform.clone(),
        apple_silicon_required: availability.apple_silicon_required,
        apple_intelligence_required: availability.apple_intelligence_required,
    }
}

fn call_record_from_response(
    title: &str,
    prompt: &str,
    response: PlainTextProviderResponse,
) -> AppleFmCallRecord {
    AppleFmCallRecord {
        title: title.to_string(),
        prompt: prompt.to_string(),
        response_text: response
            .assistant_text
            .unwrap_or_else(|| String::from("[no text response]")),
        response_id: response.response_id,
        response_model: response.response_model,
        usage: usage_summary_from_response(response.usage),
    }
}

fn usage_summary_from_response(
    usage: Option<probe_core::provider::ProviderUsage>,
) -> AppleFmUsageSummary {
    let Some(usage) = usage else {
        return AppleFmUsageSummary::default();
    };
    AppleFmUsageSummary {
        prompt_tokens: usage.prompt_tokens_detail.as_ref().map(|detail| detail.value),
        prompt_truth: usage.prompt_tokens_detail.as_ref().map(usage_truth_label),
        completion_tokens: usage
            .completion_tokens_detail
            .as_ref()
            .map(|detail| detail.value),
        completion_truth: usage
            .completion_tokens_detail
            .as_ref()
            .map(usage_truth_label),
        total_tokens: usage.total_tokens_detail.as_ref().map(|detail| detail.value),
        total_truth: usage.total_tokens_detail.as_ref().map(usage_truth_label),
    }
}

fn usage_truth_label(detail: &probe_core::provider::ProviderUsageMeasurement) -> String {
    match detail.truth {
        ProviderUsageTruth::Exact => String::from("exact"),
        ProviderUsageTruth::Estimated => String::from("estimated"),
    }
}

fn failure_from_availability_error(
    stage: impl Into<String>,
    error: &AppleFmProviderError,
) -> AppleFmFailureSummary {
    let typed = error.foundation_models_error();
    AppleFmFailureSummary {
        stage: stage.into(),
        detail: error.to_string(),
        reason_code: typed.map(|typed| typed.kind.label().to_string()),
        retryable: typed.map(|typed| typed.is_retryable()),
        failure_reason: typed.and_then(|typed| typed.failure_reason.clone()),
        recovery_suggestion: typed.and_then(|typed| typed.recovery_suggestion.clone()),
    }
}

fn failure_from_provider_error(stage: impl Into<String>, error: &ProviderError) -> AppleFmFailureSummary {
    let receipt = error.backend_turn_receipt();
    let failure = receipt.and_then(|receipt| receipt.failure);
    AppleFmFailureSummary {
        stage: stage.into(),
        detail: error.to_string(),
        reason_code: failure.as_ref().and_then(|failure| failure.code.clone()),
        retryable: failure.as_ref().and_then(|failure| failure.retryable),
        failure_reason: failure.as_ref().and_then(|failure| failure.failure_reason.clone()),
        recovery_suggestion: failure
            .as_ref()
            .and_then(|failure| failure.recovery_suggestion.clone()),
    }
}
