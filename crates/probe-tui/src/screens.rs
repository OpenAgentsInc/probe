use std::collections::VecDeque;

use probe_core::runtime::RuntimeEvent;
use probe_protocol::session::{PendingToolApproval, ToolApprovalResolution};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::bottom_pane::ComposerSubmission;
use crate::event::UiEvent;
use crate::message::{
    AppMessage, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
    AppleFmFailureSummary, AppleFmUsageSummary,
};
use crate::transcript::{ActiveTurn, RetainedTranscript, TranscriptEntry, TranscriptRole};
use crate::widgets::{InfoPanel, ModalCard, TabStrip};

const MAX_EVENT_LOG: usize = 16;
const LINE_SCROLL_STEP: u16 = 3;
const PAGE_SCROLL_STEP: u16 = 12;

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
            Self::SetupOverlay => "setup overlay",
            Self::ApprovalOverlay => "approval overlay",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Chat,
    Events,
}

impl ActiveTab {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Events => "Events",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Chat => Self::Events,
            Self::Events => Self::Chat,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Chat => Self::Events,
            Self::Events => Self::Chat,
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
    phase: Option<String>,
    round_trip: Option<usize>,
    active_tool: Option<String>,
    pending_approvals: Vec<PendingToolApproval>,
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
    events_scroll: u16,
    runtime: ProbeRuntimeState,
    setup: AppleFmSetupState,
}

impl Default for ChatScreen {
    fn default() -> Self {
        let mut screen = Self {
            active_tab: ActiveTab::Chat,
            emphasized_copy: false,
            recent_events: VecDeque::new(),
            task_events: VecDeque::new(),
            transcript: RetainedTranscript::new(),
            transcript_scroll_from_bottom: 0,
            events_scroll: 0,
            runtime: ProbeRuntimeState::default(),
            setup: AppleFmSetupState::default(),
        };
        screen.record_event("probe tui ready");
        screen.record_event("press Ctrl+R to rerun Apple FM setup");
        screen.record_event("press F1 for help");
        screen.record_event("press Tab or Shift+Tab to switch views");
        screen
    }
}

impl ChatScreen {
    pub fn active_tab(&self) -> ActiveTab {
        self.active_tab
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
        self.transcript.push_entry(TranscriptEntry::new(
            TranscriptRole::User,
            "You",
            body,
        ));
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
        self.events_scroll = 0;
        self.record_worker_event(format!(
            "queued Apple FM setup against {}",
            backend.profile_name
        ));
    }

