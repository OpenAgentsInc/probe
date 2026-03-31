use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{self, Event as CrosstermEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout};
use ratatui::{Frame, Terminal};

use crate::event::{UiEvent, event_from_key};
use crate::message::{AppMessage, BackgroundTaskKind, BackgroundTaskRequest};
use crate::screens::{ActiveTab, HelloScreen, HelpScreen, ScreenAction, ScreenId, ScreenState, TaskPhase};
use crate::widgets::{FooterBar, HeaderBar};
use crate::worker::BackgroundWorker;

const TICK_RATE: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct AppShell {
    screens: Vec<ScreenState>,
    last_status: String,
    should_quit: bool,
    worker: BackgroundWorker,
}

impl Default for AppShell {
    fn default() -> Self {
        Self {
            screens: vec![ScreenState::Hello(HelloScreen::default())],
            last_status: String::from("hello demo launched"),
            should_quit: false,
            worker: BackgroundWorker::new(),
        }
    }
}

impl AppShell {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn active_screen_id(&self) -> ScreenId {
        self.screens
            .last()
            .expect("app shell always keeps one screen")
            .id()
    }

    pub fn active_tab(&self) -> ActiveTab {
        self.base_screen().active_tab()
    }

    pub fn emphasized_copy(&self) -> bool {
        self.base_screen().emphasized_copy()
    }

    pub fn task_phase(&self) -> TaskPhase {
        self.base_screen().task_phase()
    }

    pub fn screen_depth(&self) -> usize {
        self.screens.len()
    }

    pub fn last_status(&self) -> &str {
        self.last_status.as_str()
    }

    pub fn recent_events(&self) -> Vec<String> {
        self.base_screen().recent_events().cloned().collect()
    }

    pub fn worker_events(&self) -> Vec<String> {
        self.base_screen().worker_events().cloned().collect()
    }

    pub fn dispatch(&mut self, event: UiEvent) {
        self.poll_background_messages();
        match event {
            UiEvent::Quit => {
                self.base_screen_mut().record_event("quit requested");
                self.last_status = String::from("quitting hello demo");
                self.should_quit = true;
            }
            UiEvent::Tick => {}
            _ => {
                let outcome = self
                    .screens
                    .last_mut()
                    .expect("app shell always keeps one screen")
                    .handle_event(event);
                if let Some(status) = outcome.status {
                    self.last_status = status;
                }
                match outcome.action {
                    ScreenAction::None => {}
                    ScreenAction::OpenHelp => {
                        self.base_screen_mut().record_event("help modal took focus");
                        self.screens.push(ScreenState::Help(HelpScreen::new()));
                    }
                    ScreenAction::CloseModal => {
                        if self.screens.len() > 1 {
                            self.screens.pop();
                            self.base_screen_mut()
                                .record_event("help modal released focus");
                        }
                    }
                }
                if let Some(request) = outcome.task_request {
                    if let Err(error) = self.worker.submit(request) {
                        self.apply_message(AppMessage::TaskFailed {
                            kind: request.kind(),
                            title: request.title().to_string(),
                            detail: error,
                        });
                    }
                }
            }
        }
        self.poll_background_messages();
    }

    pub fn submit_background_task(
        &mut self,
        request: BackgroundTaskRequest,
    ) -> Result<(), String> {
        self.base_screen_mut().queue_task(request);
        self.last_status = format!("queued {}", request.kind().label());
        self.worker.submit(request)
    }

    pub fn poll_background_messages(&mut self) -> usize {
        let mut applied = 0;
        loop {
            match self.worker.try_recv() {
                Ok(Some(message)) => {
                    self.apply_message(message);
                    applied += 1;
                }
                Ok(None) => break,
                Err(error) => {
                    self.apply_message(AppMessage::TaskFailed {
                        kind: BackgroundTaskKind::ProbeSetupDemo,
                        title: String::from("Probe setup demo"),
                        detail: error,
                    });
                    applied += 1;
                    break;
                }
            }
        }
        applied
    }

    pub fn apply_message(&mut self, message: AppMessage) {
        let status = self.base_screen_mut().apply_message(message);
        self.last_status = status;
    }

    pub fn render(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let sections = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);
        let focus = match self.active_screen_id() {
            ScreenId::Hello => "hello screen",
            ScreenId::Help => "help modal",
        };
        HeaderBar::new(
            "Probe TUI Hello",
            "Textual-inspired Rust shell proving app/screen/widget seams",
            focus,
        )
        .render(frame, sections[0]);

        self.screens[0].render(frame, sections[1], self.screens.len());
        for overlay in self.screens.iter().skip(1) {
            if overlay.is_modal() {
                overlay.render(frame, sections[1], self.screens.len());
            }
        }

