use std::collections::VecDeque;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::event::UiEvent;
use crate::message::{AppMessage, BackgroundTaskRequest};
use crate::widgets::{InfoPanel, ModalCard, SidebarPanel, TabStrip};

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
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenAction {
    None,
    OpenHelp,
    CloseModal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenOutcome {
    pub action: ScreenAction,
    pub status: Option<String>,
    pub task_request: Option<BackgroundTaskRequest>,
}

impl ScreenOutcome {
    pub const fn idle() -> Self {
        Self {
            action: ScreenAction::None,
            status: None,
            task_request: None,
        }
    }

    fn with_status(action: ScreenAction, status: String) -> Self {
        Self {
            action,
            status: Some(status),
            task_request: None,
        }
    }

    fn with_task(status: String, task_request: BackgroundTaskRequest) -> Self {
        Self {
            action: ScreenAction::None,
            status: Some(status),
            task_request: Some(task_request),
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
enum TaskPanelState {
    Idle,
    Queued {
        title: String,
        detail: String,
    },
    Running {
        title: String,
        step: usize,
        total_steps: usize,
        detail: String,
    },
    Succeeded {
        title: String,
        lines: Vec<String>,
    },
    Failed {
        title: String,
        detail: String,
    },
}

impl Default for TaskPanelState {
    fn default() -> Self {
        Self::Idle
    }
}

impl TaskPanelState {
    const fn phase(&self) -> TaskPhase {
        match self {
            Self::Idle => TaskPhase::Idle,
            Self::Queued { .. } => TaskPhase::Queued,
            Self::Running { .. } => TaskPhase::Running,
            Self::Succeeded { .. } => TaskPhase::Succeeded,
            Self::Failed { .. } => TaskPhase::Failed,
        }
    }

    fn render_body(&self) -> Text<'static> {
        match self {
            Self::Idle => Text::from(vec![
                Line::from("No background task is running."),
                Line::from(""),
                Line::from("Press r to start the retained probe setup demo task."),
                Line::from("The app shell will keep repainting while the worker runs."),
            ]),
            Self::Queued { title, detail } => Text::from(vec![
                Line::from(title.clone()),
                Line::from(""),
                Line::from("State: queued"),
                Line::from(detail.clone()),
            ]),
            Self::Running {
                title,
                step,
                total_steps,
                detail,
            } => Text::from(vec![
                Line::from(title.clone()),
                Line::from(""),
                Line::from(format!("State: running ({step}/{total_steps})")),
                Line::from(detail.clone()),
            ]),
            Self::Succeeded { title, lines } => {
                let mut rendered = vec![
                    Line::from(title.clone()),
                    Line::from(""),
                    Line::from("State: completed"),
                ];
                rendered.extend(lines.iter().cloned().map(Line::from));
                Text::from(rendered)
            }
            Self::Failed { title, detail } => Text::from(vec![
                Line::from(title.clone()),
                Line::from(""),
                Line::from("State: failed"),
                Line::from(detail.clone()),
            ]),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloScreen {
    active_tab: ActiveTab,
    emphasized_copy: bool,
    recent_events: VecDeque<String>,
    task_events: VecDeque<String>,
    task_state: TaskPanelState,
}

impl Default for HelloScreen {
    fn default() -> Self {
        let mut screen = Self {
            active_tab: ActiveTab::Overview,
            emphasized_copy: false,
            recent_events: VecDeque::new(),
            task_events: VecDeque::new(),
            task_state: TaskPanelState::Idle,
        };
        screen.record_event("hello demo ready");
        screen.record_event("press r to start a background task");
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
        self.task_state.phase()
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

    pub fn queue_task(&mut self, request: BackgroundTaskRequest) {
        self.task_state = TaskPanelState::Queued {
            title: request.title().to_string(),
            detail: String::from("request queued locally and waiting for the worker"),
        };
        self.record_event(format!("queued {}", request.kind().label()));
        self.record_worker_event(format!("{} queued", request.kind().label()));
    }

    pub fn apply_message(&mut self, message: AppMessage) -> String {
        match message {
            AppMessage::TaskStarted { kind, title } => {
                self.task_state = TaskPanelState::Running {
                    title,
                    step: 0,
                    total_steps: 0,
                    detail: String::from("worker accepted the request"),
                };
                self.record_worker_event(format!("{} started", kind.label()));
                format!("started {}", kind.label())
            }
            AppMessage::TaskProgress {
                kind,
                step,
                total_steps,
                detail,
            } => {
                self.task_state = TaskPanelState::Running {
                    title: String::from("Probe setup demo"),
                    step,
                    total_steps,
                    detail: detail.clone(),
                };
                self.record_worker_event(format!(
                    "{} advanced to step {step}/{total_steps}",
                    kind.label()
                ));
                format!("running {} ({step}/{total_steps})", kind.label())
            }
            AppMessage::TaskSucceeded { kind, title, lines } => {
                self.task_state = TaskPanelState::Succeeded {
                    title,
                    lines,
                };
                self.record_worker_event(format!("{} completed successfully", kind.label()));
                format!("completed {}", kind.label())
            }
            AppMessage::TaskFailed {
                kind,
                title,
                detail,
            } => {
                self.task_state = TaskPanelState::Failed { title, detail };
                self.record_worker_event(format!("{} failed", kind.label()));
                format!("failed {}", kind.label())
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
                    String::from("toggled body copy to emphasized mode")
                } else {
                    String::from("restored body copy to baseline mode")
                };
                self.record_event(status.clone());
                ScreenOutcome::with_status(ScreenAction::None, status)
            }
            UiEvent::RunBackgroundTask => {
                if matches!(
                    self.task_state.phase(),
                    TaskPhase::Queued | TaskPhase::Running
                ) {
                    let status = String::from("background task is already in flight");
                    self.record_event(status.clone());
                    return ScreenOutcome::with_status(ScreenAction::None, status);
                }
                self.queue_task(BackgroundTaskRequest::DemoSuccess);
                ScreenOutcome::with_task(
                    String::from("queued background task"),
                    BackgroundTaskRequest::DemoSuccess,
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
            "Hello Demo",
            "Textual-inspired Rust shell with a screen stack, app messages, and worker ownership.",
            self.active_tab,
        )
        .render(frame, sections[0]);

        match self.active_tab {
            ActiveTab::Overview => self.render_overview(frame, sections[1], stack_depth),
            ActiveTab::Events => self.render_events(frame, sections[1], stack_depth),
        }
    }

    fn render_overview(&self, frame: &mut Frame<'_>, area: Rect, stack_depth: usize) {
        let columns = Layout::horizontal([Constraint::Percentage(63), Constraint::Percentage(37)])
            .split(area);
        let focus_name = if stack_depth > 1 {
            "help modal"
        } else {
            "hello screen"
        };
        let body_copy = if self.emphasized_copy {
            Text::from(vec![
                Line::from("This demo now proves a retained app-message seam in Rust."),
                Line::from(""),
                Line::from("The shell owns terminal lifecycle, chrome, and worker polling."),
                Line::from("The screen owns view state and task presentation."),
                Line::from("Worker messages flow back into visible UI state without blocking."),
            ])
        } else {
            Text::from(vec![
                Line::from("Probe now has a visible terminal UI target to iterate on."),
                Line::from(""),
                Line::from("Use r to start a background task."),
                Line::from("Use Tab or Left/Right to switch views."),
                Line::from("Use ? to open the focused help modal."),
            ])
        };
        InfoPanel::new("Main Panel", body_copy).render(frame, columns[0]);

        let sidebar =
            Layout::vertical([Constraint::Length(8), Constraint::Min(10), Constraint::Min(0)])
                .split(columns[1]);
        SidebarPanel::new(
            "Screen Stack",
            vec![
                format!("depth: {stack_depth}"),
                String::from("base: hello screen"),
                format!("focus: {focus_name}"),
                format!("view: {}", self.active_tab.title()),
            ],
        )
        .render(frame, sidebar[0]);
        InfoPanel::new("Task Status", self.task_state.render_body()).render(frame, sidebar[1]);
        SidebarPanel::new("Recent UI Events", self.recent_events.iter().cloned().collect())
            .render(frame, sidebar[2]);
    }

    fn render_events(&self, frame: &mut Frame<'_>, area: Rect, stack_depth: usize) {
        let rows = Layout::vertical([Constraint::Length(7), Constraint::Min(0)]).split(area);
        InfoPanel::new(
            "App Shell Notes",
            Text::from(vec![
                Line::from("AppShell owns terminal lifecycle, chrome, dispatch, and worker polling."),
                Line::from("HelloScreen owns tab state, body copy, and task presentation."),
                Line::from("HelpScreen is modal and sits on top of the base screen."),
                Line::from(format!("Current stack depth: {stack_depth}")),
            ]),
        )
        .render(frame, rows[0]);

        let columns =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[1]);
        let ui_items = self
            .recent_events
            .iter()
            .enumerate()
            .map(|(index, entry)| ListItem::new(format!("{:>2}. {entry}", index + 1)))
            .collect::<Vec<_>>();
        frame.render_widget(
            List::new(ui_items).block(ratatui::widgets::Block::bordered().title("UI Event Log")),
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
                .block(ratatui::widgets::Block::bordered().title("Worker Event Log")),
            columns[1],
        );
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
            Line::from("Probe TUI Hello Keys"),
            Line::from(""),
            Line::from("Tab / Left / Right  switch views"),
            Line::from("r                   run retained background task"),
            Line::from("t                   toggle body copy"),
            Line::from("? or F1             open or dismiss this modal"),
            Line::from("Esc                 dismiss this modal"),
            Line::from("q or Ctrl+C         quit"),
            Line::from(""),
            Line::from(format!("Current stack depth: {stack_depth}")),
        ]));
        ModalCard::new("Help", content).render(frame, area);
    }
}