    fn apply_runtime_event(&mut self, event: RuntimeEvent) -> String {
        match event {
            RuntimeEvent::TurnStarted {
                session_id,
                profile_name,
                prompt,
                tool_loop_enabled,
            } => {
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.profile_name = Some(profile_name.clone());
                self.runtime.phase = Some(String::from("turn_started"));
                self.runtime.round_trip = None;
                self.runtime.active_tool = None;
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Assistant,
                    "Probe Runtime",
                    vec![
                        format!("profile: {profile_name}"),
                        format!("tool_loop: {}", if tool_loop_enabled { "enabled" } else { "disabled" }),
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
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.phase = Some(String::from("model_request"));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = None;
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Assistant,
                    "Model Request",
                    vec![
                        format!("backend: {:?}", backend_kind),
                        format!("round_trip: {round_trip}"),
                        format!("session: {}", short_session_id(session_id.as_str())),
                    ],
                ));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("model request started: round {round_trip}"));
                format!("model request started for round {round_trip}")
            }
            RuntimeEvent::ToolCallRequested {
                session_id,
                round_trip,
                call_id,
                tool_name,
                arguments,
            } => {
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.phase = Some(String::from("tool_requested"));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool_name.clone());
                let mut body = vec![
                    format!("round_trip: {round_trip}"),
                    format!("call_id: {call_id}"),
                    String::from("arguments"),
                ];
                body.extend(compact_json_lines(&arguments, 5));
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Tool,
                    format!("Tool Requested: {tool_name}"),
                    body,
                ));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("tool call requested: {tool_name}"));
                format!("tool call requested: {tool_name}")
            }
            RuntimeEvent::ToolExecutionStarted {
                session_id,
                round_trip,
                call_id,
                tool_name,
                risk_class,
            } => {
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.phase = Some(String::from("tool_running"));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool_name.clone());
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Tool,
                    format!("Running Tool: {tool_name}"),
                    vec![
                        format!("round_trip: {round_trip}"),
                        format!("call_id: {call_id}"),
                        format!("risk_class: {}", render_runtime_risk_class(risk_class)),
                    ],
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
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.phase = Some(String::from("tool_completed"));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool.name.clone());
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Tool,
                    format!("Completed Tool: {}", tool.name),
                    runtime_tool_body_lines(round_trip, &tool),
                ));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("tool execution completed: {}", tool.name));
                format!("tool execution completed: {}", tool.name)
            }
            RuntimeEvent::ToolRefused {
                session_id,
                round_trip,
                tool,
            } => {
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.phase = Some(String::from("tool_refused"));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool.name.clone());
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Status,
                    format!("Tool Refused: {}", tool.name),
                    runtime_tool_body_lines(round_trip, &tool),
                ));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("tool refused: {}", tool.name));
                format!("tool refused: {}", tool.name)
            }
            RuntimeEvent::ToolPaused {
                session_id,
                round_trip,
                tool,
            } => {
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.phase = Some(String::from("tool_paused"));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool.name.clone());
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Status,
                    format!("Approval Pending: {}", tool.name),
                    runtime_tool_body_lines(round_trip, &tool),
                ));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("tool paused for approval: {}", tool.name));
                format!("tool paused for approval: {}", tool.name)
            }
            RuntimeEvent::AssistantTurnCommitted {
                session_id,
                response_id,
                response_model,
                assistant_text,
            } => {
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.phase = Some(String::from("assistant_committed"));
                self.runtime.active_tool = None;
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Assistant,
                    "Probe",
                    vec![
                        format!("response_id: {response_id}"),
                        format!("model: {}", preview(response_model.as_str(), 48)),
                        preview(assistant_text.as_str(), 96),
                    ],
                ));
                self.snap_transcript_to_latest();
                self.record_worker_event(String::from("assistant turn committed"));
                String::from("assistant turn committed")
            }
        }
    }

    pub fn apply_message(&mut self, message: AppMessage) -> String {
        match message {
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
                self.transcript.clear_active_turn();
                for entry in entries {
                    let label = entry.label().to_string();
                    let title = entry.title().to_string();
                    self.transcript.push_entry(entry);
                    self.record_worker_event(format!("committed {label} row: {title}"));
                }
                self.snap_transcript_to_latest();
                format!("committed {entry_count} transcript entries")
            }
            AppMessage::TranscriptEntryCommitted { entry } => {
                let label = entry.label().to_string();
                let title = entry.title().to_string();
                self.transcript.clear_active_turn();
                self.transcript.push_entry(entry);
                self.snap_transcript_to_latest();
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
                    phase: self.runtime.phase.clone(),
                    round_trip: self.runtime.round_trip,
                    active_tool: self.runtime.active_tool.clone(),
                    pending_approvals: self.runtime.pending_approvals.clone(),
                };
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
            UiEvent::NextView => {
                self.active_tab = self.active_tab.next();
                let status = format!("switched to {} view", self.active_tab.title());
                self.record_event(status.clone());
                ScreenOutcome::with_status(ScreenAction::None, status)
            }
            UiEvent::PreviousView => {
                self.active_tab = self.active_tab.previous();
                let status = format!("switched to {} view", self.active_tab.title());
                self.record_event(status.clone());
                ScreenOutcome::with_status(ScreenAction::None, status)
            }
            UiEvent::ToggleBody => {
                self.emphasized_copy = !self.emphasized_copy;
                self.snap_transcript_to_latest();
                let status = if self.emphasized_copy {
                    String::from("showing operator notes instead of live response detail")
                } else {
                    String::from("restored live Apple FM detail view")
                };
                self.record_event(status.clone());
                ScreenOutcome::with_status(ScreenAction::None, status)
            }
            UiEvent::ScrollUp => {
                match self.active_tab {
                    ActiveTab::Chat => self.scroll_transcript_up(LINE_SCROLL_STEP),
                    ActiveTab::Events => self.scroll_events_up(LINE_SCROLL_STEP),
                }
                ScreenOutcome::idle()
            }
            UiEvent::ScrollDown => {
                match self.active_tab {
                    ActiveTab::Chat => self.scroll_transcript_down(LINE_SCROLL_STEP),
                    ActiveTab::Events => self.scroll_events_down(LINE_SCROLL_STEP),
                }
                ScreenOutcome::idle()
            }
            UiEvent::PageUp => {
                match self.active_tab {
                    ActiveTab::Chat => self.scroll_transcript_up(PAGE_SCROLL_STEP),
                    ActiveTab::Events => self.scroll_events_up(PAGE_SCROLL_STEP),
                }
                ScreenOutcome::idle()
            }
            UiEvent::PageDown => {
                match self.active_tab {
                    ActiveTab::Chat => self.scroll_transcript_down(PAGE_SCROLL_STEP),
                    ActiveTab::Events => self.scroll_events_down(PAGE_SCROLL_STEP),
                }
                ScreenOutcome::idle()
            }
            UiEvent::RunBackgroundTask => {
                self.record_event(String::from("requested Apple FM setup rerun"));
                ScreenOutcome::with_command(
                    String::from("queued Apple FM setup rerun and opened setup overlay"),
                    ScreenCommand::RunAppleFmSetup,
                )
            }
            UiEvent::OpenSetupOverlay => ScreenOutcome::with_status(
                ScreenAction::OpenSetupOverlay,
                String::from("opened setup overlay"),
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
        let sections = Layout::vertical([Constraint::Length(3), Constraint::Min(0)])
            .spacing(1)
            .split(area);
        TabStrip::new(self.active_tab).render(frame, sections[0]);

        match self.active_tab {
            ActiveTab::Chat => self.render_chat_shell(frame, sections[1], stack_depth),
            ActiveTab::Events => self.render_events(frame, sections[1], stack_depth),
        }
    }

    fn render_chat_shell(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let body = self.render_primary_body();
        let scroll_y = self.transcript_scroll_y(body.lines.len(), area.height);
        InfoPanel::new("Transcript", body)
            .with_scroll(scroll_y)
            .render(frame, area);
    }

    fn render_setup_overlay_text(&self, stack_depth: usize) -> Text<'static> {
        let mut lines = self
            .render_setup_body()
            .lines
            .iter()
            .map(ToString::to_string)
            .map(Line::from)
            .collect::<Vec<_>>();
        lines.push(Line::from(""));
        lines.push(Line::from("Setup Status"));
        for line in self.render_status_lines("setup overlay", stack_depth) {
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

    fn render_events(&self, frame: &mut Frame<'_>, area: Rect, stack_depth: usize) {
        let rows = Layout::vertical([Constraint::Length(7), Constraint::Min(0)])
            .spacing(1)
            .split(area);
        InfoPanel::new(
            "App Shell Notes",
            Text::from(vec![
                Line::from("AppShell owns terminal lifecycle, dispatch, and worker polling."),
                Line::from("Probe selected a retained transcript widget as the first shell model."),
                Line::from(
                    "Committed entries stay in app state with one explicit active-turn cell.",
                ),
                Line::from(format!("Current stack depth: {stack_depth}")),
            ]),
        )
        .render(frame, rows[0]);

        let columns = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
            .spacing(1)
            .split(rows[1]);
        let ui_body = numbered_event_log(self.recent_events.iter());
        let worker_body = numbered_event_log(self.task_events.iter());
        InfoPanel::new("UI Event Log", ui_body)
            .with_scroll(self.events_scroll)
            .render(frame, columns[0]);
        InfoPanel::new("Apple FM Timeline", worker_body)
            .with_scroll(self.events_scroll)
            .render(frame, columns[1]);
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

    fn snap_transcript_to_latest(&mut self) {
        self.transcript_scroll_from_bottom = 0;
    }

    fn scroll_transcript_up(&mut self, amount: u16) {
        let max = self.max_transcript_scroll_from_bottom();
        self.transcript_scroll_from_bottom = self
            .transcript_scroll_from_bottom
            .saturating_add(amount)
            .min(max);
    }

    fn scroll_transcript_down(&mut self, amount: u16) {
        self.transcript_scroll_from_bottom =
            self.transcript_scroll_from_bottom.saturating_sub(amount);
    }

    fn scroll_events_up(&mut self, amount: u16) {
        self.events_scroll = self.events_scroll.saturating_sub(amount);
    }

    fn scroll_events_down(&mut self, amount: u16) {
        let max = self.max_events_scroll();
        self.events_scroll = self.events_scroll.saturating_add(amount).min(max);
    }

    fn max_transcript_scroll_from_bottom(&self) -> u16 {
        self.render_primary_body()
            .lines
            .len()
            .saturating_sub(1)
            .min(u16::MAX as usize) as u16
    }

    fn max_events_scroll(&self) -> u16 {
        let max_lines = self
            .recent_events
            .len()
            .max(self.task_events.len())
            .saturating_sub(1)
            .min(u16::MAX as usize);
        max_lines as u16
    }

    fn transcript_scroll_y(&self, line_count: usize, panel_height: u16) -> u16 {
        let viewport_height = panel_height.saturating_sub(2) as usize;
        let max_top_scroll = line_count.saturating_sub(viewport_height);
        let from_bottom = usize::from(
            self.transcript_scroll_from_bottom
                .min(max_top_scroll.min(u16::MAX as usize) as u16),
        );
        max_top_scroll.saturating_sub(from_bottom).min(u16::MAX as usize) as u16
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
            Line::from("Tab / Shift+Tab     switch Chat / Events"),
            Line::from("Enter / Ctrl+J      submit / newline"),
            Line::from("Up / Down           draft history recall"),
            Line::from("Mouse wheel / PgUp  scroll active panel"),
            Line::from("PgDn                scroll back toward latest"),
            Line::from("Ctrl+O              add attachment placeholder"),
            Line::from("Ctrl+R / Ctrl+S     rerun setup / open setup"),
            Line::from("Ctrl+A              approval"),
            Line::from("Ctrl+T              toggle operator notes"),
            Line::from("F1 / Esc            toggle or dismiss help"),
            Line::from("Ctrl+C              quit"),
            Line::from("Slash commands, typed mentions, attachments, and paste state live in the draft model."),
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
                String::from("dismissed setup overlay"),
            ),
            UiEvent::RunBackgroundTask => ScreenOutcome::with_command(
                String::from("queued Apple FM setup rerun"),
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
        ModalCard::new("Setup", content).render(frame, area);
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
            Line::from(format!("requested_turn: {}", self.approval.tool_call_turn_index)),
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
    let mut lines = vec![
        format!("round_trip: {round_trip}"),
        format!("call_id: {}", tool.call_id),
        format!(
            "risk_class: {}",
            render_runtime_risk_class(tool.tool_execution.risk_class)
        ),
    ];
    if let Some(reason) = tool.tool_execution.reason.as_ref() {
        lines.push(format!("reason: {reason}"));
    }
    if !tool.tool_execution.files_touched.is_empty() {
        lines.push(format!(
            "files_touched: {}",
            tool.tool_execution.files_touched.join(", ")
        ));
    }
    lines.push(String::from("output"));
    lines.extend(compact_json_lines(&tool.output, 5));
    lines
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

fn numbered_event_log<'a>(items: impl Iterator<Item = &'a String>) -> Text<'static> {
    let lines = items
        .enumerate()
        .map(|(index, message)| Line::from(format!("{:>2}. {}", index + 1, message)))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return Text::from(vec![
            Line::from("No events yet."),
            Line::from(""),
            Line::from("Worker and UI events will accumulate here."),
        ]);
    }
    Text::from(lines)
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
