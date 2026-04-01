use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event as CrosstermEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use probe_core::backend_profiles::{
    openai_codex_subscription, psionic_apple_fm_bridge, psionic_qwen35_2b_q8_registry,
};
use probe_core::harness::resolve_prompt_contract;
use probe_core::runtime::{current_working_dir, default_probe_home};
use probe_core::server_control::{PsionicServerConfig, PsionicServerMode, ServerOperatorSummary};
use probe_core::tools::{ProbeToolChoice, ToolApprovalConfig, ToolLoopConfig};
use probe_protocol::backend::{BackendKind, BackendProfile};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout};
use ratatui::{Frame, Terminal};

use crate::bottom_pane::{BottomPane, BottomPaneState};
use crate::event::{UiEvent, event_from_key, event_from_mouse};
use crate::message::{AppMessage, BackgroundTaskRequest, ProbeRuntimeTurnConfig};
use crate::screens::{
    ActiveTab, ApprovalOverlay, ChatScreen, HelpScreen, ScreenAction, ScreenCommand, ScreenId,
    ScreenState, SetupOverlay, TaskPhase,
};
use crate::worker::BackgroundWorker;

const TICK_RATE: Duration = Duration::from_millis(33);
const BACKEND_SELECTOR_ORDER: [BackendKind; 3] = [
    BackendKind::OpenAiChatCompletions,
    BackendKind::OpenAiCodexSubscription,
    BackendKind::AppleFmBridge,
];

#[derive(Debug, Clone)]
struct BackendLaneConfig {
    label: String,
    chat_runtime: ProbeRuntimeTurnConfig,
    operator_backend: ServerOperatorSummary,
}

#[derive(Debug)]
pub struct AppShell {
    screens: Vec<ScreenState>,
    last_status: String,
    should_quit: bool,
    bottom_pane: BottomPane,
    worker: BackgroundWorker,
    backend_lanes: [BackendLaneConfig; 3],
    chat_lanes: [ChatScreen; 3],
    active_backend_index: usize,
}

#[derive(Debug, Clone)]
pub struct TuiLaunchConfig {
    pub chat_runtime: ProbeRuntimeTurnConfig,
    pub operator_backend: ServerOperatorSummary,
    pub autostart_apple_fm_setup: bool,
}

impl Default for AppShell {
    fn default() -> Self {
        Self::with_launch_config(Self::default_launch_config())
    }
}

impl AppShell {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_for_tests() -> Self {
        Self::with_launch_config(Self::test_launch_config())
    }

    pub fn new_for_tests_with_chat_config(chat_runtime: ProbeRuntimeTurnConfig) -> Self {
        Self::with_launch_config(Self::launch_config_from_chat_runtime(chat_runtime, false))
    }

    pub fn new_with_launch_config(config: TuiLaunchConfig) -> Self {
        Self::with_launch_config(config)
    }

    fn with_launch_config(config: TuiLaunchConfig) -> Self {
        let backend_lanes = build_backend_lanes(&config);
        let active_backend_index = BACKEND_SELECTOR_ORDER
            .iter()
            .position(|kind| *kind == config.operator_backend.backend_kind)
            .unwrap_or(0);
        let chat_lanes = build_chat_lanes(&backend_lanes, active_backend_index);
        let mut app = Self {
            screens: vec![ScreenState::Chat(chat_lanes[active_backend_index].clone())],
            last_status: String::from("probe tui launched"),
            should_quit: false,
            bottom_pane: BottomPane::new(),
            worker: BackgroundWorker::new(),
            backend_lanes,
            chat_lanes,
            active_backend_index,
        };
        app.sync_backend_selector();
        if config.autostart_apple_fm_setup
            && let Some(request) = app.default_setup_request()
        {
            let _ = app.submit_background_task(request);
        }
        app
    }

    fn default_launch_config() -> TuiLaunchConfig {
        let probe_home = default_probe_home().ok();
        let (profile, summary) =
            Self::chat_profile_and_summary_from_probe_home(probe_home.as_deref());
        let chat_runtime = Self::build_chat_runtime_config(probe_home, profile.clone());
        Self::launch_config_from_parts(
            chat_runtime,
            summary,
            profile.kind == BackendKind::AppleFmBridge,
        )
    }

