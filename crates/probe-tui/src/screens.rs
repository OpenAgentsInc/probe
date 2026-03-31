use std::collections::VecDeque;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::event::UiEvent;
use crate::message::{
    AppMessage, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
    AppleFmFailureSummary, AppleFmUsageSummary,
};
use crate::widgets::{InfoPanel, ModalCard, SidebarPanel, TabStrip, padded_title};

const MAX_EVENT_LOG: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenId {
    Hello,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Overview,
    Events,
}

impl ActiveTab {
    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Events => "Events",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Overview => Self::Events,
            Self::Events => Self::Overview,
        }
    }

    fn previous(self) -> Self {
        self.next()
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenAction {
    None,
    OpenHelp,
    CloseModal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenCommand {
    RunAppleFmSetup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenOutcome {
    pub action: ScreenAction,
    pub status: Option<String>,
    pub command: Option<ScreenCommand>,
}

impl ScreenOutcome {
    pub const fn idle() -> Self {
        Self {
            action: ScreenAction::None,
            status: None,
            command: None,
        }
    }

    fn with_status(action: ScreenAction, status: String) -> Self {
        Self {
            action,
            status: Some(status),
            command: None,
        }
    }

    fn with_command(status: String, command: ScreenCommand) -> Self {
        Self {
            action: ScreenAction::None,
            status: Some(status),
            command: Some(command),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenState {
    Hello(HelloScreen),
    Help(HelpScreen),
}

impl ScreenState {
    pub const fn id(&self) -> ScreenId {
        match self {
            Self::Hello(_) => ScreenId::Hello,
            Self::Help(_) => ScreenId::Help,
        }
    }

    pub const fn is_modal(&self) -> bool {
        matches!(self, Self::Help(_))
    }

    pub fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match self {
            Self::Hello(screen) => screen.handle_event(event),
            Self::Help(screen) => screen.handle_event(event),
        }
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, stack_depth: usize) {
        match self {
            Self::Hello(screen) => screen.render(frame, area, stack_depth),
            Self::Help(screen) => screen.render(frame, area, stack_depth),
        }
    }

    pub fn hello_mut(&mut self) -> Option<&mut HelloScreen> {
        match self {
            Self::Hello(screen) => Some(screen),
            Self::Help(_) => None,
        }
    }

    pub fn hello(&self) -> Option<&HelloScreen> {
        match self {
            Self::Hello(screen) => Some(screen),
            Self::Help(_) => None,
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
pub struct HelloScreen {
    active_tab: ActiveTab,
    emphasized_copy: bool,
    recent_events: VecDeque<String>,
    task_events: VecDeque<String>,
    setup: AppleFmSetupState,
}

impl Default for HelloScreen {
    fn default() -> Self {
        let mut screen = Self {
            active_tab: ActiveTab::Overview,
            emphasized_copy: false,
            recent_events: VecDeque::new(),
            task_events: VecDeque::new(),
            setup: AppleFmSetupState::default(),
        };
        screen.record_event("probe tui ready");
        screen.record_event("press r to rerun Apple FM setup");
        screen.record_event("press ? for help");
        screen.record_event("press tab to switch views");
        screen
    }
}

impl HelloScreen {
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

    pub fn apply_message(&mut self, message: AppMessage) -> String {
        match message {
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
                self.record_worker_event(format!(
                    "started call {index}/{total_calls}: {title}"
                ));
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
            AppMessage::AppleFmSetupCompleted { backend, total_calls } => {
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
                self.record_worker_event(format!(
                    "Apple FM setup failed at {stage} ({reason})"
                ));
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
                let status = if self.emphasized_copy {
                    String::from("showing operator notes instead of live response detail")
                } else {
                    String::from("restored live Apple FM detail view")
                };
                self.record_event(status.clone());
                ScreenOutcome::with_status(ScreenAction::None, status)
            }
            UiEvent::RunBackgroundTask => {
                self.record_event(String::from("requested Apple FM setup rerun"));
                ScreenOutcome::with_command(
                    String::from("queued Apple FM setup rerun"),
                    ScreenCommand::RunAppleFmSetup,
                )
            }
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            UiEvent::Tick => ScreenOutcome::idle(),
            UiEvent::Dismiss | UiEvent::Quit => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, stack_depth: usize) {
        let sections = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(area);
        TabStrip::new(
            "Apple FM Setup",
            "Probe-owned Apple Foundation Models prove-out with availability gating and live plain-text calls.",
            self.active_tab,
        )
        .render(frame, sections[0]);

        match self.active_tab {
            ActiveTab::Overview => self.render_overview(frame, sections[1], stack_depth),
            ActiveTab::Events => self.render_events(frame, sections[1], stack_depth),
        }
    }

    fn render_overview(&self, frame: &mut Frame<'_>, area: Rect, stack_depth: usize) {
        let columns = Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        let focus_name = if stack_depth > 1 {
            "help modal"
        } else {
            "setup screen"
        };
        InfoPanel::new("Current Detail", self.render_detail_body())
            .render(frame, columns[0]);

        let sidebar =
            Layout::vertical([Constraint::Length(7), Constraint::Length(8), Constraint::Min(0)])
                .split(columns[1]);
        SidebarPanel::new(
            "Setup Status",
            self.render_status_lines(focus_name, stack_depth),
        )
        .render(frame, sidebar[0]);
        SidebarPanel::new("Backend Facts", self.render_backend_lines()).render(frame, sidebar[1]);
        SidebarPanel::new("Availability", self.render_availability_lines()).render(frame, sidebar[2]);
    }

    fn render_events(&self, frame: &mut Frame<'_>, area: Rect, stack_depth: usize) {
        let rows = Layout::vertical([Constraint::Length(7), Constraint::Min(0)]).split(area);
        InfoPanel::new(
            "App Shell Notes",
            Text::from(vec![
                Line::from("AppShell owns terminal lifecycle, dispatch, and worker polling."),
                Line::from("This screen checks Apple FM availability before any inference call."),
                Line::from("The current setup flow is deliberately plain-text only."),
                Line::from(format!("Current stack depth: {stack_depth}")),
            ]),
        )
        .render(frame, rows[0]);

        let columns =
            Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(rows[1]);
        let ui_items = self
            .recent_events
            .iter()
            .enumerate()
            .map(|(index, entry)| ListItem::new(format!("{:>2}. {entry}", index + 1)))
            .collect::<Vec<_>>();
        frame.render_widget(
            List::new(ui_items)
                .block(ratatui::widgets::Block::bordered().title(padded_title("UI Event Log"))),
            columns[0],
        );

        let worker_items = self
            .task_events
            .iter()
            .enumerate()
            .map(|(index, entry)| ListItem::new(format!("{:>2}. {entry}", index + 1)))
            .collect::<Vec<_>>();
        frame.render_widget(
            List::new(worker_items)
                .block(
                    ratatui::widgets::Block::bordered().title(padded_title("Apple FM Timeline")),
                ),
            columns[1],
        );
    }

    fn render_detail_body(&self) -> Text<'static> {
        if self.emphasized_copy {
            return Text::from(vec![
                Line::from("This screen is intentionally a narrow prove-out lane."),
                Line::from(""),
                Line::from("It checks Apple FM availability before any inference."),
                Line::from("It then runs three short plain-text setup prompts."),
                Line::from("The worker thread keeps the retained UI responsive."),
                Line::from("Issue #32 stops here on purpose and does not build full chat."),
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
            return Text::from(self.render_running_detail_lines(active_call));
        }

        if let Some(last_call) = self.setup.calls.last() {
            return Text::from(self.render_completed_detail_lines(last_call));
        }

        match self.setup.phase {
            TaskPhase::Queued => Text::from(vec![
                Line::from("Apple FM setup has been queued."),
                Line::from(""),
                Line::from("Probe will check availability before issuing any inference."),
                Line::from("Use r to rerun the setup flow manually."),
            ]),
            TaskPhase::CheckingAvailability => Text::from(vec![
                Line::from("Checking whether Apple FM is available on this machine."),
                Line::from(""),
                Line::from("No inference requests will be issued until the availability gate passes."),
            ]),
            TaskPhase::Unavailable => Text::from(vec![
                Line::from("Apple FM is not ready on this machine right now."),
                Line::from(""),
                Line::from(
                    self.setup
                        .availability
                        .as_ref()
                        .and_then(|availability| availability.availability_message.clone())
                        .unwrap_or_else(|| String::from("The bridge did not provide extra availability detail.")),
                ),
                Line::from(""),
                Line::from("Press r to rerun the setup check after the machine is admitted."),
            ]),
            TaskPhase::Completed => Text::from(vec![
                Line::from("Apple FM setup completed successfully."),
                Line::from(""),
                Line::from("The timeline shows the three retained setup calls."),
                Line::from("Use t to swap between live detail and operator notes."),
            ]),
            TaskPhase::Idle | TaskPhase::Running | TaskPhase::Failed => Text::from(vec![
                Line::from("Probe TUI is ready."),
                Line::from(""),
                Line::from("Press r to start or rerun the Apple FM setup flow."),
            ]),
        }
    }

    fn render_status_lines(&self, focus_name: &str, stack_depth: usize) -> Vec<String> {
        let phase = match self.setup.phase {
            TaskPhase::Idle => "idle",
            TaskPhase::Queued => "queued",
            TaskPhase::CheckingAvailability => "checking_availability",
            TaskPhase::Unavailable => "unavailable",
            TaskPhase::Running => "running",
            TaskPhase::Completed => "completed",
            TaskPhase::Failed => "failed",
        };
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
            format!("phase: {phase}"),
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

    fn render_running_detail_lines(&self, active_call: &ActiveCall) -> Vec<Line<'static>> {
        let mut lines = self.render_completed_call_history_lines();
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(format!(
            "Running call {}/{}: {}",
            active_call.index, active_call.total_calls, active_call.title
        )));
        lines.push(Line::from(""));
        lines.push(Line::from("Prompt"));
        lines.push(Line::from(active_call.prompt.clone()));
        lines.push(Line::from(""));
        lines.push(Line::from("Response"));
        lines.push(Line::from("[waiting for Apple FM reply]"));
        lines
    }

    fn render_completed_detail_lines(&self, last_call: &AppleFmCallRecord) -> Vec<Line<'static>> {
        let mut lines = self.render_completed_call_history_lines();
        lines.push(Line::from(""));
        lines.push(Line::from(format!("last_response_id: {}", last_call.response_id)));
        lines.push(Line::from(format!("model: {}", last_call.response_model)));
        lines.extend(last_call.usage.render_lines());
        lines
    }

    fn render_completed_call_history_lines(&self) -> Vec<Line<'static>> {
        if self.setup.calls.is_empty() {
            return Vec::new();
        }

        let mut lines = vec![Line::from(format!(
            "Completed calls: {}",
            self.setup.calls.len()
        ))];
        for (index, call) in self.setup.calls.iter().enumerate() {
            lines.push(Line::from(format!("{:>2}. {}", index + 1, call.title)));
            lines.push(Line::from(format!(
                "    {}",
                compact_preview(call.response_text.as_str(), 44)
            )));
        }
        lines
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
            Line::from("Probe Apple FM Setup Keys"),
            Line::from(""),
            Line::from("Tab / Left / Right  switch views"),
            Line::from("r                   rerun Apple FM setup"),
            Line::from("t                   toggle operator notes / live detail"),
            Line::from("? or F1             open or dismiss this modal"),
            Line::from("Esc                 dismiss this modal"),
            Line::from("q or Ctrl+C         quit"),
            Line::from(""),
            Line::from(format!("Current stack depth: {stack_depth}")),
        ]));
        ModalCard::new("Help", content).render(frame, area);
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

fn render_usage_value(value: Option<u64>, truth: Option<&str>) -> String {
    match (value, truth) {
        (Some(value), Some(truth)) => format!("{value} ({truth})"),
        (Some(value), None) => value.to_string(),
        (None, _) => String::from("n/a"),
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

fn compact_preview(value: &str, max_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    preview(compact.as_str(), max_chars)
}
