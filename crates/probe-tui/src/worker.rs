use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use probe_core::provider::{
    complete_plain_text, PlainTextMessage, PlainTextProviderResponse, ProviderError,
    ProviderUsageTruth,
};
use probe_core::runtime::{
    PlainTextExecRequest, PlainTextResumeRequest, ProbeRuntime, RuntimeError, RuntimeEventSink,
};
use probe_provider_apple_fm::{AppleFmProviderClient, AppleFmProviderConfig, AppleFmProviderError};
use probe_protocol::session::{
    SessionId, SessionMetadata, ToolApprovalState, ToolExecutionRecord, ToolPolicyDecision,
    ToolRiskClass, TranscriptEvent, TranscriptItem, TranscriptItemKind,
};
use psionic_apple_fm::AppleFmSystemLanguageModelAvailability;
use serde_json::Value;

use crate::message::{
    AppMessage, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
    AppleFmFailureSummary, AppleFmUsageSummary, BackgroundTaskRequest, ProbeRuntimeTurnConfig,
};
use crate::transcript::{TranscriptEntry, TranscriptRole};

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

#[derive(Debug, Default)]
struct WorkerState {
    runtime_session: Option<ProbeRuntimeSessionState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeRuntimeSessionState {
    session_id: SessionId,
    rendered_turns: usize,
    probe_home: Option<std::path::PathBuf>,
    cwd: std::path::PathBuf,
    profile_name: String,
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
    let mut state = WorkerState::default();
    while let Ok(command) = command_rx.recv() {
        match command {
            WorkerCommand::Run(request) => run_request(request, &message_tx, &mut state),
            WorkerCommand::Shutdown => break,
        }
    }
}

fn run_request(
    request: BackgroundTaskRequest,
    message_tx: &Sender<AppMessage>,
    state: &mut WorkerState,
) {
    match request {
        BackgroundTaskRequest::AppleFmSetup { profile } => run_apple_fm_setup(profile, message_tx),
        BackgroundTaskRequest::ProbeRuntimeTurn { prompt, config } => {
            run_probe_runtime_turn(prompt, config, message_tx, state)
        }
    }
}

fn run_probe_runtime_turn(
    prompt: String,
    config: ProbeRuntimeTurnConfig,
    message_tx: &Sender<AppMessage>,
    state: &mut WorkerState,
) {
    if state
        .runtime_session
        .as_ref()
        .is_some_and(|session| !session.matches_config(&config))
    {
        state.runtime_session = None;
    }

    let runtime = match config.probe_home.clone() {
        Some(probe_home) => ProbeRuntime::new(probe_home),
        None => match ProbeRuntime::detect() {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = message_tx.send(AppMessage::TranscriptEntryCommitted {
                    entry: TranscriptEntry::new(
                        TranscriptRole::Status,
                        "Runtime Error",
                        vec![error.to_string()],
                    ),
                });
                return;
            }
        },
    };

    let result = if let Some(session) = state.runtime_session.as_ref() {
        let event_tx = message_tx.clone();
        let event_sink: Arc<dyn RuntimeEventSink> = Arc::new(move |event| {
            let _ = event_tx.send(AppMessage::ProbeRuntimeEvent { event });
        });
        runtime.continue_plain_text_session_with_events(PlainTextResumeRequest {
            session_id: session.session_id.clone(),
            profile: config.profile.clone(),
            prompt,
            tool_loop: config.tool_loop.clone(),
        }, event_sink)
    } else {
        let event_tx = message_tx.clone();
        let event_sink: Arc<dyn RuntimeEventSink> = Arc::new(move |event| {
            let _ = event_tx.send(AppMessage::ProbeRuntimeEvent { event });
        });
        runtime.exec_plain_text_with_events(PlainTextExecRequest {
            profile: config.profile.clone(),
            prompt,
            title: Some(String::from("Probe TUI Session")),
            cwd: config.cwd.clone(),
            system_prompt: config.system_prompt.clone(),
            harness_profile: config.harness_profile.clone(),
            tool_loop: config.tool_loop.clone(),
        }, event_sink)
    };