    fn test_launch_config() -> TuiLaunchConfig {
        let profile = psionic_qwen35_2b_q8_registry();
        let chat_runtime = Self::build_chat_runtime_config(None, profile.clone());
        Self::launch_config_from_parts(chat_runtime, operator_summary_from_profile(&profile), false)
    }

    fn launch_config_from_chat_runtime(
        chat_runtime: ProbeRuntimeTurnConfig,
        autostart_apple_fm_setup: bool,
    ) -> TuiLaunchConfig {
        let summary = operator_summary_from_profile(&chat_runtime.profile);
        Self::launch_config_from_parts(chat_runtime, summary, autostart_apple_fm_setup)
    }

    fn launch_config_from_parts(
        chat_runtime: ProbeRuntimeTurnConfig,
        operator_backend: ServerOperatorSummary,
        autostart_apple_fm_setup: bool,
    ) -> TuiLaunchConfig {
        TuiLaunchConfig {
            chat_runtime,
            operator_backend,
            autostart_apple_fm_setup,
        }
    }

    fn chat_profile_and_summary_from_probe_home(
        probe_home: Option<&Path>,
    ) -> (BackendProfile, ServerOperatorSummary) {
        probe_home
            .and_then(load_server_config)
            .map(|config| {
                let profile = profile_from_server_config(&config);
                (profile, config.operator_summary())
            })
            .unwrap_or_else(|| {
                let profile = psionic_qwen35_2b_q8_registry();
                let summary = operator_summary_from_profile(&profile);
                (profile, summary)
            })
    }

    fn build_chat_runtime_config(
        probe_home: Option<PathBuf>,
        profile: BackendProfile,
    ) -> ProbeRuntimeTurnConfig {
        let cwd = current_working_dir().unwrap_or_else(|_| PathBuf::from("."));
        let (system_prompt, harness_profile) = resolve_prompt_contract(
            Some("coding_bootstrap"),
            None,
            cwd.as_path(),
            None,
            profile.kind,
        )
        .unwrap_or((None, None));
        let mut tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Auto, false);
        tool_loop.approval = ToolApprovalConfig::allow_all();
        ProbeRuntimeTurnConfig {
            probe_home,
            cwd,
            profile,
            system_prompt,
            harness_profile,
            tool_loop: Some(tool_loop),
        }
    }

    fn default_setup_request(&self) -> Option<BackgroundTaskRequest> {
        (self.active_chat_runtime().profile.kind == BackendKind::AppleFmBridge).then(|| {
            BackgroundTaskRequest::apple_fm_setup(self.active_chat_runtime().profile.clone())
        })
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
        if self.active_screen_id() == ScreenId::Chat {
            match event {
                UiEvent::NextView => {
                    self.switch_backend(self.base_screen().active_tab().next());
                    self.poll_background_messages();
                    return;
                }
                UiEvent::PreviousView => {
                    self.switch_backend(self.base_screen().active_tab().previous());
                    self.poll_background_messages();
                    return;
                }
                _ => {}
            }
        }
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
                    if let Err(error) =
                        self.submit_background_task(BackgroundTaskRequest::probe_runtime_turn(
                            submitted.text.clone(),
                            self.active_chat_runtime().clone(),
                        ))
                    {
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
                        self.base_screen_mut()
                            .record_event("backend overlay took focus");
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
                            if let Some(request) = self.default_setup_request() {
                                if let Err(error) = self.submit_background_task(request) {
                                    self.last_status = error;
                                }
                            } else {
                                self.last_status =
                                    String::from("current backend is prepared on launch");
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
                                    self.active_chat_runtime().clone(),
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

        self.bottom_pane.render(
            frame,
            sections[1],
            self.bottom_status_line().as_str(),
            &pane_state,
        );
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

    fn bottom_status_line(&self) -> String {
        let runtime = self.base_screen().compact_runtime_status();
        if runtime.is_empty() {
            self.last_status.clone()
        } else {
            format!("{} | {}", self.last_status, runtime)
        }
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
                    "Composer disabled while backend overlay owns focus. Esc returns to chat.",
                ));
            }
            ScreenId::ApprovalOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Composer replaced by the active overlay.",
                ));
            }
            ScreenId::Chat => {}
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
                    "Backend check is running in the background. Composer stays live.",
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

    fn active_chat_runtime(&self) -> &ProbeRuntimeTurnConfig {
        &self.backend_lanes[self.active_backend_index].chat_runtime
    }

    fn backend_selector_labels(&self) -> Vec<String> {
        self.backend_lanes
            .iter()
            .map(|lane| lane.label.clone())
            .collect()
    }

    fn sync_backend_selector(&mut self) {
        let active_tab = ActiveTab::from_index(self.active_backend_index);
        let labels = self.backend_selector_labels();
        self.chat_lanes[self.active_backend_index].set_backend_selector(labels.clone(), active_tab);
        self.base_screen_mut()
            .set_backend_selector(labels, active_tab);
    }

    fn persist_active_chat_lane(&mut self) {
        let screen = self.base_screen().clone();
        self.chat_lanes[self.active_backend_index] = screen;
    }

    fn restore_chat_lane(&mut self, lane_index: usize, active_tab: ActiveTab) {
        let labels = self.backend_selector_labels();
        let mut screen = self.chat_lanes[lane_index].clone();
        screen.set_backend_selector(labels, active_tab);
        self.chat_lanes[lane_index] = screen.clone();
        self.screens[0] = ScreenState::Chat(screen);
    }

    fn switch_backend(&mut self, active_tab: ActiveTab) {
        if self.base_screen().has_pending_tool_approvals() {
            self.last_status = String::from("resolve pending approvals before switching backend");
            self.base_screen_mut()
                .record_event("backend switch blocked by pending approvals");
            return;
        }

        self.persist_active_chat_lane();
        self.active_backend_index = active_tab.index();
        let lane = self.backend_lanes[self.active_backend_index].clone();
        self.restore_chat_lane(self.active_backend_index, active_tab);
        self.last_status = format!("active backend: {}", lane.label);
    }
}

