use std::collections::VecDeque;
use std::path::PathBuf;

use probe_core::backend_profiles::resolved_reasoning_level_for_backend;
use probe_core::provider::normalize_openai_stream_display_text;
use probe_core::runtime::{RuntimeEvent, StreamedToolCallDelta};
use probe_core::server_control::ServerOperatorSummary;
use probe_core::tools::tool_result_model_text;
use probe_decisions::SelectedGithubIssue;
use probe_openai_auth::{OpenAiCodexAuthController, OpenAiCodexRoute};
use probe_protocol::session::{PendingToolApproval, ToolApprovalResolution};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui::widgets::Paragraph;

use crate::bottom_pane::ComposerSubmission;
use crate::event::UiEvent;
use crate::failure::classify_runtime_failure;
use crate::message::{
    AppMessage, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
    AppleFmFailureSummary, AppleFmUsageSummary,
};
use crate::transcript::{ActiveTurn, RetainedTranscript, TranscriptEntry, TranscriptRole};
use crate::widgets::ModalCard;

const MAX_EVENT_LOG: usize = 16;
const LINE_SCROLL_STEP: u16 = 3;
const PAGE_SCROLL_STEP: u16 = 12;
const OPENAI_API_KEY_ENV: &str = "PROBE_OPENAI_API_KEY";
const OPENAI_API_KEY_SOURCE_ENV: &str = "PROBE_OPENAI_API_KEY_SOURCE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenId {
    Chat,
    Help,
    SetupOverlay,
    ApprovalOverlay,
}

impl ScreenId {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Help => "help modal",
            Self::SetupOverlay => "backend overlay",
            Self::ApprovalOverlay => "approval overlay",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Primary,
    Secondary,
    Tertiary,
}

