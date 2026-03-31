use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{self, Event as CrosstermEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use probe_core::backend_profiles::{
    psionic_apple_fm_bridge, psionic_qwen35_2b_q8_registry,
};
use probe_core::harness::resolve_harness_profile;
use probe_core::runtime::{current_working_dir, default_probe_home};
use probe_core::tools::{ProbeToolChoice, ToolLoopConfig};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout};
use ratatui::{Frame, Terminal};

use crate::bottom_pane::{BottomPane, BottomPaneState};
use crate::event::{UiEvent, event_from_key};
use crate::message::{AppMessage, BackgroundTaskRequest, ProbeRuntimeTurnConfig};
use crate::screens::{
    ActiveTab, ApprovalOverlay, ChatScreen, HelpScreen, ScreenAction, ScreenCommand, ScreenId,
    ScreenState, SetupOverlay, TaskPhase,
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
    chat_runtime: ProbeRuntimeTurnConfig,
}

impl Default for AppShell {
    fn default() -> Self {
        Self::with_autostart(true, Self::default_chat_runtime_config())
    }
}

impl AppShell {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_for_tests() -> Self {
        Self::with_autostart(false, Self::default_chat_runtime_config())
    }

    pub fn new_for_tests_with_chat_config(chat_runtime: ProbeRuntimeTurnConfig) -> Self {
        Self::with_autostart(false, chat_runtime)
    }

    fn with_autostart(autostart_setup: bool, chat_runtime: ProbeRuntimeTurnConfig) -> Self {
        let mut app = Self {
            screens: vec![ScreenState::Chat(ChatScreen::default())],
            last_status: String::from("probe tui launched"),
            should_quit: false,
            bottom_pane: BottomPane::new(),
            worker: BackgroundWorker::new(),
            chat_runtime,
        };
        if autostart_setup {
            let _ = app.submit_background_task(Self::default_setup_request());
        }
        app
    }

    fn default_chat_runtime_config() -> ProbeRuntimeTurnConfig {
        let cwd = current_working_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let harness = resolve_harness_profile(Some("coding_bootstrap"), None, cwd.as_path(), None)
            .ok()
            .flatten();
        ProbeRuntimeTurnConfig {
            probe_home: default_probe_home().ok(),
            cwd,
            profile: psionic_qwen35_2b_q8_registry(),
            system_prompt: harness.as_ref().map(|resolved| resolved.system_prompt.clone()),
            harness_profile: harness.map(|resolved| resolved.profile),
            tool_loop: Some(ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Auto, false)),
        }
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