#[cfg(test)]
fn resolve_tui_chat_profile(probe_home: Option<&Path>) -> BackendProfile {
    AppShell::chat_profile_and_summary_from_probe_home(probe_home).0
}

fn load_server_config(probe_home: &Path) -> Option<PsionicServerConfig> {
    let config_path = PsionicServerConfig::config_path(probe_home);
    PsionicServerConfig::load_or_create(config_path.as_path()).ok()
}

fn load_saved_backend_config(probe_home: &Path, kind: BackendKind) -> Option<PsionicServerConfig> {
    PsionicServerConfig::load_or_default_for_backend(probe_home, kind).ok()
}

fn profile_from_server_config(config: &PsionicServerConfig) -> BackendProfile {
    let mut profile = match config.api_kind {
        BackendKind::OpenAiChatCompletions => psionic_qwen35_2b_q8_registry(),
        BackendKind::OpenAiCodexSubscription => openai_codex_subscription(),
        BackendKind::AppleFmBridge => psionic_apple_fm_bridge(),
    };
    profile.base_url = config.base_url();
    if let Some(model_id) = config.resolved_model_id() {
        profile.model = model_id;
    }
    profile
}

fn operator_summary_from_profile(profile: &BackendProfile) -> ServerOperatorSummary {
    let (host, port) = parse_profile_host_port(profile);
    let mut config = PsionicServerConfig {
        mode: PsionicServerMode::Attach,
        api_kind: profile.kind,
        host,
        port,
        backend: String::from("cpu"),
        binary_path: None,
        model_path: None,
        model_id: Some(profile.model.clone()),
        reasoning_budget: None,
    };
    config.set_api_kind(profile.kind);
    config.model_id = Some(profile.model.clone());
    config.operator_summary()
}