    match result {
        Ok(outcome) => {
            let previous_turns = state
                .runtime_session
                .as_ref()
                .map_or(0, |session| session.rendered_turns);
            if emit_session_ready(message_tx, &outcome.session, &config).is_err() {
                return;
            }
            if emit_transcript_delta(
                message_tx,
                runtime.session_store().read_transcript(&outcome.session.id),
                previous_turns,
            )
            .is_err()
            {
                return;
            }
            state.runtime_session = Some(ProbeRuntimeSessionState::from_metadata(
                &outcome.session,
                &config,
                runtime
                    .session_store()
                    .read_transcript(&outcome.session.id)
                    .map(|events| events.len())
                    .unwrap_or(previous_turns),
            ));
        }
        Err(error) => {
            let Some(session_id) = runtime_error_session_id(&error)
                .or_else(|| state.runtime_session.as_ref().map(|session| session.session_id.clone()))
            else {
                let _ = message_tx.send(AppMessage::TranscriptEntryCommitted {
                    entry: TranscriptEntry::new(
                        TranscriptRole::Status,
                        "Runtime Error",
                        vec![error.to_string()],
                    ),
                });
                return;
            };

            let metadata = runtime.session_store().read_metadata(&session_id).ok();
            if let Some(metadata) = metadata.as_ref()
                && emit_session_ready(message_tx, metadata, &config).is_err()
            {
                return;
            }
            let previous_turns = state
                .runtime_session
                .as_ref()
                .filter(|session| session.session_id == session_id)
                .map_or(0, |session| session.rendered_turns);
            let transcript = runtime.session_store().read_transcript(&session_id);
            let rendered_turns = transcript
                .as_ref()
                .map(|events| events.len())
                .unwrap_or(previous_turns);
            let had_no_new_turns = transcript
                .as_ref()
                .map(|events| events.len() == previous_turns)
                .unwrap_or(true);
            if emit_transcript_delta(message_tx, transcript, previous_turns).is_err() {
                return;
            }
            if let Some(metadata) = metadata {
                state.runtime_session = Some(ProbeRuntimeSessionState::from_metadata(
                    &metadata,
                    &config,
                    rendered_turns,
                ));
            }
            if had_no_new_turns {
                let _ = message_tx.send(AppMessage::TranscriptEntryCommitted {
                    entry: TranscriptEntry::new(
                        TranscriptRole::Status,
                        "Runtime Error",
                        vec![error.to_string()],
                    ),
                });
            }
        }
    }
}

impl ProbeRuntimeSessionState {
    fn matches_config(&self, config: &ProbeRuntimeTurnConfig) -> bool {
        self.probe_home == config.probe_home
            && self.cwd == config.cwd
            && self.profile_name == config.profile.name
    }

    fn from_metadata(
        metadata: &SessionMetadata,
        config: &ProbeRuntimeTurnConfig,
        rendered_turns: usize,
    ) -> Self {
        Self {
            session_id: metadata.id.clone(),
            rendered_turns,
            probe_home: config.probe_home.clone(),
            cwd: config.cwd.clone(),
            profile_name: config.profile.name.clone(),
        }
    }
}

fn emit_session_ready(
    message_tx: &Sender<AppMessage>,
    metadata: &SessionMetadata,
    config: &ProbeRuntimeTurnConfig,
) -> Result<(), ()> {
    message_tx
        .send(AppMessage::ProbeRuntimeSessionReady {
            session_id: metadata.id.as_str().to_string(),
            profile_name: config.profile.name.clone(),
            model_id: config.profile.model.clone(),
            cwd: metadata.cwd.display().to_string(),
        })
        .map_err(|_| ())
}

fn emit_transcript_delta(
    message_tx: &Sender<AppMessage>,
    transcript: Result<Vec<TranscriptEvent>, probe_core::session_store::SessionStoreError>,
    previous_turns: usize,
) -> Result<(), ()> {
    let transcript = transcript.map_err(|_| ())?;
    let entries = transcript
        .iter()
        .skip(previous_turns)
        .flat_map(transcript_entries_from_event)
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Ok(());
    }
    message_tx
        .send(AppMessage::TranscriptEntriesCommitted { entries })
        .map_err(|_| ())
}