        FooterBar::new(self.last_status.as_str()).render(frame, sections[2]);
    }

    pub fn render_to_string(&self, width: u16, height: u16) -> String {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test backend is infallible");
        terminal
            .draw(|frame| self.render(frame))
            .expect("test backend draw should not fail");
        buffer_to_string(terminal.backend().buffer())
    }

    fn base_screen(&self) -> &HelloScreen {
        self.screens
            .first()
            .and_then(ScreenState::hello)
            .expect("base screen is always hello")
    }

    fn base_screen_mut(&mut self) -> &mut HelloScreen {
        self.screens
            .first_mut()
            .and_then(ScreenState::hello_mut)
            .expect("base screen is always hello")
    }
}

pub fn run_hello_demo() -> io::Result<()> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_loop(&mut terminal);
    let cleanup_result = restore_terminal(&mut terminal);

    result.and(cleanup_result)
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    let mut app = AppShell::new();

    while !app.should_quit() {
        app.poll_background_messages();
        terminal.draw(|frame| app.render(frame))?;
        if event::poll(TICK_RATE)? {
            if let CrosstermEvent::Key(key) = event::read()?
                && let Some(event) = event_from_key(key)
            {
                app.dispatch(event);
            }
        } else {
            app.dispatch(UiEvent::Tick);
        }
    }

    Ok(())
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn buffer_to_string(buffer: &Buffer) -> String {
    let mut lines = Vec::new();
    for y in 0..buffer.area.height {
        let mut line = String::new();
        for x in 0..buffer.area.width {
            line.push_str(buffer[(x, y)].symbol());
        }
        lines.push(line.trim_end().to_string());
    }
    while matches!(lines.last(), Some(last) if last.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::{Duration, Instant};

    use super::AppShell;
    use crate::event::UiEvent;
    use crate::message::{AppMessage, BackgroundTaskKind, BackgroundTaskRequest};
    use crate::screens::{ActiveTab, ScreenId, TaskPhase};

    #[test]
    fn help_modal_takes_focus_and_dismisses_cleanly() {
        let mut app = AppShell::new();
        assert_eq!(app.active_screen_id(), ScreenId::Hello);
        assert_eq!(app.screen_depth(), 1);

        app.dispatch(UiEvent::OpenHelp);
        assert_eq!(app.active_screen_id(), ScreenId::Help);
        assert_eq!(app.screen_depth(), 2);

        let active_tab = app.active_tab();
        app.dispatch(UiEvent::NextView);
        assert_eq!(app.active_tab(), active_tab);

        app.dispatch(UiEvent::Dismiss);
        assert_eq!(app.active_screen_id(), ScreenId::Hello);
        assert_eq!(app.screen_depth(), 1);
    }

    #[test]
    fn hello_screen_switches_views_and_toggles_copy() {
        let mut app = AppShell::new();
        assert_eq!(app.active_tab(), ActiveTab::Overview);
        assert!(!app.emphasized_copy());

        app.dispatch(UiEvent::NextView);
        assert_eq!(app.active_tab(), ActiveTab::Events);

        app.dispatch(UiEvent::ToggleBody);
        assert!(app.emphasized_copy());

        app.dispatch(UiEvent::PreviousView);
        assert_eq!(app.active_tab(), ActiveTab::Overview);
    }

    #[test]
    fn applying_worker_messages_updates_visible_task_state() {
        let mut app = AppShell::new();
        app.apply_message(AppMessage::TaskStarted {
            kind: BackgroundTaskKind::ProbeSetupDemo,
            title: String::from("Probe setup demo"),
        });
        assert_eq!(app.task_phase(), TaskPhase::Running);

        app.apply_message(AppMessage::TaskSucceeded {
            kind: BackgroundTaskKind::ProbeSetupDemo,
            title: String::from("Probe setup demo"),
            lines: vec![String::from("task finished")],
        });
        assert_eq!(app.task_phase(), TaskPhase::Succeeded);
        assert!(
            app.worker_events()
                .iter()
                .any(|entry| entry.contains("completed successfully"))
        );
    }

    #[test]
    fn background_task_completes_without_blocking_repaint() {
        let mut app = AppShell::new();
        app.submit_background_task(BackgroundTaskRequest::DemoSuccess)
            .expect("demo task should queue");
        assert_eq!(app.task_phase(), TaskPhase::Queued);
        assert!(app.render_to_string(80, 24).contains("State: queued"));

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            app.poll_background_messages();
            if app.task_phase() == TaskPhase::Succeeded {
                break;
            }
            let _ = app.render_to_string(80, 24);
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(app.task_phase(), TaskPhase::Succeeded);
        assert!(app.render_to_string(80, 24).contains("State: completed"));
        assert!(
            app.worker_events()
                .iter()
                .any(|entry| entry.contains("completed successfully"))
        );
    }

    #[test]
    fn background_task_failure_surfaces_error_detail() {
        let mut app = AppShell::new();
        app.submit_background_task(BackgroundTaskRequest::DemoFailure)
            .expect("failing task should queue");

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            app.poll_background_messages();
            if app.task_phase() == TaskPhase::Failed {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(app.task_phase(), TaskPhase::Failed);
        assert!(app.render_to_string(80, 24).contains("State: failed"));
        assert!(app.worker_events().iter().any(|entry| entry.contains("failed")));
    }
}