    pub fn runtime_session_id(&self) -> Option<&str> {
        self.base_screen().runtime_session_id()
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
            | UiEvent::ComposerHistoryPrevious
            | UiEvent::ComposerHistoryNext
            | UiEvent::ComposerAddAttachment
            | UiEvent::ComposerPaste(_)
            | UiEvent::ComposerMoveHome
            | UiEvent::ComposerMoveEnd
            | UiEvent::ComposerNewline
            | UiEvent::ComposerSubmit
                if self.active_screen_id() == ScreenId::Chat =>
            {
                let pane_state = self.bottom_pane_state();
                if let Some(submitted) = self.bottom_pane.handle_event(event, &pane_state) {
                    let preview = submission_preview(&submitted, 48);
                    self.base_screen_mut().submit_user_turn(&submitted);
                    self.base_screen_mut()
                        .record_event(format!("queued Probe runtime turn: {preview}"));
                    self.last_status = format!(
                        "submitted chat turn ({} chars)",
                        submitted.text.chars().count()
                    );
                    if let Err(error) = self.submit_background_task(
                        BackgroundTaskRequest::probe_runtime_turn(
                            submitted.text.clone(),
                            self.chat_runtime.clone(),
                        ),
                    ) {
                        self.last_status = error;
                    }
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
                if let Some(transcript_entry) = outcome.transcript_entry {
                    self.apply_message(AppMessage::TranscriptEntryCommitted {
                        entry: transcript_entry,
                    });
                }
                match outcome.action {
                    ScreenAction::None => {}
                    ScreenAction::OpenHelp => {
                        self.base_screen_mut().record_event("help modal took focus");
                        if self.active_screen_id() != ScreenId::Help {
                            self.screens.push(ScreenState::Help(HelpScreen::new()));
                        }
                    }
                    ScreenAction::OpenSetupOverlay => {
                        self.base_screen_mut().record_event("setup overlay took focus");
                        if self.active_screen_id() != ScreenId::SetupOverlay {
                            self.screens.push(ScreenState::Setup(SetupOverlay::new()));
                        }
                    }
                    ScreenAction::OpenApprovalOverlay => {
                        let Some(approval) =
                            self.base_screen().current_pending_tool_approval().cloned()
                        else {
                            self.base_screen_mut()
                                .record_event("approval overlay requested without pending tools");
                            self.last_status = String::from("no pending approvals");
                            self.poll_background_messages();
                            return;
                        };
                        self.base_screen_mut()
                            .record_event("approval overlay took focus");
                        if self.active_screen_id() == ScreenId::ApprovalOverlay {
                            if let Some(ScreenState::Approval(screen)) = self.screens.last_mut() {
                                *screen = ApprovalOverlay::new(approval);
                            }
                        } else {
                            self.screens
                                .push(ScreenState::Approval(ApprovalOverlay::new(approval)));
                        }
                    }
                    ScreenAction::CloseModal => {
                        if self.screens.len() > 1 {
                            let released = self.active_screen_id().title().to_string();
                            self.screens.pop();
                            self.base_screen_mut()
                                .record_event(format!("{released} released focus"));
                        }
                    }
                }
                if let Some(command) = outcome.command {
                    match command {
                        ScreenCommand::RunAppleFmSetup => {
                            if self.active_screen_id() != ScreenId::SetupOverlay {
                                self.screens.push(ScreenState::Setup(SetupOverlay::new()));
                            }
                            if let Err(error) =
                                self.submit_background_task(Self::default_setup_request())
                            {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::ResolvePendingToolApproval {
                            session_id,
                            call_id,
                            resolution,
                        } => {
                            if let Err(error) = self.submit_background_task(
                                BackgroundTaskRequest::resolve_pending_tool_approval(
                                    session_id,
                                    call_id,
                                    resolution,
                                    self.chat_runtime.clone(),
                                ),
                            ) {
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
        if let Some(backend) = request.setup_backend() {
            self.base_screen_mut().prepare_for_setup(backend);
        }
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
        let pending_approvals = match &message {
            AppMessage::PendingToolApprovalsUpdated { approvals, .. } => Some(approvals.clone()),
            _ => None,
        };
        self.last_status = self.base_screen_mut().apply_message(message);
        if let Some(pending_approvals) = pending_approvals {
            self.sync_pending_approval_overlay(pending_approvals);
        }
    }

    pub fn render(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let pane_state = self.bottom_pane_state();
        let base_screen = self.base_screen();
        let replaces_composer = self
            .screens
            .last()
            .map(ScreenState::replaces_composer)
            .unwrap_or(false);

        if replaces_composer {
            self.screens[0].render(frame, area, self.screens.len(), base_screen);
            for overlay in self.screens.iter().skip(1) {
                if overlay.is_modal() {
                    overlay.render(frame, area, self.screens.len(), base_screen);
                }
            }
            return;
        }

        let sections = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(self.bottom_pane.desired_height(&pane_state)),
        ])
        .spacing(1)
        .split(area);

        self.screens[0].render(frame, sections[0], self.screens.len(), base_screen);
        for overlay in self.screens.iter().skip(1) {
            if overlay.is_modal() {
                overlay.render(frame, sections[0], self.screens.len(), base_screen);
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
        match self.active_screen_id() {
            ScreenId::Help => {
                return BottomPaneState::Disabled(String::from(
                    "Composer disabled while help owns focus. Esc or F1 returns to chat.",
                ));
            }
            ScreenId::SetupOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Composer disabled while setup owns focus. Esc returns to chat.",
                ));
            }
            ScreenId::ApprovalOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Composer replaced by the active overlay.",
                ));
            }
            ScreenId::Chat => {}
        }

        if self.active_tab() != ActiveTab::Chat {
            return BottomPaneState::Disabled(format!(
                "Composer only runs on Chat. Tab or Shift+Tab returns from {}.",
                self.active_tab().title()
            ));
        }

        if self.base_screen().has_pending_tool_approvals() {
            return BottomPaneState::Disabled(format!(
                "Resolve {} pending approval request(s) before submitting a new turn.",
                self.base_screen().pending_tool_approval_count()
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

    fn sync_pending_approval_overlay(
        &mut self,
        pending_approvals: Vec<probe_protocol::session::PendingToolApproval>,
    ) {
        if let Some(approval) = pending_approvals.first().cloned() {
            self.base_screen_mut()
                .record_event(format!("approval pending for {}", approval.tool_name));
            if self.active_screen_id() == ScreenId::ApprovalOverlay {
                if let Some(ScreenState::Approval(screen)) = self.screens.last_mut() {
                    *screen = ApprovalOverlay::new(approval);
                }
            } else {
                self.screens
                    .push(ScreenState::Approval(ApprovalOverlay::new(approval)));
            }
            return;
        }

        if self.active_screen_id() == ScreenId::ApprovalOverlay && self.screens.len() > 1 {
            self.screens.pop();
            self.base_screen_mut()
                .record_event("approval overlay released focus");
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

fn submission_preview(
    submission: &crate::bottom_pane::ComposerSubmission,
    max_chars: usize,
) -> String {
    if !submission.text.is_empty() {
        return preview(submission.text.as_str(), max_chars);
    }
    if let Some(attachment) = submission.attachments.first() {
        return format!("attachment-only ({})", attachment.label);
    }
    String::from("[empty]")
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

fn run_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    let mut app = AppShell::new();

    while !app.should_quit() {
        app.poll_background_messages();
        terminal.draw(|frame| app.render(frame))?;
        if event::poll(TICK_RATE)? {
            match event::read()? {
                CrosstermEvent::Key(key) => {
                    if let Some(event) = event_from_key(key) {
                        app.dispatch(event);
                    }
                }
                CrosstermEvent::Paste(text) => app.dispatch(UiEvent::ComposerPaste(text)),
                _ => {}
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

    use probe_core::backend_profiles::{
        psionic_apple_fm_bridge, psionic_qwen35_2b_q8_registry,
    };
    use probe_core::harness::resolve_harness_profile;
    use probe_core::tools::{ProbeToolChoice, ToolLoopConfig};
    use probe_test_support::{
        FakeAppleFmServer, FakeHttpResponse, FakeOpenAiServer, ProbeTestEnvironment,
    };
    use serde_json::json;

    use super::AppShell;
    use crate::event::UiEvent;
    use crate::message::{
        AppMessage, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
        AppleFmUsageSummary, BackgroundTaskRequest, ProbeRuntimeTurnConfig,
    };
    use crate::screens::{ActiveTab, ScreenId, TaskPhase};

    fn runtime_test_config(
        environment: &ProbeTestEnvironment,
        base_url: &str,
    ) -> ProbeRuntimeTurnConfig {
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = base_url.to_string();
        let harness = resolve_harness_profile(
            Some("coding_bootstrap"),
            None,
            environment.workspace(),
            None,
        )
        .expect("resolve coding bootstrap harness")
        .expect("coding bootstrap harness should exist");
        ProbeRuntimeTurnConfig {
            probe_home: Some(environment.probe_home().to_path_buf()),
            cwd: environment.workspace().to_path_buf(),
            profile,
            system_prompt: Some(harness.system_prompt),
            harness_profile: Some(harness.profile),
            tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                ProbeToolChoice::Auto,
                false,
            )),
        }
    }

    fn wait_for_app_condition(
        app: &mut AppShell,
        timeout: Duration,
        mut predicate: impl FnMut(&AppShell) -> bool,
    ) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            app.poll_background_messages();
            if predicate(app) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }

        panic!(
            "timed out waiting for app condition; last_status={}",
            app.last_status()
        );
    }

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
    fn events_view_switches_and_copy_toggle_still_work() {
        let mut app = AppShell::new_for_tests();
        assert_eq!(app.active_tab(), ActiveTab::Chat);
        assert!(!app.emphasized_copy());

        app.dispatch(UiEvent::NextView);
        assert_eq!(app.active_tab(), ActiveTab::Events);

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

        let rendered = app.render_to_string(120, 32);
        assert!(rendered.contains("[user] You"));
        assert!(rendered.contains("hi"));
        assert!(rendered.contains("!"));
        assert!(
            app.recent_events()
                .iter()
                .any(|entry| entry.contains("queued Probe runtime turn: hi"))
        );
    }

    #[test]
    fn composer_submission_drives_live_active_turn_then_commits_reply() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let server = FakeOpenAiServer::from_json_responses(vec![
            json!({
                "id": "chatcmpl_probe_tui_tool_1",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "call_readme_1",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"README.md\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
            json!({
                "id": "chatcmpl_probe_tui_final_1",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Probe inspected README.md through the real runtime."
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 21,
                    "completion_tokens": 9,
                    "total_tokens": 30
                }
            }),
        ]);
        let mut app = AppShell::new_for_tests_with_chat_config(runtime_test_config(
            &environment,
            server.base_url(),
        ));

        for event in [
            UiEvent::ComposerInsert('h'),
            UiEvent::ComposerInsert('e'),
            UiEvent::ComposerInsert('l'),
            UiEvent::ComposerInsert('l'),
            UiEvent::ComposerInsert('o'),
            UiEvent::ComposerSubmit,
        ] {
            app.dispatch(event);
        }

        let mut saw_active_turn = false;
        wait_for_app_condition(&mut app, Duration::from_secs(2), |app| {
            let rendered = app.render_to_string(120, 32);
            if rendered.contains("[active assistant]")
                || rendered.contains("[active tool]")
                || rendered.contains("[active status]")
            {
                saw_active_turn = true;
            }
            app.worker_events()
                .iter()
                .any(|entry| entry.contains("tool call requested: read_file"))
                && app
                    .worker_events()
                    .iter()
                    .any(|entry| entry.contains("tool execution started: read_file"))
                && app
                    .worker_events()
                    .iter()
                    .any(|entry| entry.contains("tool execution completed: read_file"))
                && app
                    .worker_events()
                    .iter()
                    .any(|entry| entry.contains("committed tool call row: read_file"))
                && app
                    .worker_events()
                    .iter()
                    .any(|entry| entry.contains("committed tool result row: read_file"))
                && app
                    .worker_events()
                    .iter()
                    .any(|entry| entry.contains("committed assistant row: Probe"))
                && app.runtime_session_id().is_some()
        });

        assert!(saw_active_turn);
        let rendered = app.render_to_string(120, 32);
        assert!(rendered.contains("[tool call] read_file"));
        assert!(rendered.contains("[tool result] read_file"));
        assert!(rendered.contains("README.md"));
        assert!(
            app.worker_events()
                .iter()
                .any(|entry| entry.contains("runtime session ready:"))
        );
    }

    #[test]
    fn later_composer_submissions_reuse_the_same_runtime_session() {
        let environment = ProbeTestEnvironment::new();
        let server = FakeOpenAiServer::from_json_responses(vec![
            json!({
                "id": "chatcmpl_probe_tui_turn_1",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "First turn complete."
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 4,
                    "total_tokens": 14
                }
            }),
            json!({
                "id": "chatcmpl_probe_tui_turn_2",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Second turn reused the same session."
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 12,
                    "completion_tokens": 7,
                    "total_tokens": 19
                }
            }),
        ]);
        let mut app = AppShell::new_for_tests_with_chat_config(runtime_test_config(
            &environment,
            server.base_url(),
        ));

        for event in [
            UiEvent::ComposerInsert('f'),
            UiEvent::ComposerInsert('i'),
            UiEvent::ComposerInsert('r'),
            UiEvent::ComposerInsert('s'),
            UiEvent::ComposerInsert('t'),
            UiEvent::ComposerSubmit,
        ] {
            app.dispatch(event);
        }

        wait_for_app_condition(&mut app, Duration::from_secs(2), |app| {
            app.render_to_string(120, 32).contains("First turn complete.")
        });
        let session_id = app
            .runtime_session_id()
            .expect("runtime session should be set after first turn")
            .to_string();

        for event in [
            UiEvent::ComposerInsert('s'),
            UiEvent::ComposerInsert('e'),
            UiEvent::ComposerInsert('c'),
            UiEvent::ComposerInsert('o'),
            UiEvent::ComposerInsert('n'),
            UiEvent::ComposerInsert('d'),
            UiEvent::ComposerSubmit,
        ] {
            app.dispatch(event);
        }

        wait_for_app_condition(&mut app, Duration::from_secs(2), |app| {
            app.render_to_string(120, 32)
                .contains("Second turn reused the same session.")
        });

        assert_eq!(app.runtime_session_id(), Some(session_id.as_str()));
    }

    #[test]
    fn paused_tool_approval_overlay_renders_real_pending_details_and_can_approve() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let server = FakeOpenAiServer::from_json_responses(vec![
            json!({
                "id": "chatcmpl_probe_tui_pause_1",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "call_patch_1",
                            "type": "function",
                            "function": {
                                "name": "apply_patch",
                                "arguments": "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
            json!({
                "id": "chatcmpl_probe_tui_pause_2",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Patched hello.txt after approval."
                    },
                    "finish_reason": "stop"
                }]
            }),
        ]);
        let mut config = runtime_test_config(&environment, server.base_url());
        let mut tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Required, false);
        tool_loop.approval = probe_core::tools::ToolApprovalConfig {
            allow_write_tools: false,
            allow_network_shell: false,
            allow_destructive_shell: false,
            denied_action: probe_core::tools::ToolDeniedAction::Pause,
        };
        config.tool_loop = Some(tool_loop);
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        for event in [
            UiEvent::ComposerInsert('p'),
            UiEvent::ComposerInsert('a'),
            UiEvent::ComposerInsert('t'),
            UiEvent::ComposerInsert('c'),
            UiEvent::ComposerInsert('h'),
            UiEvent::ComposerSubmit,
        ] {
            app.dispatch(event);
        }

        wait_for_app_condition(&mut app, Duration::from_secs(2), |app| {
            app.worker_events()
                .iter()
                .any(|entry| entry.contains("loaded 1 pending approval(s)"))
        });

        assert_eq!(app.active_screen_id(), ScreenId::ApprovalOverlay);
        let rendered = app.render_to_string(120, 32);
        assert!(rendered.contains("tool: apply_patch"));
        assert!(rendered.contains("call: call_patch_1"));
        assert!(rendered.contains("risk: write"));
        assert!(rendered.contains("hello.txt"));
        assert!(rendered.contains("old_text"));

        app.dispatch(UiEvent::ComposerSubmit);
        wait_for_app_condition(&mut app, Duration::from_secs(2), |app| {
            app.active_screen_id() == ScreenId::Chat
                && app
                    .worker_events()
                    .iter()
                    .any(|entry| entry.contains("pending approvals cleared"))
                && app
                    .worker_events()
                    .iter()
                    .any(|entry| entry.contains("committed assistant row: Probe"))
        });

        let rendered = app.render_to_string(160, 48);
        assert!(rendered.contains("[approval pending] apply_patch"));
        assert!(rendered.contains("[tool result] apply_patch"));
        assert_eq!(
            std::fs::read_to_string(environment.workspace().join("hello.txt"))
                .expect("read patched file"),
            "hello probe\n"
        );
    }

    #[test]
    fn approval_overlay_requires_real_pending_tools() {
        let mut app = AppShell::new_for_tests();

        app.dispatch(UiEvent::OpenApprovalOverlay);
        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert_eq!(app.last_status(), "no pending approvals");
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
        app.dispatch(UiEvent::OpenSetupOverlay);
        assert_eq!(app.active_screen_id(), ScreenId::SetupOverlay);
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
        app.dispatch(UiEvent::OpenSetupOverlay);
        assert_eq!(app.active_screen_id(), ScreenId::SetupOverlay);
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