fn transcript_entries_from_event(event: &TranscriptEvent) -> Vec<TranscriptEntry> {
    event.turn
        .items
        .iter()
        .filter_map(|item| transcript_entry_from_item(event.turn.index, item))
        .collect()
}

fn transcript_entry_from_item(turn_index: u64, item: &TranscriptItem) -> Option<TranscriptEntry> {
    match item.kind {
        TranscriptItemKind::UserMessage => None,
        TranscriptItemKind::AssistantMessage => Some(TranscriptEntry::new(
            TranscriptRole::Assistant,
            "Probe",
            split_body_lines(item.text.as_str()),
        )),
        TranscriptItemKind::ToolCall => Some(TranscriptEntry::new(
            TranscriptRole::Tool,
            format!(
                "Tool Call: {}",
                item.name.as_deref().unwrap_or("unknown_tool")
            ),
            tool_call_lines(turn_index, item),
        )),
        TranscriptItemKind::ToolResult => Some(TranscriptEntry::new(
            transcript_role_for_tool_result(item.tool_execution.as_ref()),
            tool_result_title(item),
            tool_result_lines(turn_index, item),
        )),
        TranscriptItemKind::Note => Some(TranscriptEntry::new(
            TranscriptRole::Status,
            "Runtime Note",
            split_body_lines(item.text.as_str()),
        )),
    }
}

fn transcript_role_for_tool_result(
    tool_execution: Option<&ToolExecutionRecord>,
) -> TranscriptRole {
    match tool_execution.map(|record| record.policy_decision) {
        Some(ToolPolicyDecision::Paused) | Some(ToolPolicyDecision::Refused) => {
            TranscriptRole::Status
        }
        Some(ToolPolicyDecision::AutoAllow) | Some(ToolPolicyDecision::Approved) | None => {
            TranscriptRole::Tool
        }
    }
}

fn tool_call_lines(turn_index: u64, item: &TranscriptItem) -> Vec<String> {
    let mut lines = vec![
        format!("turn: {turn_index}"),
        format!(
            "call_id: {}",
            item.tool_call_id.as_deref().unwrap_or("missing")
        ),
    ];
    if let Some(arguments) = item.arguments.as_ref() {
        lines.push(String::from("arguments"));
        lines.extend(pretty_json_lines(arguments));
    } else {
        lines.push(format!("arguments: {}", item.text));
    }
    lines
}

fn tool_result_title(item: &TranscriptItem) -> String {
    let name = item.name.as_deref().unwrap_or("unknown_tool");
    match item.tool_execution.as_ref().map(|record| record.policy_decision) {
        Some(ToolPolicyDecision::Paused) => format!("Approval Pending: {name}"),
        Some(ToolPolicyDecision::Refused) => format!("Tool Refused: {name}"),
        Some(ToolPolicyDecision::AutoAllow) | Some(ToolPolicyDecision::Approved) | None => {
            format!("Tool Result: {name}")
        }
    }
}

fn tool_result_lines(turn_index: u64, item: &TranscriptItem) -> Vec<String> {
    let mut lines = vec![
        format!("turn: {turn_index}"),
        format!(
            "call_id: {}",
            item.tool_call_id.as_deref().unwrap_or("missing")
        ),
    ];
    if let Some(record) = item.tool_execution.as_ref() {
        lines.push(format!("risk_class: {}", render_tool_risk_class(record.risk_class)));
        lines.push(format!(
            "policy_decision: {}",
            render_policy_decision(record.policy_decision)
        ));
        lines.push(format!(
            "approval_state: {}",
            render_approval_state(record.approval_state)
        ));
        if let Some(command) = record.command.as_ref() {
            lines.push(format!("command: {command}"));
        }
        if let Some(exit_code) = record.exit_code {
            lines.push(format!("exit_code: {exit_code}"));
        }
        if let Some(timed_out) = record.timed_out {
            lines.push(format!("timed_out: {timed_out}"));
        }
        if let Some(truncated) = record.truncated {
            lines.push(format!("truncated: {truncated}"));
        }
        if let Some(bytes_returned) = record.bytes_returned {
            lines.push(format!("bytes_returned: {bytes_returned}"));
        }
        if !record.files_touched.is_empty() {
            lines.push(format!("files_touched: {}", record.files_touched.join(", ")));
        }
        if let Some(reason) = record.reason.as_ref() {
            lines.push(format!("reason: {reason}"));
        }
    }
    lines.push(String::from("output"));
    match serde_json::from_str::<Value>(item.text.as_str()) {
        Ok(value) => lines.extend(pretty_json_lines(&value)),
        Err(_) => lines.extend(split_body_lines(item.text.as_str())),
    }
    lines
}

