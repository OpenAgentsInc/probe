use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{self, Event as CrosstermEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use probe_core::backend_profiles::psionic_apple_fm_bridge;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout};
use ratatui::{Frame, Terminal};

use crate::bottom_pane::{BottomPane, BottomPaneState};
use crate::event::{UiEvent, event_from_key};
use crate::message::{AppMessage, BackgroundTaskRequest};
use crate::screens::{
    ActiveTab, ChatScreen, HelpScreen, ScreenAction, ScreenCommand, ScreenId, ScreenState,
    TaskPhase,
};
use crate::worker::BackgroundWorker;

const TICK_RATE: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct AppShell {
    screens: Vec<ScreenState>,
    last_status: String,
    should_quit: bool,
    bottom_pane: BottomPane,
    worker: BackgroundWorker,
}

impl Default for AppShell {
    fn default() -> Self {
        Self::with_autostart(true)
    }
}

impl AppShell {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_for_tests() -> Self {
        Self::with_autostart(false)
    }

    fn with_autostart(autostart_setup: bool) -> Self {
        let mut app = Self {
            screens: vec![ScreenState::Chat(ChatScreen::default())],
            last_status: String::from("probe tui launched"),
            should_quit: false,
            bottom_pane: BottomPane::new(),
            worker: BackgroundWorker::new(),
        };
        if autostart_setup {
            let _ = app.submit_background_task(Self::default_setup_request());
        }
        app
    }

