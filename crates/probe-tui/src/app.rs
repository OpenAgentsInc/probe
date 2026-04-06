use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event as CrosstermEvent,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use probe_core::backend_profiles::{
    named_backend_profile, next_reasoning_level_for_backend, openai_codex_subscription,
    persisted_reasoning_level_for_backend, psionic_apple_fm_bridge, psionic_qwen35_2b_q8_registry,
    supported_reasoning_levels_for_backend,
};
use probe_core::harness::resolve_prompt_contract;
use probe_core::runtime::{current_working_dir, default_probe_home};
use probe_core::server_control::{PsionicServerConfig, ServerOperatorSummary};
use probe_core::session_store::FilesystemSessionStore;
use probe_core::tools::{ProbeToolChoice, ToolApprovalConfig, ToolLoopConfig};
use probe_openai_auth::OpenAiCodexAuthStore;
use probe_protocol::backend::{BackendKind, BackendProfile};
use probe_protocol::session::SessionMetadata;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout};
use ratatui::{Frame, Terminal};
use serde::{Deserialize, Serialize};

use crate::bottom_pane::{BottomPane, BottomPaneState};
use crate::event::{UiEvent, event_from_key, event_from_mouse};
use crate::message::{AppMessage, BackgroundTaskRequest, ProbeRuntimeTurnConfig};
use crate::screens::{
    ActiveTab, ApprovalOverlay, ChatScreen, ConfirmationKind, ConfirmationOverlay, HelpScreen,
    IntegrationCardView, ManagedMcpServerView, McpAddOverlay, McpEditorOverlay, McpOverlay,
    McpProviderCommandOverlay, McpServerTransportDraft, McpServersOverlay, ModelPickerOverlay,
    PlanModeOverlay, ReasoningPickerOverlay, ResumeOverlay, ResumeSessionView, ScreenAction,
    ScreenCommand, ScreenId, ScreenState, SetupOverlay, TaskPhase, UsageOverlay, WorkspaceOverlay,
};
use crate::worker::BackgroundWorker;

const TICK_RATE: Duration = Duration::from_millis(33);
const BACKEND_SELECTOR_ORDER: [BackendKind; 3] = [
    BackendKind::OpenAiCodexSubscription,
    BackendKind::OpenAiChatCompletions,
    BackendKind::AppleFmBridge,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaneOperatorMode {
    Coding,
    Plan,
}

impl LaneOperatorMode {
    fn label(self) -> &'static str {
        match self {
            Self::Coding => "coding",
            Self::Plan => "plan",
        }
    }
}