fn pretty_json_lines(value: &Value) -> Vec<String> {
    serde_json::to_string_pretty(value)
        .map(|body| split_body_lines(body.as_str()))
        .unwrap_or_else(|_| vec![value.to_string()])
}

fn split_body_lines(value: &str) -> Vec<String> {
    let lines = value
        .lines()
        .map(str::trim_end)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn runtime_error_session_id(error: &RuntimeError) -> Option<SessionId> {
    match error {
        RuntimeError::ProviderRequest { session_id, .. }
        | RuntimeError::MissingAssistantMessage { session_id, .. }
        | RuntimeError::UnsupportedBackendFeature { session_id, .. }
        | RuntimeError::ToolApprovalPending { session_id, .. }
        | RuntimeError::MaxToolRoundTrips { session_id, .. } => Some(session_id.clone()),
        RuntimeError::ProbeHomeUnavailable
        | RuntimeError::CurrentDir(_)
        | RuntimeError::SessionStore(_)
        | RuntimeError::MalformedTranscript(_) => None,
    }
}

fn render_policy_decision(value: ToolPolicyDecision) -> &'static str {
    match value {
        ToolPolicyDecision::AutoAllow => "auto_allow",
        ToolPolicyDecision::Approved => "approved",
        ToolPolicyDecision::Refused => "refused",
        ToolPolicyDecision::Paused => "paused",
    }
}

fn render_approval_state(value: ToolApprovalState) -> &'static str {
    match value {
        ToolApprovalState::NotRequired => "not_required",
        ToolApprovalState::Approved => "approved",
        ToolApprovalState::Refused => "refused",
        ToolApprovalState::Pending => "pending",
    }
}

fn render_tool_risk_class(value: ToolRiskClass) -> &'static str {
    match value {
        ToolRiskClass::ReadOnly => "read_only",
        ToolRiskClass::ShellReadOnly => "shell_read_only",
        ToolRiskClass::Write => "write",
        ToolRiskClass::Network => "network",
        ToolRiskClass::Destructive => "destructive",
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

    let provider =
        match AppleFmProviderClient::new(AppleFmProviderConfig::from_backend_profile(&profile)) {
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
                        format!(
                            "call_{call_index}_{}",
                            title.to_lowercase().replace(' ', "_")
                        ),
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
        prompt_tokens: usage
            .prompt_tokens_detail
            .as_ref()
            .map(|detail| detail.value),
        prompt_truth: usage.prompt_tokens_detail.as_ref().map(usage_truth_label),
        completion_tokens: usage
            .completion_tokens_detail
            .as_ref()
            .map(|detail| detail.value),
        completion_truth: usage
            .completion_tokens_detail
            .as_ref()
            .map(usage_truth_label),
        total_tokens: usage
            .total_tokens_detail
            .as_ref()
            .map(|detail| detail.value),
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

fn failure_from_provider_error(
    stage: impl Into<String>,
    error: &ProviderError,
) -> AppleFmFailureSummary {
    let receipt = error.backend_turn_receipt();
    let failure = receipt.and_then(|receipt| receipt.failure);
    AppleFmFailureSummary {
        stage: stage.into(),
        detail: error.to_string(),
        reason_code: failure.as_ref().and_then(|failure| failure.code.clone()),
        retryable: failure.as_ref().and_then(|failure| failure.retryable),
        failure_reason: failure
            .as_ref()
            .and_then(|failure| failure.failure_reason.clone()),
        recovery_suggestion: failure
            .as_ref()
            .and_then(|failure| failure.recovery_suggestion.clone()),
    }
}
