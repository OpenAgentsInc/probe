use std::collections::VecDeque;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::event::UiEvent;
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
}

impl ScreenOutcome {
    pub const fn idle() -> Self {
        Self {
            action: ScreenAction::None,
            status: None,
        }
    }

    fn with_status(action: ScreenAction, status: String) -> Self {
        Self {
            action,
            status: Some(status),
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
pub struct HelloScreen {
    active_tab: ActiveTab,
    emphasized_copy: bool,
    recent_events: VecDeque<String>,
}

impl Default for HelloScreen {
    fn default() -> Self {
        let mut screen = Self {
            active_tab: ActiveTab::Overview,
            emphasized_copy: false,
            recent_events: VecDeque::new(),
        };
        screen.record_event("hello demo ready");
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

    pub fn recent_events(&self) -> impl Iterator<Item = &String> {
        self.recent_events.iter()
    }

    pub fn record_event(&mut self, message: impl Into<String>) {
        self.recent_events.push_front(message.into());
        while self.recent_events.len() > MAX_EVENT_LOG {
            self.recent_events.pop_back();
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
            "Textual-inspired Rust shell with a screen stack and focused modal ownership.",
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
                Line::from("This demo proves the first Probe TUI seam in Rust."),
                Line::from(""),
                Line::from("The shell owns terminal lifecycle and chrome."),
                Line::from("The screen owns view state."),
                Line::from("The help modal owns focus when it is open."),
            ])
        } else {
            Text::from(vec![
                Line::from("Probe now has a visible terminal UI target to iterate on."),
                Line::from(""),
                Line::from("Use Tab or Left/Right to switch views."),
                Line::from("Use t to toggle the main panel copy."),
                Line::from("Use ? to open the focused help modal."),
            ])
        };
        InfoPanel::new("Main Panel", body_copy).render(frame, columns[0]);

        let sidebar =
            Layout::vertical([Constraint::Length(7), Constraint::Min(0)]).split(columns[1]);
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
        SidebarPanel::new(
            "Recent Events",
            self.recent_events.iter().cloned().collect(),
        )
        .render(frame, sidebar[1]);
    }

    fn render_events(&self, frame: &mut Frame<'_>, area: Rect, stack_depth: usize) {
        let rows = Layout::vertical([Constraint::Length(7), Constraint::Min(0)]).split(area);
        InfoPanel::new(
            "App Shell Notes",
            Text::from(vec![
                Line::from("AppShell owns terminal lifecycle, chrome, and dispatch."),
                Line::from("HelloScreen owns tab state and body copy."),
                Line::from("HelpScreen is modal and sits on top of the base screen."),
                Line::from(format!("Current stack depth: {stack_depth}")),
            ]),
        )
        .render(frame, rows[0]);

        let items = self
            .recent_events
            .iter()
            .enumerate()
            .map(|(index, entry)| ListItem::new(format!("{:>2}. {entry}", index + 1)))
            .collect::<Vec<_>>();
        frame.render_widget(
            List::new(items).block(ratatui::widgets::Block::bordered().title("UI Event Log")),
            rows[1],
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