fn runtime_for_profile(
    base: &ProbeRuntimeTurnConfig,
    profile: BackendProfile,
) -> ProbeRuntimeTurnConfig {
    let (system_prompt, harness_profile) = resolve_prompt_contract(
        Some("coding_bootstrap"),
        None,
        base.cwd.as_path(),
        None,
        profile.kind,
    )
    .unwrap_or((None, None));
    ProbeRuntimeTurnConfig {
        probe_home: base.probe_home.clone(),
        cwd: base.cwd.clone(),
        profile,
        system_prompt,
        harness_profile,
        tool_loop: base.tool_loop.clone(),
    }
}

fn default_profile_for_backend_kind(kind: BackendKind) -> BackendProfile {
    match kind {
        BackendKind::OpenAiChatCompletions => psionic_qwen35_2b_q8_registry(),
        BackendKind::OpenAiCodexSubscription => openai_codex_subscription(),
        BackendKind::AppleFmBridge => psionic_apple_fm_bridge(),
    }
}

fn backend_selector_label(summary: &ServerOperatorSummary) -> String {
    match summary.backend_kind {
        BackendKind::AppleFmBridge => String::from("Apple FM"),
        BackendKind::OpenAiCodexSubscription => String::from("Codex"),
        BackendKind::OpenAiChatCompletions if summary.is_remote_target() => String::from("Tailnet"),
        BackendKind::OpenAiChatCompletions => String::from("Qwen"),
    }
}

fn build_saved_or_default_lane(
    base: &ProbeRuntimeTurnConfig,
    backend_kind: BackendKind,
) -> BackendLaneConfig {
    let (chat_runtime, operator_backend) = base
        .probe_home
        .as_deref()
        .and_then(|probe_home| load_saved_backend_config(probe_home, backend_kind))
        .map(|config| {
            let profile = profile_from_server_config(&config);
            (
                runtime_for_profile(base, profile),
                config.operator_summary(),
            )
        })
        .unwrap_or_else(|| {
            let profile = default_profile_for_backend_kind(backend_kind);
            let runtime = runtime_for_profile(base, profile.clone());
            let summary = operator_summary_from_profile(&profile);
            (runtime, summary)
        });

    BackendLaneConfig {
        label: backend_selector_label(&operator_backend),
        chat_runtime,
        operator_backend,
    }
}

fn build_backend_lanes(config: &TuiLaunchConfig) -> [BackendLaneConfig; 3] {
    BACKEND_SELECTOR_ORDER.map(|backend_kind| {
        if backend_kind == config.operator_backend.backend_kind {
            BackendLaneConfig {
                label: backend_selector_label(&config.operator_backend),
                chat_runtime: config.chat_runtime.clone(),
                operator_backend: config.operator_backend.clone(),
            }
        } else {
            build_saved_or_default_lane(&config.chat_runtime, backend_kind)
        }
    })
}

fn build_chat_lane(
    backend_lanes: &[BackendLaneConfig; 3],
    lane_index: usize,
    active_tab: ActiveTab,
) -> ChatScreen {
    let mut screen = ChatScreen::default();
    screen.set_backend_selector(
        backend_lanes
            .iter()
            .map(|lane| lane.label.clone())
            .collect(),
        active_tab,
    );
    screen.set_probe_home(backend_lanes[lane_index].chat_runtime.probe_home.clone());
    screen.set_operator_backend(backend_lanes[lane_index].operator_backend.clone());
    screen
}

fn build_chat_lanes(
    backend_lanes: &[BackendLaneConfig; 3],
    active_backend_index: usize,
) -> [ChatScreen; 3] {
    [
        build_chat_lane(
            backend_lanes,
            0,
            ActiveTab::from_index(active_backend_index),
        ),
        build_chat_lane(
            backend_lanes,
            1,
            ActiveTab::from_index(active_backend_index),
        ),
        build_chat_lane(
            backend_lanes,
            2,
            ActiveTab::from_index(active_backend_index),
        ),
    ]
}