    fn default_setup_request() -> BackgroundTaskRequest {
        BackgroundTaskRequest::apple_fm_setup(psionic_apple_fm_bridge())
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

    pub fn call_count(&self) -> usize {
        self.base_screen().call_count()
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
                self.last_status = String::from("quitting probe tui");
                self.should_quit = true;
            }
            UiEvent::Tick => {}
            UiEvent::ComposerInsert(_)
            | UiEvent::ComposerBackspace
            | UiEvent::ComposerDelete
            | UiEvent::ComposerMoveLeft
            | UiEvent::ComposerMoveRight
            | UiEvent::ComposerMoveHome
            | UiEvent::ComposerMoveEnd
            | UiEvent::ComposerNewline
            | UiEvent::ComposerSubmit
                if self.active_screen_id() == ScreenId::Chat =>
            {
                let pane_state = self.bottom_pane_state();
                if let Some(submitted) = self.bottom_pane.handle_event(event, &pane_state) {
                    self.base_screen_mut().record_event(format!(
                        "captured composer submission: {}",
                        preview(submitted.as_str(), 48)
                    ));
                    self.last_status = format!(
                        "captured composer submission ({} chars)",
                        submitted.chars().count()
                    );
                }
            }
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
                if let Some(command) = outcome.command {
                    match command {
                        ScreenCommand::RunAppleFmSetup => {
                            if let Err(error) =
                                self.submit_background_task(Self::default_setup_request())
                            {
                                self.last_status = error;
                            }
                        }
                    }
                }
            }
        }
        self.poll_background_messages();
    }

    pub fn submit_background_task(&mut self, request: BackgroundTaskRequest) -> Result<(), String> {
        let backend = request.backend();
        self.base_screen_mut().prepare_for_setup(backend);
        self.last_status = format!("queued {}", request.title());
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
                    self.last_status = error;
                    break;
                }
            }
        }
        applied
    }

    pub fn apply_message(&mut self, message: AppMessage) {
        self.last_status = self.base_screen_mut().apply_message(message);
    }

    pub fn render(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let pane_state = self.bottom_pane_state();
        let sections = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(self.bottom_pane.desired_height()),
        ])
        .spacing(1)
        .split(area);

        self.screens[0].render(frame, sections[0], self.screens.len());
        for overlay in self.screens.iter().skip(1) {
            if overlay.is_modal() {
                overlay.render(frame, sections[0], self.screens.len());
            }
        }

        self.bottom_pane
            .render(frame, sections[1], self.last_status.as_str(), &pane_state);
        if self.active_screen_id() == ScreenId::Chat
            && let Some(cursor) = self.bottom_pane.cursor_position(sections[1], &pane_state)
        {
            frame.set_cursor_position(cursor);
        }
    }

    pub fn render_to_string(&self, width: u16, height: u16) -> String {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test backend is infallible");
        terminal
            .draw(|frame| self.render(frame))
            .expect("test backend draw should not fail");
        buffer_to_string(terminal.backend().buffer())
    }

    fn base_screen(&self) -> &ChatScreen {
        self.screens
            .first()
            .and_then(ScreenState::chat)
            .expect("base screen is always chat")
    }

    fn base_screen_mut(&mut self) -> &mut ChatScreen {
        self.screens
            .first_mut()
            .and_then(ScreenState::chat_mut)
            .expect("base screen is always chat")
    }

    fn bottom_pane_state(&self) -> BottomPaneState {
        if self.active_screen_id() != ScreenId::Chat {
            return BottomPaneState::Disabled(String::from(
                "Composer disabled while help owns focus. Esc or F1 returns to chat.",
            ));
        }

        if self.active_tab() != ActiveTab::Chat {
            return BottomPaneState::Disabled(format!(
                "Composer only runs on Chat. Tab or Shift+Tab returns from {}.",
                self.active_tab().title()
            ));
        }

        match self.task_phase() {
            TaskPhase::Queued | TaskPhase::CheckingAvailability | TaskPhase::Running => {
                BottomPaneState::Busy(String::from(
                    "Apple FM setup is running in the background. Composer stays live.",
                ))
            }
            TaskPhase::Idle | TaskPhase::Unavailable | TaskPhase::Completed | TaskPhase::Failed => {
                BottomPaneState::Active
            }
        }
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

pub fn run_probe_tui() -> io::Result<()> {
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

pub fn run_hello_demo() -> io::Result<()> {
    run_probe_tui()
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

    use probe_core::backend_profiles::psionic_apple_fm_bridge;
    use probe_test_support::{FakeAppleFmServer, FakeHttpResponse};
    use serde_json::json;

    use super::AppShell;
    use crate::event::UiEvent;
    use crate::message::{
        AppMessage, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
        AppleFmUsageSummary, BackgroundTaskRequest,
    };
    use crate::screens::{ActiveTab, ScreenId, TaskPhase};

    #[test]
    fn help_modal_takes_focus_and_dismisses_cleanly() {
        let mut app = AppShell::new_for_tests();
        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert_eq!(app.screen_depth(), 1);

        app.dispatch(UiEvent::OpenHelp);
        assert_eq!(app.active_screen_id(), ScreenId::Help);
        assert_eq!(app.screen_depth(), 2);

        let active_tab = app.active_tab();
        app.dispatch(UiEvent::NextView);
        assert_eq!(app.active_tab(), active_tab);

        app.dispatch(UiEvent::Dismiss);
        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert_eq!(app.screen_depth(), 1);
    }

    #[test]
    fn setup_screen_switches_views_and_toggles_copy() {
        let mut app = AppShell::new_for_tests();
        assert_eq!(app.active_tab(), ActiveTab::Chat);
        assert!(!app.emphasized_copy());

        app.dispatch(UiEvent::NextView);
        assert_eq!(app.active_tab(), ActiveTab::Setup);

        app.dispatch(UiEvent::ToggleBody);
        assert!(app.emphasized_copy());

        app.dispatch(UiEvent::PreviousView);
        assert_eq!(app.active_tab(), ActiveTab::Chat);
    }

    #[test]
    fn composer_submission_records_a_visible_shell_event() {
        let mut app = AppShell::new_for_tests();

        app.dispatch(UiEvent::ComposerInsert('h'));
        app.dispatch(UiEvent::ComposerInsert('i'));
        app.dispatch(UiEvent::ComposerNewline);
        app.dispatch(UiEvent::ComposerInsert('!'));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.last_status(), "captured composer submission (4 chars)");
        assert!(
            app.recent_events()
                .iter()
                .any(|entry| entry.contains("captured composer submission: hi"))
        );
    }

    #[test]
    fn applying_setup_messages_updates_visible_state() {
        let mut app = AppShell::new_for_tests();
        let backend = AppleFmBackendSummary {
            profile_name: String::from("psionic-apple-fm-bridge"),
            base_url: String::from("http://127.0.0.1:11435"),
            model_id: String::from("apple-foundation-model"),
        };
        app.apply_message(AppMessage::AppleFmSetupStarted {
            backend: backend.clone(),
        });
        assert_eq!(app.task_phase(), TaskPhase::CheckingAvailability);

        app.apply_message(AppMessage::AppleFmAvailabilityReady {
            backend: backend.clone(),
            availability: AppleFmAvailabilitySummary {
                ready: true,
                unavailable_reason: None,
                availability_message: Some(String::from("ready")),
                version: Some(String::from("1.0")),
                platform: Some(String::from("macOS")),
                apple_silicon_required: Some(true),
                apple_intelligence_required: Some(true),
            },
        });

        app.apply_message(AppMessage::AppleFmCallCompleted {
            backend: backend.clone(),
            index: 1,
            total_calls: 3,
            call: AppleFmCallRecord {
                title: String::from("Sanity Check"),
                prompt: String::from("Reply with exactly READY."),
                response_text: String::from("READY"),
                response_id: String::from("resp-1"),
                response_model: String::from("apple-foundation-model"),
                usage: AppleFmUsageSummary {
                    total_tokens: Some(12),
                    total_truth: Some(String::from("exact")),
                    ..AppleFmUsageSummary::default()
                },
            },
        });

        app.apply_message(AppMessage::AppleFmSetupCompleted {
            backend,
            total_calls: 3,
        });
        assert_eq!(app.task_phase(), TaskPhase::Completed);
        assert_eq!(app.call_count(), 1);
        assert!(
            app.worker_events()
                .iter()
                .any(|entry| entry.contains("completed after 3 calls"))
        );
    }

    #[test]
    fn apple_fm_setup_stops_when_unavailable() {
        let server = FakeAppleFmServer::from_responses(vec![FakeHttpResponse::json_status(
            200,
            json!({
                "status": "ok",
                "model_available": false,
                "unavailable_reason": "model_not_ready",
                "availability_message": "Foundation model is still preparing",
                "platform": "macOS"
            }),
        )]);

        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();

        let mut app = AppShell::new_for_tests();
        app.submit_background_task(BackgroundTaskRequest::apple_fm_setup(profile))
            .expect("setup task should queue");

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            app.poll_background_messages();
            if app.task_phase() == TaskPhase::Unavailable {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(app.task_phase(), TaskPhase::Unavailable);
        assert_eq!(app.active_tab(), ActiveTab::Chat);
        app.dispatch(UiEvent::NextView);
        assert_eq!(app.active_tab(), ActiveTab::Setup);
        assert!(
            app.render_to_string(120, 32)
                .contains("Foundation model is still preparing")
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("GET /health HTTP/1.1"));
    }

    #[test]
    fn apple_fm_setup_surfaces_typed_provider_failure() {
        let server = FakeAppleFmServer::from_responses(vec![
            FakeHttpResponse::json_status(
                200,
                json!({
                    "status": "ok",
                    "model_available": true,
                    "version": "1.0",
                    "platform": "macOS"
                }),
            ),
            FakeHttpResponse::json_status(
                503,
                json!({
                    "error": {
                        "message": "Apple Intelligence is not enabled",
                        "type": "assets_unavailable",
                        "code": "assets_unavailable",
                        "failure_reason": "Apple Intelligence is disabled",
                        "recovery_suggestion": "Enable Apple Intelligence and retry"
                    }
                }),
            ),
        ]);

        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();

        let mut app = AppShell::new_for_tests();
        app.submit_background_task(BackgroundTaskRequest::apple_fm_setup(profile))
            .expect("setup task should queue");

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            app.poll_background_messages();
            if app.task_phase() == TaskPhase::Failed {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(app.task_phase(), TaskPhase::Failed);
        assert_eq!(app.active_tab(), ActiveTab::Chat);
        app.dispatch(UiEvent::NextView);
        assert_eq!(app.active_tab(), ActiveTab::Setup);
        let rendered = app.render_to_string(120, 32);
        assert!(rendered.contains("assets_unavailable"));
        assert!(rendered.contains("Enable Apple Intelligence and retry"));
        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("GET /health HTTP/1.1"));
        assert!(requests[1].contains("POST /v1/chat/completions HTTP/1.1"));
    }

    #[test]
    fn apple_fm_setup_completes_multi_call_flow() {
        let server = FakeAppleFmServer::from_responses(vec![
            FakeHttpResponse::json_status(
                200,
                json!({
                    "status": "ok",
                    "model_available": true,
                    "version": "1.0",
                    "platform": "macOS"
                }),
            ),
            FakeHttpResponse::json_status(
                200,
                json!({
                    "id": "resp-1",
                    "model": "apple-foundation-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": "READY"},
                            "finish_reason": "stop"
                        }
                    ],
                    "usage": {
                        "prompt_tokens_detail": {"value": 12, "truth": "exact"},
                        "completion_tokens_detail": {"value": 3, "truth": "exact"},
                        "total_tokens_detail": {"value": 15, "truth": "exact"}
                    }
                }),
            ),
            FakeHttpResponse::json_status(
                200,
                json!({
                    "id": "resp-2",
                    "model": "apple-foundation-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": "Probe owns the coding-agent runtime."
                            },
                            "finish_reason": "stop"
                        }
                    ],
                    "usage": {
                        "prompt_tokens_detail": {"value": 20, "truth": "exact"},
                        "completion_tokens_detail": {"value": 8, "truth": "estimated"},
                        "total_tokens_detail": {"value": 28, "truth": "estimated"}
                    }
                }),
            ),
            FakeHttpResponse::json_status(
                200,
                json!({
                    "id": "resp-3",
                    "model": "apple-foundation-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": "The TUI should now prove live Apple FM setup truth on startup."
                            },
                            "finish_reason": "stop"
                        }
                    ],
                    "usage": {
                        "prompt_tokens_detail": {"value": 22, "truth": "exact"},
                        "completion_tokens_detail": {"value": 12, "truth": "exact"},
                        "total_tokens_detail": {"value": 34, "truth": "exact"}
                    }
                }),
            ),
        ]);

        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();

        let mut app = AppShell::new_for_tests();
        app.submit_background_task(BackgroundTaskRequest::apple_fm_setup(profile))
            .expect("setup task should queue");
        assert_eq!(app.task_phase(), TaskPhase::Queued);

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            app.poll_background_messages();
            if app.task_phase() == TaskPhase::Completed {
                break;
            }
            let _ = app.render_to_string(120, 32);
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(app.task_phase(), TaskPhase::Completed);
        assert_eq!(app.call_count(), 3);
        let rendered = app.render_to_string(120, 72);
        assert!(rendered.contains("Next Step"));
        assert!(rendered.contains("Setup Complete"));
        assert!(rendered.contains("resp-3"));
        let requests = server.finish();
        assert_eq!(requests.len(), 4);
    }
}