#[derive(Debug, Clone)]
struct BackendLaneConfig {
    label: String,
    chat_runtime: ProbeRuntimeTurnConfig,
    operator_backend: ServerOperatorSummary,
    mode: LaneOperatorMode,
    carry_forward_summary: Option<String>,
    session_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum McpServerTransport {
    Stdio,
    Http,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum McpServerSource {
    ManualLaunch,
    ProviderCommandRecipe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct McpServerRecord {
    id: String,
    name: String,
    enabled: bool,
    #[serde(default = "default_mcp_server_source")]
    source: McpServerSource,
    #[serde(default)]
    transport: Option<McpServerTransport>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    provider_setup_command: Option<String>,
    #[serde(default)]
    provider_hint: Option<String>,
    #[serde(default)]
    client_hint: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct McpRegistryFile {
    servers: Vec<McpServerRecord>,
}

impl McpServerTransport {
    fn label(&self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
        }
    }
}

impl McpServerSource {
    fn label(&self) -> &'static str {
        match self {
            Self::ManualLaunch => "manual launch",
            Self::ProviderCommandRecipe => "provider recipe",
        }
    }
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
    mcp_registry: McpRegistryFile,
}

#[derive(Debug, Clone)]
pub struct TuiLaunchConfig {
    pub chat_runtime: ProbeRuntimeTurnConfig,
    pub operator_backend: ServerOperatorSummary,
    pub autostart_apple_fm_setup: bool,
    pub resume_session_id: Option<String>,
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
        let mcp_registry =
            load_mcp_registry(registry_probe_home(&backend_lanes).as_deref()).unwrap_or_default();
        let mut app = Self {
            screens: vec![ScreenState::Chat(chat_lanes[active_backend_index].clone())],
            last_status: String::from("probe tui launched"),
            should_quit: false,
            bottom_pane: BottomPane::new(),
            worker: BackgroundWorker::new(),
            backend_lanes,
            chat_lanes,
            active_backend_index,
            mcp_registry,
        };
        app.sync_backend_selector();
        if let Some(session_id) = config.resume_session_id.as_ref() {
            let _ =
                app.submit_background_task(BackgroundTaskRequest::attach_probe_runtime_session(
                    session_id.clone(),
                    app.active_chat_runtime().clone(),
                ));
        }
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
            resume_session_id: None,
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
        build_chat_runtime_config_for_lane(
            probe_home,
            cwd,
            profile,
            LaneOperatorMode::Coding,
            None,
            0,
        )
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
        if matches!(event, UiEvent::PasteSystemClipboard) {
            match self.active_screen_id() {
                ScreenId::McpProviderCommandOverlay | ScreenId::McpEditorOverlay => {
                    match read_system_clipboard_text() {
                        Ok(text) if text.is_empty() => {
                            self.last_status = String::from("system clipboard is empty");
                        }
                        Ok(text) => {
                            let pasted_chars = text.chars().count();
                            let outcome = self
                                .screens
                                .last_mut()
                                .expect("app shell always keeps one screen")
                                .handle_event(UiEvent::ComposerPaste(text));
                            if let Some(status) = outcome.status {
                                self.last_status = status;
                            } else {
                                self.last_status =
                                    format!("pasted {pasted_chars} chars from system clipboard");
                            }
                        }
                        Err(error) => {
                            self.last_status = error;
                        }
                    }
                }
                _ => {
                    self.last_status = String::from(
                        "system clipboard paste is only available in editable MCP fields",
                    );
                }
            }
            self.poll_background_messages();
            return;
        }

        if self.active_screen_id() == ScreenId::Chat {
            match event {
                UiEvent::NextView => {
                    self.switch_backend(self.base_screen().active_tab().next());
                    self.poll_background_messages();
                    return;
                }
                UiEvent::PreviousView => {
                    if self.cycle_codex_reasoning_level() {
                        self.poll_background_messages();
                        return;
                    }
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
                    if self.handle_local_slash_submission(&submitted) {
                        self.poll_background_messages();
                        return;
                    }
                    if self.base_screen().has_pending_tool_approvals() {
                        self.base_screen_mut().record_event(
                            "blocked new turn while a tool approval is still pending",
                        );
                        self.last_status =
                            String::from("resolve pending approvals before starting another turn");
                        self.poll_background_messages();
                        return;
                    }
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
                        self.open_help_overlay();
                    }
                    ScreenAction::OpenSetupOverlay => {
                        self.open_backend_overlay();
                    }
                    ScreenAction::OpenApprovalOverlay => {
                        self.open_approval_overlay();
                    }
                    ScreenAction::OpenMcpAddOverlay => {
                        self.base_screen_mut()
                            .record_event("MCP add menu took focus");
                        if self.active_screen_id() != ScreenId::McpAddOverlay {
                            self.screens.push(ScreenState::McpAdd(McpAddOverlay::new()));
                        }
                    }
                    ScreenAction::OpenMcpServersOverlay => {
                        self.base_screen_mut()
                            .record_event("saved MCP servers overlay took focus");
                        if self.active_screen_id() != ScreenId::McpServersOverlay {
                            self.screens
                                .push(ScreenState::McpServers(McpServersOverlay::new(
                                    self.managed_mcp_server_views(),
                                )));
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
                        ScreenCommand::SetActivePlanMode { enabled } => {
                            if let Err(error) = self.apply_active_plan_mode(enabled) {
                                self.last_status = error;
                            } else if self.active_screen_id() == ScreenId::PlanModeOverlay
                                && self.screens.len() > 1
                            {
                                self.screens.pop();
                                self.base_screen_mut()
                                    .record_event("plan mode picker released focus");
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
                        ScreenCommand::SelectActiveBackendModel { model_id } => {
                            if let Err(error) = self.apply_active_backend_model(model_id) {
                                self.last_status = error;
                            } else if self.active_screen_id() == ScreenId::ModelPickerOverlay
                                && self.screens.len() > 1
                            {
                                self.screens.pop();
                                self.base_screen_mut()
                                    .record_event("model picker released focus");
                            }
                        }
                        ScreenCommand::SelectActiveReasoningLevel { level } => {
                            if let Err(error) = self.apply_active_reasoning_level(level) {
                                self.last_status = error;
                            } else if self.active_screen_id() == ScreenId::ReasoningPickerOverlay
                                && self.screens.len() > 1
                            {
                                self.screens.pop();
                                self.base_screen_mut()
                                    .record_event("reasoning picker released focus");
                            }
                        }
                        ScreenCommand::SetActiveWorkspace { cwd } => {
                            if let Err(error) = self.apply_active_workspace(cwd) {
                                self.last_status = error;
                            } else if self.active_screen_id() == ScreenId::WorkspaceOverlay
                                && self.screens.len() > 1
                            {
                                self.screens.pop();
                                self.base_screen_mut()
                                    .record_event("workspace picker released focus");
                            }
                        }
                        ScreenCommand::ResumeDetachedSession { session_id } => {
                            if let Err(error) = self.resume_detached_session(session_id) {
                                self.last_status = error;
                            } else if self.active_screen_id() == ScreenId::ResumeOverlay
                                && self.screens.len() > 1
                            {
                                self.screens.pop();
                                self.base_screen_mut()
                                    .record_event("resume picker released focus");
                            }
                        }
                        ScreenCommand::ToggleMcpServerEnabled { server_id } => {
                            if let Err(error) = self.toggle_mcp_server_enabled(server_id) {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::RemoveMcpServer { server_id } => {
                            if let Err(error) = self.remove_mcp_server(server_id) {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::OpenMcpProviderCommandOverlay => {
                            self.base_screen_mut()
                                .record_event("MCP provider command overlay took focus");
                            self.screens.push(ScreenState::McpProviderCommand(
                                McpProviderCommandOverlay::new(),
                            ));
                        }
                        ScreenCommand::OpenMcpManualEditorOverlay => {
                            self.base_screen_mut()
                                .record_event("manual MCP setup overlay took focus");
                            self.screens
                                .push(ScreenState::McpEditor(McpEditorOverlay::new()));
                        }
                        ScreenCommand::ImportMcpProviderCommand { command } => {
                            if let Err(error) = self.import_mcp_provider_command(command) {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::SaveMcpServer {
                            name,
                            transport,
                            target,
                        } => {
                            if let Err(error) = self.save_mcp_server(name, transport, target) {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::ConfirmClearActiveContext => {
                            if let Err(error) = self.clear_active_context() {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::ConfirmCompactActiveContext => {
                            if let Err(error) = self.compact_active_context() {
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
                return BottomPaneState::Disabled(String::from("Help owns focus. Esc closes it."));
            }
            ScreenId::SetupOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Backend details own focus. Esc returns to chat.",
                ));
            }
            ScreenId::ApprovalOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Approval owns focus. Enter decides the selected action.",
                ));
            }
            ScreenId::PlanModeOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Plan mode picker owns focus. Up/Down choose and Enter applies.",
                ));
            }
            ScreenId::ModelPickerOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Model picker owns focus. Up/Down choose and Enter applies.",
                ));
            }
            ScreenId::ReasoningPickerOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Reasoning picker owns focus. Up/Down choose and Enter applies.",
                ));
            }
            ScreenId::WorkspaceOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Workspace picker owns focus. Paste or type a path, Enter applies, and Esc returns.",
                ));
            }
            ScreenId::ResumeOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Resume picker owns focus. Up/Down choose, Enter attaches, and Esc returns.",
                ));
            }
            ScreenId::UsageOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Usage overlay owns focus. Esc returns to chat.",
                ));
            }
            ScreenId::McpOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "MCP overlay owns focus. Up/Down choose, Enter runs the selected action, A adds, and Esc returns to chat.",
                ));
            }
            ScreenId::McpServersOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Saved MCP servers own focus. Up/Down choose, E enables, D disables, R removes, A adds, and Esc returns.",
                ));
            }
            ScreenId::McpAddOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "MCP add menu owns focus. Up/Down choose a path and Enter continues.",
                ));
            }
            ScreenId::McpProviderCommandOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Provider command setup owns focus. Cmd+V pastes normally; Ctrl+V pulls from the system clipboard.",
                ));
            }
            ScreenId::McpEditorOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Manual MCP setup owns focus. Type values, Tab changes fields, and Enter saves.",
                ));
            }
            ScreenId::ConfirmationOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Confirmation owns focus. Enter confirms and Esc cancels.",
                ));
            }
            ScreenId::Chat => {}
        }

        match self.task_phase() {
            TaskPhase::Queued | TaskPhase::CheckingAvailability | TaskPhase::Running => {
                BottomPaneState::Busy(String::from(
                    "Backend check is running in the background. You can keep typing.",
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

    fn active_backend_kind(&self) -> BackendKind {
        self.backend_lanes[self.active_backend_index]
            .operator_backend
            .backend_kind
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

    fn persist_backend_lane_snapshot(&self, lane_index: usize) -> Result<(), String> {
        let lane = &self.backend_lanes[lane_index];
        let Some(probe_home) = lane.chat_runtime.probe_home.as_deref() else {
            return Ok(());
        };
        server_config_from_profile(&lane.chat_runtime.profile)
            .save(
                PsionicServerConfig::backend_config_path(
                    probe_home,
                    lane.chat_runtime.profile.kind,
                )
                .as_path(),
            )
            .map_err(|error| error.to_string())
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

    fn cycle_codex_reasoning_level(&mut self) -> bool {
        if self.active_backend_kind() != BackendKind::OpenAiCodexSubscription {
            return false;
        }
        if self.base_screen().has_pending_tool_approvals() {
            self.last_status =
                String::from("resolve pending approvals before changing Codex reasoning");
            self.base_screen_mut()
                .record_event("codex reasoning change blocked by pending approvals");
            return true;
        }

        let lane_index = self.active_backend_index;
        let current_level = self.backend_lanes[lane_index]
            .chat_runtime
            .profile
            .reasoning_level
            .as_deref();
        let Some(next_level) =
            next_reasoning_level_for_backend(BackendKind::OpenAiCodexSubscription, current_level)
        else {
            return false;
        };

        {
            let lane = &mut self.backend_lanes[lane_index];
            lane.chat_runtime.profile.reasoning_level = persisted_reasoning_level_for_backend(
                BackendKind::OpenAiCodexSubscription,
                Some(next_level),
            );
            lane.operator_backend = operator_summary_from_profile(&lane.chat_runtime.profile);
        }

        let active_tab = ActiveTab::from_index(lane_index);
        let mut refreshed_lane = build_chat_lane(&self.backend_lanes, lane_index, active_tab);
        refreshed_lane.record_event(format!("codex reasoning level set to {next_level}"));
        self.chat_lanes[lane_index] = refreshed_lane.clone();
        self.screens[0] = ScreenState::Chat(refreshed_lane);
        self.sync_backend_selector();
        self.last_status = format!("codex reasoning level: {next_level}");
        if let Err(error) = self.persist_backend_lane_snapshot(lane_index) {
            self.base_screen_mut()
                .record_event(format!("failed to persist Codex reasoning level: {error}"));
            self.last_status = error;
        }
        true
    }

    fn handle_local_slash_submission(
        &mut self,
        submitted: &crate::bottom_pane::ComposerSubmission,
    ) -> bool {
        match submitted.slash_command.as_deref() {
            Some("help") => {
                self.open_help_overlay();
                true
            }
            Some("backend") => {
                self.open_backend_overlay();
                true
            }
            Some("model") => {
                self.open_model_picker();
                true
            }
            Some("reasoning") => {
                self.open_reasoning_picker();
                true
            }
            Some("plan") => {
                self.open_plan_mode_overlay();
                true
            }
            Some("cwd") => {
                self.open_workspace_overlay();
                true
            }
            Some("approvals") => {
                self.open_approval_overlay();
                true
            }
            Some("new") => {
                self.open_confirmation_overlay(ConfirmationKind::FreshSession);
                true
            }
            Some("usage") => {
                self.open_usage_overlay();
                true
            }
            Some("mcp") => {
                self.open_mcp_overlay();
                true
            }
            Some("clear") => {
                self.open_confirmation_overlay(ConfirmationKind::ClearContext);
                true
            }
            Some("compact") => {
                self.open_confirmation_overlay(ConfirmationKind::CompactContext);
                true
            }
            Some("resume") => {
                self.open_resume_overlay();
                true
            }
            _ => false,
        }
    }

    fn open_help_overlay(&mut self) {
        self.base_screen_mut().record_event("help modal took focus");
        self.last_status = String::from("opened help modal");
        if self.active_screen_id() != ScreenId::Help {
            self.screens.push(ScreenState::Help(HelpScreen::new()));
        }
    }

    fn open_backend_overlay(&mut self) {
        self.base_screen_mut()
            .record_event("backend overlay took focus");
        self.last_status = String::from("opened backend overlay");
        if self.active_screen_id() != ScreenId::SetupOverlay {
            self.screens.push(ScreenState::Setup(SetupOverlay::new()));
        }
    }

    fn open_approval_overlay(&mut self) {
        let Some(approval) = self.base_screen().current_pending_tool_approval().cloned() else {
            self.base_screen_mut()
                .record_event("approval overlay requested without pending tools");
            self.last_status = String::from("no pending approvals");
            return;
        };
        self.base_screen_mut()
            .record_event("approval overlay took focus");
        self.last_status = String::from("opened approval overlay");
        if self.active_screen_id() == ScreenId::ApprovalOverlay {
            if let Some(ScreenState::Approval(screen)) = self.screens.last_mut() {
                *screen = ApprovalOverlay::new(approval);
            }
        } else {
            self.screens
                .push(ScreenState::Approval(ApprovalOverlay::new(approval)));
        }
    }

    fn open_model_picker(&mut self) {
        if let Some(reason) = self.active_runtime_reconfig_block_reason("change the active model") {
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        }
        let lane_label = self.backend_lanes[self.active_backend_index].label.clone();
        let current_model = self.backend_lanes[self.active_backend_index]
            .chat_runtime
            .profile
            .model
            .clone();
        let models = available_models_for_backend(
            self.backend_lanes[self.active_backend_index]
                .chat_runtime
                .profile
                .kind,
            current_model.as_str(),
        );
        self.base_screen_mut()
            .record_event(format!("opened model picker for {}", lane_label));
        self.last_status = format!("choose a model for {}", lane_label);
        if self.active_screen_id() == ScreenId::ModelPickerOverlay {
            if let Some(ScreenState::ModelPicker(screen)) = self.screens.last_mut() {
                *screen = ModelPickerOverlay::new(lane_label, current_model, models);
            }
            return;
        }
        self.screens
            .push(ScreenState::ModelPicker(ModelPickerOverlay::new(
                lane_label,
                current_model,
                models,
            )));
    }

    fn open_reasoning_picker(&mut self) {
        let backend_kind = self.active_backend_kind();
        let levels = supported_reasoning_levels_for_backend(backend_kind)
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();
        if levels.is_empty() {
            let reason = String::from("reasoning controls are not available for this backend");
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        }
        if let Some(reason) =
            self.active_runtime_reconfig_block_reason("change the active reasoning level")
        {
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        }
        let lane_label = self.backend_lanes[self.active_backend_index].label.clone();
        let current_level = self.backend_lanes[self.active_backend_index]
            .chat_runtime
            .profile
            .reasoning_level
            .clone()
            .unwrap_or_else(|| String::from("backend_default"));
        self.base_screen_mut()
            .record_event(format!("opened reasoning picker for {}", lane_label));
        self.last_status = format!("choose reasoning for {}", lane_label);
        if self.active_screen_id() == ScreenId::ReasoningPickerOverlay {
            if let Some(ScreenState::ReasoningPicker(screen)) = self.screens.last_mut() {
                *screen = ReasoningPickerOverlay::new(lane_label, current_level, levels);
            }
            return;
        }
        self.screens
            .push(ScreenState::ReasoningPicker(ReasoningPickerOverlay::new(
                lane_label,
                current_level,
                levels,
            )));
    }

    fn open_workspace_overlay(&mut self) {
        if let Some(reason) =
            self.active_runtime_reconfig_block_reason("change the active workspace")
        {
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        }
        let current_cwd = self.active_chat_runtime().cwd.display().to_string();
        self.base_screen_mut()
            .record_event(format!("opened workspace picker for {}", current_cwd));
        self.last_status = String::from("inspect or change the active workspace");
        if self.active_screen_id() == ScreenId::WorkspaceOverlay {
            if let Some(ScreenState::Workspace(screen)) = self.screens.last_mut() {
                *screen = WorkspaceOverlay::new(current_cwd);
            }
            return;
        }
        self.screens
            .push(ScreenState::Workspace(WorkspaceOverlay::new(current_cwd)));
    }

    fn open_resume_overlay(&mut self) {
        if let Some(reason) = self.active_runtime_reconfig_block_reason("resume another session") {
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        }
        let sessions = match self.recent_resume_sessions() {
            Ok(sessions) => sessions,
            Err(error) => {
                self.base_screen_mut().record_event(error.clone());
                self.last_status = error;
                return;
            }
        };
        self.base_screen_mut()
            .record_event("opened resume session picker");
        self.last_status = String::from("choose a session to resume");
        if self.active_screen_id() == ScreenId::ResumeOverlay {
            if let Some(ScreenState::Resume(screen)) = self.screens.last_mut() {
                *screen = ResumeOverlay::new(sessions);
            }
            return;
        }
        self.screens
            .push(ScreenState::Resume(ResumeOverlay::new(sessions)));
    }

    fn apply_active_backend_model(&mut self, model_id: String) -> Result<(), String> {
        if let Some(reason) = self.active_runtime_reconfig_block_reason("change the active model") {
            self.base_screen_mut().record_event(format!("{reason}"));
            return Err(reason);
        }

        let lane_index = self.active_backend_index;
        if self.backend_lanes[lane_index].chat_runtime.profile.model == model_id {
            self.base_screen_mut()
                .record_event(format!("model unchanged: {model_id}"));
            self.last_status = format!("active model: {model_id}");
            return Ok(());
        }

        {
            let lane = &mut self.backend_lanes[lane_index];
            lane.chat_runtime.profile.model = model_id.clone();
            lane.session_generation = lane.session_generation.saturating_add(1);
            rebuild_lane_runtime(lane);
            lane.operator_backend = operator_summary_from_profile(&lane.chat_runtime.profile);
        }

        self.refresh_active_lane_after_runtime_change(format!(
            "model set to {model_id}; next turn starts a fresh runtime session"
        ));
        self.last_status = format!("active model: {model_id}");
        if let Err(error) = self.persist_backend_lane_snapshot(lane_index) {
            self.base_screen_mut()
                .record_event(format!("failed to persist active model: {error}"));
            return Err(error);
        }
        Ok(())
    }

    fn apply_active_reasoning_level(&mut self, level: String) -> Result<(), String> {
        if let Some(reason) =
            self.active_runtime_reconfig_block_reason("change the active reasoning level")
        {
            self.base_screen_mut().record_event(reason.clone());
            return Err(reason);
        }

        let lane_index = self.active_backend_index;
        let backend_kind = self.backend_lanes[lane_index].chat_runtime.profile.kind;
        let persisted = persisted_reasoning_level_for_backend(backend_kind, Some(level.as_str()));
        if self.backend_lanes[lane_index]
            .chat_runtime
            .profile
            .reasoning_level
            == persisted
        {
            self.base_screen_mut()
                .record_event(format!("reasoning unchanged: {level}"));
            self.last_status = format!("active reasoning: {level}");
            return Ok(());
        }

        {
            let lane = &mut self.backend_lanes[lane_index];
            lane.chat_runtime.profile.reasoning_level = persisted;
            lane.session_generation = lane.session_generation.saturating_add(1);
            rebuild_lane_runtime(lane);
            lane.operator_backend = operator_summary_from_profile(&lane.chat_runtime.profile);
        }

        self.refresh_active_lane_after_runtime_change(format!(
            "reasoning set to {level}; next turn starts a fresh runtime session"
        ));
        self.last_status = format!("active reasoning: {level}");
        if let Err(error) = self.persist_backend_lane_snapshot(lane_index) {
            self.base_screen_mut()
                .record_event(format!("failed to persist active reasoning: {error}"));
            return Err(error);
        }
        Ok(())
    }

    fn apply_active_workspace(&mut self, cwd: String) -> Result<(), String> {
        if let Some(reason) =
            self.active_runtime_reconfig_block_reason("change the active workspace")
        {
            self.base_screen_mut().record_event(reason.clone());
            return Err(reason);
        }

        let trimmed = cwd.trim();
        if trimmed.is_empty() {
            return Err(String::from("enter a workspace path first"));
        }
        let path = PathBuf::from(trimmed);
        if !path.exists() {
            return Err(format!("workspace does not exist: {trimmed}"));
        }
        if !path.is_dir() {
            return Err(format!("workspace is not a directory: {trimmed}"));
        }

        let lane_index = self.active_backend_index;
        if self.backend_lanes[lane_index].chat_runtime.cwd == path {
            self.base_screen_mut()
                .record_event(format!("workspace unchanged: {}", path.display()));
            self.last_status = format!("active workspace: {}", path.display());
            return Ok(());
        }

        {
            let lane = &mut self.backend_lanes[lane_index];
            lane.chat_runtime.cwd = path.clone();
            lane.session_generation = lane.session_generation.saturating_add(1);
            rebuild_lane_runtime(lane);
            lane.operator_backend = operator_summary_from_profile(&lane.chat_runtime.profile);
        }

        self.refresh_active_lane_after_runtime_change(format!(
            "workspace set to {}; next turn starts a fresh runtime session",
            path.display()
        ));
        self.last_status = format!("active workspace: {}", path.display());
        Ok(())
    }

    fn open_usage_overlay(&mut self) {
        self.base_screen_mut()
            .record_event("usage overlay took focus");
        self.last_status = String::from("opened usage overlay");
        if self.active_screen_id() != ScreenId::UsageOverlay {
            self.screens.push(ScreenState::Usage(UsageOverlay::new()));
        }
    }

    fn open_mcp_overlay(&mut self) {
        self.base_screen_mut()
            .record_event("MCP overlay took focus");
        self.last_status = String::from("opened MCP overlay");
        if self.active_screen_id() != ScreenId::McpOverlay {
            let (configured_count, enabled_count) = self.mcp_server_counts();
            self.screens.push(ScreenState::Mcp(McpOverlay::new(
                self.integration_cards(),
                configured_count,
                enabled_count,
            )));
        }
    }

    fn open_confirmation_overlay(&mut self, kind: ConfirmationKind) {
        let blocked = match kind {
            ConfirmationKind::FreshSession => {
                self.active_context_mutation_block_reason("start a fresh task")
            }
            ConfirmationKind::ClearContext => self.active_context_mutation_block_reason("clear"),
            ConfirmationKind::CompactContext => {
                self.active_context_mutation_block_reason("compact conversation")
            }
        };
        if let Some(reason) = blocked {
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        }
        let label = kind.label().to_string();
        self.base_screen_mut()
            .record_event(format!("{label} confirmation took focus"));
        self.last_status = format!("confirm {label}");
        let summary_preview = if kind == ConfirmationKind::CompactContext {
            Some(compact_summary_preview(
                self.base_screen().compact_summary_text().as_str(),
            ))
        } else {
            None
        };
        let lane_label = self.backend_lanes[self.active_backend_index].label.clone();
        let mode_label = self.backend_lanes[self.active_backend_index]
            .mode
            .label()
            .to_string();
        let model_id = self.backend_lanes[self.active_backend_index]
            .chat_runtime
            .profile
            .model
            .clone();
        let cwd = self.base_screen().current_workspace_label();
        let overlay =
            ConfirmationOverlay::new(kind, lane_label, mode_label, model_id, cwd, summary_preview);
        if self.active_screen_id() == ScreenId::ConfirmationOverlay {
            if let Some(ScreenState::Confirmation(screen)) = self.screens.last_mut() {
                *screen = overlay;
            }
        } else {
            self.screens.push(ScreenState::Confirmation(overlay));
        }
    }

    fn open_plan_mode_overlay(&mut self) {
        if let Some(reason) = self.active_runtime_reconfig_block_reason("change plan mode") {
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        }
        let current_plan_enabled =
            self.backend_lanes[self.active_backend_index].mode == LaneOperatorMode::Plan;
        self.base_screen_mut()
            .record_event("plan mode picker took focus");
        self.last_status = String::from("choose plan mode for this lane");
        if self.active_screen_id() == ScreenId::PlanModeOverlay {
            if let Some(ScreenState::PlanMode(screen)) = self.screens.last_mut() {
                *screen = PlanModeOverlay::new(current_plan_enabled);
            }
            return;
        }
        self.screens
            .push(ScreenState::PlanMode(PlanModeOverlay::new(
                current_plan_enabled,
            )));
    }

    fn apply_active_plan_mode(&mut self, enabled: bool) -> Result<(), String> {
        if let Some(reason) = self.active_runtime_reconfig_block_reason("change plan mode") {
            self.base_screen_mut().record_event(reason.clone());
            return Err(reason);
        }
        let lane_index = self.active_backend_index;
        let new_mode = if enabled {
            LaneOperatorMode::Plan
        } else {
            LaneOperatorMode::Coding
        };
        if self.backend_lanes[lane_index].mode == new_mode {
            self.base_screen_mut()
                .record_event(format!("mode unchanged: {}", new_mode.label()));
            self.last_status = format!("mode: {}", new_mode.label());
            return Ok(());
        }
        {
            let lane = &mut self.backend_lanes[lane_index];
            lane.mode = new_mode;
            lane.session_generation = lane.session_generation.saturating_add(1);
            rebuild_lane_runtime(lane);
            lane.operator_backend = operator_summary_from_profile(&lane.chat_runtime.profile);
        }
        self.refresh_active_lane_after_runtime_change(format!(
            "{} mode enabled; next turn will {}",
            new_mode.label(),
            if new_mode == LaneOperatorMode::Plan {
                "default to planning and refuse write-capable tools"
            } else {
                "resume normal coding behavior"
            }
        ));
        self.base_screen_mut()
            .set_local_action_notice(Some(format!("{} mode on", new_mode.label())));
        self.last_status = format!("mode: {}", new_mode.label());
        Ok(())
    }

    fn clear_active_context(&mut self) -> Result<(), String> {
        if let Some(reason) = self.active_context_mutation_block_reason("clear context") {
            self.base_screen_mut().record_event(reason.clone());
            return Err(reason);
        }
        let lane_index = self.active_backend_index;
        {
            let lane = &mut self.backend_lanes[lane_index];
            lane.carry_forward_summary = None;
            lane.session_generation = lane.session_generation.saturating_add(1);
            rebuild_lane_runtime(lane);
            lane.operator_backend = operator_summary_from_profile(&lane.chat_runtime.profile);
        }
        let refreshed = build_chat_lane(
            &self.backend_lanes,
            lane_index,
            ActiveTab::from_index(self.active_backend_index),
        );
        self.chat_lanes[lane_index] = refreshed.clone();
        self.screens[0] = ScreenState::Chat(refreshed);
        self.sync_backend_selector();
        self.base_screen_mut().record_event(
            "cleared conversation context; next turn starts fresh and repo files stay untouched",
        );
        self.base_screen_mut()
            .set_local_action_notice(Some(String::from("fresh context")));
        self.last_status = String::from("cleared context; repo files unchanged");
        Ok(())
    }

    fn compact_active_context(&mut self) -> Result<(), String> {
        if let Some(reason) = self.active_context_mutation_block_reason("compact conversation") {
            self.base_screen_mut().record_event(reason.clone());
            return Err(reason);
        }
        let summary = self.base_screen().compact_summary_text();
        let lane_index = self.active_backend_index;
        {
            let lane = &mut self.backend_lanes[lane_index];
            lane.carry_forward_summary = Some(summary.clone());
            lane.session_generation = lane.session_generation.saturating_add(1);
            rebuild_lane_runtime(lane);
            lane.operator_backend = operator_summary_from_profile(&lane.chat_runtime.profile);
        }
        let refreshed = build_chat_lane(
            &self.backend_lanes,
            lane_index,
            ActiveTab::from_index(self.active_backend_index),
        );
        self.chat_lanes[lane_index] = refreshed.clone();
        self.screens[0] = ScreenState::Chat(refreshed);
        self.sync_backend_selector();
        self.base_screen_mut().record_event(
            "compacted conversation; next turn starts from a Probe carry-forward summary",
        );
        self.base_screen_mut()
            .set_local_action_notice(Some(String::from("compact summary on")));
        self.last_status = String::from("compacted conversation into a carry-forward summary");
        Ok(())
    }

    fn recent_resume_sessions(&self) -> Result<Vec<ResumeSessionView>, String> {
        let probe_home = registry_probe_home(&self.backend_lanes)
            .or_else(|| default_probe_home().ok())
            .ok_or_else(|| String::from("Probe home is not available for session resume"))?;
        let store = FilesystemSessionStore::new(probe_home);
        let mut sessions = store.list_sessions().map_err(|error| error.to_string())?;
        sessions.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
        Ok(sessions
            .into_iter()
            .take(12)
            .map(|session| ResumeSessionView {
                id: session.id.as_str().to_string(),
                title: session.title.clone(),
                backend: session
                    .backend
                    .as_ref()
                    .map(|backend| backend.profile_name.clone())
                    .unwrap_or_else(|| String::from("unknown backend")),
                cwd: session.cwd.display().to_string(),
                turns: session.next_turn_index,
            })
            .collect())
    }

    fn resume_detached_session(&mut self, session_id: String) -> Result<(), String> {
        if let Some(reason) = self.active_runtime_reconfig_block_reason("resume another session") {
            self.base_screen_mut().record_event(reason.clone());
            return Err(reason);
        }

        let probe_home = registry_probe_home(&self.backend_lanes)
            .or_else(|| default_probe_home().ok())
            .ok_or_else(|| String::from("Probe home is not available for session resume"))?;
        let store = FilesystemSessionStore::new(probe_home);
        let metadata = store
            .read_metadata(&probe_protocol::session::SessionId::new(
                session_id.as_str(),
            ))
            .map_err(|error| error.to_string())?;
        let backend = metadata
            .backend
            .as_ref()
            .ok_or_else(|| String::from("that session does not have a stored backend target"))?;
        let lane_index = backend_lane_index_for_kind(
            named_backend_profile(backend.profile_name.as_str())
                .ok_or_else(|| format!("unknown backend profile {}", backend.profile_name))?
                .kind,
        );
        {
            let lane = &mut self.backend_lanes[lane_index];
            lane.chat_runtime.profile = session_profile(&metadata)?;
            lane.chat_runtime.cwd = metadata.cwd.clone();
            lane.session_generation = lane.session_generation.saturating_add(1);
            rebuild_lane_runtime(lane);
            lane.operator_backend = operator_summary_from_profile(&lane.chat_runtime.profile);
        }
        self.persist_active_chat_lane();
        self.active_backend_index = lane_index;
        self.restore_chat_lane(lane_index, ActiveTab::from_index(lane_index));
        self.last_status = format!("resuming session {}", metadata.title);
        self.base_screen_mut().record_event(format!(
            "resuming detached session {}",
            metadata.id.as_str()
        ));
        self.submit_background_task(BackgroundTaskRequest::attach_probe_runtime_session(
            session_id,
            self.active_chat_runtime().clone(),
        ))
    }

    fn active_context_mutation_block_reason(&self, action: &str) -> Option<String> {
        if self.base_screen().has_pending_tool_approvals() {
            return Some(format!("resolve pending approvals before you {action}"));
        }
        if self.base_screen().has_in_flight_runtime_activity()
            || matches!(
                self.base_screen().task_phase(),
                TaskPhase::Queued | TaskPhase::CheckingAvailability | TaskPhase::Running
            )
        {
            return Some(format!(
                "wait for the current turn to finish before you {action}"
            ));
        }
        if action.contains("compact") && self.base_screen().committed_transcript_entry_count() == 0
        {
            return Some(String::from("there is no conversation to compact yet"));
        }
        None
    }

    fn active_runtime_reconfig_block_reason(&self, action: &str) -> Option<String> {
        if self.base_screen().has_pending_tool_approvals() {
            return Some(format!("resolve pending approvals before you {action}"));
        }
        if self.base_screen().has_in_flight_runtime_activity()
            || matches!(
                self.base_screen().task_phase(),
                TaskPhase::Queued | TaskPhase::CheckingAvailability | TaskPhase::Running
            )
        {
            return Some(format!(
                "wait for the current turn to finish before you {action}"
            ));
        }
        None
    }

    fn refresh_active_lane_after_runtime_change(&mut self, note: String) {
        let lane_index = self.active_backend_index;
        let lane = &self.backend_lanes[lane_index];
        let mut refreshed_lane = self.base_screen().clone();
        refreshed_lane.apply_runtime_config_change(
            lane.label.clone(),
            &lane.chat_runtime,
            lane.operator_backend.clone(),
            lane.mode.label(),
            lane.carry_forward_summary.as_deref(),
            note,
        );
        self.chat_lanes[lane_index] = refreshed_lane.clone();
        self.screens[0] = ScreenState::Chat(refreshed_lane);
        self.sync_backend_selector();
    }

    fn integration_cards(&self) -> Vec<IntegrationCardView> {
        let configured_count = self.mcp_registry.servers.len();
        let enabled_count = self
            .mcp_registry
            .servers
            .iter()
            .filter(|server| server.enabled)
            .count();
        let mut cards = vec![
            IntegrationCardView {
                label: String::from("Add MCP server"),
                status: String::from("ready"),
                detail_lines: vec![
                    String::from("Add a saved MCP integration entry for this Probe home."),
                    String::from("Choose the standard provider-command flow or manual setup."),
                ],
                next_step: String::from("Press Enter to open the MCP add menu."),
                toggle_server_id: None,
                remove_server_id: None,
                opens_server_list: false,
                opens_editor: true,
            },
            IntegrationCardView {
                label: String::from("Saved MCP servers"),
                status: if configured_count == 0 {
                    String::from("none")
                } else {
                    format!("{configured_count} configured")
                },
                detail_lines: vec![
                    format!("configured: {configured_count}"),
                    format!("enabled: {enabled_count}"),
                    String::from(
                        "Open this to browse the saved MCP server list for this Probe home.",
                    ),
                ],
                next_step: String::from("Press Enter to manage saved MCP servers."),
                toggle_server_id: None,
                remove_server_id: None,
                opens_server_list: true,
                opens_editor: false,
            },
            self.backend_lanes_card(),
            self.codex_auth_card(),
            self.apple_fm_bridge_card(),
        ];
        cards.push(self.generic_mcp_card());
        cards
    }

    fn mcp_server_counts(&self) -> (usize, usize) {
        let configured_count = self.mcp_registry.servers.len();
        let enabled_count = self
            .mcp_registry
            .servers
            .iter()
            .filter(|server| server.enabled)
            .count();
        (configured_count, enabled_count)
    }

    fn managed_mcp_server_views(&self) -> Vec<ManagedMcpServerView> {
        self.mcp_registry
            .servers
            .iter()
            .map(|server| {
                let mut detail_lines = vec![format!("entry type: {}", server.source.label())];
                match server.source {
                    McpServerSource::ManualLaunch => {
                        if let Some(transport) = server.transport.as_ref() {
                            detail_lines.push(format!("connection type: {}", transport.label()));
                        }
                        if let Some(target) = server.target.as_ref() {
                            detail_lines.push(format!("launch command or URL: {target}"));
                        }
                    }
                    McpServerSource::ProviderCommandRecipe => {
                        if let Some(provider_hint) = server.provider_hint.as_ref() {
                            detail_lines.push(format!("provider: {provider_hint}"));
                        }
                        if let Some(client_hint) = server.client_hint.as_ref() {
                            detail_lines.push(format!("client hint: {client_hint}"));
                        }
                        if let Some(command) = server.provider_setup_command.as_ref() {
                            detail_lines.push(format!("setup command: {command}"));
                        }
                    }
                }
                detail_lines.push(String::from(
                    "configured only: Probe does not execute external MCP servers yet.",
                ));
                ManagedMcpServerView {
                    label: server.name.clone(),
                    enabled: server.enabled,
                    status: if server.enabled {
                        String::from("enabled")
                    } else {
                        String::from("disabled")
                    },
                    detail_lines,
                    toggle_server_id: server.id.clone(),
                    remove_server_id: server.id.clone(),
                }
            })
            .collect()
    }

    fn backend_lanes_card(&self) -> IntegrationCardView {
        let active_label = self.backend_lanes[self.active_backend_index].label.clone();
        let detail_lines = self
            .backend_lanes
            .iter()
            .enumerate()
            .map(|(index, lane)| {
                let active_suffix = if index == self.active_backend_index {
                    " (active)"
                } else {
                    ""
                };
                format!(
                    "{}{}: {} · model {}",
                    lane.label,
                    active_suffix,
                    lane.operator_backend.endpoint_label(),
                    lane.chat_runtime.profile.model
                )
            })
            .collect::<Vec<_>>();
        IntegrationCardView {
            label: String::from("Backend lanes"),
            status: format!("active: {active_label}"),
            detail_lines,
            next_step: String::from(
                "Use Tab to switch lanes, /backend for details, or /model for the active lane.",
            ),
            toggle_server_id: None,
            remove_server_id: None,
            opens_server_list: false,
            opens_editor: false,
        }
    }

    fn codex_auth_card(&self) -> IntegrationCardView {
        let probe_home = self
            .backend_lanes
            .iter()
            .find_map(|lane| lane.chat_runtime.probe_home.as_deref());
        let (status, mut detail_lines, next_step) = if let Some(probe_home) = probe_home {
            let store = OpenAiCodexAuthStore::new(probe_home);
            match store.status() {
                Ok(auth) if auth.authenticated && !auth.expired => (
                    String::from("connected"),
                    vec![
                        String::from("ChatGPT subscription auth is available for hosted Codex."),
                        format!("auth file: {}", auth.path.display()),
                    ],
                    String::from("Use the Codex lane directly or switch to it with Tab."),
                ),
                Ok(auth) if auth.expired => (
                    String::from("expired"),
                    vec![
                        String::from("The saved Codex subscription auth record has expired."),
                        format!("auth file: {}", auth.path.display()),
                    ],
                    String::from("Run `cargo run -p probe-cli -- codex login --method browser`."),
                ),
                Ok(auth) => (
                    String::from("login required"),
                    vec![
                        String::from("No saved ChatGPT subscription auth record is available."),
                        format!("auth file: {}", auth.path.display()),
                    ],
                    String::from("Run `cargo run -p probe-cli -- codex login --method browser`."),
                ),
                Err(error) => (
                    String::from("unreadable"),
                    vec![format!(
                        "Probe could not read the Codex auth store: {error}"
                    )],
                    String::from("Re-run Codex login or inspect the auth file permissions."),
                ),
            }
        } else {
            (
                String::from("unavailable"),
                vec![String::from(
                    "This TUI session does not expose a probe_home, so saved Codex auth cannot be inspected.",
                )],
                String::from("Launch Probe with a normal probe_home to manage Codex auth."),
            )
        };
        if self.active_backend_kind() == BackendKind::OpenAiCodexSubscription {
            detail_lines.push(String::from(
                "The active lane uses this auth path for hosted Codex responses.",
            ));
        }
        IntegrationCardView {
            label: String::from("Codex auth"),
            status,
            detail_lines,
            next_step,
            toggle_server_id: None,
            remove_server_id: None,
            opens_server_list: false,
            opens_editor: false,
        }
    }

    fn apple_fm_bridge_card(&self) -> IntegrationCardView {
        let lane = &self.backend_lanes[2];
        let phase = self.chat_lanes[2].task_phase();
        let (status, next_step) = match phase {
            TaskPhase::Completed => (
                String::from("ready"),
                String::from("Press Tab to switch to Apple FM when you want the local bridge."),
            ),
            TaskPhase::Unavailable | TaskPhase::Failed => (
                String::from("needs attention"),
                String::from("Switch to Apple FM and press Ctrl+R to re-check the bridge."),
            ),
            TaskPhase::CheckingAvailability | TaskPhase::Queued | TaskPhase::Running => (
                String::from("checking"),
                String::from("Stay on the Apple FM lane until the bridge check finishes."),
            ),
            TaskPhase::Idle => (
                String::from("not checked"),
                String::from("Switch to Apple FM and press Ctrl+R to verify the local bridge."),
            ),
        };
        IntegrationCardView {
            label: String::from("Apple FM bridge"),
            status,
            detail_lines: vec![
                format!("lane: {}", lane.label),
                format!("target: {}", lane.operator_backend.endpoint_label()),
                format!("model: {}", lane.chat_runtime.profile.model),
            ],
            next_step,
            toggle_server_id: None,
            remove_server_id: None,
            opens_server_list: false,
            opens_editor: false,
        }
    }

    fn generic_mcp_card(&self) -> IntegrationCardView {
        IntegrationCardView {
            label: String::from("Generic MCP"),
            status: String::from("not shipped"),
            detail_lines: vec![
                String::from("Probe has no runtime registry for external MCP servers yet."),
                String::from(
                    "Configured servers are stored locally but are not yet mounted into tool execution.",
                ),
                String::from(
                    "External tool inventories and per-turn MCP usage receipts are not implemented yet.",
                ),
            ],
            next_step: String::from(
                "Use Add MCP server to create entries. Runtime tool mounting is still future work.",
            ),
            toggle_server_id: None,
            remove_server_id: None,
            opens_server_list: false,
            opens_editor: false,
        }
    }

    fn save_mcp_server(
        &mut self,
        name: String,
        transport: McpServerTransportDraft,
        target: String,
    ) -> Result<(), String> {
        let id = next_mcp_server_id(&self.mcp_registry, name.as_str());
        self.mcp_registry.servers.push(McpServerRecord {
            id,
            name: name.clone(),
            enabled: true,
            source: McpServerSource::ManualLaunch,
            transport: Some(match transport {
                McpServerTransportDraft::Stdio => McpServerTransport::Stdio,
                McpServerTransportDraft::Http => McpServerTransport::Http,
            }),
            target: Some(target),
            provider_setup_command: None,
            provider_hint: None,
            client_hint: None,
        });
        self.persist_mcp_registry()?;
        self.refresh_mcp_overlay_selecting(Some("Saved MCP servers"));
        self.refresh_mcp_servers_overlay_selecting(Some(name.as_str()));
        self.base_screen_mut()
            .record_event(format!("saved MCP server {name}"));
        self.last_status = format!("saved MCP server {name}");
        Ok(())
    }

    fn import_mcp_provider_command(&mut self, command: String) -> Result<(), String> {
        let name = imported_mcp_name(command.as_str());
        let provider_hint = infer_provider_hint(command.as_str());
        let client_hint = infer_client_hint(command.as_str());
        let id = next_mcp_server_id(&self.mcp_registry, name.as_str());
        self.mcp_registry.servers.push(McpServerRecord {
            id,
            name: name.clone(),
            enabled: true,
            source: McpServerSource::ProviderCommandRecipe,
            transport: None,
            target: None,
            provider_setup_command: Some(command),
            provider_hint,
            client_hint,
        });
        self.persist_mcp_registry()?;
        self.refresh_mcp_overlay_selecting(Some("Saved MCP servers"));
        self.refresh_mcp_servers_overlay_selecting(Some(name.as_str()));
        self.base_screen_mut()
            .record_event(format!("imported MCP recipe {name}"));
        self.last_status = format!("imported MCP recipe {name}");
        Ok(())
    }

    fn toggle_mcp_server_enabled(&mut self, server_id: String) -> Result<(), String> {
        let Some(server) = self
            .mcp_registry
            .servers
            .iter_mut()
            .find(|server| server.id == server_id)
        else {
            return Err(String::from("that MCP server no longer exists"));
        };
        server.enabled = !server.enabled;
        let status = if server.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let name = server.name.clone();
        self.persist_mcp_registry()?;
        self.refresh_mcp_overlay_selecting(Some("Saved MCP servers"));
        self.refresh_mcp_servers_overlay_selecting(Some(name.as_str()));
        self.base_screen_mut()
            .record_event(format!("{status} MCP server {name}"));
        self.last_status = format!("{status} MCP server {name}");
        Ok(())
    }

    fn remove_mcp_server(&mut self, server_id: String) -> Result<(), String> {
        let Some(index) = self
            .mcp_registry
            .servers
            .iter()
            .position(|server| server.id == server_id)
        else {
            return Err(String::from("that MCP server no longer exists"));
        };
        let name = self.mcp_registry.servers[index].name.clone();
        self.mcp_registry.servers.remove(index);
        self.persist_mcp_registry()?;
        self.refresh_mcp_overlay_selecting(Some("Saved MCP servers"));
        self.refresh_mcp_servers_overlay_selecting(None);
        self.base_screen_mut()
            .record_event(format!("removed MCP server {name}"));
        self.last_status = format!("removed MCP server {name}");
        Ok(())
    }

    fn persist_mcp_registry(&self) -> Result<(), String> {
        save_mcp_registry(
            registry_probe_home(&self.backend_lanes).as_deref(),
            &self.mcp_registry,
        )
    }

    fn refresh_mcp_overlay_selecting(&mut self, preferred_label: Option<&str>) {
        let cards = self.integration_cards();
        let (configured_count, enabled_count) = self.mcp_server_counts();
        let preferred_label = preferred_label.map(str::to_string);
        if let Some(ScreenState::Mcp(screen)) = self
            .screens
            .iter_mut()
            .find(|screen| matches!(screen, ScreenState::Mcp(_)))
        {
            let current_label = screen.selected_label().map(str::to_string);
            let selected_label = preferred_label.or(current_label);
            let selected_index = selected_label
                .as_ref()
                .and_then(|label| cards.iter().position(|card| card.label == *label))
                .unwrap_or(0);
            *screen =
                McpOverlay::with_selected(cards, configured_count, enabled_count, selected_index);
        }
    }

    fn refresh_mcp_servers_overlay_selecting(&mut self, preferred_label: Option<&str>) {
        let servers = self.managed_mcp_server_views();
        let preferred_label = preferred_label.map(str::to_string);
        if let Some(ScreenState::McpServers(screen)) = self
            .screens
            .iter_mut()
            .find(|screen| matches!(screen, ScreenState::McpServers(_)))
        {
            let current_label = screen.selected_label().map(str::to_string);
            let selected_label = preferred_label.or(current_label);
            let selected_index = selected_label
                .as_ref()
                .and_then(|label| servers.iter().position(|server| server.label == *label))
                .unwrap_or(0);
            *screen = McpServersOverlay::with_selected(servers, selected_index);
        }
    }
}

fn compact_summary_preview(summary: &str) -> String {
    summary
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(6)
        .map(|line| {
            let mut preview = line.chars().take(100).collect::<String>();
            if line.chars().count() > 100 {
                preview.push_str("...");
            }
            preview
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
fn resolve_tui_chat_profile(probe_home: Option<&Path>) -> BackendProfile {
    AppShell::chat_profile_and_summary_from_probe_home(probe_home).0
}

fn load_server_config(probe_home: &Path) -> Option<PsionicServerConfig> {
    let config_path = PsionicServerConfig::config_path(probe_home);
    PsionicServerConfig::load_or_create(config_path.as_path()).ok()
}

fn registry_probe_home(backend_lanes: &[BackendLaneConfig; 3]) -> Option<PathBuf> {
    backend_lanes
        .iter()
        .find_map(|lane| lane.chat_runtime.probe_home.clone())
}

fn default_mcp_server_source() -> McpServerSource {
    McpServerSource::ManualLaunch
}

fn mcp_registry_path(probe_home: &Path) -> PathBuf {
    probe_home.join("mcp/servers.json")
}

fn load_mcp_registry(probe_home: Option<&Path>) -> Result<McpRegistryFile, String> {
    let Some(probe_home) = probe_home else {
        return Ok(McpRegistryFile::default());
    };
    let path = mcp_registry_path(probe_home);
    if !path.exists() {
        return Ok(McpRegistryFile::default());
    }
    let raw = std::fs::read_to_string(path.as_path()).map_err(|error| error.to_string())?;
    serde_json::from_str(raw.as_str()).map_err(|error| error.to_string())
}

fn save_mcp_registry(probe_home: Option<&Path>, registry: &McpRegistryFile) -> Result<(), String> {
    let Some(probe_home) = probe_home else {
        return Err(String::from(
            "Probe needs a probe_home path before it can save MCP servers",
        ));
    };
    let path = mcp_registry_path(probe_home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let raw = serde_json::to_vec_pretty(registry).map_err(|error| error.to_string())?;
    std::fs::write(path.as_path(), raw).map_err(|error| error.to_string())
}

fn next_mcp_server_id(registry: &McpRegistryFile, name: &str) -> String {
    let slug = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    let base = if slug.is_empty() {
        String::from("mcp-server")
    } else {
        slug
    };
    let mut candidate = base.clone();
    let mut suffix = 2usize;
    while registry.servers.iter().any(|server| server.id == candidate) {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    candidate
}

fn infer_provider_hint(command: &str) -> Option<String> {
    command
        .split_whitespace()
        .map(|token| token.trim_matches(|ch: char| ch == '"' || ch == '\''))
        .find_map(|token| {
            if token.starts_with('-') {
                return None;
            }
            let package = token.split('@').next().unwrap_or(token);
            let package = package.rsplit('/').next().unwrap_or(package);
            if package.is_empty()
                || matches!(package, "pnpm" | "dlx" | "npx" | "npm" | "bunx" | "yarn")
            {
                None
            } else {
                Some(package.to_string())
            }
        })
}

fn infer_client_hint(command: &str) -> Option<String> {
    let mut tokens = command.split_whitespace();
    while let Some(token) = tokens.next() {
        if token == "--client" {
            return tokens.next().map(|value| {
                value
                    .trim_matches(|ch: char| ch == '"' || ch == '\'')
                    .to_string()
            });
        }
        if let Some(value) = token.strip_prefix("--client=") {
            return Some(
                value
                    .trim_matches(|ch: char| ch == '"' || ch == '\'')
                    .to_string(),
            );
        }
    }
    None
}

fn imported_mcp_name(command: &str) -> String {
    infer_provider_hint(command)
        .map(|provider| format!("{provider} MCP"))
        .unwrap_or_else(|| String::from("Imported MCP"))
}

fn read_system_clipboard_text() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("pbpaste")
            .output()
            .map_err(|error| format!("failed to read system clipboard: {error}"))?;
        if !output.status.success() {
            return Err(String::from("failed to read system clipboard"));
        }
        return String::from_utf8(output.stdout)
            .map(|text| text.replace("\r\n", "\n"))
            .map_err(|error| format!("clipboard is not valid UTF-8: {error}"));
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(String::from(
            "system clipboard fallback is only implemented on macOS right now",
        ))
    }
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
    profile.reasoning_level = config.reasoning_level.clone();
    profile
}

fn server_config_from_profile(profile: &BackendProfile) -> PsionicServerConfig {
    PsionicServerConfig::from_backend_profile(profile)
}

fn session_profile(metadata: &SessionMetadata) -> Result<BackendProfile, String> {
    let backend = metadata
        .backend
        .as_ref()
        .ok_or_else(|| String::from("detached session does not have a stored backend target"))?;
    let mut profile = named_backend_profile(backend.profile_name.as_str())
        .ok_or_else(|| format!("unknown backend profile {}", backend.profile_name))?;
    profile.base_url = backend.base_url.clone();
    profile.model = backend.model.clone();
    Ok(profile)
}

fn operator_summary_from_profile(profile: &BackendProfile) -> ServerOperatorSummary {
    server_config_from_profile(profile).operator_summary()
}

fn backend_lane_index_for_kind(backend_kind: BackendKind) -> usize {
    BACKEND_SELECTOR_ORDER
        .iter()
        .position(|kind| *kind == backend_kind)
        .unwrap_or(0)
}

fn runtime_for_profile(
    base: &ProbeRuntimeTurnConfig,
    profile: BackendProfile,
) -> ProbeRuntimeTurnConfig {
    build_chat_runtime_config_for_lane(
        base.probe_home.clone(),
        base.cwd.clone(),
        profile,
        LaneOperatorMode::Coding,
        None,
        base.session_generation,
    )
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

fn available_models_for_backend(kind: BackendKind, current_model: &str) -> Vec<String> {
    let mut models = match kind {
        BackendKind::OpenAiCodexSubscription => vec![
            String::from("gpt-5.4"),
            String::from("gpt-5.4-mini"),
            String::from("gpt-5.3-codex"),
            String::from("gpt-5.2"),
            String::from("gpt-5.2-codex"),
            String::from("gpt-5.1-codex"),
            String::from("gpt-5.1-codex-max"),
            String::from("gpt-5.1-codex-mini"),
        ],
        BackendKind::OpenAiChatCompletions | BackendKind::AppleFmBridge => {
            vec![current_model.to_string()]
        }
    };
    if !models.iter().any(|model| model == current_model) {
        models.insert(0, current_model.to_string());
    }
    models
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
        mode: LaneOperatorMode::Coding,
        carry_forward_summary: None,
        session_generation: 0,
    }
}

fn build_backend_lanes(config: &TuiLaunchConfig) -> [BackendLaneConfig; 3] {
    BACKEND_SELECTOR_ORDER.map(|backend_kind| {
        if backend_kind == config.operator_backend.backend_kind {
            BackendLaneConfig {
                label: backend_selector_label(&config.operator_backend),
                chat_runtime: config.chat_runtime.clone(),
                operator_backend: config.operator_backend.clone(),
                mode: LaneOperatorMode::Coding,
                carry_forward_summary: None,
                session_generation: 0,
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
    screen.set_runtime_context(
        backend_lanes[lane_index].label.clone(),
        &backend_lanes[lane_index].chat_runtime,
    );
    screen.set_backend_selector(
        backend_lanes
            .iter()
            .map(|lane| lane.label.clone())
            .collect(),
        active_tab,
    );
    screen.set_probe_home(backend_lanes[lane_index].chat_runtime.probe_home.clone());
    screen.set_operator_backend(backend_lanes[lane_index].operator_backend.clone());
    screen.set_operator_controls(
        backend_lanes[lane_index].mode.label(),
        backend_lanes[lane_index].carry_forward_summary.as_deref(),
    );
    screen
}

fn operator_system_addendum(
    mode: LaneOperatorMode,
    carry_forward_summary: Option<&str>,
) -> Option<String> {
    let mut parts = Vec::new();
    if mode == LaneOperatorMode::Plan {
        parts.push(String::from(
            "Plan mode is active.\n- Default to planning, sequencing, risk review, and implementation guidance.\n- Do not make or imply file edits as if they already happened.\n- Avoid write-capable tools unless the operator explicitly switches back to coding mode.",
        ));
    }
    if let Some(summary) = carry_forward_summary.filter(|value| !value.trim().is_empty()) {
        parts.push(format!(
            "Carry-forward context from the prior session:\n{summary}\nUse this summary instead of assuming the full prior transcript is available."
        ));
    }
    (!parts.is_empty()).then_some(parts.join("\n\n"))
}

fn tool_loop_for_mode(mode: LaneOperatorMode) -> ToolLoopConfig {
    let mut tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Auto, false);
    tool_loop.approval = match mode {
        LaneOperatorMode::Coding => ToolApprovalConfig::allow_all(),
        LaneOperatorMode::Plan => ToolApprovalConfig::conservative(),
    };
    tool_loop
}

fn build_chat_runtime_config_for_lane(
    probe_home: Option<PathBuf>,
    cwd: PathBuf,
    profile: BackendProfile,
    mode: LaneOperatorMode,
    carry_forward_summary: Option<String>,
    session_generation: u64,
) -> ProbeRuntimeTurnConfig {
    let operator_system = operator_system_addendum(mode, carry_forward_summary.as_deref());
    let (system_prompt, harness_profile) = resolve_prompt_contract(
        Some("coding_bootstrap"),
        None,
        cwd.as_path(),
        operator_system.as_deref(),
        profile.kind,
    )
    .unwrap_or((operator_system, None));
    ProbeRuntimeTurnConfig {
        probe_home,
        cwd,
        profile,
        system_prompt,
        harness_profile,
        tool_loop: Some(tool_loop_for_mode(mode)),
        session_generation,
    }
}

fn rebuild_lane_runtime(lane: &mut BackendLaneConfig) {
    lane.chat_runtime = build_chat_runtime_config_for_lane(
        lane.chat_runtime.probe_home.clone(),
        lane.chat_runtime.cwd.clone(),
        lane.chat_runtime.profile.clone(),
        lane.mode,
        lane.carry_forward_summary.clone(),
        lane.session_generation,
    );
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
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;

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
        DisableMouseCapture,
        DisableBracketedPaste
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
    use std::sync::{Mutex, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::McpServerSource;

    use probe_core::backend_profiles::{
        openai_codex_subscription, psionic_apple_fm_bridge, psionic_qwen35_2b_q8_registry,
    };
    use probe_core::harness::resolve_prompt_contract;
    use probe_core::runtime::RuntimeEvent;
    use probe_core::server_control::PsionicServerConfig;
    use probe_core::session_store::{FilesystemSessionStore, NewSession};
    use probe_core::tools::{ExecutedToolCall, ProbeToolChoice, ToolLoopConfig};
    use probe_protocol::backend::BackendKind;
    use probe_protocol::runtime::{RuntimeActivity, RuntimeActivityKind};
    use probe_protocol::session::{
        PendingToolApproval, SessionBackendTarget, SessionId, TaskFinalReceipt,
        TaskReceiptDisposition, TaskVerificationCommandStatus, TaskVerificationCommandSummary,
        TaskVerificationStatus, TaskWorkspaceSummary, TaskWorkspaceSummaryStatus,
        ToolApprovalState, ToolExecutionRecord, ToolPolicyDecision, ToolRiskClass,
    };
    use probe_test_support::{
        FakeAppleFmServer, FakeHttpResponse, FakeOpenAiServer, ProbeTestEnvironment,
    };
    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        AppShell, LaneOperatorMode, McpRegistryFile, McpServerRecord, McpServerTransport,
        TuiLaunchConfig, mcp_registry_path, profile_from_server_config, resolve_tui_chat_profile,
        save_mcp_registry,
    };
    use crate::bottom_pane::ComposerSubmission;
    use crate::event::UiEvent;
    use crate::message::{
        AppMessage, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
        AppleFmUsageSummary, BackgroundTaskRequest, ProbeRuntimeTurnConfig, SessionUsageSummary,
        UsageCountsSummary,
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
            session_generation: 0,
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
            session_generation: 0,
        }
    }

    fn apple_fm_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static APPLE_FM_TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
        APPLE_FM_TEST_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| error.into_inner())
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
            "timed out waiting for app condition; last_status={}; worker_events={:?}; recent_events={:?}; frame=\n{}",
            app.last_status(),
            app.worker_events(),
            app.recent_events(),
            app.render_to_string(120, 32)
        );
    }

    fn executed_tool(
        call_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
        output: serde_json::Value,
        risk_class: ToolRiskClass,
    ) -> ExecutedToolCall {
        ExecutedToolCall {
            call_id: call_id.to_string(),
            name: tool_name.to_string(),
            arguments,
            output,
            tool_execution: ToolExecutionRecord {
                risk_class,
                policy_decision: ToolPolicyDecision::AutoAllow,
                approval_state: ToolApprovalState::NotRequired,
                command: None,
                exit_code: None,
                timed_out: None,
                truncated: None,
                bytes_returned: None,
                files_touched: Vec::new(),
                files_changed: Vec::new(),
                reason: None,
            },
        }
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
    fn backend_selector_orders_codex_then_qwen_then_apple_fm() {
        let app = AppShell::new_for_tests();

        assert_eq!(
            app.backend_selector_labels(),
            vec![
                String::from("Codex"),
                String::from("Qwen"),
                String::from("Apple FM"),
            ]
        );
        assert_eq!(app.active_tab(), ActiveTab::Secondary);
    }

    #[test]
    fn main_shell_shows_backend_workspace_and_approval_posture_without_overlay() {
        let app = AppShell::new_for_tests();

        let rendered = app.render_to_string(120, 32);
        assert!(rendered.contains("lane: Qwen"));
        assert!(rendered.contains("backend: openai_chat_completions"));
        assert!(rendered.contains("loopback/ssh"));
        assert!(rendered.contains("127.0.0.1:8080"));
        assert!(rendered.contains("cwd:"));
        assert!(rendered.contains("tools: on"));
        assert!(rendered.contains("approvals:"));
    }

    #[test]
    fn main_shell_shows_task_receipt_and_preexisting_dirty_files() {
        let mut app = AppShell::new_for_tests();
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_qwen_receipt"),
            profile_name: String::from("psionic-qwen35-2b-q8-registry"),
            model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: Some(TaskWorkspaceSummary {
                task_start_turn_index: 3,
                status: TaskWorkspaceSummaryStatus::Changed,
                changed_files: vec![String::from("src/lib.rs")],
                touched_but_unchanged_files: vec![String::from("README.md")],
                preexisting_dirty_files: vec![String::from("Cargo.toml")],
                outside_tracking_dirty_files: Vec::new(),
                repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
                change_accounting_limited: false,
                summary_text: String::from(
                    "This task changed 1 file(s): src/lib.rs. Dirty before task start: Cargo.toml.",
                ),
            }),
            latest_task_receipt: Some(TaskFinalReceipt {
                disposition: TaskReceiptDisposition::Succeeded,
                workspace: TaskWorkspaceSummary {
                    task_start_turn_index: 3,
                    status: TaskWorkspaceSummaryStatus::Changed,
                    changed_files: vec![String::from("src/lib.rs")],
                    touched_but_unchanged_files: vec![String::from("README.md")],
                    preexisting_dirty_files: vec![String::from("Cargo.toml")],
                    outside_tracking_dirty_files: Vec::new(),
                    repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
                    change_accounting_limited: false,
                    summary_text: String::from(
                        "This task changed 1 file(s): src/lib.rs. Dirty before task start: Cargo.toml.",
                    ),
                },
                verification_status: TaskVerificationStatus::Passed,
                verification_commands: vec![TaskVerificationCommandSummary {
                    command: String::from("cargo test -p probe-tui"),
                    status: TaskVerificationCommandStatus::Passed,
                    exit_code: Some(0),
                    truncated_output: false,
                }],
                uncertainty_reasons: Vec::new(),
                summary_text: String::from(
                    "This task changed 1 file(s): src/lib.rs. Validation passed: cargo test -p probe-tui (passed).",
                ),
            }),
            recovery_note: None,
        });

        let rendered = app.render_to_string(120, 32);
        assert!(rendered.contains("edits: src/lib.rs"));
        assert!(rendered.contains("dirty_before: Cargo.toml"));
        assert!(rendered.contains("verify:"));
        assert!(!rendered.contains("edits: none yet"));
    }

    #[test]
    fn main_shell_shows_runtime_activity_banner_for_live_turns() {
        let mut app = AppShell::new_for_tests();
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_qwen_activity"),
            profile_name: String::from("psionic-qwen35-2b-q8-registry"),
            model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: Some(RuntimeActivity::new(
                RuntimeActivityKind::Validating,
                "validating with cargo test -p probe-tui",
            )),
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            recovery_note: None,
        });

        let rendered = app.render_to_string(120, 32);
        assert!(
            app.base_screen()
                .compact_runtime_status()
                .contains("activity: validating with cargo")
        );
        assert!(!rendered.contains("phase:"));
    }

    #[test]
    fn main_shell_shows_active_edit_targets_while_runtime_is_working() {
        let mut app = AppShell::new_for_tests();
        let session_id = SessionId::new("sess_qwen_editing");
        app.apply_message(AppMessage::ProbeRuntimeEvent {
            event: RuntimeEvent::ToolCallRequested {
                session_id: session_id.clone(),
                round_trip: 2,
                call_id: String::from("call_patch_1"),
                tool_name: String::from("apply_patch"),
                arguments: json!({
                    "path": "crates/probe-tui/src/screens.rs",
                    "old_text": "before",
                    "new_text": "after"
                }),
            },
        });
        app.apply_message(AppMessage::ProbeRuntimeEvent {
            event: RuntimeEvent::ToolExecutionStarted {
                session_id,
                round_trip: 2,
                call_id: String::from("call_patch_1"),
                tool_name: String::from("apply_patch"),
                risk_class: ToolRiskClass::Write,
            },
        });

        let rendered = app.render_to_string(140, 40);
        assert!(
            rendered.contains("updating: crates/probe-tui/src/screens.rs"),
            "{rendered}"
        );
        assert!(
            rendered.contains("[active status] Planning Tool Call"),
            "{rendered}"
        );
    }

    #[test]
    fn main_shell_turns_approval_waits_into_action_needed_copy() {
        let mut app = AppShell::new_for_tests();
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_qwen_approval"),
            profile_name: String::from("psionic-qwen35-2b-q8-registry"),
            model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: Some(RuntimeActivity::new(
                RuntimeActivityKind::WaitingForApproval,
                "waiting for approval: apply_patch",
            )),
            latest_task_workspace_summary: None,
            latest_task_receipt: Some(TaskFinalReceipt {
                disposition: TaskReceiptDisposition::PendingApproval,
                workspace: TaskWorkspaceSummary {
                    task_start_turn_index: 7,
                    status: TaskWorkspaceSummaryStatus::PendingApproval,
                    changed_files: Vec::new(),
                    touched_but_unchanged_files: Vec::new(),
                    preexisting_dirty_files: Vec::new(),
                    outside_tracking_dirty_files: Vec::new(),
                    repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
                    change_accounting_limited: false,
                    summary_text: String::from(
                        "This task is waiting for approval. No repo changes have landed yet.",
                    ),
                },
                verification_status: TaskVerificationStatus::NotRun,
                verification_commands: Vec::new(),
                uncertainty_reasons: vec![String::from(
                    "The task is still waiting for approval and may continue changing the workspace after approval.",
                )],
                summary_text: String::from(
                    "This task is waiting for approval. No validation command has completed yet.",
                ),
            }),
            recovery_note: Some(String::from(
                "daemon restart can resume this session after the pending approval is resolved",
            )),
        });
        app.apply_message(AppMessage::PendingToolApprovalsUpdated {
            session_id: String::from("sess_qwen_approval"),
            approvals: vec![PendingToolApproval {
                session_id: SessionId::new("sess_qwen_approval"),
                tool_call_id: String::from("call_patch_1"),
                tool_name: String::from("apply_patch"),
                arguments: json!({
                    "path": "hello.txt",
                    "old_text": "world",
                    "new_text": "probe"
                }),
                risk_class: ToolRiskClass::Write,
                reason: Some(String::from("tool `apply_patch` requires write approval")),
                tool_call_turn_index: 1,
                paused_result_turn_index: 2,
                requested_at_ms: 1,
                resolved_at_ms: None,
                resolution: None,
            }],
        });
        app.dispatch(UiEvent::Dismiss);

        let _rendered = app.render_to_string(120, 32);
        assert!(
            app.base_screen()
                .compact_runtime_status()
                .contains("activity: action needed")
        );
    }

    #[test]
    fn main_shell_surfaces_dirty_files_outside_tracked_tool_results() {
        let mut app = AppShell::new_for_tests();
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_qwen_outside_tracking"),
            profile_name: String::from("psionic-qwen35-2b-q8-registry"),
            model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: Some(TaskWorkspaceSummary {
                task_start_turn_index: 9,
                status: TaskWorkspaceSummaryStatus::ChangeAccountingLimited,
                changed_files: Vec::new(),
                touched_but_unchanged_files: Vec::new(),
                preexisting_dirty_files: Vec::new(),
                outside_tracking_dirty_files: vec![String::from("generated/schema.json")],
                repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
                change_accounting_limited: true,
                summary_text: String::from(
                    "Probe cannot confirm whether repo changes landed for this task because write-capable shell commands ran without file-level change accounting. Additional dirty files appeared during the task outside tracked tool results: generated/schema.json.",
                ),
            }),
            latest_task_receipt: None,
            recovery_note: None,
        });

        let rendered = app.render_to_string(120, 32);
        assert!(rendered.contains("dirty now:"), "{rendered}");
        assert!(rendered.contains("generated/schema.json"), "{rendered}");
    }

    #[test]
    fn shift_tab_on_codex_cycles_reasoning_and_resets_the_lane() {
        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.profile.reasoning_level = Some(String::from("high"));
        let mut app = AppShell::new_for_tests_with_chat_config(config);
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_codex"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            recovery_note: None,
        });
        app.apply_message(AppMessage::TranscriptEntryCommitted {
            entry: TranscriptEntry::new(
                TranscriptRole::User,
                "You",
                vec![String::from("codex lane message")],
            ),
        });

        app.dispatch(UiEvent::PreviousView);

        assert_eq!(app.active_tab(), ActiveTab::Primary);
        assert_eq!(
            probe_core::backend_profiles::resolved_reasoning_level_for_backend(
                BackendKind::OpenAiCodexSubscription,
                app.backend_lanes[0]
                    .chat_runtime
                    .profile
                    .reasoning_level
                    .as_deref()
            ),
            Some("xhigh")
        );
        assert_eq!(app.runtime_session_id(), None);
        let rendered = app.render_to_string(120, 32);
        assert!(!rendered.contains("Transcript is empty."));
        assert!(!rendered.contains("codex lane message"));
        assert_eq!(app.last_status(), "codex reasoning level: xhigh");
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
        assert_eq!(app.active_tab(), ActiveTab::Secondary);
        assert!(!app.emphasized_copy());
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_tailnet"),
            profile_name: String::from("psionic-qwen35-2b-q8-registry"),
            model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            recovery_note: None,
        });
        app.apply_message(AppMessage::TranscriptEntryCommitted {
            entry: TranscriptEntry::new(
                TranscriptRole::User,
                "You",
                vec![String::from("primary lane message")],
            ),
        });

        app.dispatch(UiEvent::PreviousView);
        assert_eq!(app.active_tab(), ActiveTab::Primary);
        assert_eq!(app.runtime_session_id(), None);
        let rendered_secondary = app.render_to_string(120, 32);
        assert!(!rendered_secondary.contains("Transcript is empty."));
        assert!(!rendered_secondary.contains("primary lane message"));
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_codex"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            recovery_note: None,
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

        app.dispatch(UiEvent::NextView);
        assert_eq!(app.active_tab(), ActiveTab::Secondary);
        assert_eq!(app.runtime_session_id(), Some("sess_tailnet"));
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
                session_generation: 0,
            },
            operator_backend: apple.operator_summary(),
            autostart_apple_fm_setup: false,
            resume_session_id: None,
        };
        let mut app = AppShell::new_with_launch_config(launch_config);

        app.dispatch(UiEvent::PreviousView);

        let rendered = app.render_to_string(120, 32);
        assert!(rendered.contains("100.108.56.85:8080"));
        assert_eq!(app.backend_lanes[1].label, "Tailnet");
        assert_eq!(app.backend_lanes[1].operator_backend.host, "100.108.56.85");
        assert_eq!(app.backend_lanes[1].operator_backend.port, 8080);
        assert_eq!(
            app.backend_lanes[1].chat_runtime.profile.base_url,
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

        assert_eq!(app.active_tab(), ActiveTab::Primary);
        assert_eq!(app.backend_lanes[0].label, "Codex");
        assert_eq!(
            app.backend_lanes[0]
                .chat_runtime
                .harness_profile
                .as_ref()
                .map(|profile| profile.name.as_str()),
            Some("coding_bootstrap_codex")
        );
        assert!(
            app.backend_lanes[0]
                .chat_runtime
                .system_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains("Codex harness profile v1"))
        );

        app.dispatch(UiEvent::OpenSetupOverlay);
        let rendered = app.render_to_string(120, 48);
        assert!(rendered.contains("auth"));
        assert!(rendered.contains("reasoning: backend_default"));
        assert!(rendered.contains("status: disconnected"));
        assert!(rendered.contains("probe codex login --method browser"));
    }

    #[test]
    fn model_slash_command_opens_picker_without_submitting_a_turn() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('m'));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::ModelPickerOverlay);
        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("Choose the model for the active backend."));
        assert!(rendered.contains("> gpt-5.4  current"));
        assert!(!rendered.contains("[user] You"));
        assert!(
            app.recent_events()
                .iter()
                .any(|entry| entry.contains("opened model picker for Codex"))
        );
    }

    #[test]
    fn model_picker_updates_the_active_lane_without_clearing_transcript_history() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_codex_model"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            recovery_note: None,
        });
        app.apply_message(AppMessage::TranscriptEntryCommitted {
            entry: TranscriptEntry::new(
                TranscriptRole::Assistant,
                "Probe",
                vec![String::from("Existing Codex transcript row.")],
            ),
        });

        app.dispatch(UiEvent::ComposerPaste(String::from("/model ")));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerHistoryNext);
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert_eq!(
            app.backend_lanes[0].chat_runtime.profile.model,
            String::from("gpt-5.4-mini")
        );
        assert_eq!(app.runtime_session_id(), None);
        assert_eq!(app.base_screen().committed_transcript_entry_count(), 1);
        let rendered = app.render_to_string(160, 48);
        assert!(rendered.contains("model: gpt-5.4-mini"));
        assert_eq!(app.last_status(), "active model: gpt-5.4-mini");
    }

    #[test]
    fn help_and_backend_slash_commands_open_local_overlays() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.dispatch(UiEvent::ComposerPaste(String::from("/help")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::Help);

        app.dispatch(UiEvent::Dismiss);
        app.dispatch(UiEvent::ComposerPaste(String::from("/backend")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::SetupOverlay);
        let rendered = app.render_to_string(120, 40);
        assert!(rendered.contains("lane: Codex"), "{rendered}");
    }

    #[test]
    fn reasoning_slash_command_opens_picker_and_updates_codex_lane() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.dispatch(UiEvent::ComposerPaste(String::from("/reasoning")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::ReasoningPickerOverlay);
        let rendered = app.render_to_string(120, 36);
        assert!(
            rendered.contains("Choose the reasoning effort"),
            "{rendered}"
        );
        assert!(
            rendered.contains("> backend_default  current"),
            "{rendered}"
        );

        app.dispatch(UiEvent::ComposerHistoryNext);
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert_eq!(
            app.backend_lanes[0]
                .chat_runtime
                .profile
                .reasoning_level
                .as_deref(),
            Some("low")
        );
        assert_eq!(app.last_status(), "active reasoning: low");
    }

    #[test]
    fn approvals_slash_command_opens_overlay_for_pending_approval() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_approval"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            recovery_note: None,
        });
        app.apply_message(AppMessage::PendingToolApprovalsUpdated {
            session_id: String::from("sess_approval"),
            approvals: vec![PendingToolApproval {
                session_id: SessionId::new("sess_approval"),
                tool_call_id: String::from("call_write"),
                tool_name: String::from("shell"),
                arguments: json!({"command":["touch","hello.txt"]}),
                risk_class: ToolRiskClass::Write,
                reason: Some(String::from("writes to the workspace")),
                tool_call_turn_index: 2,
                paused_result_turn_index: 2,
                requested_at_ms: 10,
                resolved_at_ms: None,
                resolution: None,
            }],
        });

        assert!(app.handle_local_slash_submission(&ComposerSubmission {
            text: String::from("/approvals"),
            slash_command: Some(String::from("approvals")),
            mentions: Vec::new(),
            attachments: Vec::new(),
            pasted_multiline: false,
        }));

        assert_eq!(app.active_screen_id(), ScreenId::ApprovalOverlay);
        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("Review this tool request."), "{rendered}");
        assert!(rendered.contains("touch"), "{rendered}");
    }

    #[test]
    fn new_and_cwd_slash_commands_open_guided_local_flows() {
        let original = tempdir().expect("original workspace");
        let updated = tempdir().expect("updated workspace");
        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.cwd = original.path().to_path_buf();
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/new")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::ConfirmationOverlay);
        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("Start Fresh Task"), "{rendered}");

        app.dispatch(UiEvent::Dismiss);
        app.dispatch(UiEvent::ComposerPaste(String::from("/cwd")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::WorkspaceOverlay);
        app.dispatch(UiEvent::ComposerPaste(updated.path().display().to_string()));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert_eq!(app.backend_lanes[0].chat_runtime.cwd, updated.path());
        assert_eq!(
            app.last_status(),
            format!("active workspace: {}", updated.path().display())
        );
    }

    #[test]
    fn resume_slash_command_lists_saved_sessions() {
        let probe_home = tempdir().expect("temp probe home");
        let workspace = tempdir().expect("workspace");
        let store = FilesystemSessionStore::new(probe_home.path());
        store
            .create_session_with(NewSession::new("Bug hunt", workspace.path()).with_backend(
                SessionBackendTarget {
                    profile_name: String::from("openai-codex-subscription"),
                    base_url: String::from("https://chatgpt.com/backend-api/codex"),
                    model: String::from("gpt-5.4"),
                },
            ))
            .expect("create saved session");
        let mut app =
            AppShell::new_for_tests_with_chat_config(AppShell::build_chat_runtime_config(
                Some(probe_home.path().to_path_buf()),
                openai_codex_subscription(),
            ));

        app.dispatch(UiEvent::ComposerPaste(String::from("/resume")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::ResumeOverlay);
        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("saved sessions: 1"), "{rendered}");
        assert!(rendered.contains("Bug hunt"), "{rendered}");
        assert!(rendered.contains("openai-codex-subscription"), "{rendered}");
    }

    #[test]
    fn plan_slash_command_opens_picker_and_updates_shell_copy() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('p'));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::PlanModeOverlay);
        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("Choose how Probe should behave on the next turn."));
        assert!(rendered.contains("> coding"));

        app.dispatch(UiEvent::ComposerHistoryNext);
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert_eq!(app.backend_lanes[0].mode, LaneOperatorMode::Plan);
        assert!(
            app.backend_lanes[0]
                .chat_runtime
                .system_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains("Plan mode is active."))
        );
        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("mode: plan"));
        assert!(rendered.contains("applied: plan mode on"));
        assert!(rendered.contains("Plan only; no edits"));
        assert_eq!(app.last_status(), "mode: plan");
    }

    #[test]
    fn plan_slash_command_is_blocked_while_runtime_work_is_in_flight() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_plan_busy"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: Some(RuntimeActivity::new(
                RuntimeActivityKind::Editing,
                "editing files",
            )),
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            recovery_note: None,
        });

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('p'));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.backend_lanes[0].mode, LaneOperatorMode::Coding);
        assert_eq!(
            app.last_status(),
            "wait for the current turn to finish before you change plan mode"
        );
    }

    #[test]
    fn usage_slash_command_opens_overlay_with_session_totals() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );
        app.apply_message(AppMessage::SessionUsageUpdated {
            session_id: String::from("sess_usage"),
            usage: SessionUsageSummary {
                latest_turn: Some(UsageCountsSummary {
                    prompt_tokens: Some(800),
                    prompt_truth: Some(String::from("observed")),
                    completion_tokens: Some(220),
                    completion_truth: Some(String::from("observed")),
                    total_tokens: Some(1_020),
                    total_truth: Some(String::from("observed")),
                }),
                aggregate: UsageCountsSummary {
                    prompt_tokens: Some(15_100),
                    prompt_truth: Some(String::from("observed")),
                    completion_tokens: Some(6_100),
                    completion_truth: Some(String::from("observed")),
                    total_tokens: Some(21_200),
                    total_truth: Some(String::from("observed")),
                },
                turns_with_usage: 3,
            },
        });

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('u'));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::UsageOverlay);
        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("turns_with_usage: 3"));
        assert!(rendered.contains("session aggregate"));
        assert!(rendered.contains("total: 21200"));
        assert!(rendered.contains("/compact"), "{rendered}");
    }

    #[test]
    fn mcp_slash_command_opens_operator_overlay() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('m'));
        app.dispatch(UiEvent::ComposerInsert('c'));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::McpOverlay);
        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("Inspect Probe's current integration boundary."));
        assert!(rendered.contains("Add MCP server"));
        assert!(rendered.contains("Saved MCP servers"));
        assert!(rendered.contains("Backend lanes"));
        assert!(rendered.contains("Codex auth"));
        assert!(rendered.contains("Generic MCP"));
    }

    #[test]
    fn mcp_overlay_navigation_surfaces_real_integration_next_steps() {
        let probe_home = tempdir().expect("temp probe home");
        let mut app =
            AppShell::new_for_tests_with_chat_config(AppShell::build_chat_runtime_config(
                Some(probe_home.path().to_path_buf()),
                openai_codex_subscription(),
            ));

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('m'));
        app.dispatch(UiEvent::ComposerInsert('c'));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerHistoryNext);
        app.dispatch(UiEvent::ComposerHistoryNext);
        app.dispatch(UiEvent::ComposerHistoryNext);

        let rendered = app.render_to_string(120, 40);
        assert!(rendered.contains("selected: Codex auth"), "{rendered}");
        assert!(rendered.contains("login required"), "{rendered}");
        assert!(
            rendered.contains("No saved ChatGPT subscription auth record"),
            "{rendered}"
        );

        app.dispatch(UiEvent::ComposerHistoryNext);
        let rendered = app.render_to_string(120, 40);
        assert!(rendered.contains("selected: Apple FM bridge"), "{rendered}");
    }

    #[test]
    fn mcp_overlay_exposes_add_server_as_a_selectable_action() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('m'));
        app.dispatch(UiEvent::ComposerInsert('c'));
        app.dispatch(UiEvent::ComposerSubmit);

        let rendered = app.render_to_string(120, 40);
        assert!(rendered.contains("selected: Add MCP server"), "{rendered}");
        assert!(
            rendered.contains("Press Enter to open the MCP add menu."),
            "{rendered}"
        );

        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::McpAddOverlay);
    }

    #[test]
    fn mcp_add_flow_defaults_to_provider_command_and_imports_a_saved_recipe() {
        let probe_home = tempdir().expect("temp probe home");
        let mut app =
            AppShell::new_for_tests_with_chat_config(AppShell::build_chat_runtime_config(
                Some(probe_home.path().to_path_buf()),
                openai_codex_subscription(),
            ));

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('m'));
        app.dispatch(UiEvent::ComposerInsert('c'));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerSubmit);

        let rendered = app.render_to_string(120, 40);
        assert!(
            rendered.contains("Paste provider setup command"),
            "{rendered}"
        );
        assert!(
            rendered.contains("pnpm dlx ... mcp init --client codex"),
            "{rendered}"
        );

        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::McpProviderCommandOverlay);
        app.dispatch(UiEvent::ComposerPaste(String::from(
            "pnpm dlx shadcn@latest mcp init --client codex",
        )));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::McpOverlay);
        assert_eq!(app.mcp_registry.servers.len(), 1);
        assert_eq!(app.mcp_registry.servers[0].name, "shadcn MCP");
        assert_eq!(
            app.mcp_registry.servers[0].source,
            McpServerSource::ProviderCommandRecipe
        );
        assert_eq!(
            app.mcp_registry.servers[0]
                .provider_setup_command
                .as_deref(),
            Some("pnpm dlx shadcn@latest mcp init --client codex")
        );
        let rendered = app.render_to_string(120, 40);
        assert!(
            rendered.contains("selected: Saved MCP servers"),
            "{rendered}"
        );
        assert!(rendered.contains("configured: 1"), "{rendered}");
    }

    #[test]
    fn mcp_overlay_shows_saved_server_counts_and_selects_new_import() {
        let probe_home = tempdir().expect("temp probe home");
        let mut app =
            AppShell::new_for_tests_with_chat_config(AppShell::build_chat_runtime_config(
                Some(probe_home.path().to_path_buf()),
                openai_codex_subscription(),
            ));

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('m'));
        app.dispatch(UiEvent::ComposerInsert('c'));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerPaste(String::from(
            "pnpm dlx shadcn@latest mcp init --client codex",
        )));
        app.dispatch(UiEvent::ComposerSubmit);

        let rendered = app.render_to_string(120, 40);
        assert!(
            rendered.contains("saved MCP servers: 1 configured, 1 enabled"),
            "{rendered}"
        );
        assert!(
            rendered.contains("selected: Saved MCP servers"),
            "{rendered}"
        );
        assert!(rendered.contains("configured: 1"), "{rendered}");
        assert_eq!(app.last_status(), "imported MCP recipe shadcn MCP");

        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::McpServersOverlay);
        let rendered = app.render_to_string(120, 40);
        assert!(rendered.contains("selected: shadcn MCP"), "{rendered}");
        assert!(rendered.contains("client hint: codex"), "{rendered}");
    }

    #[test]
    fn mcp_provider_overlay_shows_pasted_command_visibly() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('m'));
        app.dispatch(UiEvent::ComposerInsert('c'));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerPaste(String::from(
            "pnpm dlx shadcn@latest mcp init --client codex",
        )));

        let rendered = app.render_to_string(120, 40);
        assert!(rendered.contains("clipboard: pasted "), "{rendered}");
        assert!(
            rendered.contains("provider setup command preview:"),
            "{rendered}"
        );
        assert!(rendered.contains("shadcn@latest"), "{rendered}");
    }

    #[test]
    fn mcp_overlay_can_add_and_persist_a_configured_server() {
        let probe_home = tempdir().expect("temp probe home");
        let mut app =
            AppShell::new_for_tests_with_chat_config(AppShell::build_chat_runtime_config(
                Some(probe_home.path().to_path_buf()),
                openai_codex_subscription(),
            ));

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('m'));
        app.dispatch(UiEvent::ComposerInsert('c'));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::NextView);
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::McpEditorOverlay);
        for ch in "Local Files".chars() {
            app.dispatch(UiEvent::ComposerInsert(ch));
        }
        app.dispatch(UiEvent::NextView);
        app.dispatch(UiEvent::NextView);
        app.dispatch(UiEvent::ComposerPaste(String::from(
            "npx -y @modelcontextprotocol/server-filesystem .",
        )));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::McpOverlay);
        assert_eq!(app.mcp_registry.servers.len(), 1);
        assert_eq!(app.mcp_registry.servers[0].name, "Local Files");
        assert!(app.mcp_registry.servers[0].enabled);
        let rendered = app.render_to_string(120, 40);
        assert!(
            rendered.contains("saved MCP servers: 1 configured, 1 enabled"),
            "{rendered}"
        );
        assert!(
            rendered.contains("selected: Saved MCP servers"),
            "{rendered}"
        );

        let saved = std::fs::read_to_string(mcp_registry_path(probe_home.path()))
            .expect("saved mcp registry");
        assert!(saved.contains("Local Files"));
        assert!(saved.contains("server-filesystem"));
    }

    #[test]
    fn mcp_overlay_can_disable_an_enabled_server() {
        let probe_home = tempdir().expect("temp probe home");
        save_mcp_registry(
            Some(probe_home.path()),
            &McpRegistryFile {
                servers: vec![McpServerRecord {
                    id: String::from("local-files"),
                    name: String::from("Local Files"),
                    enabled: true,
                    source: McpServerSource::ManualLaunch,
                    transport: Some(McpServerTransport::Stdio),
                    target: Some(String::from(
                        "npx -y @modelcontextprotocol/server-filesystem .",
                    )),
                    provider_setup_command: None,
                    provider_hint: None,
                    client_hint: None,
                }],
            },
        )
        .expect("seed mcp registry");
        let mut app =
            AppShell::new_for_tests_with_chat_config(AppShell::build_chat_runtime_config(
                Some(probe_home.path().to_path_buf()),
                openai_codex_subscription(),
            ));

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('m'));
        app.dispatch(UiEvent::ComposerInsert('c'));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerHistoryNext);
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerInsert('d'));

        assert!(!app.mcp_registry.servers[0].enabled);
        let rendered = app.render_to_string(120, 40);
        assert!(rendered.contains("disabled"), "{rendered}");
        assert_eq!(app.last_status(), "disabled MCP server Local Files");
    }

    #[test]
    fn mcp_overlay_can_enable_a_disabled_server() {
        let probe_home = tempdir().expect("temp probe home");
        save_mcp_registry(
            Some(probe_home.path()),
            &McpRegistryFile {
                servers: vec![McpServerRecord {
                    id: String::from("local-files"),
                    name: String::from("Local Files"),
                    enabled: false,
                    source: McpServerSource::ManualLaunch,
                    transport: Some(McpServerTransport::Stdio),
                    target: Some(String::from(
                        "npx -y @modelcontextprotocol/server-filesystem .",
                    )),
                    provider_setup_command: None,
                    provider_hint: None,
                    client_hint: None,
                }],
            },
        )
        .expect("seed mcp registry");
        let mut app =
            AppShell::new_for_tests_with_chat_config(AppShell::build_chat_runtime_config(
                Some(probe_home.path().to_path_buf()),
                openai_codex_subscription(),
            ));

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('m'));
        app.dispatch(UiEvent::ComposerInsert('c'));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerHistoryNext);
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerInsert('e'));

        assert!(app.mcp_registry.servers[0].enabled);
        let rendered = app.render_to_string(120, 40);
        assert!(rendered.contains("enabled"), "{rendered}");
        assert_eq!(app.last_status(), "enabled MCP server Local Files");
    }

    #[test]
    fn mcp_overlay_can_remove_a_configured_server() {
        let probe_home = tempdir().expect("temp probe home");
        save_mcp_registry(
            Some(probe_home.path()),
            &McpRegistryFile {
                servers: vec![McpServerRecord {
                    id: String::from("local-files"),
                    name: String::from("Local Files"),
                    enabled: true,
                    source: McpServerSource::ManualLaunch,
                    transport: Some(McpServerTransport::Stdio),
                    target: Some(String::from(
                        "npx -y @modelcontextprotocol/server-filesystem .",
                    )),
                    provider_setup_command: None,
                    provider_hint: None,
                    client_hint: None,
                }],
            },
        )
        .expect("seed mcp registry");
        let mut app =
            AppShell::new_for_tests_with_chat_config(AppShell::build_chat_runtime_config(
                Some(probe_home.path().to_path_buf()),
                openai_codex_subscription(),
            ));

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('m'));
        app.dispatch(UiEvent::ComposerInsert('c'));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerHistoryNext);
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerInsert('r'));

        assert!(app.mcp_registry.servers.is_empty());
        let rendered = app.render_to_string(120, 40);
        assert!(rendered.contains("saved MCP servers: 0"), "{rendered}");
        assert_eq!(app.last_status(), "removed MCP server Local Files");
    }

    #[test]
    fn clear_slash_command_confirms_then_resets_the_active_context() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_clear"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            recovery_note: None,
        });
        app.apply_message(AppMessage::TranscriptEntryCommitted {
            entry: TranscriptEntry::new(
                TranscriptRole::Assistant,
                "Probe",
                vec![String::from("Carry this context for now.")],
            ),
        });

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('c'));
        app.dispatch(UiEvent::ComposerInsert('l'));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::ConfirmationOverlay);
        let rendered = app.render_to_string(120, 36);
        assert!(
            rendered
                .contains("Drop the current conversation context and start fresh on this lane.")
        );
        assert!(rendered.contains("workspace: /tmp/probe-workspace"));
        assert!(rendered.contains("model: gpt-5.4"));
        assert!(rendered.contains("Enter confirms. Esc cancels."));

        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert_eq!(app.base_screen().committed_transcript_entry_count(), 0);
        assert_eq!(app.runtime_session_id(), None);
        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("applied: fresh context"));
        assert_eq!(app.last_status(), "cleared context; repo files unchanged");
    }

    #[test]
    fn compact_slash_command_confirms_then_carries_a_summary_forward() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );
        app.apply_message(AppMessage::TranscriptEntryCommitted {
            entry: TranscriptEntry::new(
                TranscriptRole::User,
                "You",
                vec![String::from("Need a condensed handoff for the next turn.")],
            ),
        });

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('c'));
        app.dispatch(UiEvent::ComposerInsert('o'));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::ConfirmationOverlay);
        let rendered = app.render_to_string(120, 40);
        assert!(rendered.contains("carry-forward summary preview:"));
        assert!(rendered.contains("Recent conversation:"));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert_eq!(app.base_screen().committed_transcript_entry_count(), 0);
        assert!(
            app.backend_lanes[0]
                .carry_forward_summary
                .as_deref()
                .is_some_and(|summary| summary.contains("Need a condensed handoff"))
        );
        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("context: compact summary"));
        assert!(rendered.contains("applied: compact summary on"));
        assert_eq!(
            app.last_status(),
            "compacted conversation into a carry-forward summary"
        );
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
            if rendered.contains("[assistant] Probe") {
                break;
            }
            app.dispatch(UiEvent::PageUp);
            rendered = app.render_to_string(120, 32);
        }
        assert!(rendered.contains("[assistant] Probe"));
        assert!(!rendered.contains("[edited]"), "{rendered}");
        assert!(
            app.worker_events()
                .iter()
                .any(|entry| entry.contains("runtime session ready:"))
        );
    }

    #[test]
    fn assistant_turn_commit_renders_as_conversational_closeout() {
        let mut app = AppShell::new_for_tests();
        app.apply_message(AppMessage::ProbeRuntimeEvent {
            event: RuntimeEvent::AssistantTurnCommitted {
                session_id: SessionId::new("sess_qwen_closeout"),
                response_id: String::from("resp_123"),
                response_model: String::from("gpt-5.4"),
                assistant_text: String::from(
                    "Updated the README note and verified the surrounding section still reads cleanly.",
                ),
            },
        });

        let rendered = app.render_to_string(120, 32);
        assert!(rendered.contains("[active assistant] Probe"), "{rendered}");
        assert!(rendered.contains("Updated the README note"), "{rendered}");
        assert!(
            rendered.contains("surrounding section still reads"),
            "{rendered}"
        );
        assert!(!rendered.contains("response_id:"), "{rendered}");
        assert!(!rendered.contains("  response"), "{rendered}");
    }

    #[test]
    fn live_tool_rows_stay_visible_across_multiple_runtime_tools() {
        let mut app = AppShell::new_for_tests();
        let session_id = SessionId::new("sess_tools");

        app.apply_message(AppMessage::ProbeRuntimeEvent {
            event: RuntimeEvent::ToolCallRequested {
                session_id: session_id.clone(),
                round_trip: 1,
                call_id: String::from("call_readme_1"),
                tool_name: String::from("read_file"),
                arguments: json!({"path":"README.md"}),
            },
        });
        app.apply_message(AppMessage::ProbeRuntimeEvent {
            event: RuntimeEvent::ToolExecutionStarted {
                session_id: session_id.clone(),
                round_trip: 1,
                call_id: String::from("call_readme_1"),
                tool_name: String::from("read_file"),
                risk_class: ToolRiskClass::ReadOnly,
            },
        });
        app.apply_message(AppMessage::ProbeRuntimeEvent {
            event: RuntimeEvent::ToolExecutionCompleted {
                session_id: session_id.clone(),
                round_trip: 1,
                tool: executed_tool(
                    "call_readme_1",
                    "read_file",
                    json!({"path":"README.md"}),
                    json!({"path":"README.md","start_line":1,"end_line":2,"content":"# Probe\n"}),
                    ToolRiskClass::ReadOnly,
                ),
            },
        });
        app.apply_message(AppMessage::ProbeRuntimeEvent {
            event: RuntimeEvent::ToolCallRequested {
                session_id: session_id.clone(),
                round_trip: 1,
                call_id: String::from("call_ls_1"),
                tool_name: String::from("list_files"),
                arguments: json!({"path":"."}),
            },
        });
        app.apply_message(AppMessage::ProbeRuntimeEvent {
            event: RuntimeEvent::ToolExecutionStarted {
                session_id,
                round_trip: 1,
                call_id: String::from("call_ls_1"),
                tool_name: String::from("list_files"),
                risk_class: ToolRiskClass::ReadOnly,
            },
        });

        let rendered = app.render_to_string(140, 40);
        assert!(rendered.contains("[active status] Planning Tool Call"));

        app.apply_message(AppMessage::TranscriptEntriesCommitted {
            entries: vec![
                TranscriptEntry::tool_call("read_file", vec![String::from("README.md")]),
                TranscriptEntry::tool_result(
                    "read_file",
                    vec![String::from("README.md:1-2"), String::from("# Probe")],
                ),
                TranscriptEntry::tool_call("list_files", vec![String::from(".")]),
                TranscriptEntry::tool_result(
                    "list_files",
                    vec![
                        String::from("listed 2 entries"),
                        String::from("README.md"),
                        String::from("src"),
                    ],
                ),
                TranscriptEntry::new(
                    TranscriptRole::Assistant,
                    "Probe",
                    vec![String::from("Done.")],
                ),
            ],
        });

        let rendered = app.render_to_string(140, 40);
        assert!(!rendered.contains("[edited]"), "{rendered}");
        assert!(!rendered.contains("[active status] Running Tool: list_files"));
        assert!(rendered.contains("[assistant] Probe"));
    }

    #[test]
    fn runtime_tool_result_rows_call_out_updated_files() {
        let mut app = AppShell::new_for_tests();
        let session_id = SessionId::new("sess_qwen_patch_result");
        let mut tool = executed_tool(
            "call_patch_1",
            "apply_patch",
            json!({
                "path": "README.md",
                "old_text": "before",
                "new_text": "after"
            }),
            json!({
                "path": "README.md",
                "created": false,
                "replace_all": false,
                "bytes_written": 42
            }),
            ToolRiskClass::Write,
        );
        tool.tool_execution.files_touched = vec![String::from("README.md")];
        tool.tool_execution.files_changed = vec![String::from("README.md")];

        app.apply_message(AppMessage::ProbeRuntimeEvent {
            event: RuntimeEvent::ToolExecutionCompleted {
                session_id,
                round_trip: 3,
                tool,
            },
        });

        let rendered = app.render_to_string(140, 40);
        assert!(rendered.contains("[tool result] apply_patch"), "{rendered}");
        assert!(rendered.contains("updated: README.md"), "{rendered}");
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
        assert!(rendered.contains("[assistant] Probe"));
        assert!(rendered.contains("[edited] hello.txt"));
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
    fn backend_failure_copy_names_the_active_lane_and_target() {
        let mut app = AppShell::new_for_tests();
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_qwen_failure"),
            profile_name: String::from("psionic-qwen35-2b-q8-registry"),
            model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            recovery_note: None,
        });
        app.apply_message(AppMessage::ProbeRuntimeEvent {
            event: RuntimeEvent::ModelRequestFailed {
                session_id: SessionId::new("sess_qwen_failure"),
                round_trip: 1,
                backend_kind: BackendKind::OpenAiChatCompletions,
                error: String::from("connection refused"),
            },
        });

        let rendered = app.render_to_string(120, 32);
        assert!(rendered.contains("Backend Request Failed"));
        assert!(rendered.contains("lane: Qwen"));
        assert!(rendered.contains("target: 127.0.0.1:8080"));
        assert!(rendered.contains("next: Retry the turn or switch lanes with Tab"));
        assert!(app.last_status().contains("backend request failed on Qwen"));
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
        let _guard = apple_fm_test_lock();
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

        let deadline = Instant::now() + Duration::from_secs(5);
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
        assert!(
            requests
                .iter()
                .any(|request| request.contains("GET /health HTTP/1.1"))
        );
    }

    #[test]
    fn apple_fm_setup_surfaces_typed_provider_failure() {
        let _guard = apple_fm_test_lock();
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
        assert!(
            rendered.contains("assets_unavailable")
                || (rendered.contains("failure_reason:")
                    && rendered.contains("Apple Intelligence"))
        );
        assert!(rendered.contains("Enable Apple Intelligence and retry"));
        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .any(|request| request.contains("GET /health HTTP/"))
        );
        assert!(
            requests
                .iter()
                .any(|request| request.contains("POST /v1/chat/completions HTTP/"))
        );
    }

    #[test]
    fn apple_fm_setup_completes_multi_call_flow() {
        let _guard = apple_fm_test_lock();
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

        let deadline = Instant::now() + Duration::from_secs(5);
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
        assert!(rendered.contains("Latest proof step:"));
        assert!(rendered.contains("next: Esc returns to chat"));
        assert!(
            rendered.contains("The TUI should now prove live Apple FM setup truth on startup.")
        );
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