fn parse_profile_host_port(profile: &BackendProfile) -> (String, u16) {
    let without_scheme = profile
        .base_url
        .strip_prefix("http://")
        .or_else(|| profile.base_url.strip_prefix("https://"))
        .unwrap_or(profile.base_url.as_str());
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    let (host, port) = authority
        .rsplit_once(':')
        .map(|(host, port)| {
            (
                host.to_string(),
                port.parse::<u16>()
                    .unwrap_or_else(|_| default_port_for_kind(profile.kind)),
            )
        })
        .unwrap_or_else(|| (authority.to_string(), default_port_for_kind(profile.kind)));
    (host, port)
}

fn default_port_for_kind(kind: BackendKind) -> u16 {
    match kind {
        BackendKind::OpenAiChatCompletions => 8080,
        BackendKind::OpenAiCodexSubscription => 443,
        BackendKind::AppleFmBridge => 11435,
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
    run_probe_tui_with_config(AppShell::default_launch_config())
}

pub fn run_probe_tui_with_config(config: TuiLaunchConfig) -> io::Result<()> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_loop(&mut terminal, config);
    let cleanup_result = restore_terminal(&mut terminal);

    result.and(cleanup_result)
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    config: TuiLaunchConfig,
) -> io::Result<()> {
    let mut app = AppShell::new_with_launch_config(config);

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
                CrosstermEvent::Mouse(mouse) => {
                    if let Some(event) = event_from_mouse(mouse) {
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
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
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
    use std::path::PathBuf;
    use std::thread;
    use std::time::{Duration, Instant};

    use probe_core::backend_profiles::{
        openai_codex_subscription, psionic_apple_fm_bridge, psionic_qwen35_2b_q8_registry,
    };
    use probe_core::harness::resolve_prompt_contract;
    use probe_core::server_control::PsionicServerConfig;
    use probe_core::tools::{ProbeToolChoice, ToolLoopConfig};
    use probe_protocol::backend::BackendKind;
    use probe_test_support::{
        FakeAppleFmServer, FakeHttpResponse, FakeOpenAiServer, ProbeTestEnvironment,
    };
    use serde_json::json;
    use tempfile::tempdir;

    use super::{AppShell, TuiLaunchConfig, profile_from_server_config, resolve_tui_chat_profile};
    use crate::event::UiEvent;
    use crate::message::{
        AppMessage, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
        AppleFmUsageSummary, BackgroundTaskRequest, ProbeRuntimeTurnConfig,
    };
    use crate::screens::{ActiveTab, ScreenId, TaskPhase};
    use crate::transcript::{TranscriptEntry, TranscriptRole};

    fn runtime_test_config(
        environment: &ProbeTestEnvironment,
        base_url: &str,
    ) -> ProbeRuntimeTurnConfig {
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = base_url.to_string();
        let (system_prompt, harness_profile) = resolve_prompt_contract(
            Some("coding_bootstrap"),
            None,
            environment.workspace(),
            None,
            profile.kind,
        )
        .expect("resolve coding bootstrap prompt contract");
        ProbeRuntimeTurnConfig {
            probe_home: Some(environment.probe_home().to_path_buf()),
            cwd: environment.workspace().to_path_buf(),
            profile,
            system_prompt,
            harness_profile,
            tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                ProbeToolChoice::Auto,
                false,
            )),
        }
    }

    fn apple_fm_test_config(base_url: &str) -> ProbeRuntimeTurnConfig {
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = base_url.to_string();
        ProbeRuntimeTurnConfig {
            probe_home: None,
            cwd: PathBuf::from("."),
            profile,
            system_prompt: None,
            harness_profile: None,
            tool_loop: None,
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
    fn tui_chat_profile_uses_probe_server_config_when_present() {
        let probe_home = tempdir().expect("temp probe home");
        let mut config = PsionicServerConfig::default();
        config.set_api_kind(BackendKind::AppleFmBridge);
        config.port = 19091;
        config.model_id = Some(String::from("apple-foundation-model"));
        config
            .save(PsionicServerConfig::config_path(probe_home.path()).as_path())
            .expect("save server config");

        let profile = resolve_tui_chat_profile(Some(probe_home.path()));
        assert_eq!(profile.kind, BackendKind::AppleFmBridge);
        assert_eq!(profile.base_url, "http://127.0.0.1:19091");
        assert_eq!(profile.model, "apple-foundation-model");
    }

    #[test]
    fn server_config_profile_conversion_preserves_openai_base_url_and_model() {
        let mut config = PsionicServerConfig::default();
        config.host = String::from("127.0.0.1");
        config.port = 18080;
        config.model_id = Some(String::from("custom-qwen.gguf"));

        let profile = profile_from_server_config(&config);
        assert_eq!(profile.kind, BackendKind::OpenAiChatCompletions);
        assert_eq!(profile.base_url, "http://127.0.0.1:18080/v1");
        assert_eq!(profile.model, "custom-qwen.gguf");
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
    fn tab_switches_backend_and_copy_toggle_still_work() {
        let mut app = AppShell::new_for_tests();
        assert_eq!(app.active_tab(), ActiveTab::Primary);
        assert!(!app.emphasized_copy());
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_primary"),
            profile_name: String::from("psionic-qwen35-2b-q8-registry"),
            model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            cwd: String::from("/tmp/probe-workspace"),
        });
        app.apply_message(AppMessage::TranscriptEntryCommitted {
            entry: TranscriptEntry::new(
                TranscriptRole::User,
                "You",
                vec![String::from("primary lane message")],
            ),
        });

        app.dispatch(UiEvent::NextView);
        assert_eq!(app.active_tab(), ActiveTab::Secondary);
        assert_eq!(app.runtime_session_id(), None);
        let rendered_secondary = app.render_to_string(120, 32);
        assert!(rendered_secondary.contains("Transcript is empty."));
        assert!(!rendered_secondary.contains("primary lane message"));
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_secondary"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.3-codex"),
            cwd: String::from("/tmp/probe-workspace"),
        });
        app.apply_message(AppMessage::TranscriptEntryCommitted {
            entry: TranscriptEntry::new(
                TranscriptRole::User,
                "You",
                vec![String::from("secondary lane message")],
            ),
        });

        app.dispatch(UiEvent::ToggleBody);
        assert!(app.emphasized_copy());

        app.dispatch(UiEvent::PreviousView);
        assert_eq!(app.active_tab(), ActiveTab::Primary);
        assert_eq!(app.runtime_session_id(), Some("sess_primary"));
        let rendered_primary = app.render_to_string(120, 32);
        assert!(rendered_primary.contains("primary lane message"));
        assert!(!rendered_primary.contains("secondary lane message"));
    }

    #[test]
    fn backend_switch_uses_saved_backend_snapshot_for_alternate_lane() {
        let probe_home = tempdir().expect("temp probe home");

        let mut qwen = PsionicServerConfig::default();
        qwen.host = String::from("100.108.56.85");
        qwen.port = 8080;
        qwen.model_id = Some(String::from("qwen3.5-2b-q8_0-registry.gguf"));
        qwen.save(
            PsionicServerConfig::backend_config_path(
                probe_home.path(),
                BackendKind::OpenAiChatCompletions,
            )
            .as_path(),
        )
        .expect("save qwen snapshot");

        let mut apple = PsionicServerConfig::default();
        apple.set_api_kind(BackendKind::AppleFmBridge);
        apple.port = 11435;
        apple.model_id = Some(String::from("apple-foundation-model"));

        let launch_config = TuiLaunchConfig {
            chat_runtime: ProbeRuntimeTurnConfig {
                probe_home: Some(probe_home.path().to_path_buf()),
                cwd: PathBuf::from("."),
                profile: profile_from_server_config(&apple),
                system_prompt: None,
                harness_profile: None,
                tool_loop: None,
            },
            operator_backend: apple.operator_summary(),
            autostart_apple_fm_setup: false,
        };
        let mut app = AppShell::new_with_launch_config(launch_config);

        app.dispatch(UiEvent::NextView);

        let rendered = app.render_to_string(120, 32);
        assert!(rendered.contains("100.108.56.85:8080"));
        assert_eq!(app.backend_lanes[0].label, "Tailnet");
        assert_eq!(app.backend_lanes[0].operator_backend.host, "100.108.56.85");
        assert_eq!(app.backend_lanes[0].operator_backend.port, 8080);
        assert_eq!(
            app.backend_lanes[0].chat_runtime.profile.base_url,
            "http://100.108.56.85:8080/v1"
        );
        assert_eq!(app.last_status(), "active backend: Tailnet");
    }

    #[test]
    fn codex_lane_uses_codex_prompt_contract_and_auth_overlay() {
        let probe_home = tempdir().expect("temp probe home");
        let profile = openai_codex_subscription();
        let config = AppShell::build_chat_runtime_config(
            Some(probe_home.path().to_path_buf()),
            profile.clone(),
        );
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        assert_eq!(app.active_tab(), ActiveTab::Secondary);
        assert_eq!(app.backend_lanes[1].label, "Codex");
        assert_eq!(
            app.backend_lanes[1]
                .chat_runtime
                .harness_profile
                .as_ref()
                .map(|profile| profile.name.as_str()),
            Some("coding_bootstrap_codex")
        );
        assert!(
            app.backend_lanes[1]
                .chat_runtime
                .system_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains("Codex harness profile v1"))
        );

        app.dispatch(UiEvent::OpenSetupOverlay);
        let rendered = app.render_to_string(120, 48);
        assert!(rendered.contains("OpenAI Subscription Auth"));
        assert!(rendered.contains("status: disconnected"));
        assert!(rendered.contains("probe codex login --method browser"));
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
        wait_for_app_condition(&mut app, Duration::from_secs(5), |app| {
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
        let mut rendered = app.render_to_string(120, 32);
        for _ in 0..6 {
            if rendered.contains("[tool call] read_file")
                && rendered.contains("[tool result] read_file")
            {
                break;
            }
            app.dispatch(UiEvent::PageUp);
            rendered = app.render_to_string(120, 32);
        }
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

        wait_for_app_condition(&mut app, Duration::from_secs(5), |app| {
            app.render_to_string(120, 32)
                .contains("First turn complete.")
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

        wait_for_app_condition(&mut app, Duration::from_secs(5), |app| {
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

        wait_for_app_condition(&mut app, Duration::from_secs(5), |app| {
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
        wait_for_app_condition(&mut app, Duration::from_secs(5), |app| {
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

        let mut app =
            AppShell::new_for_tests_with_chat_config(apple_fm_test_config(server.base_url()));
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();
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
        assert_eq!(app.active_tab(), ActiveTab::Tertiary);
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

        let mut app =
            AppShell::new_for_tests_with_chat_config(apple_fm_test_config(server.base_url()));
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();
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
        assert_eq!(app.active_tab(), ActiveTab::Tertiary);
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

        let mut app =
            AppShell::new_for_tests_with_chat_config(apple_fm_test_config(server.base_url()));
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();
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
        let rendered = app.render_to_string(120, 32);
        assert!(!rendered.contains("Next Step"));
        assert!(!rendered.contains("resp-3"));

        app.dispatch(UiEvent::OpenSetupOverlay);
        let rendered = app.render_to_string(120, 72);
        assert!(rendered.contains("Next Step"));
        assert!(rendered.contains("phase: completed"));
        assert!(rendered.contains("resp-3"));
        let requests = server.finish();
        assert_eq!(requests.len(), 4);
    }

    #[test]
    fn non_apple_fm_launch_does_not_autostart_local_setup() {
        let app = AppShell::new_for_tests();
        assert_eq!(app.task_phase(), TaskPhase::Idle);
        let tool_loop = app
            .active_chat_runtime()
            .tool_loop
            .as_ref()
            .expect("test launch should keep tool loop enabled");
        assert_eq!(
            tool_loop.approval,
            probe_core::tools::ToolApprovalConfig::allow_all()
        );
        assert!(
            app.recent_events()
                .iter()
                .any(|entry| entry.contains("backend target: openai_chat_completions"))
        );
    }
}