impl ActiveTab {
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Primary => 0,
            Self::Secondary => 1,
            Self::Tertiary => 2,
        }
    }

    pub(crate) const fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Primary,
            1 => Self::Secondary,
            _ => Self::Tertiary,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Primary => Self::Secondary,
            Self::Secondary => Self::Tertiary,
            Self::Tertiary => Self::Primary,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn previous(self) -> Self {
        match self {
            Self::Primary => Self::Tertiary,
            Self::Secondary => Self::Primary,
            Self::Tertiary => Self::Secondary,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPhase {
    Idle,
    Queued,
    CheckingAvailability,
    Unavailable,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ProbeRuntimeState {
    session_id: Option<String>,
    profile_name: Option<String>,
    model_id: Option<String>,
    cwd: Option<String>,
    backend_kind: Option<String>,
    phase: Option<String>,
    round_trip: Option<usize>,
    active_tool: Option<String>,
    pending_approvals: Vec<PendingToolApproval>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssistantStreamMode {
    Delta,
    Snapshot,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StreamToolCallState {
    tool_index: usize,
    call_id: Option<String>,
    tool_name: Option<String>,
    arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AssistantStreamState {
    round_trip: usize,
    response_id: String,
    response_model: String,
    mode: AssistantStreamMode,
    backend_kind: Option<String>,
    first_chunk_ms: Option<u64>,
    assistant_text: String,
    tool_calls: Vec<StreamToolCallState>,
    finish_reason: Option<String>,
    failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenAction {
    None,
    OpenHelp,
    OpenSetupOverlay,
    OpenApprovalOverlay,
    CloseModal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenCommand {
    RunAppleFmSetup,
    ResolvePendingToolApproval {
        session_id: String,
        call_id: String,
        resolution: ToolApprovalResolution,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenOutcome {
    pub action: ScreenAction,
    pub status: Option<String>,
    pub command: Option<ScreenCommand>,
    pub transcript_entry: Option<TranscriptEntry>,
}

impl ScreenOutcome {
    pub const fn idle() -> Self {
        Self {
            action: ScreenAction::None,
            status: None,
            command: None,
            transcript_entry: None,
        }
    }

    fn with_status(action: ScreenAction, status: String) -> Self {
        Self {
            action,
            status: Some(status),
            command: None,
            transcript_entry: None,
        }
    }

    fn with_command(status: String, command: ScreenCommand) -> Self {
        Self {
            action: ScreenAction::None,
            status: Some(status),
            command: Some(command),
            transcript_entry: None,
        }
    }

    fn with_action_and_command(
        action: ScreenAction,
        status: String,
        command: ScreenCommand,
    ) -> Self {
        Self {
            action,
            status: Some(status),
            command: Some(command),
            transcript_entry: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenState {
    Chat(ChatScreen),
    Help(HelpScreen),
    Setup(SetupOverlay),
    Approval(ApprovalOverlay),
}

impl ScreenState {
    pub const fn id(&self) -> ScreenId {
        match self {
            Self::Chat(_) => ScreenId::Chat,
            Self::Help(_) => ScreenId::Help,
            Self::Setup(_) => ScreenId::SetupOverlay,
            Self::Approval(_) => ScreenId::ApprovalOverlay,
        }
    }

    pub const fn is_modal(&self) -> bool {
        !matches!(self, Self::Chat(_))
    }

    pub const fn replaces_composer(&self) -> bool {
        matches!(self, Self::Approval(_))
    }

    pub fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match self {
            Self::Chat(screen) => screen.handle_event(event),
            Self::Help(screen) => screen.handle_event(event),
            Self::Setup(screen) => screen.handle_event(event),
            Self::Approval(screen) => screen.handle_event(event),
        }
    }

    pub fn render(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        stack_depth: usize,
        base_screen: &ChatScreen,
    ) {
        match self {
            Self::Chat(screen) => screen.render(frame, area, stack_depth),
            Self::Help(screen) => screen.render(frame, area, stack_depth),
            Self::Setup(screen) => screen.render(frame, area, stack_depth, base_screen),
            Self::Approval(screen) => screen.render(frame, area, stack_depth),
        }
    }

    pub fn chat_mut(&mut self) -> Option<&mut ChatScreen> {
        match self {
            Self::Chat(screen) => Some(screen),
            Self::Help(_) | Self::Setup(_) | Self::Approval(_) => None,
        }
    }

    pub fn chat(&self) -> Option<&ChatScreen> {
        match self {
            Self::Chat(screen) => Some(screen),
            Self::Help(_) | Self::Setup(_) | Self::Approval(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveCall {
    title: String,
    prompt: String,
    index: usize,
    total_calls: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AppleFmSetupState {
    phase: TaskPhase,
    backend: Option<AppleFmBackendSummary>,
    availability: Option<AppleFmAvailabilitySummary>,
    calls: Vec<AppleFmCallRecord>,
    active_call: Option<ActiveCall>,
    failure: Option<AppleFmFailureSummary>,
}

impl Default for AppleFmSetupState {
    fn default() -> Self {
        Self {
            phase: TaskPhase::Idle,
            backend: None,
            availability: None,
            calls: Vec::new(),
            active_call: None,
            failure: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatScreen {
    active_tab: ActiveTab,
    emphasized_copy: bool,
    recent_events: VecDeque<String>,
    task_events: VecDeque<String>,
    transcript: RetainedTranscript,
    transcript_scroll_from_bottom: u16,
    transcript_follow_latest: bool,
    transcript_line_count: usize,
    runtime: ProbeRuntimeState,
    stream: Option<AssistantStreamState>,
    selected_issue: Option<SelectedGithubIssue>,
    operator_backend: Option<ServerOperatorSummary>,
    probe_home: Option<PathBuf>,
    codex_route_badge: Option<String>,
    setup: AppleFmSetupState,
}

impl Default for ChatScreen {
    fn default() -> Self {
        let mut screen = Self {
            active_tab: ActiveTab::Primary,
            emphasized_copy: false,
            recent_events: VecDeque::new(),
            task_events: VecDeque::new(),
            transcript: RetainedTranscript::new(),
            transcript_scroll_from_bottom: 0,
            transcript_follow_latest: true,
            transcript_line_count: 0,
            runtime: ProbeRuntimeState::default(),
            stream: None,
            selected_issue: None,
            operator_backend: None,
            probe_home: None,
            codex_route_badge: None,
            setup: AppleFmSetupState::default(),
        };
        screen.record_event("probe tui ready");
        screen.record_event("the default TUI path picks the backend automatically");
        screen.record_event("press Ctrl+R to rerun backend check when supported");
        screen.record_event("press Ctrl+S to inspect backend status");
        screen.record_event("press F1 for help");
        screen
    }
}

impl ChatScreen {
    pub fn active_tab(&self) -> ActiveTab {
        self.active_tab
    }

    pub fn set_backend_selector(&mut self, _labels: Vec<String>, active_tab: ActiveTab) {
        self.active_tab = active_tab;
    }

    pub fn set_probe_home(&mut self, probe_home: Option<PathBuf>) {
        self.probe_home = probe_home;
        self.refresh_codex_route_badge();
    }

    pub fn emphasized_copy(&self) -> bool {
        self.emphasized_copy
    }

    pub fn task_phase(&self) -> TaskPhase {
        self.setup.phase
    }

    pub fn call_count(&self) -> usize {
        self.setup.calls.len()
    }

    pub fn recent_events(&self) -> impl Iterator<Item = &String> {
        self.recent_events.iter()
    }

    pub fn worker_events(&self) -> impl Iterator<Item = &String> {
        self.task_events.iter()
    }

    pub fn runtime_session_id(&self) -> Option<&str> {
        self.runtime.session_id.as_deref()
    }

    pub fn has_pending_tool_approvals(&self) -> bool {
        !self.runtime.pending_approvals.is_empty()
    }

    pub fn pending_tool_approval_count(&self) -> usize {
        self.runtime.pending_approvals.len()
    }

    pub fn current_pending_tool_approval(&self) -> Option<&PendingToolApproval> {
        self.runtime.pending_approvals.first()
    }

    pub fn has_active_runtime_work(&self) -> bool {
        matches!(
            self.runtime.phase.as_deref(),
            Some(
                "turn_started"
                    | "model_request"
                    | "assistant_streaming"
                    | "assistant_snapshot_streaming"
                    | "assistant_snapshot"
                    | "tool_call_streaming"
                    | "tool_requested"
                    | "tool_running"
            )
        )
    }

    pub fn set_operator_backend(&mut self, summary: ServerOperatorSummary) {
        self.runtime.backend_kind = Some(render_backend_kind(summary.backend_kind).to_string());
        self.runtime.model_id = summary.model_id.clone();
        self.operator_backend = Some(summary.clone());
        self.refresh_codex_route_badge();
        self.record_event(format!(
            "backend target: {} {} {} model={}",
            render_backend_kind(summary.backend_kind),
            summary.attach_mode_label(),
            summary.endpoint_label(),
            summary.model_id.as_deref().unwrap_or("unknown")
        ));
        self.record_event(format!(
            "backend transport: {} ({})",
            summary.target_kind_label(),
            summary.base_url
        ));
        if summary.is_remote_target() {
            self.record_event(
                "remote inference only: tools, approvals, transcripts, and UI stay local",
            );
        } else if summary.target_kind_label() == "loopback_or_ssh_forward" {
            self.record_event(
                "loopback attach may be local or an SSH-forwarded remote Psionic target",
            );
        }
    }

    pub fn record_event(&mut self, message: impl Into<String>) {
        self.recent_events.push_front(message.into());
        while self.recent_events.len() > MAX_EVENT_LOG {
            self.recent_events.pop_back();
        }
    }

    fn record_worker_event(&mut self, message: impl Into<String>) {
        self.task_events.push_front(message.into());
        while self.task_events.len() > MAX_EVENT_LOG {
            self.task_events.pop_back();
        }
    }

    pub fn submit_user_turn(&mut self, submission: &ComposerSubmission) {
        let mut body = if submission.text.is_empty() {
            vec![String::from("[attachment-only draft]")]
        } else {
            submission
                .text
                .split('\n')
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        };
        if let Some(command) = &submission.slash_command {
            body.push(format!("slash_command: /{command}"));
        }
        if !submission.mentions.is_empty() {
            body.push(format!(
                "mentions: {}",
                submission
                    .mentions
                    .iter()
                    .map(|mention| format!("{}:{}", mention.kind.label(), mention.value))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !submission.attachments.is_empty() {
            body.push(format!(
                "attachments: {}",
                submission
                    .attachments
                    .iter()
                    .map(|attachment| attachment.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if submission.pasted_multiline {
            body.push(String::from("paste_mode: multiline"));
        }
        self.transcript
            .push_entry(TranscriptEntry::new(TranscriptRole::User, "You", body));
        self.snap_transcript_to_latest();
        self.record_event(format!(
            "submitted chat turn ({} chars)",
            submission.text.chars().count()
        ));
    }

    pub fn prepare_for_setup(&mut self, backend: AppleFmBackendSummary) {
        self.setup = AppleFmSetupState {
            phase: TaskPhase::Queued,
            backend: Some(backend.clone()),
            ..AppleFmSetupState::default()
        };
        self.task_events.clear();
        self.record_worker_event(format!(
            "queued Apple FM setup against {}",
            backend.profile_name
        ));
    }

    fn uses_apple_fm_backend(&self) -> bool {
        self.operator_backend.as_ref().is_some_and(|summary| {
            summary.backend_kind == probe_protocol::backend::BackendKind::AppleFmBridge
        })
    }

    pub fn composer_header_status(&self) -> Vec<String> {
        let model = self
            .runtime
            .model_id
            .as_deref()
            .or_else(|| {
                self.operator_backend
                    .as_ref()
                    .and_then(|summary| summary.model_id.as_deref())
            })
            .unwrap_or("pending");
        let reasoning = self.operator_backend.as_ref().and_then(|summary| {
            resolved_reasoning_level_for_backend(
                summary.backend_kind,
                summary.reasoning_level.as_deref(),
            )
        });
        let mut parts = vec![preview(model, 24)];
        if let Some(reasoning) = reasoning {
            parts.push(display_reasoning_level(reasoning).to_string());
        }
        if let Some(route_badge) = self.codex_route_badge.as_ref() {
            parts.push(route_badge.clone());
        }
        if let Some(issue) = self.selected_issue.as_ref() {
            parts.push(preview(
                format!("{}#{} {}", issue.repo_name, issue.issue_number, issue.title).as_str(),
                52,
            ));
        }
        parts.push(status_marker(self).to_string());
        parts
    }

    fn clear_stream(&mut self) {
        self.stream = None;
    }

    fn start_stream(
        &mut self,
        session_id: &str,
        round_trip: usize,
        response_id: String,
        response_model: String,
        mode: AssistantStreamMode,
    ) {
        self.runtime.session_id = Some(session_id.to_string());
        self.runtime.phase = Some(match mode {
            AssistantStreamMode::Delta => String::from("assistant_streaming"),
            AssistantStreamMode::Snapshot => String::from("assistant_snapshot_streaming"),
        });
        self.runtime.round_trip = Some(round_trip);
        self.runtime.active_tool = None;
        self.stream = Some(AssistantStreamState {
            round_trip,
            response_id,
            response_model,
            mode,
            backend_kind: self.runtime.backend_kind.clone(),
            first_chunk_ms: None,
            assistant_text: String::new(),
            tool_calls: Vec::new(),
            finish_reason: None,
            failure: None,
        });
        self.sync_stream_active_turn();
    }

    fn note_first_stream_chunk(&mut self, session_id: &str, round_trip: usize, milliseconds: u64) {
        self.runtime.session_id = Some(session_id.to_string());
        self.runtime.phase = Some(String::from("assistant_streaming"));
        self.runtime.round_trip = Some(round_trip);
        if let Some(stream) = self.stream.as_mut() {
            stream.first_chunk_ms = Some(milliseconds);
            stream.failure = None;
        }
        self.sync_stream_active_turn();
    }

    fn append_stream_delta(&mut self, session_id: &str, round_trip: usize, delta: &str) {
        self.runtime.session_id = Some(session_id.to_string());
        self.runtime.phase = Some(String::from("assistant_streaming"));
        self.runtime.round_trip = Some(round_trip);
        let stream = self.stream.get_or_insert_with(|| AssistantStreamState {
            round_trip,
            response_id: String::from("pending"),
            response_model: String::from("pending"),
            mode: AssistantStreamMode::Delta,
            backend_kind: self.runtime.backend_kind.clone(),
            first_chunk_ms: None,
            assistant_text: String::new(),
            tool_calls: Vec::new(),
            finish_reason: None,
            failure: None,
        });
        stream.mode = AssistantStreamMode::Delta;
        stream.assistant_text.push_str(delta);
        stream.failure = None;
        self.sync_stream_active_turn();
    }

    fn update_stream_snapshot(&mut self, session_id: &str, round_trip: usize, snapshot: &str) {
        self.runtime.session_id = Some(session_id.to_string());
        self.runtime.phase = Some(String::from("assistant_snapshot"));
        self.runtime.round_trip = Some(round_trip);
        let stream = self.stream.get_or_insert_with(|| AssistantStreamState {
            round_trip,
            response_id: String::from("pending"),
            response_model: String::from("pending"),
            mode: AssistantStreamMode::Snapshot,
            backend_kind: self.runtime.backend_kind.clone(),
            first_chunk_ms: None,
            assistant_text: String::new(),
            tool_calls: Vec::new(),
            finish_reason: None,
            failure: None,
        });
        stream.mode = AssistantStreamMode::Snapshot;
        stream.assistant_text = snapshot.to_string();
        stream.failure = None;
        self.sync_stream_active_turn();
    }

    fn update_stream_tool_calls(
        &mut self,
        session_id: &str,
        round_trip: usize,
        deltas: &[StreamedToolCallDelta],
    ) {
        self.runtime.session_id = Some(session_id.to_string());
        self.runtime.phase = Some(String::from("tool_call_streaming"));
        self.runtime.round_trip = Some(round_trip);
        let stream = self.stream.get_or_insert_with(|| AssistantStreamState {
            round_trip,
            response_id: String::from("pending"),
            response_model: String::from("pending"),
            mode: AssistantStreamMode::Delta,
            backend_kind: self.runtime.backend_kind.clone(),
            first_chunk_ms: None,
            assistant_text: String::new(),
            tool_calls: Vec::new(),
            finish_reason: None,
            failure: None,
        });
        for delta in deltas {
            if let Some(existing) = stream
                .tool_calls
                .iter_mut()
                .find(|tool| tool.tool_index == delta.tool_index)
            {
                if let Some(call_id) = delta.call_id.as_ref() {
                    existing.call_id = Some(call_id.clone());
                }
                if let Some(tool_name) = delta.tool_name.as_ref() {
                    existing.tool_name = Some(tool_name.clone());
                }
                if let Some(arguments_delta) = delta.arguments_delta.as_ref() {
                    existing.arguments.push_str(arguments_delta);
                }
            } else {
                stream.tool_calls.push(StreamToolCallState {
                    tool_index: delta.tool_index,
                    call_id: delta.call_id.clone(),
                    tool_name: delta.tool_name.clone(),
                    arguments: delta.arguments_delta.clone().unwrap_or_default(),
                });
                stream.tool_calls.sort_by_key(|tool| tool.tool_index);
            }
        }
        stream.failure = None;
        self.sync_stream_active_turn();
    }

    fn finish_stream(
        &mut self,
        session_id: &str,
        round_trip: usize,
        response_id: String,
        response_model: String,
        finish_reason: Option<String>,
    ) {
        self.runtime.session_id = Some(session_id.to_string());
        self.runtime.phase = Some(String::from("assistant_stream_finished"));
        self.runtime.round_trip = Some(round_trip);
        self.runtime.active_tool = None;
        if let Some(stream) = self.stream.as_mut() {
            stream.round_trip = round_trip;
            stream.response_id = response_id;
            stream.response_model = response_model;
            stream.finish_reason = finish_reason;
            stream.failure = None;
        }
        self.sync_stream_active_turn();
    }

    fn fail_stream(
        &mut self,
        session_id: &str,
        round_trip: usize,
        backend_kind: &str,
        error: &str,
    ) {
        self.runtime.session_id = Some(session_id.to_string());
        self.runtime.phase = Some(String::from("model_request_failed"));
        self.runtime.round_trip = Some(round_trip);
        self.runtime.active_tool = None;
        self.runtime.backend_kind = Some(backend_kind.to_string());
        let stream = self.stream.get_or_insert_with(|| AssistantStreamState {
            round_trip,
            response_id: String::from("failed"),
            response_model: String::from("failed"),
            mode: AssistantStreamMode::Delta,
            backend_kind: Some(backend_kind.to_string()),
            first_chunk_ms: None,
            assistant_text: String::new(),
            tool_calls: Vec::new(),
            finish_reason: None,
            failure: None,
        });
        stream.backend_kind = Some(backend_kind.to_string());
        stream.failure = Some(error.to_string());
        self.sync_stream_active_turn();
    }

    fn sync_stream_active_turn(&mut self) {
        let Some(stream) = self.stream.as_ref() else {
            return;
        };
        self.transcript.set_active_turn(render_stream_active_turn(
            stream,
            self.operator_backend.as_ref(),
        ));
        self.snap_transcript_to_latest();
    }

    fn apply_runtime_event(&mut self, event: RuntimeEvent) -> String {
        match event {
            RuntimeEvent::TurnStarted {
                session_id,
                profile_name,
                prompt,
                tool_loop_enabled,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.profile_name = Some(profile_name.clone());
                self.runtime.backend_kind = None;
                self.runtime.phase = Some(String::from("turn_started"));
                self.runtime.round_trip = None;
                self.runtime.active_tool = None;
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Assistant,
                    "Probe Runtime",
                    vec![
                        format!("profile: {profile_name}"),
                        format!(
                            "tool_loop: {}",
                            if tool_loop_enabled {
                                "enabled"
                            } else {
                                "disabled"
                            }
                        ),
                        format!("prompt_preview: {}", preview(prompt.as_str(), 72)),
                        format!("session: {}", short_session_id(session_id.as_str())),
                    ],
                ));
                self.snap_transcript_to_latest();
                self.record_worker_event(String::from("runtime turn started"));
                String::from("runtime turn started")
            }
            RuntimeEvent::ModelRequestStarted {
                session_id,
                round_trip,
                backend_kind,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.backend_kind = Some(format!("{backend_kind:?}"));
                self.runtime.phase = Some(String::from("model_request"));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = None;
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Assistant,
                    "Waiting for Reply",
                    Vec::new(),
                ));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("model request started: round {round_trip}"));
                format!("model request started for round {round_trip}")
            }
            RuntimeEvent::AssistantStreamStarted {
                session_id,
                round_trip,
                response_id,
                response_model,
            } => {
                self.start_stream(
                    session_id.as_str(),
                    round_trip,
                    response_id,
                    response_model,
                    AssistantStreamMode::Delta,
                );
                self.record_worker_event(format!("assistant stream started: round {round_trip}"));
                format!("assistant stream started for round {round_trip}")
            }
            RuntimeEvent::TimeToFirstTokenObserved {
                session_id,
                round_trip,
                milliseconds,
            } => {
                self.note_first_stream_chunk(session_id.as_str(), round_trip, milliseconds);
                self.record_worker_event(format!(
                    "first streamed chunk observed after {milliseconds}ms"
                ));
                format!("time to first token observed for round {round_trip}")
            }
            RuntimeEvent::AssistantDelta {
                session_id,
                round_trip,
                delta,
            } => {
                self.append_stream_delta(session_id.as_str(), round_trip, delta.as_str());
                self.record_worker_event(format!(
                    "assistant delta appended (+{} chars)",
                    delta.chars().count()
                ));
                format!("assistant delta appended for round {round_trip}")
            }
            RuntimeEvent::AssistantSnapshot {
                session_id,
                round_trip,
                snapshot,
            } => {
                self.update_stream_snapshot(session_id.as_str(), round_trip, snapshot.as_str());
                self.record_worker_event(format!(
                    "assistant snapshot updated ({} chars)",
                    snapshot.chars().count()
                ));
                format!("assistant snapshot updated for round {round_trip}")
            }
            RuntimeEvent::ToolCallDelta {
                session_id,
                round_trip,
                deltas,
            } => {
                self.update_stream_tool_calls(session_id.as_str(), round_trip, &deltas);
                self.record_worker_event(format!("streamed {} tool call delta(s)", deltas.len()));
                String::from("tool call delta received")
            }
            RuntimeEvent::ToolCallRequested {
                session_id,
                round_trip,
                call_id: _call_id,
                tool_name,
                arguments,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.phase = Some(String::from("tool_requested"));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool_name.clone());
                self.transcript.clear_active_turn();
                self.transcript
                    .push_live_entry(runtime_tool_call_entry(tool_name.as_str(), &arguments));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("tool call requested: {tool_name}"));
                format!("tool call requested: {tool_name}")
            }
            RuntimeEvent::ToolExecutionStarted {
                session_id,
                round_trip,
                call_id: _call_id,
                tool_name,
                risk_class,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.phase = Some(String::from("tool_running"));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool_name.clone());
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Status,
                    format!("Running Tool: {tool_name}"),
                    vec![format!("risk: {}", render_runtime_risk_class(risk_class))],
                ));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("tool execution started: {tool_name}"));
                format!("tool execution started: {tool_name}")
            }
            RuntimeEvent::ToolExecutionCompleted {
                session_id,
                round_trip,
                tool,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.phase = Some(String::from("tool_completed"));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool.name.clone());
                self.transcript.clear_active_turn();
                self.transcript
                    .push_live_entry(runtime_tool_result_entry(round_trip, &tool));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("tool execution completed: {}", tool.name));
                format!("tool execution completed: {}", tool.name)
            }
            RuntimeEvent::ToolRefused {
                session_id,
                round_trip,
                tool,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.phase = Some(String::from("tool_refused"));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool.name.clone());
                self.transcript.clear_active_turn();
                self.transcript
                    .push_live_entry(runtime_tool_result_entry(round_trip, &tool));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("tool refused: {}", tool.name));
                format!("tool refused: {}", tool.name)
            }
            RuntimeEvent::ToolPaused {
                session_id,
                round_trip,
                tool,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.phase = Some(String::from("tool_paused"));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool.name.clone());
                self.transcript.clear_active_turn();
                self.transcript
                    .push_live_entry(runtime_tool_result_entry(round_trip, &tool));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("tool paused for approval: {}", tool.name));
                format!("tool paused for approval: {}", tool.name)
            }
            RuntimeEvent::AssistantStreamFinished {
                session_id,
                round_trip,
                response_id,
                response_model,
                finish_reason,
            } => {
                self.finish_stream(
                    session_id.as_str(),
                    round_trip,
                    response_id,
                    response_model,
                    finish_reason,
                );
                self.record_worker_event(String::from("assistant stream finished"));
                String::from("assistant stream finished")
            }
            RuntimeEvent::ModelRequestFailed {
                session_id,
                round_trip,
                backend_kind,
                error,
            } => {
                let backend_kind = format!("{backend_kind:?}");
                self.fail_stream(
                    session_id.as_str(),
                    round_trip,
                    backend_kind.as_str(),
                    error.as_str(),
                );
                self.record_worker_event(String::from("model request failed"));
                String::from("model request failed")
            }
            RuntimeEvent::AssistantTurnCommitted {
                session_id,
                response_id: _,
                response_model: _,
                assistant_text,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.phase = Some(String::from("assistant_committed"));
                self.runtime.active_tool = None;
                let body = split_text_lines(
                    normalize_openai_stream_display_text(assistant_text.as_str()).as_str(),
                );
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Assistant,
                    "Probe",
                    body,
                ));
                self.snap_transcript_to_latest();
                self.record_worker_event(String::from("assistant turn committed"));
                String::from("assistant turn committed")
            }
        }
    }

    pub fn apply_message(&mut self, message: AppMessage) -> String {
        match message {
            AppMessage::AssistantStreamStarted {
                session_id,
                round_trip,
                response_id,
                response_model,
            } => {
                self.start_stream(
                    session_id.as_str(),
                    round_trip,
                    response_id,
                    response_model,
                    AssistantStreamMode::Delta,
                );
                self.record_worker_event(format!("assistant stream started: round {round_trip}"));
                format!("assistant stream started for round {round_trip}")
            }
            AppMessage::AssistantFirstChunkObserved {
                session_id,
                round_trip,
                milliseconds,
            } => {
                self.note_first_stream_chunk(session_id.as_str(), round_trip, milliseconds);
                self.record_worker_event(format!(
                    "first streamed chunk observed after {milliseconds}ms"
                ));
                format!("time to first token observed for round {round_trip}")
            }
            AppMessage::AssistantDeltaAppended {
                session_id,
                round_trip,
                delta,
            } => {
                self.append_stream_delta(session_id.as_str(), round_trip, delta.as_str());
                self.record_worker_event(format!(
                    "assistant delta appended (+{} chars)",
                    delta.chars().count()
                ));
                format!("assistant delta appended for round {round_trip}")
            }
            AppMessage::AssistantSnapshotUpdated {
                session_id,
                round_trip,
                snapshot,
            } => {
                self.update_stream_snapshot(session_id.as_str(), round_trip, snapshot.as_str());
                self.record_worker_event(format!(
                    "assistant snapshot updated ({} chars)",
                    snapshot.chars().count()
                ));
                format!("assistant snapshot updated for round {round_trip}")
            }
            AppMessage::AssistantToolCallDeltaUpdated {
                session_id,
                round_trip,
                deltas,
            } => {
                self.update_stream_tool_calls(session_id.as_str(), round_trip, deltas.as_slice());
                self.record_worker_event(format!("streamed {} tool call delta(s)", deltas.len()));
                format!("tool call delta updated for round {round_trip}")
            }
            AppMessage::AssistantStreamFinished {
                session_id,
                round_trip,
                response_id,
                response_model,
                finish_reason,
            } => {
                self.finish_stream(
                    session_id.as_str(),
                    round_trip,
                    response_id,
                    response_model,
                    finish_reason,
                );
                self.record_worker_event(String::from("assistant stream finished"));
                String::from("assistant stream finished")
            }
            AppMessage::AssistantStreamFailed {
                session_id,
                round_trip,
                backend_kind,
                error,
            } => {
                let backend_kind = format!("{backend_kind:?}");
                self.fail_stream(
                    session_id.as_str(),
                    round_trip,
                    backend_kind.as_str(),
                    error.as_str(),
                );
                self.record_worker_event(String::from("model request failed"));
                String::from("model request failed")
            }
            AppMessage::TranscriptActiveTurnSet { turn } => {
                let role = turn.role().label().to_string();
                let title = turn.title().to_string();
                self.transcript.set_active_turn(turn);
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("updated active {role} turn: {title}"));
                format!("updated active {role} turn")
            }
            AppMessage::TranscriptEntriesCommitted { entries } => {
                let entry_count = entries.len();
                self.clear_stream();
                self.transcript.clear_active_turn();
                self.transcript.clear_live_entries();
                for entry in entries {
                    let label = entry.label().to_string();
                    let title = entry.title().to_string();
                    self.transcript.push_entry(entry);
                    self.record_worker_event(format!("committed {label} row: {title}"));
                }
                self.snap_transcript_to_latest();
                self.refresh_codex_route_badge();
                format!("committed {entry_count} transcript entries")
            }
            AppMessage::TranscriptEntryCommitted { entry } => {
                let label = entry.label().to_string();
                let title = entry.title().to_string();
                self.clear_stream();
                self.transcript.clear_active_turn();
                self.transcript.commit_live_entries();
                self.transcript.push_entry(entry);
                self.snap_transcript_to_latest();
                self.refresh_codex_route_badge();
                self.record_worker_event(format!("committed {label} row: {title}"));
                format!("committed {label} row")
            }
            AppMessage::ProbeRuntimeSessionReady {
                session_id,
                profile_name,
                model_id,
                cwd,
            } => {
                self.runtime = ProbeRuntimeState {
                    session_id: Some(session_id.clone()),
                    profile_name: Some(profile_name.clone()),
                    model_id: Some(model_id.clone()),
                    cwd: Some(cwd),
                    backend_kind: self.runtime.backend_kind.clone(),
                    phase: self.runtime.phase.clone(),
                    round_trip: self.runtime.round_trip,
                    active_tool: self.runtime.active_tool.clone(),
                    pending_approvals: self.runtime.pending_approvals.clone(),
                };
                self.refresh_codex_route_badge();
                self.record_worker_event(format!(
                    "runtime session ready: {} via {}",
                    short_session_id(session_id.as_str()),
                    profile_name
                ));
                format!(
                    "runtime session {} ready",
                    short_session_id(session_id.as_str())
                )
            }
            AppMessage::ProbeRuntimeEvent { event } => self.apply_runtime_event(event),
            AppMessage::GithubIssueSelectionResolved { priority, decision } => {
                self.selected_issue = decision.selected_issue.clone();
                let entry = if let Some(issue) = decision.selected_issue.as_ref() {
                    TranscriptEntry::new(
                        TranscriptRole::Status,
                        "GitHub Issue",
                        vec![
                            format!("priority: {}", preview(priority.as_str(), 72)),
                            format!(
                                "selected: {}#{} {}",
                                issue.repo_name, issue.issue_number, issue.title
                            ),
                            format!("repo: {}", issue.repo_slug()),
                            format!("reason: {}", issue.reason),
                        ],
                    )
                } else {
                    TranscriptEntry::new(
                        TranscriptRole::Status,
                        "GitHub Issue",
                        vec![
                            format!("priority: {}", preview(priority.as_str(), 72)),
                            format!("result: {}", decision.reason),
                        ],
                    )
                };
                self.transcript.push_entry(entry);
                self.snap_transcript_to_latest();
                self.record_worker_event(String::from("GitHub issue selection updated"));
                decision.reason
            }
            AppMessage::GithubIssueSelectionFailed {
                priority,
                error,
                selected_issue,
            } => {
                self.selected_issue = selected_issue;
                self.transcript.push_entry(TranscriptEntry::new(
                    TranscriptRole::Status,
                    "GitHub Issue",
                    vec![
                        format!("priority: {}", preview(priority.as_str(), 72)),
                        format!("result: {error}"),
                    ],
                ));
                self.snap_transcript_to_latest();
                self.record_worker_event(String::from("GitHub issue selection failed"));
                error
            }
            AppMessage::PendingToolApprovalsUpdated {
                session_id,
                approvals,
            } => {
                self.runtime.session_id = Some(session_id.clone());
                self.runtime.pending_approvals = approvals.clone();
                if approvals.is_empty() {
                    self.record_worker_event(String::from("pending approvals cleared"));
                    String::from("pending approvals cleared")
                } else {
                    let next = approvals
                        .first()
                        .map(|approval| approval.tool_name.as_str())
                        .unwrap_or("unknown");
                    self.record_worker_event(format!(
                        "loaded {} pending approval(s); next: {}",
                        approvals.len(),
                        next
                    ));
                    format!("loaded {} pending approval(s)", approvals.len())
                }
            }
            AppMessage::AppleFmSetupStarted { backend } => {
                self.setup.backend = Some(backend.clone());
                self.setup.phase = TaskPhase::CheckingAvailability;
                self.record_worker_event(format!(
                    "checking Apple FM availability via {}",
                    backend.base_url
                ));
                String::from("checking Apple FM availability")
            }
            AppMessage::AppleFmAvailabilityReady {
                backend,
                availability,
            } => {
                let platform = availability
                    .platform
                    .clone()
                    .unwrap_or_else(|| String::from("unknown platform"));
                self.setup.backend = Some(backend);
                self.setup.availability = Some(availability);
                self.record_worker_event(format!("Apple FM availability ready on {platform}"));
                String::from("Apple FM availability check passed")
            }
            AppMessage::AppleFmAvailabilityUnavailable {
                backend,
                availability,
            } => {
                let reason = availability
                    .unavailable_reason
                    .clone()
                    .unwrap_or_else(|| String::from("unknown"));
                self.setup.backend = Some(backend);
                self.setup.availability = Some(availability);
                self.setup.active_call = None;
                self.setup.phase = TaskPhase::Unavailable;
                self.record_worker_event(format!("Apple FM unavailable: {reason}"));
                String::from("Apple FM unavailable")
            }
            AppMessage::AppleFmCallStarted {
                backend,
                index,
                total_calls,
                title,
                prompt,
            } => {
                self.setup.backend = Some(backend);
                self.setup.phase = TaskPhase::Running;
                self.setup.active_call = Some(ActiveCall {
                    title: title.clone(),
                    prompt,
                    index,
                    total_calls,
                });
                self.record_worker_event(format!("started call {index}/{total_calls}: {title}"));
                format!("running Apple FM call {index}/{total_calls}")
            }
            AppMessage::AppleFmCallCompleted {
                backend,
                index,
                total_calls,
                call,
            } => {
                let preview = preview(call.response_text.as_str(), 36);
                self.setup.backend = Some(backend);
                self.setup.calls.push(call);
                self.setup.active_call = None;
                self.record_worker_event(format!(
                    "completed call {index}/{total_calls}: {preview}"
                ));
                format!("completed Apple FM call {index}/{total_calls}")
            }
            AppMessage::AppleFmSetupCompleted {
                backend,
                total_calls,
            } => {
                self.setup.backend = Some(backend);
                self.setup.phase = TaskPhase::Completed;
                self.setup.active_call = None;
                self.record_worker_event(format!(
                    "Apple FM setup completed after {total_calls} calls"
                ));
                String::from("Apple FM setup completed")
            }
            AppMessage::AppleFmSetupFailed { backend, failure } => {
                let stage = failure.stage.clone();
                let reason = failure
                    .reason_code
                    .clone()
                    .unwrap_or_else(|| String::from("untyped"));
                self.setup.backend = Some(backend);
                self.setup.failure = Some(failure);
                self.setup.phase = TaskPhase::Failed;
                self.setup.active_call = None;
                self.record_worker_event(format!("Apple FM setup failed at {stage} ({reason})"));
                format!("Apple FM setup failed at {stage}")
            }
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::NextView => ScreenOutcome::idle(),
            UiEvent::PreviousView => ScreenOutcome::idle(),
            UiEvent::ToggleBody => {
                self.emphasized_copy = !self.emphasized_copy;
                self.resume_transcript_follow();
                self.sync_transcript_scroll_after_update();
                let status = if self.emphasized_copy {
                    String::from("showing operator notes instead of live response detail")
                } else {
                    String::from("restored live Apple FM detail view")
                };
                self.record_event(status.clone());
                ScreenOutcome::with_status(ScreenAction::None, status)
            }
            UiEvent::ScrollUp => {
                self.scroll_transcript_up(LINE_SCROLL_STEP);
                ScreenOutcome::idle()
            }
            UiEvent::ScrollDown => {
                self.scroll_transcript_down(LINE_SCROLL_STEP);
                ScreenOutcome::idle()
            }
            UiEvent::PageUp => {
                self.scroll_transcript_up(PAGE_SCROLL_STEP);
                ScreenOutcome::idle()
            }
            UiEvent::PageDown => {
                self.scroll_transcript_down(PAGE_SCROLL_STEP);
                ScreenOutcome::idle()
            }
            UiEvent::RunBackgroundTask => {
                if self.uses_apple_fm_backend() {
                    self.record_event(String::from("requested Apple FM backend check rerun"));
                    ScreenOutcome::with_command(
                        String::from("queued Apple FM backend check and opened backend overlay"),
                        ScreenCommand::RunAppleFmSetup,
                    )
                } else {
                    self.record_event(String::from(
                        "current backend is prepared on launch; opened backend overlay",
                    ));
                    ScreenOutcome::with_status(
                        ScreenAction::OpenSetupOverlay,
                        String::from(
                            "current backend is prepared on launch; opened backend overlay",
                        ),
                    )
                }
            }
            UiEvent::OpenSetupOverlay => ScreenOutcome::with_status(
                ScreenAction::OpenSetupOverlay,
                String::from("opened backend overlay"),
            ),
            UiEvent::OpenApprovalOverlay => ScreenOutcome::with_status(
                ScreenAction::OpenApprovalOverlay,
                String::from("opened approval overlay"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            UiEvent::Tick => ScreenOutcome::idle(),
            UiEvent::Dismiss
            | UiEvent::Quit
            | UiEvent::ComposerInsert(_)
            | UiEvent::ComposerBackspace
            | UiEvent::ComposerDelete
            | UiEvent::ComposerMoveLeft
            | UiEvent::ComposerMoveRight
            | UiEvent::ComposerHistoryPrevious
            | UiEvent::ComposerHistoryNext
            | UiEvent::ComposerAddAttachment
            | UiEvent::ComposerPaste(_)
            | UiEvent::ComposerMoveHome
            | UiEvent::ComposerMoveEnd
            | UiEvent::ComposerNewline
            | UiEvent::ComposerSubmit => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, stack_depth: usize) {
        self.render_chat_shell(frame, area, stack_depth);
    }

    fn render_chat_shell(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let body = self.render_primary_body();
        let scroll_y = self.transcript_scroll_y(body.lines.len(), area.height);
        frame.render_widget(
            Paragraph::new(body)
                .scroll((scroll_y, 0))
                .wrap(ratatui::widgets::Wrap { trim: false }),
            area,
        );
    }

    fn render_setup_overlay_text(&self, stack_depth: usize) -> Text<'static> {
        if !self.uses_apple_fm_backend() {
            return self.render_remote_backend_overlay_text(stack_depth);
        }
        let mut lines = self
            .render_setup_body()
            .lines
            .iter()
            .map(ToString::to_string)
            .map(Line::from)
            .collect::<Vec<_>>();
        lines.push(Line::from(""));
        lines.push(Line::from("Backend Status"));
        for line in self.render_status_lines("backend overlay", stack_depth) {
            lines.push(Line::from(format!("  {line}")));
        }
        lines.push(Line::from(""));
        lines.push(Line::from("Backend Facts"));
        for line in self.render_backend_lines() {
            lines.push(Line::from(format!("  {line}")));
        }
        lines.push(Line::from(""));
        lines.push(Line::from("Availability"));
        for line in self.render_availability_lines() {
            lines.push(Line::from(format!("  {line}")));
        }
        Text::from(lines)
    }

    fn render_remote_backend_overlay_text(&self, stack_depth: usize) -> Text<'static> {
        let Some(summary) = self.operator_backend.as_ref() else {
            return Text::from(vec![
                Line::from("Probe does not have a prepared backend summary yet."),
                Line::from(""),
                Line::from("Restart the TUI from `probe tui` after preparing a backend target."),
            ]);
        };

        let headline = match summary.backend_kind {
            probe_protocol::backend::BackendKind::OpenAiCodexSubscription => {
                "Probe is attached to the hosted Codex subscription backend."
            }
            probe_protocol::backend::BackendKind::OpenAiChatCompletions
            | probe_protocol::backend::BackendKind::AppleFmBridge => {
                "Probe is attached to a prepared backend target."
            }
        };

        let mut lines = vec![
            Line::from(headline),
            Line::from(""),
            Line::from(format!(
                "backend_kind: {}",
                render_backend_kind(summary.backend_kind)
            )),
            Line::from(format!("target: {}", summary.endpoint_label())),
            Line::from(format!("base_url: {}", summary.base_url)),
            Line::from(format!("attach_mode: {}", summary.attach_mode_label())),
            Line::from(format!("transport: {}", summary.target_kind_label())),
            Line::from(format!(
                "model: {}",
                summary.model_id.as_deref().unwrap_or("unknown")
            )),
            Line::from(format!(
                "reasoning_level: {}",
                summary
                    .reasoning_level
                    .as_deref()
                    .or_else(|| {
                        resolved_reasoning_level_for_backend(summary.backend_kind, None)
                    })
                    .unwrap_or("none")
            )),
            Line::from(""),
            Line::from("Contract"),
            Line::from("State"),
            Line::from(format!(
                "  phase: {}",
                self.runtime.phase.as_deref().unwrap_or("idle")
            )),
            Line::from(format!("  stack_depth: {stack_depth}")),
        ];
        for line in render_remote_contract_lines(summary) {
            lines.insert(lines.len() - 3, Line::from(format!("  {line}")));
        }
        lines.insert(lines.len() - 3, Line::from(""));
        if let Some(round_trip) = self.runtime.round_trip {
            lines.push(Line::from(format!("  round_trip: {round_trip}")));
        }
        if let Some(tool) = self.runtime.active_tool.as_deref() {
            lines.push(Line::from(format!("  active_tool: {tool}")));
        }
        if summary.backend_kind == probe_protocol::backend::BackendKind::OpenAiCodexSubscription {
            lines.push(Line::from(""));
            lines.push(Line::from("OpenAI Subscription Auth"));
            for line in self.render_codex_auth_lines() {
                lines.push(Line::from(format!("  {line}")));
            }
        }
        Text::from(lines)
    }

    fn render_primary_body(&self) -> Text<'static> {
        if self.emphasized_copy {
            return Text::from(vec![
                Line::from("Probe now renders real user, tool, and assistant turns."),
                Line::from(""),
                Line::from(
                    "Committed transcript entries stay in app state rather than terminal scrollback.",
                ),
                Line::from(
                    "A single explicit active-turn cell renders in-flight runtime or assistant work.",
                ),
                Line::from(
                    "Composer submission now lands a user turn and drives a real Probe runtime turn.",
                ),
                Line::from("The worker now owns a real session-backed runtime loop."),
            ]);
        }
        self.transcript.as_text()
    }

    fn render_setup_body(&self) -> Text<'static> {
        if !self.uses_apple_fm_backend() {
            return Text::from(vec![
                Line::from("Probe is using a non-Apple-FM backend target."),
                Line::from(""),
                Line::from(
                    "The backend overlay shows the prepared attach target and operator contract.",
                ),
                Line::from("Use Ctrl+S to inspect the target and Ctrl+R to reopen this overlay."),
            ]);
        }
        if self.emphasized_copy {
            return Text::from(vec![
                Line::from("Apple FM setup is now a secondary Probe surface."),
                Line::from(""),
                Line::from("The primary home screen is the chat shell."),
                Line::from("This tab remains the honest backend admission and setup view."),
            ]);
        }

        if let Some(failure) = &self.setup.failure {
            return Text::from(vec![
                Line::from(format!("Stage: {}", failure.stage)),
                Line::from(""),
                Line::from(failure.detail.clone()),
                Line::from(format!(
                    "reason_code: {}",
                    failure
                        .reason_code
                        .clone()
                        .unwrap_or_else(|| String::from("none"))
                )),
                Line::from(format!(
                    "retryable: {}",
                    failure
                        .retryable
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| String::from("unknown"))
                )),
                Line::from(format!(
                    "failure_reason: {}",
                    failure
                        .failure_reason
                        .clone()
                        .unwrap_or_else(|| String::from("none"))
                )),
                Line::from(format!(
                    "recovery_suggestion: {}",
                    failure
                        .recovery_suggestion
                        .clone()
                        .unwrap_or_else(|| String::from("none"))
                )),
            ]);
        }

        if let Some(active_call) = &self.setup.active_call {
            let mut lines = Vec::new();
            if let Some(last_call) = self.setup.calls.last() {
                lines.extend([
                    Line::from(format!("Completed calls: {}", self.setup.calls.len())),
                    Line::from(""),
                    Line::from(format!("Last completed call: {}", last_call.title)),
                    Line::from("Response"),
                    Line::from(last_call.response_text.clone()),
                    Line::from(""),
                ]);
            }
            lines.extend([
                Line::from(format!(
                    "Running call {}/{}: {}",
                    active_call.index, active_call.total_calls, active_call.title
                )),
                Line::from(""),
                Line::from("Prompt"),
                Line::from(active_call.prompt.clone()),
                Line::from(""),
                Line::from("Response"),
                Line::from("[waiting for Apple FM reply]"),
            ]);
            return Text::from(lines);
        }

        if let Some(last_call) = self.setup.calls.last() {
            let mut lines = vec![
                Line::from(format!("Last completed call: {}", last_call.title)),
                Line::from(""),
                Line::from("Response"),
                Line::from(last_call.response_text.clone()),
                Line::from(""),
                Line::from(format!("response_id: {}", last_call.response_id)),
                Line::from(format!("model: {}", last_call.response_model)),
            ];
            lines.extend(last_call.usage.render_lines());
            return Text::from(lines);
        }

        match self.setup.phase {
            TaskPhase::Queued => Text::from(vec![
                Line::from("Apple FM setup has been queued."),
                Line::from(""),
                Line::from("Probe will check availability before issuing any inference."),
                Line::from("Use Ctrl+R to rerun the setup flow manually."),
            ]),
            TaskPhase::CheckingAvailability => Text::from(vec![
                Line::from("Checking whether Apple FM is available on this machine."),
                Line::from(""),
                Line::from(
                    "No inference requests will be issued until the availability gate passes.",
                ),
            ]),
            TaskPhase::Unavailable => Text::from(vec![
                Line::from("Apple FM is not ready on this machine right now."),
                Line::from(""),
                Line::from(
                    self.setup
                        .availability
                        .as_ref()
                        .and_then(|availability| availability.availability_message.clone())
                        .unwrap_or_else(|| {
                            String::from("The bridge did not provide extra availability detail.")
                        }),
                ),
                Line::from(""),
                Line::from("Press Ctrl+R to rerun the setup check after the machine is admitted."),
            ]),
            TaskPhase::Completed => Text::from(vec![
                Line::from("Apple FM setup completed successfully."),
                Line::from(""),
                Line::from("The transcript shell remains the primary home screen."),
                Line::from("This tab keeps backend setup details reachable."),
            ]),
            TaskPhase::Idle | TaskPhase::Running | TaskPhase::Failed => Text::from(vec![
                Line::from("Probe setup surface is ready."),
                Line::from(""),
                Line::from("Press Ctrl+R to start or rerun the Apple FM setup flow."),
            ]),
        }
    }

    fn render_status_lines(&self, focus_name: &str, stack_depth: usize) -> Vec<String> {
        let progress = if let Some(active_call) = &self.setup.active_call {
            format!("call: {}/{}", active_call.index, active_call.total_calls)
        } else {
            format!("calls_done: {}", self.setup.calls.len())
        };
        let availability_ready = match self.setup.availability.as_ref() {
            Some(availability) => availability.ready.to_string(),
            None => match self.setup.phase {
                TaskPhase::Failed => String::from("failed"),
                TaskPhase::Unavailable => String::from("false"),
                TaskPhase::Idle => String::from("idle"),
                _ => String::from("pending"),
            },
        };
        vec![
            format!("phase: {}", self.render_phase_label()),
            progress,
            format!("focus: {focus_name}"),
            format!("stack_depth: {stack_depth}"),
            format!("availability_ready: {availability_ready}"),
        ]
    }

    fn render_backend_lines(&self) -> Vec<String> {
        let Some(backend) = &self.setup.backend else {
            return vec![
                String::from("profile: pending"),
                String::from("base_url: pending"),
                String::from("model: pending"),
            ];
        };
        vec![
            format!("profile: {}", backend.profile_name),
            format!("base_url: {}", backend.base_url),
            format!("model: {}", backend.model_id),
        ]
    }

    fn render_availability_lines(&self) -> Vec<String> {
        let Some(availability) = &self.setup.availability else {
            if let Some(failure) = &self.setup.failure {
                return vec![
                    String::from("ready: failed"),
                    format!(
                        "reason: {}",
                        failure
                            .reason_code
                            .clone()
                            .unwrap_or_else(|| String::from("transport_or_unknown"))
                    ),
                    format!("message: {}", preview(failure.detail.as_str(), 64)),
                ];
            }
            return vec![
                String::from("ready: pending"),
                String::from("reason: pending"),
                String::from("message: waiting for /health"),
            ];
        };
        vec![
            format!("ready: {}", availability.ready),
            format!(
                "reason: {}",
                availability
                    .unavailable_reason
                    .clone()
                    .unwrap_or_else(|| String::from("none"))
            ),
            format!(
                "platform: {}",
                availability
                    .platform
                    .clone()
                    .unwrap_or_else(|| String::from("unknown"))
            ),
            format!(
                "version: {}",
                availability
                    .version
                    .clone()
                    .unwrap_or_else(|| String::from("unknown"))
            ),
            format!(
                "message: {}",
                availability
                    .availability_message
                    .clone()
                    .unwrap_or_else(|| String::from("none"))
            ),
        ]
    }

    fn render_codex_auth_lines(&self) -> Vec<String> {
        let Some(probe_home) = self.probe_home.as_ref() else {
            return vec![
                String::from("status: unavailable"),
                String::from("reason: no probe_home configured for this lane"),
            ];
        };
        let controller = match OpenAiCodexAuthController::new(probe_home) {
            Ok(controller) => controller,
            Err(error) => {
                return vec![
                    String::from("status: unavailable"),
                    format!("reason: failed to load Codex auth controller: {error}"),
                ];
            }
        };
        let route_summary = codex_route_summary(probe_home.as_path());
        let api_key_source = current_openai_api_key_source();
        match controller.status() {
            Ok(status) if status.authenticated => {
                let mut lines = vec![
                    String::from("status: connected"),
                    format!("path: {}", status.path.display()),
                    format!("accounts: {}", status.account_count),
                    format!("expired: {}", status.expired),
                    format!(
                        "account_id: {}",
                        status.account_id.as_deref().unwrap_or("none")
                    ),
                    format!(
                        "selected: {}",
                        status
                            .selected_account_label
                            .as_deref()
                            .or(status.selected_account_email.as_deref())
                            .unwrap_or_else(|| {
                                status.selected_account_key.as_deref().unwrap_or("none")
                            })
                    ),
                ];
                if let Some(route_summary) = route_summary.as_ref() {
                    lines.push(format!("selected_route: {}", route_summary.route_label));
                    if route_summary.api_key_fallback_available {
                        lines.push(String::from("api_key_fallback: available"));
                    }
                }
                if let Some(source) = api_key_source.as_deref() {
                    lines.push(format!("api_key_source: {source}"));
                }
                lines.push(String::from(
                    "manage: `probe codex status` / `probe codex logout`",
                ));
                lines
            }
            Ok(status) => {
                let mut lines = vec![
                    String::from("status: disconnected"),
                    format!("path: {}", status.path.display()),
                ];
                if let Some(route_summary) = route_summary.as_ref() {
                    lines.push(format!("selected_route: {}", route_summary.route_label));
                    if route_summary.api_key_fallback_available {
                        lines.push(String::from("api_key_fallback: available"));
                    }
                }
                if let Some(source) = api_key_source.as_deref() {
                    lines.push(format!("api_key_source: {source}"));
                }
                lines.push(String::from(
                    "connect: `probe codex login --method browser`",
                ));
                lines.push(String::from(
                    "headless: `probe codex login --method headless`",
                ));
                lines
            }
            Err(error) => vec![String::from("status: error"), format!("detail: {error}")],
        }
    }

    fn refresh_codex_route_badge(&mut self) {
        self.codex_route_badge = self
            .operator_backend
            .as_ref()
            .filter(|summary| {
                summary.backend_kind
                    == probe_protocol::backend::BackendKind::OpenAiCodexSubscription
            })
            .and_then(|_| {
                self.probe_home.as_deref().and_then(|probe_home| {
                    codex_route_summary(probe_home).map(|summary| summary.footer_badge)
                })
            })
            .flatten();
    }

    fn render_phase_label(&self) -> &'static str {
        match self.setup.phase {
            TaskPhase::Idle => "idle",
            TaskPhase::Queued => "queued",
            TaskPhase::CheckingAvailability => "checking",
            TaskPhase::Unavailable => "unavailable",
            TaskPhase::Running => "running",
            TaskPhase::Completed => "completed",
            TaskPhase::Failed => "failed",
        }
    }

    fn resume_transcript_follow(&mut self) {
        self.transcript_follow_latest = true;
        self.transcript_scroll_from_bottom = 0;
    }

    fn sync_transcript_scroll_after_update(&mut self) {
        let line_count = self.render_primary_body().lines.len();
        if self.transcript_follow_latest {
            self.transcript_scroll_from_bottom = 0;
        } else if line_count > self.transcript_line_count {
            let added = line_count.saturating_sub(self.transcript_line_count);
            self.transcript_scroll_from_bottom = self
                .transcript_scroll_from_bottom
                .saturating_add(added.min(u16::MAX as usize) as u16)
                .min(self.max_transcript_scroll_from_bottom_for_line_count(line_count));
        } else {
            self.transcript_scroll_from_bottom = self
                .transcript_scroll_from_bottom
                .min(self.max_transcript_scroll_from_bottom_for_line_count(line_count));
        }
        self.transcript_line_count = line_count;
    }

    fn snap_transcript_to_latest(&mut self) {
        self.sync_transcript_scroll_after_update();
    }

    fn scroll_transcript_up(&mut self, amount: u16) {
        let max = self.max_transcript_scroll_from_bottom();
        self.transcript_scroll_from_bottom = self
            .transcript_scroll_from_bottom
            .saturating_add(amount)
            .min(max);
        self.transcript_follow_latest = self.transcript_scroll_from_bottom == 0;
        self.transcript_line_count = self.render_primary_body().lines.len();
    }

    fn scroll_transcript_down(&mut self, amount: u16) {
        self.transcript_scroll_from_bottom =
            self.transcript_scroll_from_bottom.saturating_sub(amount);
        self.transcript_follow_latest = self.transcript_scroll_from_bottom == 0;
        self.transcript_line_count = self.render_primary_body().lines.len();
    }

    fn max_transcript_scroll_from_bottom_for_line_count(&self, line_count: usize) -> u16 {
        line_count.saturating_sub(1).min(u16::MAX as usize) as u16
    }

    fn max_transcript_scroll_from_bottom(&self) -> u16 {
        self.max_transcript_scroll_from_bottom_for_line_count(
            self.render_primary_body().lines.len(),
        )
    }

    fn transcript_scroll_y(&self, line_count: usize, panel_height: u16) -> u16 {
        let viewport_height = panel_height as usize;
        let max_top_scroll = line_count.saturating_sub(viewport_height);
        let from_bottom = usize::from(
            self.transcript_scroll_from_bottom
                .min(max_top_scroll.min(u16::MAX as usize) as u16),
        );
        max_top_scroll
            .saturating_sub(from_bottom)
            .min(u16::MAX as usize) as u16
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpScreen;

impl HelpScreen {
    pub fn new() -> Self {
        Self
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::Dismiss | UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, stack_depth: usize) {
        let content = Paragraph::new(Text::from(vec![
            Line::from("Probe Chat Shell Keys"),
            Line::from(""),
            Line::from("Plain Enter sends · Shift/Ctrl/Opt+Enter or Ctrl+J newline"),
            Line::from("Up / Down           draft history recall"),
            Line::from("Mouse wheel / PgUp  scroll active panel"),
            Line::from("PgDn                scroll back toward latest"),
            Line::from("Ctrl+O              add attachment placeholder"),
            Line::from("Ctrl+R / Ctrl+S     backend check / backend overlay"),
            Line::from("Ctrl+A              approval"),
            Line::from("Ctrl+T              toggle operator notes"),
            Line::from("F1 / Esc            toggle or dismiss help"),
            Line::from("Ctrl+C              quit"),
            Line::from(""),
            Line::from("Local slash commands"),
            Line::from("/help /backend /approvals /reasoning /clear"),
            Line::from("Backend choice on the default TUI path is automatic and Codex-first."),
            Line::from(
                "Slash commands, typed mentions, attachments, and paste state live in the draft model.",
            ),
            Line::from(format!("Current stack depth: {stack_depth}")),
        ]));
        ModalCard::new("Help", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupOverlay;

impl SetupOverlay {
    pub fn new() -> Self {
        Self
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::Dismiss | UiEvent::OpenSetupOverlay => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed backend overlay"),
            ),
            UiEvent::RunBackgroundTask => ScreenOutcome::with_command(
                String::from("requested backend check"),
                ScreenCommand::RunAppleFmSetup,
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        stack_depth: usize,
        base_screen: &ChatScreen,
    ) {
        let content = Paragraph::new(base_screen.render_setup_overlay_text(stack_depth));
        ModalCard::new("Backend", content).render(frame, area);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalChoice {
    Approve,
    Reject,
}

impl ApprovalChoice {
    fn next(self) -> Self {
        match self {
            Self::Approve => Self::Reject,
            Self::Reject => Self::Approve,
        }
    }

    fn previous(self) -> Self {
        self.next()
    }

    fn label(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalOverlay {
    selected: ApprovalChoice,
    approval: PendingToolApproval,
}

impl ApprovalOverlay {
    pub fn new(approval: PendingToolApproval) -> Self {
        Self {
            selected: ApprovalChoice::Approve,
            approval,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::NextView => {
                self.selected = self.selected.next();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {} in approval overlay", self.selected.label()),
                )
            }
            UiEvent::PreviousView => {
                self.selected = self.selected.previous();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {} in approval overlay", self.selected.label()),
                )
            }
            UiEvent::ComposerSubmit => ScreenOutcome::with_action_and_command(
                ScreenAction::CloseModal,
                format!(
                    "queued {} for pending tool {}",
                    self.selected.label(),
                    self.approval.tool_name
                ),
                ScreenCommand::ResolvePendingToolApproval {
                    session_id: self.approval.session_id.as_str().to_string(),
                    call_id: self.approval.tool_call_id.clone(),
                    resolution: match self.selected {
                        ApprovalChoice::Approve => ToolApprovalResolution::Approved,
                        ApprovalChoice::Reject => ToolApprovalResolution::Rejected,
                    },
                },
            ),
            UiEvent::Dismiss | UiEvent::OpenApprovalOverlay => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed approval overlay"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, stack_depth: usize) {
        let approve_marker = if self.selected == ApprovalChoice::Approve {
            ">"
        } else {
            " "
        };
        let reject_marker = if self.selected == ApprovalChoice::Reject {
            ">"
        } else {
            " "
        };
        let mut lines = vec![
            Line::from(format!("tool: {}", self.approval.tool_name)),
            Line::from(format!(
                "call: {}",
                preview(self.approval.tool_call_id.as_str(), 24)
            )),
            Line::from(format!(
                "risk: {}",
                render_runtime_risk_class(self.approval.risk_class)
            )),
            Line::from(format!(
                "requested_turn: {}",
                self.approval.tool_call_turn_index
            )),
            Line::from(format!(
                "paused_turn: {}",
                self.approval.paused_result_turn_index
            )),
            Line::from(format!(
                "reason: {}",
                self.approval
                    .reason
                    .as_deref()
                    .unwrap_or("pending operator decision")
            )),
            Line::from(""),
            Line::from("arguments"),
        ];
        for line in compact_json_lines(&self.approval.arguments, 5) {
            lines.push(Line::from(format!("  {line}")));
        }
        lines.extend([
            Line::from(""),
            Line::from(format!("{approve_marker} Approve the action")),
            Line::from(format!("{reject_marker} Reject the action")),
            Line::from(""),
            Line::from("Tab/Shift+Tab changes selection. Enter resolves. Esc dismisses."),
            Line::from(format!("Current stack depth: {stack_depth}")),
        ]);
        let content = Paragraph::new(Text::from(lines));
        ModalCard::new("Approval", content).render(frame, area);
    }
}

fn render_stream_active_turn(
    stream: &AssistantStreamState,
    operator_backend: Option<&ServerOperatorSummary>,
) -> ActiveTurn {
    let display_text = normalize_openai_stream_display_text(stream.assistant_text.as_str());
    let failure = stream
        .failure
        .as_deref()
        .map(|error| classify_runtime_failure(error, operator_backend));
    let is_waiting = display_text.is_empty() && stream.tool_calls.is_empty() && failure.is_none();
    let mut body = Vec::new();
    if let Some(summary) = failure.as_ref() {
        body.extend(summary.body_lines());
    }

    if !stream.tool_calls.is_empty() {
        for tool in &stream.tool_calls {
            body.push(format!(
                "{} {}",
                tool.tool_index + 1,
                tool.tool_name.as_deref().unwrap_or("unknown")
            ));
            if let Some(call_id) = tool.call_id.as_deref() {
                body.push(format!("call: {}", preview(call_id, 48)));
            }
            if !tool.arguments.is_empty() {
                body.push(format!("args: {}", tool.arguments));
            }
        }
    }

    if !display_text.is_empty() {
        if failure.is_some() {
            body.push(String::from("partial_response:"));
        }
        body.extend(split_text_lines(display_text.as_str()));
    }

    let role = if is_waiting {
        TranscriptRole::Assistant
    } else if display_text.is_empty() && !stream.tool_calls.is_empty() {
        TranscriptRole::Tool
    } else if failure.is_some() {
        TranscriptRole::Status
    } else {
        TranscriptRole::Assistant
    };
    let title = if is_waiting {
        "Waiting for Reply"
    } else if let Some(summary) = failure.as_ref() {
        summary.title
    } else if display_text.is_empty() && !stream.tool_calls.is_empty() {
        "Streaming Tool Call"
    } else {
        "Probe"
    };
    ActiveTurn::new(role, title, body)
}

impl AppleFmUsageSummary {
    fn render_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if self.total_tokens.is_some() {
            lines.push(Line::from(format!(
                "usage_total: {}",
                render_usage_value(self.total_tokens, self.total_truth.as_deref())
            )));
        }
        if self.prompt_tokens.is_some() {
            lines.push(Line::from(format!(
                "usage_prompt: {}",
                render_usage_value(self.prompt_tokens, self.prompt_truth.as_deref())
            )));
        }
        if self.completion_tokens.is_some() {
            lines.push(Line::from(format!(
                "usage_completion: {}",
                render_usage_value(self.completion_tokens, self.completion_truth.as_deref())
            )));
        }
        lines
    }
}

fn runtime_tool_body_lines(
    round_trip: usize,
    tool: &probe_core::tools::ExecutedToolCall,
) -> Vec<String> {
    let _ = round_trip;
    match tool.tool_execution.policy_decision {
        probe_protocol::session::ToolPolicyDecision::Paused => vec![
            runtime_tool_subject(tool),
            format!(
                "needs approval: {}",
                compact_runtime_policy_reason(
                    tool.tool_execution.reason.as_deref(),
                    tool.name.as_str()
                )
            ),
        ],
        probe_protocol::session::ToolPolicyDecision::Refused => vec![
            runtime_tool_subject(tool),
            format!(
                "blocked: {}",
                compact_runtime_policy_reason(
                    tool.tool_execution.reason.as_deref(),
                    tool.name.as_str()
                )
            ),
        ],
        probe_protocol::session::ToolPolicyDecision::AutoAllow
        | probe_protocol::session::ToolPolicyDecision::Approved => {
            compact_runtime_tool_output_lines(tool)
        }
    }
}

fn runtime_tool_call_entry(tool_name: &str, arguments: &serde_json::Value) -> TranscriptEntry {
    TranscriptEntry::tool_call(
        tool_name.to_string(),
        vec![runtime_tool_argument_summary(arguments)],
    )
}

fn runtime_tool_result_entry(
    round_trip: usize,
    tool: &probe_core::tools::ExecutedToolCall,
) -> TranscriptEntry {
    match tool.tool_execution.policy_decision {
        probe_protocol::session::ToolPolicyDecision::Paused => TranscriptEntry::approval_pending(
            tool.name.clone(),
            runtime_tool_body_lines(round_trip, tool),
        ),
        probe_protocol::session::ToolPolicyDecision::Refused => TranscriptEntry::tool_refused(
            tool.name.clone(),
            runtime_tool_body_lines(round_trip, tool),
        ),
        probe_protocol::session::ToolPolicyDecision::AutoAllow
        | probe_protocol::session::ToolPolicyDecision::Approved => TranscriptEntry::tool_result(
            tool.name.clone(),
            runtime_tool_body_lines(round_trip, tool),
        ),
    }
}

fn compact_json_lines(value: &serde_json::Value, max_lines: usize) -> Vec<String> {
    let mut lines = serde_json::to_string_pretty(value)
        .unwrap_or_else(|_| value.to_string())
        .lines()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        lines.push(String::from("..."));
    }
    lines
}

fn compact_text_lines(value: &str, max_lines: usize) -> Vec<String> {
    let mut lines = value.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        lines.push(String::from("..."));
    }
    lines
}

fn runtime_tool_subject(tool: &probe_core::tools::ExecutedToolCall) -> String {
    tool.tool_execution
        .command
        .as_ref()
        .cloned()
        .unwrap_or_else(|| runtime_tool_argument_summary(&tool.arguments))
}

fn runtime_tool_argument_summary(arguments: &serde_json::Value) -> String {
    if let Some(command) = arguments.get("command").and_then(serde_json::Value::as_str) {
        return command.to_string();
    }
    if let Some(path) = arguments.get("path").and_then(serde_json::Value::as_str) {
        if let Some(pattern) = arguments.get("pattern").and_then(serde_json::Value::as_str) {
            return format!("{pattern} in {path}");
        }
        if let Some(start_line) = arguments
            .get("start_line")
            .and_then(serde_json::Value::as_u64)
            && let Some(end_line) = arguments
                .get("end_line")
                .and_then(serde_json::Value::as_u64)
        {
            return format!("{path}:{start_line}-{end_line}");
        }
        return path.to_string();
    }
    if let Some(question) = arguments
        .get("question")
        .and_then(serde_json::Value::as_str)
    {
        return preview(question, 72);
    }
    preview(
        serde_json::to_string(arguments)
            .unwrap_or_else(|_| arguments.to_string())
            .as_str(),
        72,
    )
}

fn compact_runtime_tool_output_lines(tool: &probe_core::tools::ExecutedToolCall) -> Vec<String> {
    if let Some(mut lines) = structured_runtime_tool_output_lines(&tool.output) {
        let subject = runtime_tool_subject(tool);
        if lines
            .first()
            .is_some_and(|first| first.as_str() == subject.as_str())
        {
            lines.remove(0);
        }
        if !lines.is_empty() {
            return lines;
        }
    }

    let subject = runtime_tool_subject(tool);
    let summary = preview(
        tool_result_model_text(tool.name.as_str(), &tool.output).as_str(),
        120,
    );
    let mut lines = vec![subject];
    if lines
        .last()
        .map_or(true, |existing| existing.as_str() != summary.as_str())
    {
        lines.push(summary);
    }
    lines
}

fn structured_runtime_tool_output_lines(value: &serde_json::Value) -> Option<Vec<String>> {
    if let Some(path) = value.get("path").and_then(serde_json::Value::as_str) {
        let start_line = value.get("start_line").and_then(serde_json::Value::as_u64);
        let end_line = value.get("end_line").and_then(serde_json::Value::as_u64);
        let mut lines = vec![match (start_line, end_line) {
            (Some(start), Some(end)) => format!("{path}:{start}-{end}"),
            _ => path.to_string(),
        }];
        if let Some(content) = value.get("content").and_then(serde_json::Value::as_str) {
            lines.extend(compact_text_lines(content, 4));
        }
        return Some(lines);
    }
    if let Some(command) = value.get("command").and_then(serde_json::Value::as_str) {
        let mut lines = vec![command.to_string()];
        if let Some(stdout) = value.get("stdout").and_then(serde_json::Value::as_str)
            && !stdout.trim().is_empty()
        {
            lines.extend(compact_text_lines(stdout, 4));
            return Some(lines);
        }
        if let Some(stderr) = value.get("stderr").and_then(serde_json::Value::as_str)
            && !stderr.trim().is_empty()
        {
            lines.extend(compact_text_lines(stderr, 4));
            return Some(lines);
        }
        return Some(lines);
    }
    if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
        return Some(vec![format!("error: {}", preview(error, 96))]);
    }
    if let Some(entries) = value.get("entries").and_then(serde_json::Value::as_array) {
        let mut lines = vec![format!("listed {} entries", entries.len())];
        for entry in entries.iter().take(4).filter_map(serde_json::Value::as_str) {
            lines.push(entry.to_string());
        }
        return Some(lines);
    }
    if let Some(matches) = value.get("matches").and_then(serde_json::Value::as_array) {
        let mut lines = vec![format!("found {} matches", matches.len())];
        for summary in matches.iter().take(3).filter_map(|entry| {
            let path = entry.get("path").and_then(serde_json::Value::as_str)?;
            let line = entry.get("line").and_then(serde_json::Value::as_u64)?;
            Some(format!("{path}:{line}"))
        }) {
            lines.push(summary);
        }
        return Some(lines);
    }
    if let Some(answer) = value
        .get("oracle_answer")
        .and_then(serde_json::Value::as_str)
    {
        return Some(compact_text_lines(answer, 4));
    }
    if let Some(analysis) = value.get("analysis").and_then(serde_json::Value::as_str) {
        return Some(compact_text_lines(analysis, 4));
    }
    None
}

fn compact_runtime_policy_reason(reason: Option<&str>, tool_name: &str) -> String {
    let fallback = "approval required";
    let value = reason.unwrap_or(fallback);
    let prefix = format!("tool `{tool_name}` requires ");
    if let Some(stripped) = value.strip_prefix(prefix.as_str()) {
        return stripped.to_string();
    }
    if value == "tool execution blocked by local approval policy" {
        return fallback.to_string();
    }
    value.to_string()
}

fn render_remote_contract_lines(summary: &ServerOperatorSummary) -> Vec<&'static str> {
    match summary.backend_kind {
        probe_protocol::backend::BackendKind::OpenAiCodexSubscription => vec![
            "model inference is sent to ChatGPT's hosted Codex backend",
            "the local OpenAI subscription token is attached from Probe auth state",
            "tool execution, approvals, transcripts, and the TUI remain local to Probe",
        ],
        probe_protocol::backend::BackendKind::OpenAiChatCompletions
            if matches!(
                summary.control_plane,
                Some(probe_protocol::backend::BackendControlPlaneKind::PsionicInferenceMesh)
            ) =>
        {
            vec![
                "Probe discovers routed inventory from Psionic mesh management state before attach",
                "mesh startup, warmup, and route execution remain owned by Psionic",
                "tool execution, approvals, transcripts, and the TUI remain local to Probe",
            ]
        }
        probe_protocol::backend::BackendKind::OpenAiChatCompletions
        | probe_protocol::backend::BackendKind::AppleFmBridge => vec![
            "only model inference is delegated to the backend target",
            "tool execution, approvals, transcripts, and the TUI remain local to Probe",
            "switch targets through saved backend configs or `probe server`",
        ],
    }
}

fn render_backend_kind(value: probe_protocol::backend::BackendKind) -> &'static str {
    match value {
        probe_protocol::backend::BackendKind::OpenAiChatCompletions => "openai_chat_completions",
        probe_protocol::backend::BackendKind::OpenAiCodexSubscription => {
            "openai_codex_subscription"
        }
        probe_protocol::backend::BackendKind::AppleFmBridge => "apple_fm_bridge",
    }
}

fn display_reasoning_level(value: &str) -> &str {
    match value {
        "backend_default" => "auto",
        other => other,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexRouteSummary {
    route_label: String,
    footer_badge: Option<String>,
    api_key_fallback_available: bool,
}

fn codex_route_summary(probe_home: &std::path::Path) -> Option<CodexRouteSummary> {
    let controller = OpenAiCodexAuthController::new(probe_home).ok()?;
    let plan = controller.routing_plan(Some(OPENAI_API_KEY_ENV)).ok()?;
    let route_label = match plan.routes.first() {
        Some(OpenAiCodexRoute::SubscriptionAccount(_)) => String::from("subscription"),
        Some(OpenAiCodexRoute::ApiKeyFallback(_)) => String::from("api key"),
        None => String::from("none"),
    };
    let footer_badge = matches!(
        plan.routes.first(),
        Some(OpenAiCodexRoute::ApiKeyFallback(_))
    )
    .then(|| String::from("api key"));
    Some(CodexRouteSummary {
        route_label,
        footer_badge,
        api_key_fallback_available: plan.api_key_fallback_available,
    })
}

fn current_openai_api_key_source() -> Option<&'static str> {
    let source = std::env::var(OPENAI_API_KEY_SOURCE_ENV).ok()?;
    if source.starts_with("workspace_secret:") {
        Some("workspace secret")
    } else if source.starts_with("env:") {
        Some("env")
    } else {
        Some("configured")
    }
}

fn status_marker(screen: &ChatScreen) -> char {
    if screen.pending_tool_approval_count() > 0 {
        return '!';
    }

    let phase = screen.runtime.phase.as_deref();
    if matches!(
        screen.setup.phase,
        TaskPhase::Failed | TaskPhase::Unavailable
    ) || matches!(phase, Some("model_request_failed"))
    {
        return 'x';
    }

    if matches!(
        screen.setup.phase,
        TaskPhase::Queued | TaskPhase::CheckingAvailability | TaskPhase::Running
    ) || matches!(
        phase,
        Some(
            "turn_started"
                | "model_request"
                | "assistant_streaming"
                | "assistant_snapshot_streaming"
                | "assistant_snapshot"
                | "tool_call_streaming"
        )
    ) {
        return '*';
    }

    '.'
}

fn render_runtime_risk_class(value: probe_protocol::session::ToolRiskClass) -> &'static str {
    match value {
        probe_protocol::session::ToolRiskClass::ReadOnly => "read_only",
        probe_protocol::session::ToolRiskClass::ShellReadOnly => "shell_read_only",
        probe_protocol::session::ToolRiskClass::Write => "write",
        probe_protocol::session::ToolRiskClass::Network => "network",
        probe_protocol::session::ToolRiskClass::Destructive => "destructive",
    }
}

fn render_usage_value(value: Option<u64>, truth: Option<&str>) -> String {
    match (value, truth) {
        (Some(value), Some(truth)) => format!("{value} ({truth})"),
        (Some(value), None) => value.to_string(),
        (None, _) => String::from("n/a"),
    }
}

fn split_text_lines(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    value.split('\n').map(ToOwned::to_owned).collect()
}

fn short_session_id(value: &str) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(16).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

fn preview(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let preview = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}
