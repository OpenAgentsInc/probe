use std::collections::HashMap;
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
use probe_client::{ProbeClient, ProbeClientConfig, ProbeClientTransportConfig};
use probe_core::backend_profiles::{
    named_backend_profile, next_reasoning_level_for_backend, openai_codex_subscription,
    persisted_reasoning_level_for_backend, psionic_apple_fm_bridge, psionic_inference_mesh,
    psionic_qwen35_2b_q8_registry, supported_reasoning_levels_for_backend,
};
use probe_core::harness::resolve_prompt_contract;
use probe_core::runtime::{current_working_dir, default_probe_home};
use probe_core::server_control::{PsionicServerConfig, ServerOperatorSummary};
use probe_core::session_store::FilesystemSessionStore;
use probe_core::tools::{ProbeToolChoice, ToolApprovalConfig, ToolLoopConfig};
use probe_openai_auth::OpenAiCodexAuthStore;
use probe_protocol::backend::{BackendKind, BackendProfile};
use probe_protocol::runtime::{DetachedSessionRecoveryState, DetachedSessionStatus};
use probe_protocol::session::{
    PendingToolApproval, SessionBranchState, SessionDeliveryArtifact, SessionDeliveryState,
    SessionDeliveryStatus, SessionMcpConnectionStatus, SessionMetadata, SessionWorkspaceBootMode,
    SessionWorkspaceState, TaskWorkspaceSummary,
};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout};
use ratatui::{Frame, Terminal};
use serde::{Deserialize, Serialize};

use crate::bottom_pane::{BottomPane, BottomPaneState};
use crate::event::{UiEvent, event_from_key, event_from_mouse};
use crate::memory::{ProbeMemoryStack, load_probe_memory_stack};
use crate::message::{AppMessage, BackgroundTaskRequest, ProbeRuntimeTurnConfig};
use crate::screens::{
    ActiveTab, ApprovalOverlay, BackgroundModeOverlay, BranchOverlay, ChatScreen,
    CheckpointOverlay, CommitOverlay, ConfirmationKind, ConfirmationOverlay, DiffOverlay,
    DoctorOverlay, GitOverlay, HelpScreen, IntegrationCardView, ManagedMcpServerView,
    McpAddOverlay, McpEditorOverlay, McpOverlay, McpProviderCommandOverlay,
    McpServerTransportDraft, McpServersOverlay, MemoryEditorOverlay, MemoryOverlay,
    ModelPickerOverlay, PlanModeOverlay, PrCommentsOverlay, PrFeedbackItemView, PrOverlay,
    PushOverlay, ReasoningPickerOverlay, RecipesOverlay, ResumeOverlay, ResumeSessionView,
    RevertOverlay, ReviewModeOverlay, ScreenAction, ScreenCommand, ScreenId, ScreenState,
    SetupOverlay, StageOverlay, StatusOverlay, TaskDiffFileView, TaskPhase, UsageOverlay,
    WorkspaceOverlay,
};
use crate::transcript::TranscriptMode;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaneLaunchMode {
    Foreground,
    Background,
    Delegate,
}

impl LaneLaunchMode {
    fn label(self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::Background => "background",
            Self::Delegate => "delegate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaneReviewMode {
    AutoSafe,
    ReviewRisky,
    ReviewAll,
}

impl LaneReviewMode {
    fn label(self) -> &'static str {
        match self {
            Self::AutoSafe => "auto-safe",
            Self::ReviewRisky => "review-risky",
            Self::ReviewAll => "review-all",
        }
    }
}

#[derive(Debug, Clone)]
struct BackendLaneConfig {
    label: String,
    chat_runtime: ProbeRuntimeTurnConfig,
    operator_backend: ServerOperatorSummary,
    mode: LaneOperatorMode,
    launch_mode: LaneLaunchMode,
    review_mode: LaneReviewMode,
    transcript_mode: TranscriptMode,
    memory_stack: ProbeMemoryStack,
    carry_forward_summary: Option<String>,
    session_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPrReviewSummary {
    number: u64,
    title: String,
    url: String,
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    is_draft: bool,
    head_ref_name: String,
    base_ref_name: String,
    #[serde(default)]
    comments: Vec<GhPrComment>,
    #[serde(default)]
    reviews: Vec<GhPrReview>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPrComment {
    #[serde(default)]
    author: Option<GhActor>,
    #[serde(default)]
    body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPrReview {
    #[serde(default)]
    author: Option<GhActor>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct GhActor {
    login: String,
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
            Self::ManualLaunch => "manual runtime server",
            Self::ProviderCommandRecipe => "saved recipe",
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
            LaneReviewMode::AutoSafe,
            None,
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
                ScreenId::McpProviderCommandOverlay
                | ScreenId::McpEditorOverlay
                | ScreenId::MemoryEditorOverlay => match read_system_clipboard_text() {
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
                },
                _ => {
                    self.last_status = String::from(
                        "system clipboard paste is only available in editable setup fields",
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
                UiEvent::Dismiss => {
                    let pane_state = self.bottom_pane_state();
                    if self.bottom_pane.dismiss_slash_palette(&pane_state) {
                        self.last_status = String::from("closed command list");
                        self.poll_background_messages();
                        return;
                    }
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
                    let launch_mode = self.backend_lanes[self.active_backend_index].launch_mode;
                    if launch_mode != LaneLaunchMode::Background
                        && self.base_screen().has_pending_tool_approvals()
                    {
                        self.base_screen_mut().record_event(
                            "blocked new turn while a tool approval is still pending",
                        );
                        self.last_status =
                            String::from("resolve pending approvals before starting another turn");
                        self.poll_background_messages();
                        return;
                    }
                    self.refresh_lane_memory_state(self.active_backend_index);
                    let preview = submission_preview(&submitted, 48);
                    self.base_screen_mut().submit_user_turn(&submitted);
                    if launch_mode == LaneLaunchMode::Delegate
                        && self.base_screen().runtime_session_id().is_none()
                    {
                        self.base_screen_mut().record_event(
                            "blocked delegated task because this lane does not have a parent session yet",
                        );
                        self.last_status = String::from(
                            "start one normal turn on this lane before you delegate child tasks",
                        );
                        self.poll_background_messages();
                        return;
                    }
                    self.last_status = if launch_mode == LaneLaunchMode::Background {
                        format!(
                            "queued background task launch ({} chars)",
                            submitted.text.chars().count()
                        )
                    } else if launch_mode == LaneLaunchMode::Delegate {
                        format!(
                            "queued delegated child task ({} chars)",
                            submitted.text.chars().count()
                        )
                    } else {
                        format!(
                            "submitted chat turn ({} chars)",
                            submitted.text.chars().count()
                        )
                    };
                    let request = if launch_mode == LaneLaunchMode::Background {
                        self.base_screen_mut()
                            .record_event(format!("queued background task: {preview}"));
                        BackgroundTaskRequest::background_probe_runtime_turn(
                            submitted.text.clone(),
                            self.active_chat_runtime().clone(),
                        )
                    } else if launch_mode == LaneLaunchMode::Delegate {
                        let parent_session_id = self
                            .base_screen()
                            .runtime_session_id()
                            .map(str::to_owned)
                            .unwrap_or_default();
                        self.base_screen_mut()
                            .record_event(format!("queued delegated child task: {preview}"));
                        BackgroundTaskRequest::delegated_probe_runtime_turn(
                            parent_session_id,
                            submitted.text.clone(),
                            self.active_chat_runtime().clone(),
                        )
                    } else {
                        self.base_screen_mut()
                            .record_event(format!("queued Probe runtime turn: {preview}"));
                        BackgroundTaskRequest::probe_runtime_turn(
                            submitted.text.clone(),
                            self.active_chat_runtime().clone(),
                        )
                    };
                    if let Err(error) = self.submit_background_task(request) {
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
                    ScreenAction::OpenStatusOverlay => {
                        self.open_status_overlay();
                    }
                    ScreenAction::OpenDoctorOverlay => {
                        self.open_doctor_overlay();
                    }
                    ScreenAction::OpenSetupOverlay => {
                        self.open_backend_overlay();
                    }
                    ScreenAction::OpenApprovalOverlay => {
                        self.open_approval_overlay();
                    }
                    ScreenAction::OpenGitOverlay => {
                        self.open_git_overlay();
                    }
                    ScreenAction::OpenRecipesOverlay => {
                        self.open_recipes_overlay();
                    }
                    ScreenAction::OpenTasksOverlay => {
                        self.open_resume_overlay();
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
                        ScreenCommand::SetActiveLaunchMode { mode_label } => {
                            let mode = match mode_label.as_str() {
                                "background" => LaneLaunchMode::Background,
                                "delegate" => LaneLaunchMode::Delegate,
                                _ => LaneLaunchMode::Foreground,
                            };
                            if let Err(error) = self.apply_active_launch_mode(mode) {
                                self.last_status = error;
                            } else if self.active_screen_id() == ScreenId::BackgroundModeOverlay
                                && self.screens.len() > 1
                            {
                                self.screens.pop();
                                self.base_screen_mut()
                                    .record_event("background mode picker released focus");
                            }
                        }
                        ScreenCommand::SetActiveReviewMode { mode_label } => {
                            let mode = match mode_label.as_str() {
                                "review-risky" => LaneReviewMode::ReviewRisky,
                                "review-all" => LaneReviewMode::ReviewAll,
                                _ => LaneReviewMode::AutoSafe,
                            };
                            if let Err(error) = self.apply_active_review_mode(mode) {
                                self.last_status = error;
                            } else if self.active_screen_id() == ScreenId::ReviewModeOverlay
                                && self.screens.len() > 1
                            {
                                self.screens.pop();
                                self.base_screen_mut()
                                    .record_event("review mode picker released focus");
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
                        ScreenCommand::CreateOrSwitchBranch { repo_root, name } => {
                            if let Err(error) = self.create_or_switch_branch(repo_root, name) {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::StageCurrentRepo { repo_root } => {
                            if let Err(error) = self.stage_current_repo(repo_root) {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::CommitCurrentRepo { repo_root, message } => {
                            if let Err(error) = self.commit_current_repo(repo_root, message) {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::PushCurrentBranch {
                            repo_root,
                            branch_name,
                            set_upstream,
                        } => {
                            if let Err(error) =
                                self.push_current_branch(repo_root, branch_name, set_upstream)
                            {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::CreateDraftPullRequest {
                            repo_root,
                            title,
                            base_branch,
                            head_branch,
                        } => {
                            if let Err(error) = self.create_draft_pull_request(
                                repo_root,
                                title,
                                base_branch,
                                head_branch,
                            ) {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::SeedComposerDraft { text } => {
                            self.bottom_pane.replace_draft(text);
                            self.base_screen_mut()
                                .record_event("loaded a guided next step into the composer");
                            self.base_screen_mut()
                                .set_local_action_notice(Some(String::from("next step ready")));
                        }
                        ScreenCommand::OpenMcpProviderCommandOverlay => {
                            self.base_screen_mut()
                                .record_event("MCP provider command overlay took focus");
                            self.screens.push(ScreenState::McpProviderCommand(
                                McpProviderCommandOverlay::new(),
                            ));
                        }
                        ScreenCommand::OpenMcpManualEditorOverlay { server_id } => {
                            if let Err(error) = self.open_mcp_manual_editor_overlay(server_id) {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::OpenMemoryEditor { label, path } => {
                            if let Err(error) = self.open_memory_editor(label, path) {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::ImportMcpProviderCommand { command } => {
                            if let Err(error) = self.import_mcp_provider_command(command) {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::SaveMcpServer {
                            server_id,
                            name,
                            transport,
                            target,
                        } => {
                            if let Err(error) =
                                self.save_mcp_server(server_id, name, transport, target)
                            {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::SaveMemoryFile { label, path, body } => {
                            if let Err(error) = self.save_memory_file(label, path, body) {
                                self.last_status = error;
                            }
                        }
                        ScreenCommand::RevertLastTask { session_id } => {
                            if !self.base_screen().can_execute_revert() {
                                self.last_status = String::from(
                                    "latest task is not yet safe to revert automatically",
                                );
                            } else {
                                let session_id = if session_id.is_empty() {
                                    self.base_screen()
                                        .runtime_session_id()
                                        .map(str::to_owned)
                                        .unwrap_or_default()
                                } else {
                                    session_id
                                };
                                if session_id.is_empty() {
                                    self.last_status = String::from(
                                        "no runtime session is attached on this lane yet",
                                    );
                                } else if let Err(error) = self.submit_background_task(
                                    BackgroundTaskRequest::revert_last_task(
                                        session_id,
                                        self.active_chat_runtime().clone(),
                                    ),
                                ) {
                                    self.last_status = error;
                                }
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
            Constraint::Length(self.bottom_pane.desired_height(area.width, &pane_state)),
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
            ScreenId::StatusOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Status owns focus. Esc returns to chat.",
                ));
            }
            ScreenId::DoctorOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Doctor owns focus. Esc returns to chat.",
                ));
            }
            ScreenId::RecipesOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Recipes owns focus. Up/Down chooses a workflow, Enter loads the first step, and Esc returns.",
                ));
            }
            ScreenId::BackgroundModeOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Launch mode picker owns focus. Up/Down choose, Enter applies, and Esc returns.",
                ));
            }
            ScreenId::GitOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Git overlay owns focus. Esc returns to chat.",
                ));
            }
            ScreenId::BranchOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Branch overlay owns focus. Type a branch name, Enter applies, and Esc returns.",
                ));
            }
            ScreenId::StageOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Stage overlay owns focus. Enter stages current repo changes, and Esc returns.",
                ));
            }
            ScreenId::CommitOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Commit overlay owns focus. Type a message, Enter commits, and Esc returns.",
                ));
            }
            ScreenId::PushOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Push overlay owns focus. Enter pushes the current branch, and Esc returns.",
                ));
            }
            ScreenId::PrOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "PR overlay owns focus. Type a title, Enter creates the draft PR, and Esc returns.",
                ));
            }
            ScreenId::PrCommentsOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "PR comments owns focus. Up/Down choose feedback when available, Enter loads it, and Esc returns.",
                ));
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
            ScreenId::ReviewModeOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Review mode picker owns focus. Up/Down choose and Enter applies.",
                ));
            }
            ScreenId::MemoryOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Memory overlay owns focus. Up/Down choose a layer, PgUp/PgDn scroll, and Esc returns.",
                ));
            }
            ScreenId::MemoryEditorOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Memory editor owns focus. Type to edit, Ctrl+J inserts a newline, and Enter saves.",
                ));
            }
            ScreenId::DiffOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Diff overlay owns focus. Up/Down choose a file, PgUp/PgDn scroll the diff, and Esc returns.",
                ));
            }
            ScreenId::CheckpointOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Checkpoint overlay owns focus. Enter or Esc returns to chat.",
                ));
            }
            ScreenId::RevertOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Revert overlay owns focus. A or Enter reverts when available, and Esc returns to chat.",
                ));
            }
            ScreenId::WorkspaceOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Workspace picker owns focus. Paste or type a path, Enter applies, and Esc returns.",
                ));
            }
            ScreenId::ResumeOverlay => {
                return BottomPaneState::Disabled(String::from(
                    "Task list owns focus. Up/Down choose, Enter reopens, and Esc returns.",
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
                    "Saved MCP servers own focus. Up/Down choose, Enter edits setup, E enables, D disables, R removes, A adds, and Esc returns.",
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
                    "MCP setup owns focus. Type values, Tab changes fields, and Enter saves.",
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
        self.refresh_lane_memory_state(self.active_backend_index);
        let lane = self.backend_lanes[self.active_backend_index].clone();
        self.restore_chat_lane(self.active_backend_index, active_tab);
        self.last_status = format!("active backend: {}", lane.label);
    }

    fn refresh_lane_memory_state(&mut self, lane_index: usize) {
        {
            let lane = &mut self.backend_lanes[lane_index];
            rebuild_lane_runtime(lane);
            lane.operator_backend = operator_summary_from_profile(&lane.chat_runtime.profile);
        }
        let memory_stack = self.backend_lanes[lane_index].memory_stack.clone();
        self.chat_lanes[lane_index].set_memory_stack(memory_stack.clone());
        if lane_index == self.active_backend_index {
            self.base_screen_mut().set_memory_stack(memory_stack);
        }
        self.refresh_memory_overlay_instances();
    }

    fn refresh_memory_overlay_instances(&mut self) {
        let active_stack = self.backend_lanes[self.active_backend_index]
            .memory_stack
            .clone();
        for screen in self.screens.iter_mut() {
            if let ScreenState::Memory(memory) = screen {
                *memory = MemoryOverlay::new(active_stack.clone());
            }
        }
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
            Some("status") => {
                self.open_status_overlay();
                true
            }
            Some("doctor") => {
                self.open_doctor_overlay();
                true
            }
            Some("recipes") => {
                self.open_recipes_overlay();
                true
            }
            Some("git") => {
                self.open_git_overlay();
                true
            }
            Some("branch") => {
                self.open_branch_overlay();
                true
            }
            Some("stage") => {
                self.open_stage_overlay();
                true
            }
            Some("commit") => {
                self.open_commit_overlay();
                true
            }
            Some("push") => {
                self.open_push_overlay();
                true
            }
            Some("pr") => {
                self.open_pr_overlay();
                true
            }
            Some("pr_comments") => {
                self.open_pr_comments_overlay();
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
            Some("memory") => {
                self.open_memory_overlay();
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
            Some("review_mode") => {
                self.open_review_mode_overlay();
                true
            }
            Some("background") => {
                self.open_launch_mode_overlay(None);
                true
            }
            Some("delegate") => {
                self.open_launch_mode_overlay(Some(LaneLaunchMode::Delegate));
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
            Some("diff") => {
                self.open_diff_overlay();
                true
            }
            Some("checkpoint") => {
                self.open_checkpoint_overlay();
                true
            }
            Some("revert") => {
                self.open_revert_overlay();
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
            Some("conversation") => {
                self.apply_active_transcript_mode(TranscriptMode::Conversation);
                true
            }
            Some("trace") => {
                self.apply_active_transcript_mode(TranscriptMode::Trace);
                true
            }
            Some("tasks") => {
                self.open_resume_overlay();
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

    fn open_status_overlay(&mut self) {
        let (configured_count, enabled_count) = self.mcp_server_counts();
        let mcp_summary_lines = self.status_overlay_mcp_summary_lines();
        self.base_screen_mut()
            .record_event("status overlay took focus");
        self.last_status = String::from("opened status overlay");
        let overlay = StatusOverlay::new(configured_count, enabled_count, mcp_summary_lines);
        if self.active_screen_id() == ScreenId::StatusOverlay {
            if let Some(ScreenState::Status(screen)) = self.screens.last_mut() {
                *screen = overlay;
            }
        } else {
            self.screens.push(ScreenState::Status(overlay));
        }
    }

    fn open_launch_mode_overlay(&mut self, selected: Option<LaneLaunchMode>) {
        self.base_screen_mut()
            .record_event("launch mode picker took focus");
        self.last_status = String::from("opened launch mode picker");
        let current = self.backend_lanes[self.active_backend_index].launch_mode;
        let mut overlay = BackgroundModeOverlay::new(current.label());
        if let Some(selected) = selected {
            overlay = BackgroundModeOverlay::with_selected(current.label(), selected.label());
        }
        if self.active_screen_id() == ScreenId::BackgroundModeOverlay {
            if let Some(ScreenState::BackgroundMode(screen)) = self.screens.last_mut() {
                *screen = overlay;
            }
        } else {
            self.screens.push(ScreenState::BackgroundMode(overlay));
        }
    }

    fn open_doctor_overlay(&mut self) {
        let (configured_count, enabled_count) = self.mcp_server_counts();
        let mcp_summary_lines = self.doctor_overlay_mcp_summary_lines();
        self.base_screen_mut()
            .record_event("doctor overlay took focus");
        self.last_status = String::from("opened doctor overlay");
        let overlay = DoctorOverlay::new(configured_count, enabled_count, mcp_summary_lines);
        if self.active_screen_id() == ScreenId::DoctorOverlay {
            if let Some(ScreenState::Doctor(screen)) = self.screens.last_mut() {
                *screen = overlay;
            }
        } else {
            self.screens.push(ScreenState::Doctor(overlay));
        }
    }

    fn open_recipes_overlay(&mut self) {
        self.base_screen_mut()
            .record_event("recipes overlay took focus");
        self.last_status = String::from("opened recipes overlay");
        if self.active_screen_id() == ScreenId::RecipesOverlay {
            if let Some(ScreenState::Recipes(screen)) = self.screens.last_mut() {
                *screen = RecipesOverlay::new();
            }
        } else {
            self.screens
                .push(ScreenState::Recipes(RecipesOverlay::new()));
        }
    }

    fn open_git_overlay(&mut self) {
        self.base_screen_mut()
            .record_event("git overlay took focus");
        self.last_status = String::from("opened git overlay");
        if self.active_screen_id() == ScreenId::GitOverlay {
            if let Some(ScreenState::Git(screen)) = self.screens.last_mut() {
                *screen = GitOverlay::new();
            }
        } else {
            self.screens.push(ScreenState::Git(GitOverlay::new()));
        }
    }

    fn open_branch_overlay(&mut self) {
        if let Some(reason) = self.active_git_mutation_block_reason("change branches") {
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        }
        let summary = git_repo_status_summary(self.active_chat_runtime().cwd.as_path());
        let branch_label = summary
            .as_ref()
            .map(|summary| summary.branch_label.clone())
            .unwrap_or_else(|| String::from("no repo"));
        let suggested = summary
            .as_ref()
            .map(suggested_branch_name)
            .unwrap_or_else(|| String::from("codex/probe-task"));
        let overlay = BranchOverlay::new(
            summary.as_ref().map(|summary| summary.repo_root.clone()),
            branch_label,
            suggested,
            summary.as_ref().is_some_and(|summary| {
                summary.unstaged_count > 0
                    || summary.untracked_count > 0
                    || summary.staged_count > 0
            }),
        );
        self.base_screen_mut()
            .record_event("branch overlay took focus");
        self.last_status = String::from("opened branch overlay");
        if self.active_screen_id() == ScreenId::BranchOverlay {
            if let Some(ScreenState::Branch(screen)) = self.screens.last_mut() {
                *screen = overlay;
            }
        } else {
            self.screens.push(ScreenState::Branch(overlay));
        }
    }

    fn open_stage_overlay(&mut self) {
        if let Some(reason) = self.active_git_mutation_block_reason("stage repo changes") {
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        }
        let summary = git_repo_status_summary(self.active_chat_runtime().cwd.as_path());
        let overlay = StageOverlay::new(
            summary.as_ref().map(|summary| summary.repo_root.clone()),
            summary
                .as_ref()
                .map(|summary| summary.branch_label.clone())
                .unwrap_or_else(|| String::from("no repo")),
            summary.as_ref().map_or(0, |summary| summary.staged_count),
            summary.as_ref().map_or(0, |summary| summary.unstaged_count),
            summary
                .as_ref()
                .map_or(0, |summary| summary.untracked_count),
            summary
                .as_ref()
                .map(|summary| summary.preview_paths.clone())
                .unwrap_or_default(),
        );
        self.base_screen_mut()
            .record_event("stage overlay took focus");
        self.last_status = String::from("opened stage overlay");
        if self.active_screen_id() == ScreenId::StageOverlay {
            if let Some(ScreenState::Stage(screen)) = self.screens.last_mut() {
                *screen = overlay;
            }
        } else {
            self.screens.push(ScreenState::Stage(overlay));
        }
    }

    fn open_commit_overlay(&mut self) {
        if let Some(reason) = self.active_git_mutation_block_reason("commit repo changes") {
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        }
        let summary = git_repo_status_summary(self.active_chat_runtime().cwd.as_path());
        let suggested_message = summary
            .as_ref()
            .map(suggested_commit_message)
            .unwrap_or_else(|| String::from("Update repo changes"));
        let overlay = CommitOverlay::new(
            summary.as_ref().map(|summary| summary.repo_root.clone()),
            summary
                .as_ref()
                .map(|summary| summary.branch_label.clone())
                .unwrap_or_else(|| String::from("no repo")),
            summary.as_ref().map_or(0, |summary| summary.staged_count),
            summary.as_ref().map_or(0, |summary| summary.unstaged_count),
            summary
                .as_ref()
                .map_or(0, |summary| summary.untracked_count),
            summary
                .as_ref()
                .map(|summary| summary.staged_preview_paths())
                .unwrap_or_default(),
            suggested_message,
        );
        self.base_screen_mut()
            .record_event("commit overlay took focus");
        self.last_status = String::from("opened commit overlay");
        if self.active_screen_id() == ScreenId::CommitOverlay {
            if let Some(ScreenState::Commit(screen)) = self.screens.last_mut() {
                *screen = overlay;
            }
        } else {
            self.screens.push(ScreenState::Commit(overlay));
        }
    }

    fn open_push_overlay(&mut self) {
        if let Some(reason) = self.active_git_mutation_block_reason("push the current branch") {
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        }
        let branch_state = session_branch_state_local(self.active_chat_runtime().cwd.as_path());
        let repo_root = branch_state.as_ref().map(|state| state.repo_root.clone());
        let branch_label = branch_state
            .as_ref()
            .map(|state| state.head_ref.clone())
            .unwrap_or_else(|| String::from("no repo"));
        let remote_name = branch_state
            .as_ref()
            .and_then(|state| {
                state
                    .upstream_ref
                    .as_ref()
                    .and_then(|value| value.split('/').next().map(str::to_owned))
                    .or_else(|| git_default_remote_name(state.repo_root.as_path()))
            })
            .unwrap_or_else(|| String::from("none"));
        let upstream_label = branch_state
            .as_ref()
            .and_then(|state| state.upstream_ref.clone())
            .unwrap_or_else(|| String::from("none"));
        let (can_push, set_upstream, blocked_reason) =
            branch_state.as_ref().map(push_overlay_state).unwrap_or((
                false,
                false,
                String::from("Probe could not detect a git repo for the current workspace"),
            ));
        let overlay = PushOverlay::new(
            repo_root,
            branch_label,
            remote_name,
            upstream_label,
            branch_state.as_ref().and_then(|state| state.ahead_by),
            branch_state.as_ref().and_then(|state| state.behind_by),
            branch_state
                .as_ref()
                .is_some_and(|state| state.working_tree_dirty),
            can_push,
            set_upstream,
            blocked_reason,
        );
        self.base_screen_mut()
            .record_event("push overlay took focus");
        self.last_status = String::from("opened push overlay");
        if self.active_screen_id() == ScreenId::PushOverlay {
            if let Some(ScreenState::Push(screen)) = self.screens.last_mut() {
                *screen = overlay;
            }
        } else {
            self.screens.push(ScreenState::Push(overlay));
        }
    }

    fn open_pr_overlay(&mut self) {
        if let Some(reason) = self.active_git_mutation_block_reason("create a pull request") {
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        }
        let branch_state = session_branch_state_local(self.active_chat_runtime().cwd.as_path());
        let repo_root = branch_state.as_ref().map(|state| state.repo_root.clone());
        let head_branch = branch_state
            .as_ref()
            .map(|state| state.head_ref.clone())
            .unwrap_or_else(|| String::from("no repo"));
        let remote_name = branch_state
            .as_ref()
            .and_then(|state| {
                state
                    .upstream_ref
                    .as_ref()
                    .and_then(|value| value.split('/').next().map(str::to_owned))
                    .or_else(|| git_default_remote_name(state.repo_root.as_path()))
            })
            .unwrap_or_else(|| String::from("none"));
        let base_branch = branch_state
            .as_ref()
            .and_then(|state| {
                git_remote_default_branch(state.repo_root.as_path(), remote_name.as_str())
            })
            .unwrap_or_else(|| String::from("main"));
        let upstream_label = branch_state
            .as_ref()
            .and_then(|state| state.upstream_ref.clone())
            .unwrap_or_else(|| String::from("none"));
        let (can_create, blocked_reason) = branch_state
            .as_ref()
            .map(|state| pr_overlay_state(state, base_branch.as_str()))
            .unwrap_or((
                false,
                String::from("Probe could not detect a git repo for the current workspace"),
            ));
        let title = branch_state
            .as_ref()
            .map(|state| suggested_pr_title(state.repo_root.as_path(), state.head_ref.as_str()))
            .unwrap_or_else(|| String::from("Open draft PR"));
        let overlay = PrOverlay::new(
            repo_root,
            head_branch,
            base_branch,
            remote_name,
            upstream_label,
            title,
            can_create,
            blocked_reason,
        );
        self.base_screen_mut().record_event("PR overlay took focus");
        self.last_status = String::from("opened PR overlay");
        if self.active_screen_id() == ScreenId::PrOverlay {
            if let Some(ScreenState::Pr(screen)) = self.screens.last_mut() {
                *screen = overlay;
            }
        } else {
            self.screens.push(ScreenState::Pr(overlay));
        }
    }

    fn open_pr_comments_overlay(&mut self) {
        let overlay = build_pr_comments_overlay(self.active_chat_runtime().cwd.as_path());
        self.base_screen_mut()
            .record_event("PR comments overlay took focus");
        self.last_status = String::from("opened PR comments overlay");
        if self.active_screen_id() == ScreenId::PrCommentsOverlay {
            if let Some(ScreenState::PrComments(screen)) = self.screens.last_mut() {
                *screen = overlay;
            }
        } else {
            self.screens.push(ScreenState::PrComments(overlay));
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
        self.base_screen_mut().record_event("opened task list");
        self.last_status = String::from("choose a task to reopen");
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

    fn open_memory_overlay(&mut self) {
        self.refresh_lane_memory_state(self.active_backend_index);
        self.base_screen_mut()
            .record_event("memory overlay took focus");
        self.last_status = String::from("opened memory overlay");
        let overlay = MemoryOverlay::new(self.base_screen().memory_stack().clone());
        if self.active_screen_id() == ScreenId::MemoryOverlay {
            if let Some(ScreenState::Memory(screen)) = self.screens.last_mut() {
                *screen = overlay;
            }
        } else {
            self.screens.push(ScreenState::Memory(overlay));
        }
    }

    fn open_memory_editor(&mut self, label: String, path: PathBuf) -> Result<(), String> {
        let (body, existed, load_note) = load_memory_editor_seed(path.as_path());
        self.base_screen_mut()
            .record_event(format!("memory editor took focus for {label}"));
        self.last_status = format!("editing {label}");
        let overlay = MemoryEditorOverlay::new(label, path, body, existed, load_note);
        self.screens.push(ScreenState::MemoryEditor(overlay));
        Ok(())
    }

    fn open_diff_overlay(&mut self) {
        let lane_label = self.backend_lanes[self.active_backend_index].label.clone();
        if let Some(approval) = self.base_screen().current_pending_tool_approval().cloned()
            && let Some(overlay) =
                build_pending_approval_diff_overlay(lane_label.as_str(), &approval)
        {
            self.base_screen_mut().record_event(format!(
                "proposed diff overlay took focus for {}",
                lane_label
            ));
            self.last_status = String::from("opened proposed diff overlay");
            if self.active_screen_id() == ScreenId::DiffOverlay {
                if let Some(ScreenState::Diff(screen)) = self.screens.last_mut() {
                    *screen = overlay;
                }
            } else {
                self.screens.push(ScreenState::Diff(overlay));
            }
            return;
        }

        let Some(workspace) = self.base_screen().latest_task_workspace_summary().cloned() else {
            let reason = String::from("there is no proposed or applied diff to inspect yet");
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        };
        if workspace.changed_files.is_empty() {
            let reason = String::from("there is no proposed or applied diff to inspect yet");
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        }

        let overlay = build_diff_overlay(lane_label.as_str(), &workspace);
        self.base_screen_mut()
            .record_event(format!("diff overlay took focus for {}", lane_label));
        self.last_status = String::from("opened diff overlay");
        if self.active_screen_id() == ScreenId::DiffOverlay {
            if let Some(ScreenState::Diff(screen)) = self.screens.last_mut() {
                *screen = overlay;
            }
        } else {
            self.screens.push(ScreenState::Diff(overlay));
        }
    }

    fn open_checkpoint_overlay(&mut self) {
        self.base_screen_mut()
            .record_event("checkpoint overlay took focus");
        self.last_status = String::from("opened checkpoint overlay");
        if self.active_screen_id() != ScreenId::CheckpointOverlay {
            self.screens
                .push(ScreenState::Checkpoint(CheckpointOverlay::new()));
        }
    }

    fn open_revert_overlay(&mut self) {
        self.base_screen_mut()
            .record_event("revert overlay took focus");
        self.last_status = String::from("opened revert overlay");
        if self.active_screen_id() != ScreenId::RevertOverlay {
            let blocked_status = self
                .base_screen()
                .latest_task_workspace_summary()
                .map(|summary| summary.revertibility.summary_text.clone())
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| {
                    String::from("latest task is not yet safe to revert automatically")
                });
            self.screens.push(ScreenState::Revert(RevertOverlay::new(
                self.base_screen().can_execute_revert(),
                self.base_screen().runtime_session_id().map(str::to_owned),
                blocked_status,
            )));
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

    fn open_mcp_manual_editor_overlay(&mut self, server_id: Option<String>) -> Result<(), String> {
        let overlay = if let Some(server_id) = server_id {
            let Some(server) = self
                .mcp_registry
                .servers
                .iter()
                .find(|server| server.id == server_id)
            else {
                return Err(String::from("that MCP server no longer exists"));
            };
            let recommended_target = recommended_runtime_target(server.provider_hint.as_deref());
            let recommendation_note = provider_runtime_guidance(server.provider_hint.as_deref());
            McpEditorOverlay::seeded(
                Some(server.id.clone()),
                server.name.clone(),
                match server.transport.as_ref() {
                    Some(McpServerTransport::Http) => McpServerTransportDraft::Http,
                    _ => McpServerTransportDraft::Stdio,
                },
                server.target.clone().unwrap_or_default(),
                server.provider_setup_command.clone(),
                server.provider_hint.clone(),
                recommended_target,
                recommendation_note,
            )
        } else {
            McpEditorOverlay::new()
        };
        self.base_screen_mut()
            .record_event("manual MCP setup overlay took focus");
        self.screens.push(ScreenState::McpEditor(overlay));
        Ok(())
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

    fn open_review_mode_overlay(&mut self) {
        if let Some(reason) = self.active_runtime_reconfig_block_reason("change review mode") {
            self.base_screen_mut().record_event(reason.clone());
            self.last_status = reason;
            return;
        }
        let current = self.backend_lanes[self.active_backend_index]
            .review_mode
            .label()
            .to_string();
        self.base_screen_mut()
            .record_event("review mode picker took focus");
        self.last_status = String::from("choose review mode for this lane");
        if self.active_screen_id() == ScreenId::ReviewModeOverlay {
            if let Some(ScreenState::ReviewMode(screen)) = self.screens.last_mut() {
                *screen = ReviewModeOverlay::new(current.as_str());
            }
            return;
        }
        self.screens
            .push(ScreenState::ReviewMode(ReviewModeOverlay::new(
                current.as_str(),
            )));
    }

    fn apply_active_launch_mode(&mut self, mode: LaneLaunchMode) -> Result<(), String> {
        let lane_index = self.active_backend_index;
        if self.backend_lanes[lane_index].launch_mode == mode {
            self.base_screen_mut()
                .record_event(format!("launch mode unchanged: {}", mode.label()));
            self.last_status = format!("launch mode: {}", mode.label());
            return Ok(());
        }
        self.backend_lanes[lane_index].launch_mode = mode;
        let refreshed = build_chat_lane(
            &self.backend_lanes,
            lane_index,
            ActiveTab::from_index(self.active_backend_index),
        );
        self.chat_lanes[lane_index] = refreshed.clone();
        self.screens[0] = ScreenState::Chat(refreshed);
        self.sync_backend_selector();
        self.base_screen_mut()
            .record_event(format!("launch mode set to {}", mode.label()));
        self.base_screen_mut()
            .set_local_action_notice(Some(format!("{} turns on", mode.label())));
        self.last_status = format!("launch mode: {}", mode.label());
        Ok(())
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

    fn apply_active_review_mode(&mut self, mode: LaneReviewMode) -> Result<(), String> {
        if let Some(reason) = self.active_runtime_reconfig_block_reason("change review mode") {
            self.base_screen_mut().record_event(reason.clone());
            return Err(reason);
        }
        let lane_index = self.active_backend_index;
        if self.backend_lanes[lane_index].review_mode == mode {
            self.base_screen_mut()
                .record_event(format!("review mode unchanged: {}", mode.label()));
            self.last_status = format!("review mode: {}", mode.label());
            return Ok(());
        }
        {
            let lane = &mut self.backend_lanes[lane_index];
            lane.review_mode = mode;
            lane.session_generation = lane.session_generation.saturating_add(1);
            rebuild_lane_runtime(lane);
            lane.operator_backend = operator_summary_from_profile(&lane.chat_runtime.profile);
        }
        self.refresh_active_lane_after_runtime_change(format!(
            "review mode set to {}; next turn uses the updated approval posture",
            mode.label()
        ));
        self.base_screen_mut()
            .set_local_action_notice(Some(format!("review: {}", mode.label())));
        self.last_status = format!("review mode: {}", mode.label());
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

    fn apply_active_transcript_mode(&mut self, mode: TranscriptMode) {
        let lane_index = self.active_backend_index;
        if self.backend_lanes[lane_index].transcript_mode == mode {
            self.base_screen_mut()
                .record_event(format!("view unchanged: {}", mode.label()));
            self.base_screen_mut()
                .set_local_action_notice(Some(format!("{} view on", mode.label())));
            self.last_status = format!("view: {}", mode.label());
            return;
        }

        self.backend_lanes[lane_index].transcript_mode = mode;
        self.chat_lanes[lane_index].set_transcript_mode(mode);
        self.chat_lanes[lane_index]
            .set_local_action_notice(Some(format!("{} view on", mode.label())));
        self.base_screen_mut().set_transcript_mode(mode);
        self.base_screen_mut()
            .set_local_action_notice(Some(format!("{} view on", mode.label())));
        self.base_screen_mut()
            .record_event(format!("transcript view set to {}", mode.label()));
        self.last_status = format!("view: {}", mode.label());
    }

    fn recent_resume_sessions(&self) -> Result<Vec<ResumeSessionView>, String> {
        let probe_home = registry_probe_home(&self.backend_lanes)
            .or_else(|| default_probe_home().ok())
            .ok_or_else(|| String::from("Probe home is not available for task discovery"))?;
        if let Ok(detached) = detached_task_sessions(probe_home.as_path()) {
            if !detached.is_empty() {
                return Ok(detached);
            }
        }

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
                status: String::from("saved session"),
                detail_lines: vec![
                    String::from("source: local session history"),
                    format!("turns: {}", session.next_turn_index),
                ],
                next_hint: String::from(
                    "next: Enter reopens this saved session on the matching lane.",
                ),
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

    fn active_git_mutation_block_reason(&self, action: &str) -> Option<String> {
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
            lane.launch_mode.label(),
            lane.review_mode.label(),
            lane.transcript_mode,
            lane.carry_forward_summary.as_deref(),
            note,
        );
        self.chat_lanes[lane_index] = refreshed_lane.clone();
        self.screens[0] = ScreenState::Chat(refreshed_lane);
        self.sync_backend_selector();
    }

    fn refresh_active_git_workspace_state(&mut self) {
        let cwd = self.active_chat_runtime().cwd.clone();
        let branch_state = session_branch_state_local(cwd.as_path());
        let delivery_state = branch_state
            .as_ref()
            .map(|branch_state| session_delivery_state_local(branch_state, 1));
        let workspace_state = if branch_state.is_some() {
            Some(SessionWorkspaceState {
                boot_mode: SessionWorkspaceBootMode::Fresh,
                baseline: None,
                snapshot: None,
                execution_host: None,
                provenance_note: None,
            })
        } else {
            None
        };
        self.base_screen_mut().set_git_workspace_state(
            workspace_state,
            branch_state,
            delivery_state,
        );
    }

    fn integration_cards(&self) -> Vec<IntegrationCardView> {
        let configured_count = self.mcp_registry.servers.len();
        let enabled_count = self
            .mcp_registry
            .servers
            .iter()
            .filter(|server| server.enabled)
            .count();
        let attached_count = self
            .base_screen()
            .runtime_mcp_state()
            .map(|state| state.servers.len())
            .unwrap_or(0);
        let mut cards = vec![
            IntegrationCardView {
                label: String::from("Add MCP server"),
                status: String::from("ready"),
                detail_lines: vec![String::from("Add an MCP integration for this Probe home.")],
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
                    format!("enabled: {enabled_count} · attached now: {attached_count}"),
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

    fn mcp_source_counts(&self) -> (usize, usize, usize, usize) {
        let mut recipe_count = 0;
        let mut enabled_recipe_count = 0;
        let mut manual_count = 0;
        let mut enabled_manual_count = 0;
        for server in &self.mcp_registry.servers {
            match server.source {
                McpServerSource::ProviderCommandRecipe => {
                    recipe_count += 1;
                    if server.enabled {
                        enabled_recipe_count += 1;
                    }
                }
                McpServerSource::ManualLaunch => {
                    manual_count += 1;
                    if server.enabled {
                        enabled_manual_count += 1;
                    }
                }
            }
        }
        (
            recipe_count,
            enabled_recipe_count,
            manual_count,
            enabled_manual_count,
        )
    }

    fn status_overlay_mcp_summary_lines(&self) -> Vec<String> {
        let (recipe_count, enabled_recipe_count, manual_count, enabled_manual_count) =
            self.mcp_source_counts();
        let mut lines = Vec::new();
        if recipe_count > 0 {
            let enabled_note = if enabled_recipe_count > 0 {
                format!("{enabled_recipe_count} enabled")
            } else {
                String::from("none enabled")
            };
            lines.push(format!(
                "mcp config: {recipe_count} saved recipe(s) still need conversion ({enabled_note})"
            ));
        }
        if manual_count > 0 {
            lines.push(format!(
                "mcp runtime entries: {manual_count} manual runtime server(s), {enabled_manual_count} enabled"
            ));
        }
        if lines.is_empty() {
            lines.push(String::from(
                "mcp config: no saved recipes or manual runtime servers yet",
            ));
        }
        lines
    }

    fn doctor_overlay_mcp_summary_lines(&self) -> Vec<String> {
        let (recipe_count, enabled_recipe_count, manual_count, enabled_manual_count) =
            self.mcp_source_counts();
        let mut lines = Vec::new();
        if enabled_recipe_count > 0 {
            lines.push(format!(
                "mcp config: action - {enabled_recipe_count} enabled saved recipe(s) still need conversion before Probe can run them"
            ));
        } else if recipe_count > 0 {
            lines.push(format!(
                "mcp config: info - {recipe_count} saved recipe(s) can be converted into runtime servers from /mcp"
            ));
        }
        if enabled_manual_count > 0 && self.base_screen().runtime_mcp_state().is_none() {
            lines.push(format!(
                "mcp runtime: info - start a turn to attach {enabled_manual_count} enabled manual runtime server(s)"
            ));
        } else if manual_count > 0 && enabled_manual_count == 0 {
            lines.push(format!(
                "mcp runtime: action - {manual_count} manual runtime server(s) are saved, but none are enabled"
            ));
        }
        lines
    }

    fn managed_mcp_server_views(&self) -> Vec<ManagedMcpServerView> {
        let runtime_state = self.base_screen().runtime_mcp_state();
        let attached_servers = runtime_state
            .map(|state| {
                state
                    .servers
                    .iter()
                    .map(|server| (server.id.as_str(), server))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.mcp_registry
            .servers
            .iter()
            .map(|server| {
                let attached_snapshot = attached_servers
                    .iter()
                    .find_map(|(attached_id, snapshot)| {
                        (*attached_id == server.id.as_str()).then_some(*snapshot)
                    });
                let mut detail_lines = vec![format!("entry type: {}", server.source.label())];
                let (status, session_line, next_step) = if let Some(snapshot) = attached_snapshot {
                    let tools = snapshot.discovered_tools.len();
                    match snapshot
                        .connection_status
                        .as_ref()
                        .unwrap_or(&SessionMcpConnectionStatus::Connected)
                    {
                        SessionMcpConnectionStatus::Connected => (
                            format!("connected now · {tools} tools"),
                            format!(
                                "session: connected now in this runtime session with {tools} discovered tool(s)"
                            ),
                            if server.source == McpServerSource::ProviderCommandRecipe {
                                String::from(
                                    "next: Enter edits this setup. E/D controls enablement. R removes it.",
                                )
                            } else {
                                String::from(
                                    "next: Enter edits this runtime server. D disables it. R removes it.",
                                )
                            },
                        ),
                        SessionMcpConnectionStatus::Failed => (
                            String::from("attach failed"),
                            String::from(
                                "session: attach failed in this runtime session; open setup to fix the launch details",
                            ),
                            String::from(
                                "next: Enter edits setup. D disables it if you want it out of the next turn. R removes it.",
                            ),
                        ),
                        SessionMcpConnectionStatus::Unsupported => (
                            String::from("unsupported in Probe today"),
                            String::from(
                                "session: Probe cannot mount this entry in the current runtime yet",
                            ),
                            String::from(
                                "next: Enter edits setup. Use stdio for now if you need a working runtime MCP.",
                            ),
                        ),
                    }
                } else {
                    match server.source {
                        McpServerSource::ProviderCommandRecipe => {
                            if server.enabled {
                                (
                                    String::from("saved recipe · needs conversion"),
                                    String::from(
                                        "session: saved only; this recipe still needs runtime setup before Probe can run it",
                                    ),
                                    String::from(
                                        "next: Enter completes setup and turns this into a runnable runtime server.",
                                    ),
                                )
                            } else {
                                (
                                    String::from("saved recipe · disabled"),
                                    String::from(
                                        "session: saved only; this recipe is disabled and still needs conversion before Probe can run it",
                                    ),
                                    String::from(
                                        "next: Enter completes setup. E enables it. R removes it.",
                                    ),
                                )
                            }
                        }
                        McpServerSource::ManualLaunch => {
                            if server.enabled {
                                (
                                    String::from("ready after next turn"),
                                    if runtime_state.is_some() {
                                        String::from(
                                            "session: enabled here, but not attached to this runtime session yet",
                                        )
                                    } else {
                                        String::from(
                                            "session: enabled and ready after the next turn starts a runtime session",
                                        )
                                    },
                                    String::from(
                                        "next: Start a turn to attach it now, or press Enter to edit setup.",
                                    ),
                                )
                            } else {
                                (
                                    String::from("disabled"),
                                    String::from(
                                        "session: disabled, so Probe will not attach it to the runtime",
                                    ),
                                    String::from(
                                        "next: Enter edits setup. E enables it for the next turn. R removes it.",
                                    ),
                                )
                            }
                        }
                    }
                };
                detail_lines.push(session_line);
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
                        detail_lines.push(String::from(
                            "support note: provider recipes are saved from docs, but they need conversion into a manual stdio runtime server before Probe can run them",
                        ));
                    }
                }
                if let Some(snapshot) = attached_snapshot {
                    if let Some(note) = snapshot.connection_note.as_deref() {
                        detail_lines.push(format!("runtime note: {note}"));
                    }
                    if !snapshot.discovered_tools.is_empty() {
                        let tool_names = snapshot
                            .discovered_tools
                            .iter()
                            .map(|tool| tool.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        detail_lines.push(format!("tools: {tool_names}"));
                    }
                } else if runtime_state.is_some() {
                    detail_lines.push(String::from(
                        "runtime note: this entry is not attached to the current session yet.",
                    ));
                } else {
                    detail_lines.push(String::from(
                        "runtime note: start a turn to attach enabled manual runtime servers and see live MCP results.",
                    ));
                }
                ManagedMcpServerView {
                    label: server.name.clone(),
                    enabled: server.enabled,
                    status,
                    detail_lines,
                    toggle_server_id: server.id.clone(),
                    remove_server_id: server.id.clone(),
                    edit_server_id: Some(server.id.clone()),
                    enter_hint: next_step,
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
        let (recipe_count, _, manual_count, _) = self.mcp_source_counts();
        let runtime_detail = self
            .base_screen()
            .runtime_mcp_state()
            .map(|state| {
                if let Some(error) = state.load_error.as_deref() {
                    vec![
                        String::from(
                            "The active runtime session could not snapshot MCP registry state.",
                        ),
                        format!("detail: {error}"),
                    ]
                } else {
                    let connected = state
                        .servers
                        .iter()
                        .filter(|server| {
                            server.connection_status
                                == Some(SessionMcpConnectionStatus::Connected)
                        })
                        .count();
                    let failed = state
                        .servers
                        .iter()
                        .filter(|server| {
                            server.connection_status == Some(SessionMcpConnectionStatus::Failed)
                        })
                        .count();
                    let unsupported = state
                        .servers
                        .iter()
                        .filter(|server| {
                            server.connection_status
                                == Some(SessionMcpConnectionStatus::Unsupported)
                        })
                        .count();
                    let discovered_tools = state
                        .servers
                        .iter()
                        .map(|server| server.discovered_tools.len())
                        .sum::<usize>();
                    vec![
                        format!(
                            "The active runtime session carries {} enabled MCP entr{}.",
                            state.servers.len(),
                            if state.servers.len() == 1 { "y" } else { "ies" }
                        ),
                        format!(
                            "runtime result: {connected} connected, {failed} failed, {unsupported} unsupported, {discovered_tools} tool(s) discovered"
                        ),
                    ]
                }
            })
            .unwrap_or_else(|| {
                vec![String::from(
                    "The next runtime session will attach enabled manual stdio MCP servers from this Probe home.",
                )]
            });
        let mut detail_lines = vec![
            String::from("Probe can run connected manual stdio MCP servers during turns."),
            if recipe_count > 0 {
                format!(
                    "Saved provider recipes: {recipe_count}. These still need conversion before Probe can run them."
                )
            } else {
                String::from(
                    "Saved provider recipes are supported as import records, then converted into runtime servers.",
                )
            },
            String::from(
                "Connected MCP tools appear in the normal tool loop with approvals, transcript rows, and receipts.",
            ),
        ];
        detail_lines.extend(runtime_detail);
        if manual_count == 0 {
            detail_lines.push(String::from(
                "Add a manual stdio runtime server, or convert a saved recipe from the server list.",
            ));
        }
        IntegrationCardView {
            label: String::from("Generic MCP"),
            status: if self.base_screen().runtime_mcp_state().is_some() {
                String::from("runtime active")
            } else {
                String::from("saved config")
            },
            detail_lines,
            next_step: String::from(
                "Use /mcp to convert saved recipes or manage runtime MCP servers.",
            ),
            toggle_server_id: None,
            remove_server_id: None,
            opens_server_list: false,
            opens_editor: false,
        }
    }

    fn save_mcp_server(
        &mut self,
        server_id: Option<String>,
        name: String,
        transport: McpServerTransportDraft,
        target: String,
    ) -> Result<(), String> {
        let target = target;
        let transport = match transport {
            McpServerTransportDraft::Stdio => McpServerTransport::Stdio,
            McpServerTransportDraft::Http => McpServerTransport::Http,
        };
        let mut converted_from_recipe = false;
        if let Some(server_id) = server_id {
            let Some(server) = self
                .mcp_registry
                .servers
                .iter_mut()
                .find(|server| server.id == server_id)
            else {
                return Err(String::from("that MCP server no longer exists"));
            };
            converted_from_recipe = server.source == McpServerSource::ProviderCommandRecipe;
            server.name = name.clone();
            server.enabled = true;
            server.source = McpServerSource::ManualLaunch;
            server.transport = Some(transport);
            server.target = Some(target.clone());
        } else {
            let id = next_mcp_server_id(&self.mcp_registry, name.as_str());
            self.mcp_registry.servers.push(McpServerRecord {
                id,
                name: name.clone(),
                enabled: true,
                source: McpServerSource::ManualLaunch,
                transport: Some(transport),
                target: Some(target.clone()),
                provider_setup_command: None,
                provider_hint: None,
                client_hint: None,
            });
        }
        self.persist_mcp_registry()?;
        self.refresh_all_lanes_after_mcp_registry_change();
        self.refresh_mcp_overlay_selecting(Some("Saved MCP servers"));
        self.refresh_mcp_servers_overlay_selecting(Some(name.as_str()));
        self.base_screen_mut()
            .record_event(if converted_from_recipe {
                format!("completed MCP runtime setup for {name}")
            } else {
                format!("saved MCP server {name}")
            });
        self.last_status = if converted_from_recipe {
            format!("completed runtime setup for {name}; start a turn to test it")
        } else {
            format!("saved MCP server {name}; next turn starts a fresh runtime session")
        };
        Ok(())
    }

    fn save_memory_file(
        &mut self,
        label: String,
        path: PathBuf,
        body: String,
    ) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(path.as_path(), body).map_err(|error| error.to_string())?;
        self.refresh_lane_memory_state(self.active_backend_index);
        self.base_screen_mut()
            .record_event(format!("saved {label} at {}", path.display()));
        self.base_screen_mut()
            .set_local_action_notice(Some(format!("{label} saved")));
        self.last_status = format!("saved {label}");
        Ok(())
    }

    fn stage_current_repo(&mut self, repo_root: PathBuf) -> Result<(), String> {
        let summary = git_repo_status_summary(repo_root.as_path())
            .ok_or_else(|| String::from("Probe could not detect a git repo to stage"))?;
        if summary.unstaged_count == 0 && summary.untracked_count == 0 {
            return Err(if summary.staged_count > 0 {
                String::from("all current repo changes are already staged; /commit is ready")
            } else {
                String::from("there are no repo changes to stage right now")
            });
        }
        let output = Command::new("git")
            .arg("-C")
            .arg(&repo_root)
            .arg("add")
            .arg("-A")
            .arg("--")
            .arg(".")
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(git_command_error("stage repo changes", &output));
        }
        self.refresh_active_git_workspace_state();
        let staged_paths = summary.preview_paths.clone();
        self.base_screen_mut()
            .record_event(format!("staged repo changes in {}", repo_root.display()));
        self.base_screen_mut().set_local_action_notice(Some(format!(
            "staged {} path(s); /commit is ready",
            staged_paths
                .len()
                .max(summary.unstaged_count + summary.untracked_count)
        )));
        self.last_status = format!(
            "staged repo changes; {}",
            summarize_paths(staged_paths.as_slice(), 3)
        );
        Ok(())
    }

    fn create_or_switch_branch(&mut self, repo_root: PathBuf, name: String) -> Result<(), String> {
        let summary = git_repo_status_summary(repo_root.as_path())
            .ok_or_else(|| String::from("Probe could not detect a git repo to manage branches"))?;
        let branch_name = name.trim();
        if branch_name.is_empty() {
            return Err(String::from("enter a branch name before you continue"));
        }
        if !git_branch_name_is_valid(repo_root.as_path(), branch_name) {
            return Err(format!(
                "`{branch_name}` is not a valid git branch name for this repo"
            ));
        }
        if summary.branch_label == branch_name {
            self.base_screen_mut()
                .set_local_action_notice(Some(format!("already on branch: {branch_name}")));
            self.last_status = format!("already on branch: {branch_name}");
            return Ok(());
        }

        let branch_exists = git_local_branch_exists(repo_root.as_path(), branch_name);
        let mut command = Command::new("git");
        command.arg("-C").arg(&repo_root).arg("switch");
        if !branch_exists {
            command.arg("-c");
        }
        command.arg(branch_name);
        let output = command.output().map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(git_command_error("change branches", &output));
        }

        self.refresh_active_git_workspace_state();
        let action = if branch_exists { "switched" } else { "created" };
        self.base_screen_mut()
            .record_event(format!("{action} git branch {branch_name}"));
        self.base_screen_mut()
            .set_local_action_notice(Some(format!("branch ready: {branch_name}")));
        self.last_status = format!("{action} branch: {branch_name}");
        Ok(())
    }

    fn commit_current_repo(&mut self, repo_root: PathBuf, message: String) -> Result<(), String> {
        let summary = git_repo_status_summary(repo_root.as_path())
            .ok_or_else(|| String::from("Probe could not detect a git repo to commit"))?;
        if summary.staged_count == 0 {
            return Err(
                if summary.unstaged_count > 0 || summary.untracked_count > 0 {
                    String::from("stage the repo changes first, then commit them")
                } else {
                    String::from("there are no staged repo changes to commit")
                },
            );
        }
        if message.trim().is_empty() {
            return Err(String::from("enter a commit message before you commit"));
        }
        let output = Command::new("git")
            .arg("-C")
            .arg(&repo_root)
            .arg("commit")
            .arg("-m")
            .arg(message.trim())
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            let error = git_command_error("create the commit", &output);
            if error.contains("Author identity unknown")
                || error.contains("unable to auto-detect email address")
            {
                return Err(String::from(
                    "git user.name and user.email must be configured before Probe can commit",
                ));
            }
            return Err(error);
        }
        self.refresh_active_git_workspace_state();
        self.base_screen_mut()
            .record_event(format!("created git commit in {}", repo_root.display()));
        self.base_screen_mut().set_local_action_notice(Some(format!(
            "commit created: {}",
            preview(message.trim(), 48)
        )));
        self.last_status = format!("created commit: {}", preview(message.trim(), 64));
        Ok(())
    }

    fn push_current_branch(
        &mut self,
        repo_root: PathBuf,
        branch_name: String,
        set_upstream: bool,
    ) -> Result<(), String> {
        let branch_state = session_branch_state_local(repo_root.as_path())
            .ok_or_else(|| String::from("Probe could not detect a git repo to push"))?;
        let (can_push, _, blocked_reason) = push_overlay_state(&branch_state);
        if !can_push {
            return Err(blocked_reason);
        }
        let remote_name = branch_state
            .upstream_ref
            .as_ref()
            .and_then(|value| value.split('/').next().map(str::to_owned))
            .or_else(|| git_default_remote_name(repo_root.as_path()))
            .ok_or_else(|| String::from("Probe could not find a git remote to push to"))?;
        let branch_name = branch_name.trim();
        let mut command = Command::new("git");
        command.arg("-C").arg(&repo_root).arg("push");
        if set_upstream {
            command.arg("-u").arg(&remote_name).arg(branch_name);
        }
        let output = command.output().map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(git_command_error("push the current branch", &output));
        }
        self.refresh_active_git_workspace_state();
        self.base_screen_mut()
            .record_event(format!("pushed branch {branch_name}"));
        self.base_screen_mut()
            .set_local_action_notice(Some(format!("pushed branch: {branch_name}")));
        self.last_status = if set_upstream {
            format!("published branch: {branch_name}")
        } else {
            format!("pushed branch: {branch_name}")
        };
        Ok(())
    }

    fn create_draft_pull_request(
        &mut self,
        repo_root: PathBuf,
        title: String,
        base_branch: String,
        head_branch: String,
    ) -> Result<(), String> {
        let branch_state = session_branch_state_local(repo_root.as_path())
            .ok_or_else(|| String::from("Probe could not detect a git repo for PR creation"))?;
        let (can_create, blocked_reason) = pr_overlay_state(&branch_state, base_branch.as_str());
        if !can_create {
            return Err(blocked_reason);
        }
        let gh_program = gh_program_for_repo(repo_root.as_path());
        let output = Command::new(&gh_program)
            .arg("pr")
            .arg("create")
            .arg("--draft")
            .arg("--title")
            .arg(title.trim())
            .arg("--body")
            .arg(format!(
                "Draft PR opened from `{head_branch}` into `{base_branch}` via Probe."
            ))
            .arg("--base")
            .arg(base_branch.as_str())
            .arg("--head")
            .arg(head_branch.as_str())
            .current_dir(&repo_root)
            .output()
            .map_err(|error| format!("Probe could not start gh for PR creation: {error}"))?;
        if !output.status.success() {
            return Err(gh_command_error("create the draft PR", &output));
        }
        let location = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let location = if location.is_empty() {
            title.trim().to_string()
        } else {
            location
        };
        self.base_screen_mut()
            .record_event(format!("created draft PR for {head_branch}"));
        self.base_screen_mut().set_local_action_notice(Some(format!(
            "draft PR created: {}",
            preview(location.as_str(), 56)
        )));
        self.last_status = format!("draft PR created: {}", preview(location.as_str(), 72));
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
        self.refresh_all_lanes_after_mcp_registry_change();
        self.refresh_mcp_overlay_selecting(Some("Saved MCP servers"));
        self.refresh_mcp_servers_overlay_selecting(Some(name.as_str()));
        self.base_screen_mut()
            .record_event(format!("imported MCP recipe {name}"));
        self.last_status = format!("saved MCP recipe {name}; complete setup to make it runnable");
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
        self.refresh_all_lanes_after_mcp_registry_change();
        self.refresh_mcp_overlay_selecting(Some("Saved MCP servers"));
        self.refresh_mcp_servers_overlay_selecting(Some(name.as_str()));
        self.base_screen_mut()
            .record_event(format!("{status} MCP server {name}"));
        self.last_status =
            format!("{status} MCP server {name}; next turn starts a fresh runtime session");
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
        self.refresh_all_lanes_after_mcp_registry_change();
        self.refresh_mcp_overlay_selecting(Some("Saved MCP servers"));
        self.refresh_mcp_servers_overlay_selecting(None);
        self.base_screen_mut()
            .record_event(format!("removed MCP server {name}"));
        self.last_status =
            format!("removed MCP server {name}; next turn starts a fresh runtime session");
        Ok(())
    }

    fn persist_mcp_registry(&self) -> Result<(), String> {
        save_mcp_registry(
            registry_probe_home(&self.backend_lanes).as_deref(),
            &self.mcp_registry,
        )
    }

    fn refresh_all_lanes_after_mcp_registry_change(&mut self) {
        for lane in &mut self.backend_lanes {
            lane.session_generation = lane.session_generation.saturating_add(1);
            rebuild_lane_runtime(lane);
            lane.operator_backend = operator_summary_from_profile(&lane.chat_runtime.profile);
        }
        for lane_index in 0..self.chat_lanes.len() {
            self.refresh_lane_memory_state(lane_index);
        }
        self.refresh_active_lane_after_runtime_change(String::from(
            "MCP registry changed; the next turn will start a fresh runtime session",
        ));
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

fn detached_task_sessions(probe_home: &Path) -> Result<Vec<ResumeSessionView>, String> {
    let mut client = resolve_tui_probe_client(probe_home)?;
    let store = FilesystemSessionStore::new(probe_home);
    let sessions = store.list_sessions().map_err(|error| error.to_string())?;
    let backend_map = sessions
        .iter()
        .map(|session| {
            (
                session.id.as_str().to_string(),
                session
                    .backend
                    .as_ref()
                    .map(|backend| backend.profile_name.clone())
                    .unwrap_or_else(|| String::from("unknown backend")),
            )
        })
        .collect::<HashMap<_, _>>();
    let title_map = sessions
        .iter()
        .map(|session| (session.id.as_str().to_string(), session.title.clone()))
        .collect::<HashMap<_, _>>();
    let parent_map = sessions
        .iter()
        .filter_map(|session| {
            session.parent_link.as_ref().map(|link| {
                (
                    session.id.as_str().to_string(),
                    link.session_id.as_str().to_string(),
                )
            })
        })
        .collect::<HashMap<_, _>>();
    let child_count_map = sessions
        .iter()
        .map(|session| (session.id.as_str().to_string(), session.child_links.len()))
        .collect::<HashMap<_, _>>();

    let mut sessions = client
        .list_detached_sessions()
        .map_err(|error| error.to_string())?;
    sessions.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
    Ok(sessions
        .into_iter()
        .take(12)
        .map(|summary| {
            detached_summary_view(
                summary,
                &backend_map,
                &title_map,
                &parent_map,
                &child_count_map,
            )
        })
        .collect())
}

fn resolve_tui_probe_client(probe_home: &Path) -> Result<ProbeClient, String> {
    let mut config = ProbeClientConfig::new(probe_home, "probe-tui");
    config.client_version = Some(String::from(env!("CARGO_PKG_VERSION")));
    config.transport = ProbeClientTransportConfig::LocalDaemon { socket_path: None };
    ProbeClient::connect_or_autostart_local_daemon(config, Duration::from_secs(3))
        .map_err(|error| error.to_string())
}

fn detached_summary_view(
    summary: probe_protocol::runtime::DetachedSessionSummary,
    backend_map: &HashMap<String, String>,
    title_map: &HashMap<String, String>,
    parent_map: &HashMap<String, String>,
    child_count_map: &HashMap<String, usize>,
) -> ResumeSessionView {
    let mut status = detached_status_label(summary.status);
    let backend = backend_map
        .get(summary.session_id.as_str())
        .cloned()
        .unwrap_or_else(|| String::from("unknown backend"));
    let parent_session_id = parent_map.get(summary.session_id.as_str());
    if parent_session_id.is_some() {
        status = format!("{status} · child task");
    }
    let mut detail_lines = vec![
        format!("state: {}", detached_status_label(summary.status)),
        format!("queued turns: {}", summary.queued_turn_count),
        format!("pending approvals: {}", summary.pending_approval_count),
    ];
    if let Some(parent_session_id) = parent_session_id {
        let parent_title = title_map
            .get(parent_session_id)
            .cloned()
            .unwrap_or_else(|| String::from("parent session"));
        detail_lines.push(format!("delegated from: {parent_title}"));
    }
    if let Some(child_count) = child_count_map.get(summary.session_id.as_str())
        && *child_count > 0
    {
        detail_lines.push(format!("delegated tasks: {child_count}"));
    }
    if let Some(note) = summary.recovery_note.as_deref() {
        detail_lines.push(format!(
            "recovery: {}",
            compact_detached_recovery_note(note)
        ));
    }
    if summary.recovery_state != DetachedSessionRecoveryState::Clean {
        detail_lines.push(format!(
            "resume state: {}",
            detached_recovery_label(summary.recovery_state)
        ));
    }
    let next_hint = match summary.status {
        DetachedSessionStatus::Running => {
            String::from("next: Enter reattaches this running task to its lane.")
        }
        DetachedSessionStatus::Queued => {
            if parent_session_id.is_some() {
                String::from("next: Enter opens this queued child task on its lane.")
            } else {
                String::from("next: Enter opens this queued task on its lane.")
            }
        }
        DetachedSessionStatus::ApprovalPaused => {
            String::from("next: Enter reopens this task so you can resolve approvals.")
        }
        DetachedSessionStatus::Completed => {
            String::from("next: Enter reopens this completed task and its receipt.")
        }
        DetachedSessionStatus::Failed
        | DetachedSessionStatus::Cancelled
        | DetachedSessionStatus::TimedOut => String::from(
            "next: Enter reopens this task so you can inspect the failure and recover.",
        ),
        DetachedSessionStatus::Idle => {
            String::from("next: Enter reopens this detached task on its lane.")
        }
    };
    ResumeSessionView {
        id: summary.session_id.as_str().to_string(),
        title: summary.title,
        backend,
        cwd: summary.cwd.display().to_string(),
        status,
        detail_lines,
        next_hint,
    }
}

fn detached_status_label(status: DetachedSessionStatus) -> String {
    match status {
        DetachedSessionStatus::Idle => String::from("idle"),
        DetachedSessionStatus::Running => String::from("running now"),
        DetachedSessionStatus::Queued => String::from("queued"),
        DetachedSessionStatus::ApprovalPaused => String::from("needs approval"),
        DetachedSessionStatus::Completed => String::from("completed"),
        DetachedSessionStatus::Failed => String::from("failed"),
        DetachedSessionStatus::Cancelled => String::from("cancelled"),
        DetachedSessionStatus::TimedOut => String::from("timed out"),
    }
}

fn detached_recovery_label(state: DetachedSessionRecoveryState) -> &'static str {
    match state {
        DetachedSessionRecoveryState::Clean => "clean",
        DetachedSessionRecoveryState::ApprovalPausedResumable => "approval-paused resumable",
        DetachedSessionRecoveryState::RunningTurnFailedOnRestart => "restart recovery needed",
    }
}

fn compact_detached_recovery_note(note: &str) -> String {
    let compact = note.split_whitespace().collect::<Vec<_>>().join(" ");
    preview(compact.as_str(), 92)
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

fn load_memory_editor_seed(path: &Path) -> (String, bool, Option<String>) {
    if !path.exists() {
        return (String::new(), false, None);
    }
    match std::fs::read_to_string(path) {
        Ok(body) => (body, true, None),
        Err(error) => (
            String::new(),
            true,
            Some(format!(
                "Probe could not read the current file contents ({error}). Saving here will replace the file with valid UTF-8 markdown text."
            )),
        ),
    }
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

fn recommended_runtime_target(provider_hint: Option<&str>) -> Option<String> {
    match provider_hint.map(|value| value.to_ascii_lowercase()) {
        Some(provider) if provider == "shadcn" => Some(String::from("npx shadcn@latest mcp")),
        _ => None,
    }
}

fn provider_runtime_guidance(provider_hint: Option<&str>) -> Option<String> {
    match provider_hint.map(|value| value.to_ascii_lowercase()) {
        Some(provider) if provider == "shadcn" => Some(String::from(
            "Probe can usually run shadcn with `npx shadcn@latest mcp`; the provider init command only writes client config.",
        )),
        _ => None,
    }
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
        BackendKind::OpenAiChatCompletions
            if matches!(
                config.control_plane,
                Some(probe_protocol::backend::BackendControlPlaneKind::PsionicInferenceMesh)
            ) =>
        {
            psionic_inference_mesh()
        }
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
        LaneReviewMode::AutoSafe,
        None,
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
        BackendKind::OpenAiChatCompletions
            if matches!(
                summary.control_plane,
                Some(probe_protocol::backend::BackendControlPlaneKind::PsionicInferenceMesh)
            ) =>
        {
            String::from("Mesh")
        }
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
    let (mut chat_runtime, operator_backend) = base
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

    let memory_stack = load_probe_memory_stack(
        chat_runtime.probe_home.as_deref(),
        chat_runtime.cwd.as_path(),
    );
    chat_runtime = build_chat_runtime_config_for_lane(
        chat_runtime.probe_home.clone(),
        chat_runtime.cwd.clone(),
        chat_runtime.profile.clone(),
        LaneOperatorMode::Coding,
        LaneReviewMode::AutoSafe,
        Some(&memory_stack),
        None,
        0,
    );

    BackendLaneConfig {
        label: backend_selector_label(&operator_backend),
        chat_runtime,
        operator_backend,
        mode: LaneOperatorMode::Coding,
        launch_mode: LaneLaunchMode::Foreground,
        review_mode: LaneReviewMode::AutoSafe,
        transcript_mode: TranscriptMode::Conversation,
        memory_stack,
        carry_forward_summary: None,
        session_generation: 0,
    }
}

fn build_backend_lanes(config: &TuiLaunchConfig) -> [BackendLaneConfig; 3] {
    BACKEND_SELECTOR_ORDER.map(|backend_kind| {
        if backend_kind == config.operator_backend.backend_kind {
            let memory_stack = load_probe_memory_stack(
                config.chat_runtime.probe_home.as_deref(),
                config.chat_runtime.cwd.as_path(),
            );
            BackendLaneConfig {
                label: backend_selector_label(&config.operator_backend),
                chat_runtime: build_chat_runtime_config_for_lane(
                    config.chat_runtime.probe_home.clone(),
                    config.chat_runtime.cwd.clone(),
                    config.chat_runtime.profile.clone(),
                    LaneOperatorMode::Coding,
                    LaneReviewMode::AutoSafe,
                    Some(&memory_stack),
                    None,
                    0,
                ),
                operator_backend: config.operator_backend.clone(),
                mode: LaneOperatorMode::Coding,
                launch_mode: LaneLaunchMode::Foreground,
                review_mode: LaneReviewMode::AutoSafe,
                transcript_mode: TranscriptMode::Conversation,
                memory_stack,
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
        backend_lanes[lane_index].launch_mode.label(),
        backend_lanes[lane_index].review_mode.label(),
        backend_lanes[lane_index].carry_forward_summary.as_deref(),
    );
    screen.set_transcript_mode(backend_lanes[lane_index].transcript_mode);
    screen.set_memory_stack(backend_lanes[lane_index].memory_stack.clone());
    screen
}

fn operator_system_addendum(
    mode: LaneOperatorMode,
    review_mode: LaneReviewMode,
    memory_stack: Option<&ProbeMemoryStack>,
    carry_forward_summary: Option<&str>,
) -> Option<String> {
    let mut parts = Vec::new();
    if mode == LaneOperatorMode::Plan {
        parts.push(String::from(
            "Plan mode is active.\n- Default to planning, sequencing, risk review, and implementation guidance.\n- Do not make or imply file edits as if they already happened.\n- Avoid write-capable tools unless the operator explicitly switches back to coding mode.",
        ));
    }
    if mode == LaneOperatorMode::Coding {
        parts.push(match review_mode {
            LaneReviewMode::AutoSafe => String::from(
                "Review mode is auto-safe.\n- Continue with normal coding behavior.\n- When you make edits, keep the human informed with concise file-level summaries.",
            ),
            LaneReviewMode::ReviewRisky => String::from(
                "Review mode is review-risky.\n- Expect Probe to pause risky write, network, and destructive actions for approval before they land.\n- Gather enough context first so the approval step is easy to review.",
            ),
            LaneReviewMode::ReviewAll => String::from(
                "Review mode is review-all.\n- Treat write-capable work as approval-first.\n- Explain what you intend to change before you rely on a write-capable tool.",
            ),
        });
    }
    if let Some(summary) = carry_forward_summary.filter(|value| !value.trim().is_empty()) {
        parts.push(format!(
            "Carry-forward context from the prior session:\n{summary}\nUse this summary instead of assuming the full prior transcript is available."
        ));
    }
    if let Some(memory_addendum) = memory_stack.and_then(ProbeMemoryStack::prompt_addendum) {
        parts.push(memory_addendum);
    }
    (!parts.is_empty()).then_some(parts.join("\n\n"))
}

fn tool_loop_for_mode(mode: LaneOperatorMode, review_mode: LaneReviewMode) -> ToolLoopConfig {
    let mut tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Auto, false);
    tool_loop.approval = match mode {
        LaneOperatorMode::Coding => match review_mode {
            LaneReviewMode::AutoSafe => ToolApprovalConfig::allow_all(),
            LaneReviewMode::ReviewRisky | LaneReviewMode::ReviewAll => ToolApprovalConfig {
                allow_write_tools: false,
                allow_network_shell: false,
                allow_destructive_shell: false,
                denied_action: probe_core::tools::ToolDeniedAction::Pause,
            },
        },
        LaneOperatorMode::Plan => ToolApprovalConfig::conservative(),
    };
    tool_loop
}

fn build_chat_runtime_config_for_lane(
    probe_home: Option<PathBuf>,
    cwd: PathBuf,
    profile: BackendProfile,
    mode: LaneOperatorMode,
    review_mode: LaneReviewMode,
    memory_stack: Option<&ProbeMemoryStack>,
    carry_forward_summary: Option<String>,
    session_generation: u64,
) -> ProbeRuntimeTurnConfig {
    let loaded_memory_stack;
    let memory_stack = if let Some(memory_stack) = memory_stack {
        Some(memory_stack)
    } else {
        loaded_memory_stack = load_probe_memory_stack(probe_home.as_deref(), cwd.as_path());
        Some(&loaded_memory_stack)
    };
    let operator_system = operator_system_addendum(
        mode,
        review_mode,
        memory_stack,
        carry_forward_summary.as_deref(),
    );
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
        tool_loop: Some(tool_loop_for_mode(mode, review_mode)),
        session_generation,
    }
}

fn rebuild_lane_runtime(lane: &mut BackendLaneConfig) {
    lane.memory_stack = load_probe_memory_stack(
        lane.chat_runtime.probe_home.as_deref(),
        lane.chat_runtime.cwd.as_path(),
    );
    lane.chat_runtime = build_chat_runtime_config_for_lane(
        lane.chat_runtime.probe_home.clone(),
        lane.chat_runtime.cwd.clone(),
        lane.chat_runtime.profile.clone(),
        lane.mode,
        lane.review_mode,
        Some(&lane.memory_stack),
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

fn build_diff_overlay(lane_label: &str, workspace: &TaskWorkspaceSummary) -> DiffOverlay {
    let mut summary_lines = vec![
        format!("Inspect the latest task diff for {lane_label}."),
        format!("files changed: {}", workspace.changed_files.len()),
        String::from(
            "Use Up/Down to choose a file. PgUp/PgDn scroll. Enter resets scroll. Esc closes.",
        ),
    ];
    if workspace.checkpoint.status == probe_protocol::session::TaskCheckpointStatus::Limited {
        summary_lines.push(String::from("checkpoint: limited coverage for this task"));
    }

    let files = workspace
        .changed_files
        .iter()
        .map(|path| {
            if let Some(preview) = workspace
                .diff_previews
                .iter()
                .find(|preview| preview.path == *path)
            {
                return TaskDiffFileView {
                    path: preview.path.clone(),
                    diff_lines: preview.diff_lines.clone(),
                    truncated: preview.truncated,
                };
            }
            if let Some(repo_root) = workspace.repo_root.as_deref() {
                return build_task_diff_file_view(repo_root, path);
            }
            TaskDiffFileView {
                path: path.clone(),
                diff_lines: vec![String::from(
                    "No recorded diff preview is available for this file yet.",
                )],
                truncated: false,
            }
        })
        .collect::<Vec<_>>();

    if workspace.repo_root.is_none() && workspace.diff_previews.is_empty() {
        summary_lines.push(String::from(
            "Diff inspection is limited because this task was not recorded against a git workspace root.",
        ));
    } else if !workspace.diff_previews.is_empty() {
        summary_lines.push(String::from(
            "Preview source: recorded with the latest task receipt.",
        ));
    }

    DiffOverlay::new("Diff", summary_lines, files)
}

fn build_pending_approval_diff_overlay(
    lane_label: &str,
    approval: &PendingToolApproval,
) -> Option<DiffOverlay> {
    if approval.tool_name != "apply_patch" {
        return None;
    }
    let file = build_pending_apply_patch_diff_view(approval)?;
    let summary_lines = vec![
        format!("Inspect the proposed diff waiting for approval on {lane_label}."),
        String::from("status: pending approval"),
        String::from("files changed: 1 proposed"),
        String::from(
            "Use Up/Down to choose a file. PgUp/PgDn scroll. Enter resets scroll. Esc closes.",
        ),
    ];
    Some(DiffOverlay::new("Diff", summary_lines, vec![file]))
}

fn build_pending_apply_patch_diff_view(approval: &PendingToolApproval) -> Option<TaskDiffFileView> {
    if let Some(proposed) = approval.proposed_edit.as_ref() {
        let path = proposed.changed_files.first()?.clone();
        let mut diff_lines = vec![
            format!("diff --probe a/{path} b/{path}"),
            format!("--- a/{path}"),
            format!("+++ b/{path}"),
            String::from("@@ proposed @@"),
        ];
        if proposed.preview_lines.is_empty() {
            diff_lines.push(String::from("  [no textual diff preview available]"));
        } else {
            diff_lines.extend(proposed.preview_lines.clone());
        }
        let truncated = diff_lines.len() > 180;
        if truncated {
            diff_lines.truncate(180);
        }
        return Some(TaskDiffFileView {
            path,
            diff_lines,
            truncated,
        });
    }
    let path = approval
        .arguments
        .get("path")
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let old_text = approval
        .arguments
        .get("old_text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let new_text = approval
        .arguments
        .get("new_text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let mut diff_lines = proposed_apply_patch_diff_lines(path.as_str(), old_text, new_text);
    let truncated = diff_lines.len() > 180;
    if truncated {
        diff_lines.truncate(180);
    }
    Some(TaskDiffFileView {
        path,
        diff_lines,
        truncated,
    })
}

fn proposed_apply_patch_diff_lines(path: &str, old_text: &str, new_text: &str) -> Vec<String> {
    let mut lines = vec![
        format!("diff --probe a/{path} b/{path}"),
        format!("--- a/{path}"),
        format!("+++ b/{path}"),
    ];

    let old_lines = normalized_patch_lines(old_text);
    let new_lines = normalized_patch_lines(new_text);
    let max_lines = old_lines.len().max(new_lines.len());
    if max_lines == 0 {
        lines.push(String::from("@@"));
        lines.push(String::from("  [no textual diff preview available]"));
        return lines;
    }

    lines.push(String::from("@@ proposed @@"));
    for index in 0..max_lines {
        let old_line = old_lines.get(index);
        let new_line = new_lines.get(index);
        match (old_line, new_line) {
            (Some(old_line), Some(new_line)) if old_line == new_line => {
                lines.push(format!("  {}", display_diff_line(old_line)));
            }
            (Some(old_line), Some(new_line)) => {
                lines.push(format!("- {}", display_diff_line(old_line)));
                lines.push(format!("+ {}", display_diff_line(new_line)));
            }
            (Some(old_line), None) => {
                lines.push(format!("- {}", display_diff_line(old_line)));
            }
            (None, Some(new_line)) => {
                lines.push(format!("+ {}", display_diff_line(new_line)));
            }
            (None, None) => {}
        }
    }
    lines
}

fn normalized_patch_lines(value: &str) -> Vec<String> {
    value.split('\n').map(ToOwned::to_owned).collect()
}

fn display_diff_line(line: &str) -> String {
    if line.is_empty() {
        String::from("[blank]")
    } else {
        line.to_string()
    }
}

fn build_task_diff_file_view(repo_root: &Path, path: &str) -> TaskDiffFileView {
    let mut diff_text = git_diff_for_path(repo_root, path);
    if diff_text.trim().is_empty() {
        diff_text = git_diff_for_untracked_path(repo_root, path).unwrap_or_default();
    }
    if diff_text.trim().is_empty() {
        diff_text = String::from("No git diff output is available for this file right now.");
    }
    let mut diff_lines = diff_text.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let truncated = diff_lines.len() > 180;
    if truncated {
        diff_lines.truncate(180);
    }
    TaskDiffFileView {
        path: path.to_string(),
        diff_lines,
        truncated,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitRepoStatusSummary {
    repo_root: PathBuf,
    branch_label: String,
    staged_count: usize,
    unstaged_count: usize,
    untracked_count: usize,
    preview_paths: Vec<String>,
    staged_paths: Vec<String>,
}

impl GitRepoStatusSummary {
    fn staged_preview_paths(&self) -> Vec<String> {
        if !self.staged_paths.is_empty() {
            return self.staged_paths.clone();
        }
        self.preview_paths.clone()
    }
}

fn git_repo_status_summary(cwd: &Path) -> Option<GitRepoStatusSummary> {
    let repo_root = resolve_git_repo_root_local(cwd)?;
    let branch_label = git_run_string(
        repo_root.as_path(),
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )
    .or_else(|| git_run_string(repo_root.as_path(), &["rev-parse", "--short", "HEAD"]))
    .unwrap_or_else(|| String::from("detached"));
    let status_output = git_run_stdout(
        repo_root.as_path(),
        &["status", "--porcelain", "--untracked-files=all"],
    )
    .unwrap_or_default();
    let mut staged_count = 0usize;
    let mut unstaged_count = 0usize;
    let mut untracked_count = 0usize;
    let mut preview_paths = Vec::new();
    let mut staged_paths = Vec::new();
    for line in status_output.lines() {
        if let Some((x, y, path)) = parse_git_status_line(line) {
            if x != ' ' && x != '?' {
                staged_count += 1;
                staged_paths.push(path.clone());
            }
            if y != ' ' {
                unstaged_count += 1;
            }
            if x == '?' && y == '?' {
                untracked_count += 1;
            }
            preview_paths.push(path);
        }
    }
    preview_paths.sort();
    preview_paths.dedup();
    staged_paths.sort();
    staged_paths.dedup();
    Some(GitRepoStatusSummary {
        repo_root,
        branch_label,
        staged_count,
        unstaged_count,
        untracked_count,
        preview_paths,
        staged_paths,
    })
}

fn git_diff_for_path(repo_root: &Path, path: &str) -> String {
    Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("diff")
        .arg("--")
        .arg(path)
        .output()
        .ok()
        .filter(|output| output.status.success() || !output.stdout.is_empty())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
}

fn git_diff_for_untracked_path(repo_root: &Path, path: &str) -> Option<String> {
    let absolute_path = repo_root.join(path);
    if !absolute_path.exists() {
        return None;
    }
    Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("diff")
        .arg("--no-index")
        .arg("--")
        .arg("/dev/null")
        .arg(&absolute_path)
        .output()
        .ok()
        .filter(|output| {
            output.status.code().is_some_and(|code| code <= 1) && !output.stdout.is_empty()
        })
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_git_status_line(line: &str) -> Option<(char, char, String)> {
    if line.len() < 3 {
        return None;
    }
    let mut chars = line.chars();
    let x = chars.next()?;
    let y = chars.next()?;
    let path = line.get(3..)?.trim();
    if path.is_empty() {
        return None;
    }
    Some((x, y, path.to_string()))
}

fn resolve_git_repo_root_local(cwd: &Path) -> Option<PathBuf> {
    git_run_string(cwd, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

fn git_run_string(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn git_run_stdout(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_ahead_behind_local(value: &str) -> Option<(Option<u64>, Option<u64>)> {
    let mut parts = value.split_whitespace();
    let ahead = parts.next()?.parse().ok()?;
    let behind = parts.next()?.parse().ok()?;
    Some((Some(ahead), Some(behind)))
}

fn session_branch_state_local(cwd: &Path) -> Option<SessionBranchState> {
    let repo_root = resolve_git_repo_root_local(cwd)?;
    let head_commit = git_run_string(repo_root.as_path(), &["rev-parse", "HEAD"])?;
    let head_ref = git_run_string(
        repo_root.as_path(),
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )
    .or_else(|| git_run_string(repo_root.as_path(), &["rev-parse", "--short", "HEAD"]))?;
    let detached_head = git_run_string(
        repo_root.as_path(),
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )
    .is_none();
    let working_tree_dirty = git_run_string(repo_root.as_path(), &["status", "--porcelain"])
        .is_some_and(|output| !output.trim().is_empty());
    let upstream_ref = git_run_string(
        repo_root.as_path(),
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    );
    let (ahead_by, behind_by) = upstream_ref
        .as_ref()
        .and_then(|_| {
            git_run_string(
                repo_root.as_path(),
                &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
            )
        })
        .and_then(|counts| parse_ahead_behind_local(counts.as_str()))
        .unwrap_or((None, None));
    Some(SessionBranchState {
        repo_root,
        head_ref,
        head_commit,
        detached_head,
        working_tree_dirty,
        upstream_ref,
        ahead_by,
        behind_by,
    })
}

fn session_delivery_state_local(
    branch_state: &SessionBranchState,
    updated_at_ms: u64,
) -> SessionDeliveryState {
    let status = if branch_state.working_tree_dirty {
        SessionDeliveryStatus::NeedsCommit
    } else if branch_state.behind_by.unwrap_or(0) > 0 {
        SessionDeliveryStatus::Diverged
    } else if branch_state.ahead_by.unwrap_or(0) > 0 {
        SessionDeliveryStatus::NeedsPush
    } else if branch_state.upstream_ref.is_some() {
        SessionDeliveryStatus::Synced
    } else {
        SessionDeliveryStatus::LocalOnly
    };
    let branch_name = (!branch_state.detached_head).then(|| branch_state.head_ref.clone());
    let compare_ref = branch_state.upstream_ref.as_ref().and_then(|upstream_ref| {
        branch_name
            .as_ref()
            .map(|branch_name| format!("{upstream_ref}...{branch_name}"))
    });
    let mut artifacts = vec![SessionDeliveryArtifact {
        kind: String::from("head_commit"),
        value: branch_state.head_commit.clone(),
        label: Some(String::from("Head commit")),
    }];
    artifacts.push(SessionDeliveryArtifact {
        kind: String::from("head_ref"),
        value: branch_state.head_ref.clone(),
        label: Some(String::from("Head ref")),
    });
    SessionDeliveryState {
        status,
        branch_name,
        remote_tracking_ref: branch_state.upstream_ref.clone(),
        compare_ref,
        updated_at_ms,
        artifacts,
    }
}

fn suggested_branch_name(summary: &GitRepoStatusSummary) -> String {
    let branch_hint = if summary.preview_paths.len() == 1 {
        Path::new(summary.preview_paths[0].as_str())
            .file_stem()
            .and_then(|value| value.to_str())
            .map(str::to_owned)
    } else if matches!(
        summary.branch_label.as_str(),
        "main" | "master" | "develop" | "trunk" | "detached"
    ) {
        summary
            .repo_root
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_owned)
    } else {
        Some(summary.branch_label.clone())
    }
    .unwrap_or_else(|| String::from("probe-task"));
    format!("codex/{}", sanitize_branch_component(branch_hint.as_str()))
}

fn push_overlay_state(branch_state: &SessionBranchState) -> (bool, bool, String) {
    if branch_state.detached_head {
        return (
            false,
            false,
            String::from("Probe cannot push from a detached HEAD. Switch to a branch first."),
        );
    }
    let remote_name = branch_state
        .upstream_ref
        .as_ref()
        .and_then(|value| value.split('/').next().map(str::to_owned))
        .or_else(|| git_default_remote_name(branch_state.repo_root.as_path()));
    let Some(remote_name) = remote_name else {
        return (
            false,
            false,
            String::from(
                "Probe could not find a git remote for this repo. Add one before pushing.",
            ),
        );
    };
    if branch_state.behind_by.unwrap_or(0) > 0 {
        return (
            false,
            false,
            format!(
                "This branch is behind {}. Pull or rebase before you push.",
                branch_state
                    .upstream_ref
                    .as_deref()
                    .unwrap_or(remote_name.as_str())
            ),
        );
    }
    if branch_state.upstream_ref.is_some() && branch_state.ahead_by.unwrap_or(0) == 0 {
        return (
            false,
            false,
            String::from("This branch is already synced. There are no local commits to push."),
        );
    }
    (
        true,
        branch_state.upstream_ref.is_none(),
        String::from("push is ready"),
    )
}

fn pr_overlay_state(branch_state: &SessionBranchState, base_branch: &str) -> (bool, String) {
    if branch_state.detached_head {
        return (
            false,
            String::from("Switch to a branch before you open a draft PR."),
        );
    }
    if branch_state.head_ref == base_branch {
        return (
            false,
            format!(
                "The current branch already matches the base branch ({base_branch}). Create a work branch first."
            ),
        );
    }
    if branch_state.upstream_ref.is_none() {
        return (
            false,
            String::from(
                "Push this branch first so Probe knows what head branch GitHub should review.",
            ),
        );
    }
    if branch_state.behind_by.unwrap_or(0) > 0 {
        return (
            false,
            String::from(
                "This branch is behind its remote. Pull or rebase before opening a draft PR.",
            ),
        );
    }
    (true, String::from("draft PR is ready"))
}

fn build_pr_comments_overlay(cwd: &Path) -> PrCommentsOverlay {
    let Some(branch_state) = session_branch_state_local(cwd) else {
        return PrCommentsOverlay::new(
            vec![
                String::from("Probe could not detect a git repo for the current workspace."),
                String::from(""),
                String::from("next: open a repo workspace first, then try /pr_comments again."),
            ],
            Vec::new(),
        );
    };
    if branch_state.upstream_ref.is_none() {
        return PrCommentsOverlay::new(
            vec![
                format!("branch: {}", branch_state.head_ref),
                String::from("Push this branch first so Probe can find the PR tied to it."),
                String::from(""),
                String::from("next: use /push, then come back to /pr_comments."),
            ],
            Vec::new(),
        );
    }
    match load_current_pr_review_summary(branch_state.repo_root.as_path()) {
        Ok(summary) => render_pr_comments_overlay_summary(summary),
        Err(error) => PrCommentsOverlay::new(
            vec![
                format!("branch: {}", branch_state.head_ref),
                error.clone(),
                String::from(""),
                pr_comments_next_step(error.as_str()),
            ],
            Vec::new(),
        ),
    }
}

fn load_current_pr_review_summary(repo_root: &Path) -> Result<GhPrReviewSummary, String> {
    let gh_program = gh_program_for_repo(repo_root);
    let output = Command::new(&gh_program)
        .arg("pr")
        .arg("view")
        .arg("--json")
        .arg("number,title,url,reviewDecision,isDraft,headRefName,baseRefName,comments,reviews")
        .current_dir(repo_root)
        .output()
        .map_err(|error| format!("Probe could not start gh to inspect PR comments: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if detail.contains("no pull requests found") || detail.contains("no pull request found") {
            return Err(String::from(
                "No pull request is linked to this branch yet.",
            ));
        }
        if detail.contains("authentication") || detail.contains("not logged into any GitHub hosts")
        {
            return Err(String::from(
                "GitHub auth is required before Probe can load PR comments.",
            ));
        }
        return Err(gh_command_error("inspect PR comments", &output));
    }
    serde_json::from_slice::<GhPrReviewSummary>(&output.stdout)
        .map_err(|error| format!("Probe could not parse the current PR review payload: {error}"))
}

fn render_pr_comments_overlay_summary(summary: GhPrReviewSummary) -> PrCommentsOverlay {
    let mut lines = vec![
        format!("PR #{}: {}", summary.number, summary.title),
        format!("url: {}", summary.url),
        format!(
            "state: {} · decision: {}",
            if summary.is_draft { "draft" } else { "ready" },
            summary
                .review_decision
                .clone()
                .unwrap_or_else(|| String::from("not set"))
        ),
        format!(
            "branch pair: {} -> {}",
            summary.head_ref_name, summary.base_ref_name
        ),
        format!(
            "feedback: {} issue comment(s) · {} review(s)",
            summary.comments.len(),
            summary.reviews.len()
        ),
        String::from(""),
    ];

    let mut added_feedback = false;
    if !summary.comments.is_empty() {
        lines.push(String::from("recent comments:"));
        for comment in summary.comments.iter().take(3) {
            let author = comment
                .author
                .as_ref()
                .map(|actor| actor.login.as_str())
                .unwrap_or("unknown");
            lines.push(format!(
                "- {}: {}",
                author,
                preview(summarize_review_text(comment.body.as_str()).as_str(), 96)
            ));
        }
        lines.push(String::from(""));
        added_feedback = true;
    }
    let review_bodies = summary
        .reviews
        .iter()
        .filter(|review| !review.body.trim().is_empty())
        .take(3)
        .collect::<Vec<_>>();
    if !review_bodies.is_empty() {
        lines.push(String::from("recent reviews:"));
        for review in review_bodies {
            let author = review
                .author
                .as_ref()
                .map(|actor| actor.login.as_str())
                .unwrap_or("unknown");
            lines.push(format!(
                "- {} ({}) {}",
                author,
                review.state.to_lowercase(),
                preview(summarize_review_text(review.body.as_str()).as_str(), 88)
            ));
        }
        lines.push(String::from(""));
        added_feedback = true;
    }

    if !added_feedback {
        lines.push(String::from(
            "No review comments or review bodies are visible yet.",
        ));
        return PrCommentsOverlay::new(lines, Vec::new());
    }

    PrCommentsOverlay::new(lines, pr_feedback_items(&summary))
}

fn summarize_review_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn pr_feedback_items(summary: &GhPrReviewSummary) -> Vec<PrFeedbackItemView> {
    let mut items = Vec::new();

    for comment in summary
        .comments
        .iter()
        .filter(|comment| !comment.body.trim().is_empty())
    {
        let author = comment
            .author
            .as_ref()
            .map(|actor| actor.login.as_str())
            .unwrap_or("unknown");
        let normalized = summarize_review_text(comment.body.as_str());
        items.push(PrFeedbackItemView {
            label: format!("issue comment · {author}"),
            preview: preview(normalized.as_str(), 88),
            detail_lines: vec![
                format!("source: issue comment from {author}"),
                format!("pr: #{} {}", summary.number, summary.title),
                String::from("feedback:"),
                normalized,
            ],
            seed_text: format!(
                "Please address this PR feedback from {author} on PR #{} ({}): {}",
                summary.number,
                summary.title,
                comment.body.trim()
            ),
        });
    }

    for review in summary
        .reviews
        .iter()
        .filter(|review| !review.body.trim().is_empty())
    {
        let author = review
            .author
            .as_ref()
            .map(|actor| actor.login.as_str())
            .unwrap_or("unknown");
        let normalized = summarize_review_text(review.body.as_str());
        items.push(PrFeedbackItemView {
            label: format!("review · {} · {}", author, review.state.to_lowercase()),
            preview: preview(normalized.as_str(), 82),
            detail_lines: vec![
                format!(
                    "source: {} review from {}",
                    review.state.to_lowercase(),
                    author
                ),
                format!("pr: #{} {}", summary.number, summary.title),
                String::from("feedback:"),
                normalized,
            ],
            seed_text: format!(
                "Please address this PR review feedback from {} on PR #{} ({}) [{}]: {}",
                author,
                summary.number,
                summary.title,
                review.state,
                review.body.trim()
            ),
        });
    }

    items
}

fn pr_comments_next_step(error: &str) -> String {
    if error.contains("No pull request is linked") {
        return String::from("next: use /pr after /push once this branch is ready.");
    }
    if error.contains("GitHub auth is required") {
        return String::from("next: authenticate gh, then try /pr_comments again.");
    }
    String::from("next: fix the GitHub/PR issue above, then retry /pr_comments.")
}

fn suggested_pr_title(repo_root: &Path, branch_name: &str) -> String {
    git_run_string(repo_root, &["log", "-1", "--pretty=%s"]).unwrap_or_else(|| {
        format!(
            "Draft: {}",
            branch_name
                .strip_prefix("codex/")
                .unwrap_or(branch_name)
                .replace('-', " ")
        )
    })
}

fn suggested_commit_message(summary: &GitRepoStatusSummary) -> String {
    let paths = summary.staged_preview_paths();
    match paths.as_slice() {
        [] => String::from("Update repo changes"),
        [single] => format!("Update {}", single),
        _ => format!("Update {} files", paths.len()),
    }
}

fn summarize_paths(paths: &[String], max_items: usize) -> String {
    if paths.is_empty() {
        return String::from("no paths reported");
    }
    let shown = paths.iter().take(max_items).cloned().collect::<Vec<_>>();
    if paths.len() > max_items {
        format!("{} +{} more", shown.join(", "), paths.len() - max_items)
    } else {
        shown.join(", ")
    }
}

fn sanitize_branch_component(value: &str) -> String {
    let mut sanitized = String::new();
    let mut previous_dash = false;
    for ch in value.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            sanitized.push(lower);
            previous_dash = false;
        } else if matches!(lower, '-' | '_' | '/' | ' ' | '.') && !previous_dash {
            sanitized.push('-');
            previous_dash = true;
        }
    }
    let sanitized = sanitized.trim_matches('-').to_string();
    if sanitized.is_empty() {
        String::from("probe-task")
    } else {
        sanitized
    }
}

fn git_branch_name_is_valid(repo_root: &Path, name: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("check-ref-format")
        .arg("--branch")
        .arg(name)
        .output()
        .ok()
        .is_some_and(|output| output.status.success())
}

fn git_local_branch_exists(repo_root: &Path, name: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("show-ref")
        .arg("--verify")
        .arg("--quiet")
        .arg(format!("refs/heads/{name}"))
        .output()
        .ok()
        .is_some_and(|output| output.status.success())
}

fn git_default_remote_name(repo_root: &Path) -> Option<String> {
    git_run_stdout(repo_root, &["remote"]).and_then(|output| {
        output
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
    })
}

fn git_remote_default_branch(repo_root: &Path, remote_name: &str) -> Option<String> {
    git_run_string(
        repo_root,
        &[
            "symbolic-ref",
            "--quiet",
            &format!("refs/remotes/{remote_name}/HEAD"),
        ],
    )
    .and_then(|value| value.rsplit('/').next().map(str::to_owned))
}

fn gh_program_for_repo(repo_root: &Path) -> String {
    git_run_string(repo_root, &["config", "--get", "probe.ghBin"])
        .unwrap_or_else(|| String::from("gh"))
}

fn git_command_error(action: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("git exited with status {}", output.status)
    };
    format!("Probe could not {action}: {detail}")
}

fn gh_command_error(action: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("gh exited with status {}", output.status)
    };
    format!("Probe could not {action}: {detail}")
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
    use std::collections::HashMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{Mutex, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        DetachedSessionRecoveryState, DetachedSessionStatus, McpServerSource, detached_summary_view,
    };

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
        PendingToolApproval, SessionBackendTarget, SessionBranchState, SessionDeliveryState,
        SessionDeliveryStatus, SessionId, SessionMcpConnectionStatus, SessionMcpServer,
        SessionMcpServerSource, SessionMcpServerTransport, SessionMcpState,
        SessionWorkspaceBootMode, SessionWorkspaceState, TaskCheckpointStatus,
        TaskCheckpointSummary, TaskDiffPreview, TaskFinalReceipt, TaskReceiptDisposition,
        TaskRevertibilityStatus, TaskRevertibilitySummary, TaskVerificationCommandStatus,
        TaskVerificationCommandSummary, TaskVerificationStatus, TaskWorkspaceSummary,
        TaskWorkspaceSummaryStatus, ToolApprovalState, ToolExecutionRecord, ToolPolicyDecision,
        ToolRiskClass,
    };
    use probe_test_support::{
        FakeAppleFmServer, FakeHttpResponse, FakeOpenAiServer, ProbeTestEnvironment,
    };
    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        AppShell, LaneLaunchMode, LaneOperatorMode, LaneReviewMode, McpRegistryFile,
        McpServerRecord, McpServerTransport, TuiLaunchConfig, build_chat_runtime_config_for_lane,
        mcp_registry_path, operator_summary_from_profile, profile_from_server_config,
        resolve_tui_chat_profile, save_mcp_registry,
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

    fn init_git_repo(path: &Path) {
        let status = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["init", "-q"])
            .status()
            .expect("initialize git repo");
        assert!(status.success(), "git init failed for {}", path.display());
    }

    fn seed_memory_fixture(probe_home: &Path, repo_root: &Path) -> PathBuf {
        fs::create_dir_all(probe_home.join("memory")).expect("create probe memory dir");
        fs::write(
            probe_home.join("memory/USER.md"),
            "Always prefer concise teammate-style handoffs.",
        )
        .expect("write user memory");
        fs::write(
            repo_root.join("AGENTS.md"),
            "Repo memory from AGENTS fallback.",
        )
        .expect("write repo memory");
        let nested_dir = repo_root.join("src/features");
        fs::create_dir_all(&nested_dir).expect("create nested workspace");
        fs::write(
            nested_dir.join("PROBE.md"),
            "Feature-specific rule for src/features work.",
        )
        .expect("write directory memory");
        nested_dir
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

    fn checkpoint(status: TaskCheckpointStatus, summary_text: &str) -> TaskCheckpointSummary {
        TaskCheckpointSummary {
            status,
            summary_text: String::from(summary_text),
        }
    }

    fn revertibility(
        status: TaskRevertibilityStatus,
        summary_text: &str,
    ) -> TaskRevertibilitySummary {
        TaskRevertibilitySummary {
            status,
            summary_text: String::from(summary_text),
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

        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("lane: Qwen"));
        assert!(rendered.contains("workspace:"));
        assert!(rendered.contains("safety: auto-safe"));
        assert!(!rendered.contains("transport:"));
        assert!(!rendered.contains("tools: on"));
        assert!(!rendered.contains("approvals:"));
        assert!(!rendered.contains("keys: F2 status"));
    }

    #[test]
    fn main_shell_uses_compact_top_session_panel_on_narrow_terminals() {
        let app = AppShell::new_for_tests();

        let rendered = app.render_to_string(88, 36);

        assert!(rendered.contains("Session"), "{rendered}");
        assert!(rendered.contains("lane: Qwen"), "{rendered}");
        assert!(rendered.contains("workspace:"), "{rendered}");
        assert!(rendered.contains("safety: auto-safe"), "{rendered}");
        assert!(rendered.contains("details: /status"), "{rendered}");
        assert!(
            rendered.contains("Start Here") || rendered.contains("Transcript"),
            "{rendered}"
        );
    }

    #[test]
    fn main_shell_shows_active_memory_summary_when_layers_are_loaded() {
        let probe_home = tempdir().expect("temp probe home");
        let repo_root = tempdir().expect("temp repo root");
        init_git_repo(repo_root.path());
        let cwd = seed_memory_fixture(probe_home.path(), repo_root.path());
        let profile = openai_codex_subscription();
        let launch_config = TuiLaunchConfig {
            chat_runtime: build_chat_runtime_config_for_lane(
                Some(probe_home.path().to_path_buf()),
                cwd,
                profile.clone(),
                LaneOperatorMode::Coding,
                LaneReviewMode::AutoSafe,
                None,
                None,
                0,
            ),
            operator_backend: operator_summary_from_profile(&profile),
            autostart_apple_fm_setup: false,
            resume_session_id: None,
        };

        let app = AppShell::new_with_launch_config(launch_config);
        let rendered = app.render_to_string(140, 36);

        assert_eq!(app.base_screen().memory_stack().layers.len(), 3);
        assert!(rendered.contains("memory:"), "{rendered}");
        assert!(rendered.contains("repo/AGENTS"), "{rendered}");
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
                checkpoint: checkpoint(
                    TaskCheckpointStatus::Captured,
                    "Probe captured a pre-edit checkpoint before changes landed in src/lib.rs.",
                ),
                revertibility: revertibility(
                    TaskRevertibilityStatus::Exact,
                    "Probe has enough checkpoint coverage to attempt an exact restore for src/lib.rs.",
                ),
                diff_previews: Vec::new(),
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
                    checkpoint: checkpoint(
                        TaskCheckpointStatus::Captured,
                        "Probe captured a pre-edit checkpoint before changes landed in src/lib.rs.",
                    ),
                    revertibility: revertibility(
                        TaskRevertibilityStatus::Exact,
                        "Probe has enough checkpoint coverage to attempt an exact restore for src/lib.rs.",
                    ),
                    diff_previews: Vec::new(),
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
            mcp_state: None,
            recovery_note: None,
        });

        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("last task: applied"));
        assert!(rendered.contains("checkpoint: captured"));
        assert!(rendered.contains("revert: available"));
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
            mcp_state: None,
            recovery_note: None,
        });

        let rendered = app.render_to_string(120, 44);
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
                    checkpoint: checkpoint(
                        TaskCheckpointStatus::NotCaptured,
                        "No pre-edit checkpoint was needed because no repo changes landed.",
                    ),
                    revertibility: revertibility(
                        TaskRevertibilityStatus::Unavailable,
                        "No applied repo changes are currently available for automated revert.",
                    ),
                    diff_previews: Vec::new(),
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
            mcp_state: None,
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
                proposed_edit: None,
                resolved_at_ms: None,
                resolution: None,
            }],
        });
        app.dispatch(UiEvent::Dismiss);

        let _rendered = app.render_to_string(120, 32);
        assert!(
            app.base_screen()
                .compact_runtime_status()
                .contains("activity: review changes:")
        );
    }

    #[test]
    fn main_shell_surfaces_pending_apply_patch_as_proposed_review_state() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_proposed_review"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: Some(RuntimeActivity::new(
                RuntimeActivityKind::WaitingForApproval,
                "waiting for approval: apply_patch",
            )),
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });
        app.apply_message(AppMessage::PendingToolApprovalsUpdated {
            session_id: String::from("sess_proposed_review"),
            approvals: vec![PendingToolApproval {
                session_id: SessionId::new("sess_proposed_review"),
                tool_call_id: String::from("call_patch_review"),
                tool_name: String::from("apply_patch"),
                arguments: json!({
                    "path": "src/lib.rs",
                    "old_text": "pub fn old_name() {}\n",
                    "new_text": "pub fn new_name() {}\n"
                }),
                risk_class: ToolRiskClass::Write,
                reason: Some(String::from("review-risky pauses write-capable tools")),
                tool_call_turn_index: 3,
                paused_result_turn_index: 3,
                requested_at_ms: 10,
                proposed_edit: None,
                resolved_at_ms: None,
                resolution: None,
            }],
        });
        app.dispatch(UiEvent::Dismiss);

        let rendered = app.render_to_string(120, 40);
        assert!(rendered.contains("Review Changes"), "{rendered}");
        assert!(rendered.contains("activity: review changes:"), "{rendered}");
        assert!(rendered.contains("src/lib.rs"), "{rendered}");
        assert!(rendered.contains("next: /diff previews"), "{rendered}");
        assert!(rendered.contains("A applies"), "{rendered}");
        assert!(rendered.contains("last task: proposed"), "{rendered}");
        assert!(
            rendered.contains("checkpoint: pending review"),
            "{rendered}"
        );
        assert!(
            rendered.contains("edits: proposed -> src/lib.rs"),
            "{rendered}"
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
                checkpoint: checkpoint(
                    TaskCheckpointStatus::Limited,
                    "Probe observed write-capable work but cannot confirm a complete pre-edit checkpoint for this task.",
                ),
                revertibility: revertibility(
                    TaskRevertibilityStatus::Limited,
                    "Probe observed write-capable work but can only offer limited revert guidance, so manual review is still required.",
                ),
                diff_previews: Vec::new(),
                summary_text: String::from(
                    "Probe cannot confirm whether repo changes landed for this task because write-capable shell commands ran without file-level change accounting. Additional dirty files appeared during the task outside tracked tool results: generated/schema.json.",
                ),
            }),
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });

        let rendered = app.render_to_string(120, 32);
        assert!(
            rendered.contains("last task: limited visibility"),
            "{rendered}"
        );
        assert!(rendered.contains("checkpoint: limited"), "{rendered}");
        assert!(rendered.contains("dirty now:"), "{rendered}");
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
            mcp_state: None,
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
            mcp_state: None,
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
            mcp_state: None,
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
            mcp_state: None,
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
    fn status_slash_command_opens_operator_status_overlay() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.dispatch(UiEvent::ComposerPaste(String::from("/status")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::StatusOverlay);
        let rendered = app.render_to_string(140, 42);
        assert!(rendered.contains("Inspect the current operator state for the active lane."));
        assert!(rendered.contains("lane: Codex"), "{rendered}");
        assert!(rendered.contains("next controls: /doctor"), "{rendered}");
        assert_eq!(app.last_status(), "opened status overlay");
    }

    #[test]
    fn doctor_slash_command_opens_health_check_overlay() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.dispatch(UiEvent::ComposerPaste(String::from("/doctor")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::DoctorOverlay);
        let rendered = app.render_to_string(140, 44);
        assert!(rendered.contains("Run a quick health check for the active lane."));
        assert!(rendered.contains("workspace: ok"), "{rendered}");
        assert!(rendered.contains("mcp:"), "{rendered}");
        assert!(rendered.contains("codex auth:"), "{rendered}");
        assert_eq!(app.last_status(), "opened doctor overlay");
    }

    #[test]
    fn keyboard_shortcuts_open_operator_overlays() {
        let probe_home = tempdir().expect("temp probe home");
        let workspace = tempdir().expect("workspace");
        let store = FilesystemSessionStore::new(probe_home.path());
        store
            .create_session_with(NewSession::new("Bug hunt", workspace.path()).with_backend(
                SessionBackendTarget {
                    profile_name: String::from("openai-codex-subscription"),
                    base_url: String::from("https://chatgpt.com/backend-api/codex"),
                    model: String::from("gpt-5.4"),
                    control_plane: None,
                    psionic_mesh: None,
                },
            ))
            .expect("create saved session");
        let mut app =
            AppShell::new_for_tests_with_chat_config(AppShell::build_chat_runtime_config(
                Some(probe_home.path().to_path_buf()),
                openai_codex_subscription(),
            ));

        app.dispatch(UiEvent::OpenStatusOverlay);
        assert_eq!(app.active_screen_id(), ScreenId::StatusOverlay);

        app.dispatch(UiEvent::Dismiss);
        app.dispatch(UiEvent::OpenDoctorOverlay);
        assert_eq!(app.active_screen_id(), ScreenId::DoctorOverlay);

        app.dispatch(UiEvent::Dismiss);
        app.dispatch(UiEvent::OpenGitOverlay);
        assert_eq!(app.active_screen_id(), ScreenId::GitOverlay);

        app.dispatch(UiEvent::Dismiss);
        app.dispatch(UiEvent::OpenTasksOverlay);
        assert_eq!(app.active_screen_id(), ScreenId::ResumeOverlay);

        app.dispatch(UiEvent::Dismiss);
        app.dispatch(UiEvent::OpenRecipesOverlay);
        assert_eq!(app.active_screen_id(), ScreenId::RecipesOverlay);
    }

    #[test]
    fn recipes_slash_command_opens_overlay_and_loads_first_step_into_composer() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.dispatch(UiEvent::ComposerPaste(String::from("/recipes")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::RecipesOverlay);
        let rendered = app.render_to_string(140, 44);
        assert!(rendered.contains("Use a guided workflow"), "{rendered}");
        assert!(rendered.contains("> Review a risky edit"), "{rendered}");
        assert!(
            rendered.contains("Enter loads `/review_mode`"),
            "{rendered}"
        );

        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        let rendered = app.render_to_string(140, 44);
        assert!(rendered.contains("/review_mode"), "{rendered}");
        assert!(rendered.contains("applied: next step ready"), "{rendered}");
        assert_eq!(app.last_status(), "loaded /review_mode into the composer");
    }

    #[test]
    fn git_slash_command_opens_overlay_with_branch_delivery_truth() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_git_overlay"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });
        app.apply_message(AppMessage::ProbeRuntimeWorkspaceStateUpdated {
            session_id: String::from("sess_git_overlay"),
            workspace_state: Some(SessionWorkspaceState {
                boot_mode: SessionWorkspaceBootMode::PreparedBaseline,
                baseline: None,
                snapshot: None,
                execution_host: None,
                provenance_note: Some(String::from("Prepared from the repo baseline.")),
            }),
            branch_state: Some(SessionBranchState {
                repo_root: PathBuf::from("/tmp/probe-workspace"),
                head_ref: String::from("feature/git-ux"),
                head_commit: String::from("1234567890abcdef"),
                detached_head: false,
                working_tree_dirty: true,
                upstream_ref: Some(String::from("origin/feature/git-ux")),
                ahead_by: Some(2),
                behind_by: Some(0),
            }),
            delivery_state: Some(SessionDeliveryState {
                status: SessionDeliveryStatus::NeedsPush,
                branch_name: Some(String::from("feature/git-ux")),
                remote_tracking_ref: Some(String::from("origin/feature/git-ux")),
                compare_ref: Some(String::from("origin/feature/git-ux...HEAD")),
                updated_at_ms: 1,
                artifacts: Vec::new(),
            }),
        });

        app.dispatch(UiEvent::ComposerPaste(String::from("/git")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::GitOverlay);
        let rendered = app.render_to_string(140, 44);
        assert!(
            rendered.contains(
                "Inspect branch, delivery, and workspace boot state for the active lane."
            )
        );
        assert!(rendered.contains("branch: feature/git-ux"), "{rendered}");
        assert!(rendered.contains("delivery: needs push"), "{rendered}");
        assert!(
            rendered.contains("workspace boot: prepared baseline"),
            "{rendered}"
        );
        assert_eq!(app.last_status(), "opened git overlay");
    }

    #[test]
    fn status_and_doctor_surfaces_git_truth() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_git_status"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });
        app.apply_message(AppMessage::ProbeRuntimeWorkspaceStateUpdated {
            session_id: String::from("sess_git_status"),
            workspace_state: Some(SessionWorkspaceState {
                boot_mode: SessionWorkspaceBootMode::Fresh,
                baseline: None,
                snapshot: None,
                execution_host: None,
                provenance_note: None,
            }),
            branch_state: Some(SessionBranchState {
                repo_root: PathBuf::from("/tmp/probe-workspace"),
                head_ref: String::from("main"),
                head_commit: String::from("abcdef1234567890"),
                detached_head: false,
                working_tree_dirty: false,
                upstream_ref: Some(String::from("origin/main")),
                ahead_by: Some(0),
                behind_by: Some(0),
            }),
            delivery_state: Some(SessionDeliveryState {
                status: SessionDeliveryStatus::Synced,
                branch_name: Some(String::from("main")),
                remote_tracking_ref: Some(String::from("origin/main")),
                compare_ref: Some(String::from("origin/main...HEAD")),
                updated_at_ms: 1,
                artifacts: Vec::new(),
            }),
        });

        app.dispatch(UiEvent::ComposerPaste(String::from("/status")));
        app.dispatch(UiEvent::ComposerSubmit);
        let status_rendered = app.render_to_string(140, 44);
        assert!(
            status_rendered.contains("git: main · clean · synced"),
            "{status_rendered}"
        );
        assert!(
            status_rendered.contains("repo: /tmp/probe-workspace"),
            "{status_rendered}"
        );
        assert!(
            status_rendered.contains("workspace boot: fresh"),
            "{status_rendered}"
        );

        app.dispatch(UiEvent::Dismiss);
        app.dispatch(UiEvent::ComposerPaste(String::from("/doctor")));
        app.dispatch(UiEvent::ComposerSubmit);
        let doctor_rendered = app.render_to_string(140, 44);
        assert!(
            doctor_rendered.contains("git: ok - main (clean working tree)"),
            "{doctor_rendered}"
        );
        assert!(
            doctor_rendered.contains("delivery: ok - branch is synced with its tracked remote"),
            "{doctor_rendered}"
        );
        assert!(
            doctor_rendered.contains("workspace boot: ok - fresh"),
            "{doctor_rendered}"
        );
    }

    #[test]
    fn branch_slash_command_opens_overlay_with_suggested_codex_branch_name() {
        let repo = tempdir().expect("repo tempdir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo.path())
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.name", "Probe"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.name");
        std::process::Command::new("git")
            .args(["config", "user.email", "probe@example.com"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.email");
        std::fs::write(repo.path().join("example.md"), "hello\n").expect("write initial");
        std::process::Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .current_dir(repo.path())
            .output()
            .expect("git commit");
        std::fs::write(repo.path().join("example.md"), "hello from Probe\n").expect("edit file");

        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.cwd = repo.path().to_path_buf();
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/branch")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::BranchOverlay);
        let rendered = app.render_to_string(140, 42);
        assert!(
            rendered.contains("Create a new branch or switch to an existing branch"),
            "{rendered}"
        );
        assert!(rendered.contains("codex/example"), "{rendered}");
        assert_eq!(app.last_status(), "opened branch overlay");
    }

    #[test]
    fn branch_slash_command_creates_and_switches_to_new_branch() {
        let repo = tempdir().expect("repo tempdir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo.path())
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.name", "Probe"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.name");
        std::process::Command::new("git")
            .args(["config", "user.email", "probe@example.com"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.email");
        std::fs::write(repo.path().join("example.md"), "hello\n").expect("write initial");
        std::process::Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .current_dir(repo.path())
            .output()
            .expect("git commit");

        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.cwd = repo.path().to_path_buf();
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/branch")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::BranchOverlay);
        app.dispatch(UiEvent::ComposerPaste(String::from("codex/review-flow")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert!(
            app.last_status()
                .contains("created branch: codex/review-flow"),
            "{}",
            app.last_status()
        );
        let rendered = app.render_to_string(140, 42);
        assert!(rendered.contains("codex/review-flow"), "{rendered}");
        assert!(rendered.contains("git: codex/review-flow"), "{rendered}");
        let output = std::process::Command::new("git")
            .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
            .current_dir(repo.path())
            .output()
            .expect("git symbolic-ref");
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(branch, "codex/review-flow");
    }

    #[test]
    fn branch_slash_command_switches_to_existing_branch() {
        let repo = tempdir().expect("repo tempdir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo.path())
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.name", "Probe"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.name");
        std::process::Command::new("git")
            .args(["config", "user.email", "probe@example.com"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.email");
        std::fs::write(repo.path().join("example.md"), "hello\n").expect("write initial");
        std::process::Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .current_dir(repo.path())
            .output()
            .expect("git commit");
        let initial_branch_output = std::process::Command::new("git")
            .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
            .current_dir(repo.path())
            .output()
            .expect("git symbolic-ref initial");
        let initial_branch = String::from_utf8_lossy(&initial_branch_output.stdout)
            .trim()
            .to_string();
        std::process::Command::new("git")
            .args(["switch", "-c", "codex/existing-branch"])
            .current_dir(repo.path())
            .output()
            .expect("git switch new branch");
        std::process::Command::new("git")
            .args(["switch", initial_branch.as_str()])
            .current_dir(repo.path())
            .output()
            .expect("git switch back");

        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.cwd = repo.path().to_path_buf();
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/branch")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::BranchOverlay);
        app.dispatch(UiEvent::ComposerPaste(String::from(
            "codex/existing-branch",
        )));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert!(
            app.last_status()
                .contains("switched branch: codex/existing-branch"),
            "{}",
            app.last_status()
        );
        let output = std::process::Command::new("git")
            .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
            .current_dir(repo.path())
            .output()
            .expect("git symbolic-ref");
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(branch, "codex/existing-branch");
    }

    #[test]
    fn stage_slash_command_stages_current_repo_changes() {
        let repo = tempdir().expect("repo tempdir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo.path())
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.name", "Probe"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.name");
        std::process::Command::new("git")
            .args(["config", "user.email", "probe@example.com"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.email");
        std::fs::write(repo.path().join("example.md"), "hello\n").expect("write initial");
        std::process::Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .current_dir(repo.path())
            .output()
            .expect("git commit");
        std::fs::write(repo.path().join("example.md"), "hello from Probe\n").expect("edit file");

        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.cwd = repo.path().to_path_buf();
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/stage")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::StageOverlay);
        let stage_render = app.render_to_string(140, 42);
        assert!(
            stage_render.contains("0 staged · 1 unstaged · 0 untracked"),
            "{stage_render}"
        );

        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert!(
            app.last_status().contains("staged repo changes"),
            "{}",
            app.last_status()
        );
        let rendered = app.render_to_string(140, 42);
        assert!(
            rendered.contains("applied: staged 1 path(s);"),
            "{rendered}"
        );
        assert!(rendered.contains("/commit is ready"), "{rendered}");
    }

    #[test]
    fn commit_slash_command_creates_a_git_commit() {
        let repo = tempdir().expect("repo tempdir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo.path())
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.name", "Probe"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.name");
        std::process::Command::new("git")
            .args(["config", "user.email", "probe@example.com"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.email");
        std::fs::write(repo.path().join("example.md"), "hello\n").expect("write initial");
        std::process::Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .current_dir(repo.path())
            .output()
            .expect("git commit");
        std::fs::write(repo.path().join("example.md"), "hello from Probe\n").expect("edit file");
        std::process::Command::new("git")
            .arg("add")
            .arg("-A")
            .arg("--")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add updated file");

        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.cwd = repo.path().to_path_buf();
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/commit")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::CommitOverlay);
        let commit_render = app.render_to_string(140, 42);
        assert!(
            commit_render.contains("1 staged · 0 unstaged · 0 untracked"),
            "{commit_render}"
        );

        app.dispatch(UiEvent::ComposerPaste(String::from(
            "Refine example greeting",
        )));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert!(
            app.last_status()
                .contains("created commit: Refine example greeting"),
            "{}",
            app.last_status()
        );
        let output = std::process::Command::new("git")
            .arg("log")
            .arg("-1")
            .arg("--pretty=%s")
            .current_dir(repo.path())
            .output()
            .expect("git log");
        let subject = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(subject, "Refine example greeting");
    }

    #[test]
    fn push_slash_command_opens_overlay_with_remote_guidance() {
        let remote = tempdir().expect("remote tempdir");
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(remote.path())
            .output()
            .expect("git init bare");

        let repo = tempdir().expect("repo tempdir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo.path())
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.name", "Probe"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.name");
        std::process::Command::new("git")
            .args(["config", "user.email", "probe@example.com"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.email");
        std::process::Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                remote.path().to_str().expect("remote path"),
            ])
            .current_dir(repo.path())
            .output()
            .expect("git remote add");
        std::fs::write(repo.path().join("example.md"), "hello\n").expect("write initial");
        std::process::Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .current_dir(repo.path())
            .output()
            .expect("git commit");

        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.cwd = repo.path().to_path_buf();
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/push")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::PushOverlay);
        let rendered = app.render_to_string(140, 42);
        assert!(rendered.contains("remote: origin"), "{rendered}");
        assert!(rendered.contains("upstream: none"), "{rendered}");
        assert!(rendered.contains("sets upstream on origin"), "{rendered}");
    }

    #[test]
    fn push_slash_command_publishes_branch_and_sets_upstream() {
        let remote = tempdir().expect("remote tempdir");
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(remote.path())
            .output()
            .expect("git init bare");

        let repo = tempdir().expect("repo tempdir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo.path())
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.name", "Probe"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.name");
        std::process::Command::new("git")
            .args(["config", "user.email", "probe@example.com"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.email");
        std::process::Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                remote.path().to_str().expect("remote path"),
            ])
            .current_dir(repo.path())
            .output()
            .expect("git remote add");
        std::fs::write(repo.path().join("example.md"), "hello\n").expect("write initial");
        std::process::Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .current_dir(repo.path())
            .output()
            .expect("git commit");
        let branch_output = std::process::Command::new("git")
            .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
            .current_dir(repo.path())
            .output()
            .expect("git symbolic-ref");
        let branch_name = String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string();

        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.cwd = repo.path().to_path_buf();
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/push")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::PushOverlay);
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert!(
            app.last_status()
                .contains(format!("published branch: {branch_name}").as_str()),
            "{}",
            app.last_status()
        );
        let upstream_output = std::process::Command::new("git")
            .args([
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ])
            .current_dir(repo.path())
            .output()
            .expect("git rev-parse upstream");
        let upstream = String::from_utf8_lossy(&upstream_output.stdout)
            .trim()
            .to_string();
        assert_eq!(upstream, format!("origin/{branch_name}"));
        let remote_output = std::process::Command::new("git")
            .args(["for-each-ref", "--format=%(refname:short)", "refs/heads"])
            .current_dir(remote.path())
            .output()
            .expect("git for-each-ref remote");
        let remote_refs = String::from_utf8_lossy(&remote_output.stdout).to_string();
        assert!(
            remote_refs.lines().any(|line| line == branch_name),
            "{remote_refs}"
        );
    }

    #[test]
    fn pr_slash_command_blocks_when_current_branch_matches_base_branch() {
        let repo = tempdir().expect("repo tempdir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo.path())
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.name", "Probe"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.name");
        std::process::Command::new("git")
            .args(["config", "user.email", "probe@example.com"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.email");
        std::fs::write(repo.path().join("example.md"), "hello\n").expect("write initial");
        std::process::Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .current_dir(repo.path())
            .output()
            .expect("git commit");

        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.cwd = repo.path().to_path_buf();
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/pr")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::PrOverlay);
        let rendered = app.render_to_string(140, 42);
        assert!(
            rendered.contains("Create a work branch first"),
            "{rendered}"
        );
    }

    #[test]
    fn pr_slash_command_creates_draft_pr_with_gh() {
        let remote = tempdir().expect("remote tempdir");
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(remote.path())
            .output()
            .expect("git init bare");

        let repo = tempdir().expect("repo tempdir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo.path())
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.name", "Probe"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.name");
        std::process::Command::new("git")
            .args(["config", "user.email", "probe@example.com"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.email");
        std::process::Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                remote.path().to_str().expect("remote path"),
            ])
            .current_dir(repo.path())
            .output()
            .expect("git remote add");
        std::fs::write(repo.path().join("example.md"), "hello\n").expect("write initial");
        std::process::Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .current_dir(repo.path())
            .output()
            .expect("git commit");
        std::process::Command::new("git")
            .args(["switch", "-c", "codex/review-flow"])
            .current_dir(repo.path())
            .output()
            .expect("git switch new branch");
        let branch_output = std::process::Command::new("git")
            .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
            .current_dir(repo.path())
            .output()
            .expect("git symbolic-ref");
        let branch_name = String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string();
        std::process::Command::new("git")
            .args(["push", "-u", "origin", branch_name.as_str()])
            .current_dir(repo.path())
            .output()
            .expect("git push");

        let gh_log = repo.path().join("gh-log.txt");
        let gh_stub = repo.path().join("gh-stub.sh");
        fs::write(
            &gh_stub,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\nprintf 'https://example.com/pr/123\\n'\n",
                gh_log.display()
            ),
        )
        .expect("write gh stub");
        let mut perms = fs::metadata(&gh_stub)
            .expect("gh stub metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&gh_stub, perms).expect("chmod gh stub");
        std::process::Command::new("git")
            .args([
                "config",
                "probe.ghBin",
                gh_stub.to_str().expect("gh stub path"),
            ])
            .current_dir(repo.path())
            .output()
            .expect("git config probe.ghBin");

        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.cwd = repo.path().to_path_buf();
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/pr")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::PrOverlay);
        app.dispatch(UiEvent::ComposerPaste(String::from("Refine draft PR flow")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert!(
            app.last_status()
                .contains("draft PR created: https://example.com/pr/123"),
            "{}",
            app.last_status()
        );
        let gh_args = fs::read_to_string(&gh_log).expect("read gh log");
        assert!(gh_args.contains("pr"), "{gh_args}");
        assert!(gh_args.contains("create"), "{gh_args}");
        assert!(gh_args.contains("--draft"), "{gh_args}");
        assert!(gh_args.contains("Refine draft PR flow"), "{gh_args}");
    }

    #[test]
    fn pr_comments_slash_command_blocks_until_branch_is_pushed() {
        let repo = tempdir().expect("repo tempdir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo.path())
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.name", "Probe"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.name");
        std::process::Command::new("git")
            .args(["config", "user.email", "probe@example.com"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.email");
        std::fs::write(repo.path().join("example.md"), "hello\n").expect("write initial");
        std::process::Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .current_dir(repo.path())
            .output()
            .expect("git commit");
        std::process::Command::new("git")
            .args(["switch", "-c", "codex/review-flow"])
            .current_dir(repo.path())
            .output()
            .expect("git switch branch");

        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.cwd = repo.path().to_path_buf();
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/pr_comments")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::PrCommentsOverlay);
        let rendered = app.render_to_string(140, 42);
        assert!(rendered.contains("Push this branch first"), "{rendered}");
        assert!(rendered.contains("next: use /push"), "{rendered}");
    }

    #[test]
    fn pr_comments_slash_command_explains_when_no_pr_is_linked() {
        let remote = tempdir().expect("remote tempdir");
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(remote.path())
            .output()
            .expect("git init bare");

        let repo = tempdir().expect("repo tempdir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo.path())
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.name", "Probe"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.name");
        std::process::Command::new("git")
            .args(["config", "user.email", "probe@example.com"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.email");
        std::process::Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                remote.path().to_str().expect("remote path"),
            ])
            .current_dir(repo.path())
            .output()
            .expect("git remote add");
        std::fs::write(repo.path().join("example.md"), "hello\n").expect("write initial");
        std::process::Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .current_dir(repo.path())
            .output()
            .expect("git commit");
        std::process::Command::new("git")
            .args(["switch", "-c", "codex/review-flow"])
            .current_dir(repo.path())
            .output()
            .expect("git switch branch");
        std::process::Command::new("git")
            .args(["push", "-u", "origin", "codex/review-flow"])
            .current_dir(repo.path())
            .output()
            .expect("git push");

        let gh_stub = repo.path().join("gh-stub-no-pr.sh");
        fs::write(
            &gh_stub,
            "#!/bin/sh\necho 'no pull requests found for branch \"codex/review-flow\"' >&2\nexit 1\n",
        )
        .expect("write gh stub");
        let mut perms = fs::metadata(&gh_stub).expect("stub metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&gh_stub, perms).expect("chmod gh stub");
        std::process::Command::new("git")
            .args([
                "config",
                "probe.ghBin",
                gh_stub.to_str().expect("gh stub path"),
            ])
            .current_dir(repo.path())
            .output()
            .expect("git config probe.ghBin");

        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.cwd = repo.path().to_path_buf();
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/pr_comments")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::PrCommentsOverlay);
        let rendered = app.render_to_string(140, 42);
        assert!(
            rendered.contains("No pull request is linked to this branch yet."),
            "{rendered}"
        );
        assert!(rendered.contains("next: use /pr"), "{rendered}");
    }

    #[test]
    fn pr_comments_slash_command_surfaces_current_pr_feedback() {
        let remote = tempdir().expect("remote tempdir");
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(remote.path())
            .output()
            .expect("git init bare");

        let repo = tempdir().expect("repo tempdir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo.path())
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.name", "Probe"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.name");
        std::process::Command::new("git")
            .args(["config", "user.email", "probe@example.com"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.email");
        std::process::Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                remote.path().to_str().expect("remote path"),
            ])
            .current_dir(repo.path())
            .output()
            .expect("git remote add");
        std::fs::write(repo.path().join("example.md"), "hello\n").expect("write initial");
        std::process::Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .current_dir(repo.path())
            .output()
            .expect("git commit");
        std::process::Command::new("git")
            .args(["switch", "-c", "codex/review-flow"])
            .current_dir(repo.path())
            .output()
            .expect("git switch branch");
        std::process::Command::new("git")
            .args(["push", "-u", "origin", "codex/review-flow"])
            .current_dir(repo.path())
            .output()
            .expect("git push");

        let gh_log = repo.path().join("gh-pr-comments-log.txt");
        let gh_stub = repo.path().join("gh-pr-comments.sh");
        fs::write(
            &gh_stub,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\ncat <<'JSON'\n{{\"number\":42,\"title\":\"Improve review flow\",\"url\":\"https://example.com/pr/42\",\"reviewDecision\":\"CHANGES_REQUESTED\",\"isDraft\":true,\"headRefName\":\"codex/review-flow\",\"baseRefName\":\"main\",\"comments\":[{{\"author\":{{\"login\":\"teammate\"}},\"body\":\"Please tighten the copy in the push overlay.\"}}],\"reviews\":[{{\"author\":{{\"login\":\"reviewer\"}},\"state\":\"CHANGES_REQUESTED\",\"body\":\"The Git delivery flow is strong. Please add a PR comment intake surface too.\"}}]}}\nJSON\n",
                gh_log.display()
            ),
        )
        .expect("write gh stub");
        let mut perms = fs::metadata(&gh_stub).expect("stub metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&gh_stub, perms).expect("chmod gh stub");
        std::process::Command::new("git")
            .args([
                "config",
                "probe.ghBin",
                gh_stub.to_str().expect("gh stub path"),
            ])
            .current_dir(repo.path())
            .output()
            .expect("git config probe.ghBin");

        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.cwd = repo.path().to_path_buf();
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/pr_comments")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::PrCommentsOverlay);
        let rendered = app.render_to_string(160, 48);
        assert!(
            rendered.contains("PR #42: Improve review flow"),
            "{rendered}"
        );
        assert!(
            rendered.contains("decision: CHANGES_REQUESTED"),
            "{rendered}"
        );
        assert!(rendered.contains("recent comments:"), "{rendered}");
        assert!(rendered.contains("teammate:"), "{rendered}");
        assert!(rendered.contains("recent reviews:"), "{rendered}");
        assert!(
            rendered.contains("reviewer (changes_requested)"),
            "{rendered}"
        );
        let gh_args = fs::read_to_string(&gh_log).expect("read gh log");
        assert!(gh_args.contains("pr"), "{gh_args}");
        assert!(gh_args.contains("view"), "{gh_args}");
        assert!(gh_args.contains("comments"), "{gh_args}");
        assert!(gh_args.contains("reviews"), "{gh_args}");
    }

    #[test]
    fn pr_comments_slash_command_loads_selected_feedback_into_composer() {
        let remote = tempdir().expect("remote tempdir");
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(remote.path())
            .output()
            .expect("git init bare");

        let repo = tempdir().expect("repo tempdir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo.path())
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.name", "Probe"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.name");
        std::process::Command::new("git")
            .args(["config", "user.email", "probe@example.com"])
            .current_dir(repo.path())
            .output()
            .expect("git config user.email");
        std::process::Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                remote.path().to_str().expect("remote path"),
            ])
            .current_dir(repo.path())
            .output()
            .expect("git remote add");
        std::fs::write(repo.path().join("example.md"), "hello\n").expect("write initial");
        std::process::Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .current_dir(repo.path())
            .output()
            .expect("git commit");
        std::process::Command::new("git")
            .args(["switch", "-c", "codex/review-flow"])
            .current_dir(repo.path())
            .output()
            .expect("git switch branch");
        std::process::Command::new("git")
            .args(["push", "-u", "origin", "codex/review-flow"])
            .current_dir(repo.path())
            .output()
            .expect("git push");

        let gh_stub = repo.path().join("gh-pr-comments-seed.sh");
        fs::write(
            &gh_stub,
            "#!/bin/sh\ncat <<'JSON'\n{\"number\":42,\"title\":\"Improve review flow\",\"url\":\"https://example.com/pr/42\",\"reviewDecision\":\"CHANGES_REQUESTED\",\"isDraft\":true,\"headRefName\":\"codex/review-flow\",\"baseRefName\":\"main\",\"comments\":[{\"author\":{\"login\":\"teammate\"},\"body\":\"Tighten the git overlay copy.\"}],\"reviews\":[{\"author\":{\"login\":\"reviewer\"},\"state\":\"CHANGES_REQUESTED\",\"body\":\"Add a clearer next-step hint after push.\"}]}\nJSON\n",
        )
        .expect("write gh stub");
        let mut perms = fs::metadata(&gh_stub).expect("stub metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&gh_stub, perms).expect("chmod gh stub");
        std::process::Command::new("git")
            .args([
                "config",
                "probe.ghBin",
                gh_stub.to_str().expect("gh stub path"),
            ])
            .current_dir(repo.path())
            .output()
            .expect("git config probe.ghBin");

        let mut config = AppShell::build_chat_runtime_config(None, openai_codex_subscription());
        config.cwd = repo.path().to_path_buf();
        let mut app = AppShell::new_for_tests_with_chat_config(config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/pr_comments")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::PrCommentsOverlay);

        app.dispatch(UiEvent::ComposerHistoryNext);
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        let rendered = app.render_to_string(160, 48);
        assert!(
            rendered.contains("Please address this PR review feedback from reviewer"),
            "{rendered}"
        );
        assert!(
            rendered.contains("Add a clearer next-step hint after push."),
            "{rendered}"
        );
        assert!(
            app.last_status().contains("loaded review · reviewer"),
            "{}",
            app.last_status()
        );
    }

    #[test]
    fn status_overlay_shows_runtime_mcp_session_snapshot() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_mcp_status"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            mcp_state: Some(SessionMcpState {
                load_error: None,
                servers: vec![SessionMcpServer {
                    id: String::from("filesystem"),
                    name: String::from("Filesystem"),
                    enabled: true,
                    source: SessionMcpServerSource::ManualLaunch,
                    transport: Some(SessionMcpServerTransport::Stdio),
                    target: Some(String::from(
                        "npx -y @modelcontextprotocol/server-filesystem .",
                    )),
                    provider_setup_command: None,
                    provider_hint: None,
                    client_hint: None,
                    connection_status: Some(SessionMcpConnectionStatus::Connected),
                    connection_note: Some(String::from(
                        "Attached at session start and discovered 1 tool(s).",
                    )),
                    discovered_tools: vec![probe_protocol::session::SessionMcpTool {
                        name: String::from("filesystem/read"),
                        description: Some(String::from("Read files from the workspace.")),
                        input_schema: None,
                    }],
                }],
            }),
            recovery_note: None,
        });

        app.dispatch(UiEvent::ComposerPaste(String::from("/status")));
        app.dispatch(UiEvent::ComposerSubmit);

        let rendered = app.render_to_string(140, 42);
        assert!(
            rendered.contains("mcp session: 1 attached · 1 connected · 0 failed · 1 tools"),
            "{rendered}"
        );
    }

    #[test]
    fn mcp_registry_change_clears_attached_runtime_session_snapshot() {
        let probe_home = tempdir().expect("temp probe home");
        let mut app =
            AppShell::new_for_tests_with_chat_config(AppShell::build_chat_runtime_config(
                Some(probe_home.path().to_path_buf()),
                openai_codex_subscription(),
            ));
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_mcp_runtime"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            mcp_state: Some(SessionMcpState {
                load_error: None,
                servers: vec![SessionMcpServer {
                    id: String::from("filesystem"),
                    name: String::from("Filesystem"),
                    enabled: true,
                    source: SessionMcpServerSource::ManualLaunch,
                    transport: Some(SessionMcpServerTransport::Stdio),
                    target: Some(String::from(
                        "npx -y @modelcontextprotocol/server-filesystem .",
                    )),
                    provider_setup_command: None,
                    provider_hint: None,
                    client_hint: None,
                    connection_status: Some(SessionMcpConnectionStatus::Connected),
                    connection_note: Some(String::from(
                        "Attached at session start and discovered 1 tool(s).",
                    )),
                    discovered_tools: vec![probe_protocol::session::SessionMcpTool {
                        name: String::from("filesystem/read"),
                        description: None,
                        input_schema: None,
                    }],
                }],
            }),
            recovery_note: None,
        });

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

        assert_eq!(app.runtime_session_id(), None);
        assert!(app.base_screen().runtime_mcp_state().is_none());
        assert_eq!(app.backend_lanes[0].session_generation, 1);
        assert_eq!(
            app.last_status(),
            "saved MCP recipe shadcn MCP; complete setup to make it runnable"
        );
    }

    #[test]
    fn memory_slash_command_opens_overlay_with_loaded_layers() {
        let probe_home = tempdir().expect("temp probe home");
        let repo_root = tempdir().expect("temp repo root");
        init_git_repo(repo_root.path());
        let cwd = seed_memory_fixture(probe_home.path(), repo_root.path());
        let profile = openai_codex_subscription();
        let launch_config = TuiLaunchConfig {
            chat_runtime: build_chat_runtime_config_for_lane(
                Some(probe_home.path().to_path_buf()),
                cwd,
                profile.clone(),
                LaneOperatorMode::Coding,
                LaneReviewMode::AutoSafe,
                None,
                None,
                0,
            ),
            operator_backend: operator_summary_from_profile(&profile),
            autostart_apple_fm_setup: false,
            resume_session_id: None,
        };
        let mut app = AppShell::new_with_launch_config(launch_config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/memory")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::MemoryOverlay);
        let rendered = app.render_to_string(140, 40);
        assert!(rendered.contains("active memory: user + repo/AGENTS + 1 dir"));
        assert!(rendered.contains("precedence:"), "{rendered}");
        assert!(rendered.contains("Edit user memory"));
        assert!(rendered.contains("Edit repo memory"));
        assert!(rendered.contains("Edit folder memory"));
        assert!(rendered.contains("loaded layers"), "{rendered}");
        assert!(rendered.contains("Edit loaded layer: repo"), "{rendered}");
        assert!(
            rendered.contains("Edit loaded layer: dir:src/features"),
            "{rendered}"
        );
    }

    #[test]
    fn lane_system_prompt_includes_loaded_memory_layers() {
        let probe_home = tempdir().expect("temp probe home");
        let repo_root = tempdir().expect("temp repo root");
        init_git_repo(repo_root.path());
        let cwd = seed_memory_fixture(probe_home.path(), repo_root.path());

        let runtime = build_chat_runtime_config_for_lane(
            Some(probe_home.path().to_path_buf()),
            cwd.clone(),
            openai_codex_subscription(),
            LaneOperatorMode::Coding,
            LaneReviewMode::AutoSafe,
            None,
            None,
            0,
        );

        let prompt = runtime
            .system_prompt
            .expect("memory-aware runtime should build a system prompt");
        assert!(prompt.contains("Persistent memory and rules are active"));
        assert!(prompt.contains("Always prefer concise teammate-style handoffs."));
        assert!(prompt.contains("Repo memory from AGENTS fallback."));
        assert!(prompt.contains("Feature-specific rule for src/features work."));
        assert!(prompt.contains(&cwd.join("PROBE.md").display().to_string()));
    }

    #[test]
    fn memory_editor_can_create_user_memory_and_refresh_lane_state() {
        let probe_home = tempdir().expect("temp probe home");
        let repo_root = tempdir().expect("temp repo root");
        init_git_repo(repo_root.path());
        let profile = openai_codex_subscription();
        let launch_config = TuiLaunchConfig {
            chat_runtime: build_chat_runtime_config_for_lane(
                Some(probe_home.path().to_path_buf()),
                repo_root.path().to_path_buf(),
                profile.clone(),
                LaneOperatorMode::Coding,
                LaneReviewMode::AutoSafe,
                None,
                None,
                0,
            ),
            operator_backend: operator_summary_from_profile(&profile),
            autostart_apple_fm_setup: false,
            resume_session_id: None,
        };
        let mut app = AppShell::new_with_launch_config(launch_config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/memory")));
        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::MemoryOverlay);

        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::MemoryEditorOverlay);

        app.dispatch(UiEvent::ComposerPaste(String::from(
            "Always explain runtime status in plain English.",
        )));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::MemoryOverlay);
        let saved_path = probe_home.path().join("memory/USER.md");
        assert_eq!(
            fs::read_to_string(saved_path).expect("read saved user memory"),
            "Always explain runtime status in plain English."
        );
        assert_eq!(app.base_screen().memory_stack().active_label(), "user");
        let rendered = app.render_to_string(140, 40);
        assert!(rendered.contains("active memory: user"));
    }

    #[test]
    fn memory_editor_can_append_to_existing_repo_memory() {
        let probe_home = tempdir().expect("temp probe home");
        let repo_root = tempdir().expect("temp repo root");
        init_git_repo(repo_root.path());
        seed_memory_fixture(probe_home.path(), repo_root.path());
        let profile = openai_codex_subscription();
        let launch_config = TuiLaunchConfig {
            chat_runtime: build_chat_runtime_config_for_lane(
                Some(probe_home.path().to_path_buf()),
                repo_root.path().join("src/features"),
                profile.clone(),
                LaneOperatorMode::Coding,
                LaneReviewMode::AutoSafe,
                None,
                None,
                0,
            ),
            operator_backend: operator_summary_from_profile(&profile),
            autostart_apple_fm_setup: false,
            resume_session_id: None,
        };
        let mut app = AppShell::new_with_launch_config(launch_config);

        app.dispatch(UiEvent::ComposerPaste(String::from("/memory")));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerHistoryNext);
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::MemoryEditorOverlay);
        let rendered = app.render_to_string(160, 42);
        assert!(rendered.contains("Repo memory from AGENTS fallback."));

        app.dispatch(UiEvent::ComposerNewline);
        app.dispatch(UiEvent::ComposerPaste(String::from(
            "Prefer terse summaries in this repo.",
        )));
        app.dispatch(UiEvent::ComposerSubmit);

        let repo_memory_path = repo_root.path().join("AGENTS.md");
        let saved = fs::read_to_string(repo_memory_path).expect("read repo memory");
        assert!(saved.contains("Repo memory from AGENTS fallback."));
        assert!(saved.contains("Prefer terse summaries in this repo."));
        let prompt = app
            .active_chat_runtime()
            .system_prompt
            .as_ref()
            .expect("memory-aware prompt");
        assert!(prompt.contains("Prefer terse summaries in this repo."));
    }

    #[test]
    fn unreadable_memory_file_surfaces_recovery_copy_and_editor_note() {
        let probe_home = tempdir().expect("temp probe home");
        let repo_root = tempdir().expect("temp repo root");
        init_git_repo(repo_root.path());
        fs::create_dir_all(probe_home.path().join("memory")).expect("create memory dir");
        fs::write(
            probe_home.path().join("memory/USER.md"),
            vec![0xff, 0xfe, 0xfd],
        )
        .expect("write unreadable memory");
        let profile = openai_codex_subscription();
        let launch_config = TuiLaunchConfig {
            chat_runtime: build_chat_runtime_config_for_lane(
                Some(probe_home.path().to_path_buf()),
                repo_root.path().to_path_buf(),
                profile.clone(),
                LaneOperatorMode::Coding,
                LaneReviewMode::AutoSafe,
                None,
                None,
                0,
            ),
            operator_backend: operator_summary_from_profile(&profile),
            autostart_apple_fm_setup: false,
            resume_session_id: None,
        };
        let mut app = AppShell::new_with_launch_config(launch_config);

        let rendered = app.render_to_string(140, 36);
        assert!(rendered.contains("memory issue:"), "{rendered}");

        app.dispatch(UiEvent::ComposerPaste(String::from("/memory")));
        app.dispatch(UiEvent::ComposerSubmit);
        let rendered = app.render_to_string(160, 44);
        assert!(rendered.contains("issue:"), "{rendered}");
        assert!(rendered.contains("recovery:"), "{rendered}");
        assert!(rendered.contains("Edit user memory"), "{rendered}");

        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::MemoryEditorOverlay);
        let rendered = app.render_to_string(160, 44);
        assert!(
            rendered.contains("could not read the current file contents"),
            "{rendered}"
        );
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
            mcp_state: None,
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
                proposed_edit: None,
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
        assert!(rendered.contains("Review Changes"), "{rendered}");
        assert!(rendered.contains("command preview"), "{rendered}");
        assert!(rendered.contains("touch hello.txt"), "{rendered}");
        assert!(rendered.contains("> Apply"), "{rendered}");
    }

    #[test]
    fn apply_patch_approval_overlay_reads_like_review_before_apply() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_patch_review"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });
        app.apply_message(AppMessage::PendingToolApprovalsUpdated {
            session_id: String::from("sess_patch_review"),
            approvals: vec![PendingToolApproval {
                session_id: SessionId::new("sess_patch_review"),
                tool_call_id: String::from("call_patch"),
                tool_name: String::from("apply_patch"),
                arguments: json!({
                    "path": "src/lib.rs",
                    "old_text": "fn old_name() {}\n",
                    "new_text": "fn new_name() {}\n"
                }),
                risk_class: ToolRiskClass::Write,
                reason: Some(String::from("review-risky pauses write-capable tools")),
                tool_call_turn_index: 4,
                paused_result_turn_index: 4,
                requested_at_ms: 10,
                proposed_edit: None,
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

        let rendered = app.render_to_string(120, 40);
        assert!(rendered.contains("Review Changes"), "{rendered}");
        assert!(
            rendered.contains("summary: Probe wants to update `src/lib.rs`"),
            "{rendered}"
        );
        assert!(rendered.contains("files"), "{rendered}");
        assert!(rendered.contains("src/lib.rs"), "{rendered}");
        assert!(rendered.contains("proposed patch"), "{rendered}");
        assert!(rendered.contains("- fn old_name() {}"), "{rendered}");
        assert!(rendered.contains("+ fn new_name() {}"), "{rendered}");
        assert!(
            rendered.contains(
                "A applies. R rejects. Tab changes selection. Enter decides. Esc closes."
            ),
            "{rendered}"
        );
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
    fn tasks_slash_command_lists_saved_sessions_when_no_live_tasks_are_available() {
        let probe_home = tempdir().expect("temp probe home");
        let workspace = tempdir().expect("workspace");
        let store = FilesystemSessionStore::new(probe_home.path());
        store
            .create_session_with(NewSession::new("Bug hunt", workspace.path()).with_backend(
                SessionBackendTarget {
                    profile_name: String::from("openai-codex-subscription"),
                    base_url: String::from("https://chatgpt.com/backend-api/codex"),
                    model: String::from("gpt-5.4"),
                    control_plane: None,
                    psionic_mesh: None,
                },
            ))
            .expect("create saved session");
        let mut app =
            AppShell::new_for_tests_with_chat_config(AppShell::build_chat_runtime_config(
                Some(probe_home.path().to_path_buf()),
                openai_codex_subscription(),
            ));

        app.dispatch(UiEvent::ComposerPaste(String::from("/tasks")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::ResumeOverlay);
        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("tasks discovered: 1"), "{rendered}");
        assert!(rendered.contains("Bug hunt"), "{rendered}");
        assert!(rendered.contains("openai-codex-subscription"), "{rendered}");
        assert!(rendered.contains("saved session"), "{rendered}");
    }

    #[test]
    fn resume_slash_command_remains_an_alias_for_tasks() {
        let probe_home = tempdir().expect("temp probe home");
        let workspace = tempdir().expect("workspace");
        let store = FilesystemSessionStore::new(probe_home.path());
        store
            .create_session_with(NewSession::new("Bug hunt", workspace.path()).with_backend(
                SessionBackendTarget {
                    profile_name: String::from("openai-codex-subscription"),
                    base_url: String::from("https://chatgpt.com/backend-api/codex"),
                    model: String::from("gpt-5.4"),
                    control_plane: None,
                    psionic_mesh: None,
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
        assert!(rendered.contains("Tasks"), "{rendered}");
        assert!(rendered.contains("Bug hunt"), "{rendered}");
    }

    #[test]
    fn detached_summary_view_surfaces_live_task_status_and_recovery() {
        let view = detached_summary_view(
            probe_protocol::runtime::DetachedSessionSummary {
                session_id: probe_protocol::session::SessionId::new("sess_background"),
                title: String::from("Investigate bug"),
                cwd: PathBuf::from("/tmp/probe-work"),
                status: DetachedSessionStatus::ApprovalPaused,
                runtime_owner: None,
                workspace_state: None,
                hosted_receipts: None,
                mounted_refs: Vec::new(),
                summary_artifact_refs: Vec::new(),
                participants: Vec::new(),
                controller_lease: None,
                active_turn_id: Some(String::from("turn_1")),
                queued_turn_count: 1,
                pending_approval_count: 2,
                last_terminal_turn_id: None,
                last_terminal_status: None,
                registered_at_ms: 1,
                updated_at_ms: 2,
                recovery_state: DetachedSessionRecoveryState::ApprovalPausedResumable,
                recovery_note: Some(String::from(
                    "daemon restart can resume this session after the pending approval is resolved",
                )),
            },
            &HashMap::from([(
                String::from("sess_background"),
                String::from("openai-codex-subscription"),
            )]),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(view.status, "needs approval");
        assert_eq!(view.backend, "openai-codex-subscription");
        assert!(
            view.detail_lines
                .iter()
                .any(|line| line.contains("pending approvals: 2"))
        );
        assert!(
            view.detail_lines
                .iter()
                .any(|line| line.contains("approval-paused resumable"))
        );
        assert!(view.next_hint.contains("resolve approvals"));
    }

    #[test]
    fn detached_summary_view_marks_child_tasks_with_parent_context() {
        let view = detached_summary_view(
            probe_protocol::runtime::DetachedSessionSummary {
                session_id: probe_protocol::session::SessionId::new("sess_child"),
                title: String::from("Fix failing review item"),
                cwd: PathBuf::from("/tmp/probe-work"),
                status: DetachedSessionStatus::Queued,
                runtime_owner: None,
                workspace_state: None,
                hosted_receipts: None,
                mounted_refs: Vec::new(),
                summary_artifact_refs: Vec::new(),
                participants: Vec::new(),
                controller_lease: None,
                active_turn_id: None,
                queued_turn_count: 1,
                pending_approval_count: 0,
                last_terminal_turn_id: None,
                last_terminal_status: None,
                registered_at_ms: 1,
                updated_at_ms: 2,
                recovery_state: DetachedSessionRecoveryState::Clean,
                recovery_note: None,
            },
            &HashMap::from([(
                String::from("sess_child"),
                String::from("openai-codex-subscription"),
            )]),
            &HashMap::from([(
                String::from("sess_parent"),
                String::from("Main coding lane"),
            )]),
            &HashMap::from([(String::from("sess_child"), String::from("sess_parent"))]),
            &HashMap::from([(String::from("sess_parent"), 2usize)]),
        );

        assert!(view.status.contains("child task"), "{:?}", view.status);
        assert!(
            view.detail_lines
                .iter()
                .any(|line| line.contains("delegated from: Main coding lane")),
            "{:?}",
            view.detail_lines
        );
        assert!(
            view.next_hint.contains("queued child task"),
            "{}",
            view.next_hint
        );
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
            mcp_state: None,
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
    fn background_slash_command_opens_picker_and_updates_shell_copy() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.dispatch(UiEvent::ComposerPaste(String::from("/background")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::BackgroundModeOverlay);
        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("Choose how the next turn should run on this lane."));
        assert!(rendered.contains("> foreground"));

        app.dispatch(UiEvent::ComposerHistoryNext);
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert_eq!(app.backend_lanes[0].launch_mode, LaneLaunchMode::Background);
        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("launch: background"), "{rendered}");
        assert!(
            rendered.contains("applied: background turns on"),
            "{rendered}"
        );
        assert!(rendered.contains("Enter queues the next"), "{rendered}");
        assert!(rendered.contains("/tasks"), "{rendered}");
        assert_eq!(app.last_status(), "launch mode: background");
    }

    #[test]
    fn background_task_queue_message_adds_a_handoff_receipt() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.apply_message(AppMessage::BackgroundTaskQueued {
            session_id: String::from("sess_background_task"),
            title: String::from("Investigate approvals"),
            cwd: String::from("/tmp/probe-workspace"),
            status: String::from("queued"),
            parent_title: None,
        });

        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("[status] Background Task"), "{rendered}");
        assert!(
            rendered.contains("Queued `Investigate approvals` to run in the background."),
            "{rendered}"
        );
        assert!(
            rendered.contains("next: use /tasks to reopen it"),
            "{rendered}"
        );
        assert!(rendered.contains("applied: background task"), "{rendered}");
        assert!(rendered.contains("queued"), "{rendered}");
        assert!(
            app.last_status()
                .starts_with("queued background task sess_background_"),
            "{}",
            app.last_status()
        );
    }

    #[test]
    fn delegate_slash_command_preselects_delegate_mode_and_updates_shell_copy() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.dispatch(UiEvent::ComposerPaste(String::from("/delegate")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::BackgroundModeOverlay);
        let rendered = app.render_to_string(120, 38);
        assert!(rendered.contains("> delegate"), "{rendered}");

        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert_eq!(app.backend_lanes[0].launch_mode, LaneLaunchMode::Delegate);
        let rendered = app.render_to_string(120, 38);
        assert!(rendered.contains("launch: delegate"), "{rendered}");
        assert!(
            rendered.contains("applied: delegate turns on"),
            "{rendered}"
        );
        assert!(rendered.contains("child"), "{rendered}");
        assert_eq!(app.last_status(), "launch mode: delegate");
    }

    #[test]
    fn delegated_task_queue_message_adds_parent_context() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.apply_message(AppMessage::BackgroundTaskQueued {
            session_id: String::from("sess_child_task"),
            title: String::from("Fix review comments"),
            cwd: String::from("/tmp/probe-workspace"),
            status: String::from("queued"),
            parent_title: Some(String::from("Main coding lane")),
        });

        let rendered = app.render_to_string(120, 38);
        assert!(rendered.contains("[status] Delegated Task"), "{rendered}");
        assert!(
            rendered.contains("delegated from: Main coding lane"),
            "{rendered}"
        );
        assert!(rendered.contains("/tasks"), "{rendered}");
        assert!(
            rendered.contains("applied: delegated task queued"),
            "{rendered}"
        );
    }

    #[test]
    fn review_mode_slash_command_opens_picker_and_updates_shell_copy() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );

        app.dispatch(UiEvent::ComposerPaste(String::from("/review_mode")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::ReviewModeOverlay);
        let rendered = app.render_to_string(120, 36);
        assert!(
            rendered.contains("Choose how Probe should treat write-capable work on this lane.")
        );
        assert!(rendered.contains("> auto-safe"));

        app.dispatch(UiEvent::ComposerHistoryNext);
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::Chat);
        assert_eq!(
            app.backend_lanes[0].review_mode,
            LaneReviewMode::ReviewRisky
        );
        assert!(
            app.backend_lanes[0]
                .chat_runtime
                .system_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains("Review mode is review-risky."))
        );
        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("review: review-risky"));
        assert!(rendered.contains("applied: review: review-risky"));
        let approval = &app.backend_lanes[0]
            .chat_runtime
            .tool_loop
            .as_ref()
            .expect("tool loop should be configured")
            .approval;
        assert!(!approval.allow_write_tools);
        assert!(!approval.allow_network_shell);
        assert!(!approval.allow_destructive_shell);
        assert_eq!(app.last_status(), "review mode: review-risky");
    }

    #[test]
    fn review_mode_slash_command_is_blocked_while_runtime_work_is_in_flight() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_review_busy"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: Some(RuntimeActivity::new(
                RuntimeActivityKind::Editing,
                "editing files",
            )),
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });

        app.dispatch(UiEvent::ComposerPaste(String::from("/review_mode")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.backend_lanes[0].review_mode, LaneReviewMode::AutoSafe);
        assert_eq!(
            app.last_status(),
            "wait for the current turn to finish before you change review mode"
        );
    }

    #[test]
    fn trace_and_conversation_slash_commands_switch_transcript_views() {
        let mut app = AppShell::new_for_tests();
        app.apply_message(AppMessage::TranscriptEntriesCommitted {
            entries: vec![
                TranscriptEntry::new(
                    TranscriptRole::User,
                    "You",
                    vec![String::from("inspect the README")],
                ),
                TranscriptEntry::tool_call("read_file", vec![String::from("README.md")]),
                TranscriptEntry::tool_result("read_file", vec![String::from("README.md:1-5")]),
                TranscriptEntry::new(
                    TranscriptRole::Assistant,
                    "Probe",
                    vec![String::from("I checked the README note for you.")],
                ),
            ],
        });

        let rendered = app.render_to_string(120, 36);
        assert!(!rendered.contains("│ view: conversation"), "{rendered}");
        assert!(!rendered.contains("[tool call] read_file"), "{rendered}");

        app.dispatch(UiEvent::ComposerPaste(String::from("/trace")));
        app.dispatch(UiEvent::ComposerSubmit);

        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("│ view: trace"), "{rendered}");
        assert!(rendered.contains("applied: trace view on"), "{rendered}");
        assert!(rendered.contains("[tool call] read_file"), "{rendered}");
        assert_eq!(app.last_status(), "view: trace");

        app.dispatch(UiEvent::ComposerPaste(String::from("/conversation")));
        app.dispatch(UiEvent::ComposerSubmit);

        let rendered = app.render_to_string(120, 36);
        assert!(!rendered.contains("│ view: conversation"), "{rendered}");
        assert!(
            rendered.contains("applied: conversation view on"),
            "{rendered}"
        );
        assert!(!rendered.contains("[tool call] read_file"), "{rendered}");
        assert_eq!(app.last_status(), "view: conversation");
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
    fn dismiss_closes_the_slash_palette_without_clearing_the_draft() {
        let mut app = AppShell::new_for_tests();

        app.dispatch(UiEvent::ComposerInsert('/'));
        app.dispatch(UiEvent::ComposerInsert('m'));

        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("/model"), "{rendered}");

        app.dispatch(UiEvent::Dismiss);

        let rendered = app.render_to_string(120, 36);
        assert!(
            rendered.contains("status: closed command list"),
            "{rendered}"
        );
        assert!(rendered.contains("/m"), "{rendered}");
        assert!(!rendered.contains("/model"), "{rendered}");

        app.dispatch(UiEvent::ComposerInsert('o'));

        let rendered = app.render_to_string(120, 36);
        assert!(rendered.contains("/model"), "{rendered}");
    }

    #[test]
    fn diff_slash_command_opens_overlay_for_latest_task_changes() {
        let repo = tempdir().expect("repo tempdir");
        std::fs::create_dir_all(repo.path().join("src")).expect("create src dir");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(repo.path())
            .output()
            .expect("git init");
        std::fs::write(
            repo.path().join("src/lib.rs"),
            "pub fn greeting() -> &'static str {\n    \"hello\"\n}\n",
        )
        .expect("write initial file");
        std::process::Command::new("git")
            .arg("-c")
            .arg("user.name=Probe")
            .arg("-c")
            .arg("user.email=probe@example.com")
            .arg("add")
            .arg(".")
            .current_dir(repo.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .arg("-c")
            .arg("user.name=Probe")
            .arg("-c")
            .arg("user.email=probe@example.com")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .current_dir(repo.path())
            .output()
            .expect("git commit");
        std::fs::write(
            repo.path().join("src/lib.rs"),
            "pub fn greeting() -> &'static str {\n    \"hello from probe\"\n}\n",
        )
        .expect("write updated file");

        let mut app = AppShell::new_for_tests();
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_qwen_diff"),
            profile_name: String::from("psionic-qwen35-2b-q8-registry"),
            model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            cwd: repo.path().display().to_string(),
            runtime_activity: None,
            latest_task_workspace_summary: Some(TaskWorkspaceSummary {
                task_start_turn_index: 4,
                status: TaskWorkspaceSummaryStatus::Changed,
                changed_files: vec![String::from("src/lib.rs")],
                touched_but_unchanged_files: Vec::new(),
                preexisting_dirty_files: Vec::new(),
                outside_tracking_dirty_files: Vec::new(),
                repo_root: Some(repo.path().to_path_buf()),
                change_accounting_limited: false,
                checkpoint: checkpoint(
                    TaskCheckpointStatus::Captured,
                    "Probe captured a pre-edit checkpoint before changes landed in src/lib.rs.",
                ),
                revertibility: revertibility(
                    TaskRevertibilityStatus::Exact,
                    "Probe has enough checkpoint coverage to attempt an exact restore for src/lib.rs.",
                ),
                diff_previews: Vec::new(),
                summary_text: String::from("This task changed 1 file(s): src/lib.rs."),
            }),
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });

        app.dispatch(UiEvent::ComposerPaste(String::from("/diff")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::DiffOverlay);
        let rendered = app.render_to_string(140, 42);
        assert!(
            rendered.contains("Inspect the latest task diff"),
            "{rendered}"
        );
        assert!(rendered.contains("diff preview: src/lib.rs"), "{rendered}");
        assert!(rendered.contains("-    \"hello\""), "{rendered}");
        assert!(rendered.contains("+    \"hello from probe\""), "{rendered}");
    }

    #[test]
    fn diff_slash_command_prefers_recorded_task_preview_when_available() {
        let mut app = AppShell::new_for_tests();
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_recorded_diff"),
            profile_name: String::from("psionic-qwen35-2b-q8-registry"),
            model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: Some(TaskWorkspaceSummary {
                task_start_turn_index: 4,
                status: TaskWorkspaceSummaryStatus::Changed,
                changed_files: vec![String::from("example.md")],
                touched_but_unchanged_files: Vec::new(),
                preexisting_dirty_files: Vec::new(),
                outside_tracking_dirty_files: Vec::new(),
                repo_root: None,
                change_accounting_limited: false,
                checkpoint: checkpoint(
                    TaskCheckpointStatus::Captured,
                    "Probe captured a pre-edit checkpoint before changes landed in example.md.",
                ),
                revertibility: revertibility(
                    TaskRevertibilityStatus::Exact,
                    "Probe has enough checkpoint coverage to attempt an exact restore for example.md.",
                ),
                diff_previews: vec![TaskDiffPreview {
                    path: String::from("example.md"),
                    diff_lines: vec![
                        String::from("diff --git a/example.md b/example.md"),
                        String::from("--- a/example.md"),
                        String::from("+++ b/example.md"),
                        String::from("@@ -1 +1,2 @@"),
                        String::from(" # Example"),
                        String::from("+updated line"),
                    ],
                    truncated: false,
                }],
                summary_text: String::from("This task changed 1 file(s): example.md."),
            }),
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });

        app.dispatch(UiEvent::ComposerPaste(String::from("/diff")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::DiffOverlay);
        let rendered = app.render_to_string(140, 42);
        assert!(
            rendered.contains("Preview source: recorded with the latest task receipt."),
            "{rendered}"
        );
        assert!(rendered.contains("diff preview: example.md"), "{rendered}");
        assert!(rendered.contains("+updated line"), "{rendered}");
    }

    #[test]
    fn diff_slash_command_prefers_pending_review_diff_before_apply() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_pending_diff"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });
        app.apply_message(AppMessage::PendingToolApprovalsUpdated {
            session_id: String::from("sess_pending_diff"),
            approvals: vec![PendingToolApproval {
                session_id: SessionId::new("sess_pending_diff"),
                tool_call_id: String::from("call_patch_pending"),
                tool_name: String::from("apply_patch"),
                arguments: json!({
                    "path": "src/lib.rs",
                    "old_text": "pub fn old_name() {\n    true\n}\n",
                    "new_text": "pub fn new_name() {\n    true\n}\n"
                }),
                risk_class: ToolRiskClass::Write,
                reason: Some(String::from("review-risky pauses write-capable tools")),
                tool_call_turn_index: 6,
                paused_result_turn_index: 6,
                requested_at_ms: 10,
                proposed_edit: None,
                resolved_at_ms: None,
                resolution: None,
            }],
        });
        app.dispatch(UiEvent::Dismiss);

        app.dispatch(UiEvent::ComposerPaste(String::from("/diff")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::DiffOverlay);
        let rendered = app.render_to_string(140, 42);
        assert!(
            rendered.contains("Inspect the proposed diff waiting for approval"),
            "{rendered}"
        );
        assert!(rendered.contains("status: pending approval"), "{rendered}");
        assert!(rendered.contains("diff preview: src/lib.rs"), "{rendered}");
        assert!(
            rendered.contains("diff --probe a/src/lib.rs b/src/lib.rs"),
            "{rendered}"
        );
        assert!(rendered.contains("- pub fn old_name() {"), "{rendered}");
        assert!(rendered.contains("+ pub fn new_name() {"), "{rendered}");
    }

    #[test]
    fn checkpoint_slash_command_opens_overlay_with_latest_task_coverage() {
        let mut app = AppShell::new_for_tests();
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_checkpoint"),
            profile_name: String::from("psionic-qwen35-2b-q8-registry"),
            model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: Some(TaskWorkspaceSummary {
                task_start_turn_index: 8,
                status: TaskWorkspaceSummaryStatus::Changed,
                changed_files: vec![String::from("src/lib.rs")],
                touched_but_unchanged_files: Vec::new(),
                preexisting_dirty_files: vec![String::from("README.md")],
                outside_tracking_dirty_files: Vec::new(),
                repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
                change_accounting_limited: false,
                checkpoint: checkpoint(
                    TaskCheckpointStatus::Captured,
                    "Probe captured a pre-edit checkpoint before changes landed in src/lib.rs.",
                ),
                revertibility: revertibility(
                    TaskRevertibilityStatus::Exact,
                    "Probe has enough checkpoint coverage to attempt an exact restore for src/lib.rs.",
                ),
                diff_previews: Vec::new(),
                summary_text: String::from("This task changed 1 file(s): src/lib.rs."),
            }),
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });

        app.dispatch(UiEvent::ComposerPaste(String::from("/checkpoint")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::CheckpointOverlay);
        let rendered = app.render_to_string(140, 40);
        assert!(
            rendered.contains("Inspect the latest checkpoint coverage"),
            "{rendered}"
        );
        assert!(rendered.contains("checkpoint: captured"), "{rendered}");
        assert!(rendered.contains("revert: available"), "{rendered}");
        assert!(rendered.contains("changed: src/lib.rs"), "{rendered}");
        assert!(
            rendered.contains("restore path: Probe has enough checkpoint coverage"),
            "{rendered}"
        );
    }

    #[test]
    fn revert_slash_command_explains_pending_review_before_apply() {
        let mut app = AppShell::new_for_tests_with_chat_config(
            AppShell::build_chat_runtime_config(None, openai_codex_subscription()),
        );
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_revert_pending"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: Some(RuntimeActivity::new(
                RuntimeActivityKind::WaitingForApproval,
                "waiting for approval: apply_patch",
            )),
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });
        app.apply_message(AppMessage::PendingToolApprovalsUpdated {
            session_id: String::from("sess_revert_pending"),
            approvals: vec![PendingToolApproval {
                session_id: SessionId::new("sess_revert_pending"),
                tool_call_id: String::from("call_patch_revert"),
                tool_name: String::from("apply_patch"),
                arguments: json!({
                    "path": "src/lib.rs",
                    "old_text": "old\n",
                    "new_text": "new\n"
                }),
                risk_class: ToolRiskClass::Write,
                reason: Some(String::from("review-risky pauses write-capable tools")),
                tool_call_turn_index: 2,
                paused_result_turn_index: 2,
                requested_at_ms: 10,
                proposed_edit: None,
                resolved_at_ms: None,
                resolution: None,
            }],
        });
        app.dispatch(UiEvent::Dismiss);

        app.dispatch(UiEvent::ComposerPaste(String::from("/revert")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::RevertOverlay);
        let rendered = app.render_to_string(140, 40);
        assert!(
            rendered.contains("revert: reject before apply"),
            "{rendered}"
        );
        assert!(
            rendered.contains("There is nothing applied to revert yet"),
            "{rendered}"
        );
        assert!(
            rendered.contains("next: inspect /diff, then A applies or R rejects."),
            "{rendered}"
        );
    }

    #[test]
    fn revert_overlay_surfaces_exact_restore_action_when_available() {
        let mut app = AppShell::new_for_tests();
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_revert_exact"),
            profile_name: String::from("psionic-qwen35-2b-q8-registry"),
            model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: Some(TaskWorkspaceSummary {
                task_start_turn_index: 9,
                status: TaskWorkspaceSummaryStatus::Changed,
                changed_files: vec![String::from("src/lib.rs")],
                touched_but_unchanged_files: Vec::new(),
                preexisting_dirty_files: Vec::new(),
                outside_tracking_dirty_files: Vec::new(),
                repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
                change_accounting_limited: false,
                checkpoint: checkpoint(
                    TaskCheckpointStatus::Captured,
                    "Probe captured a pre-edit checkpoint before changes landed in src/lib.rs.",
                ),
                revertibility: revertibility(
                    TaskRevertibilityStatus::Exact,
                    "Probe has enough checkpoint coverage to attempt an exact restore for src/lib.rs.",
                ),
                diff_previews: Vec::new(),
                summary_text: String::from("This task changed 1 file(s): src/lib.rs."),
            }),
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });

        app.dispatch(UiEvent::ComposerPaste(String::from("/revert")));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::RevertOverlay);
        let rendered = app.render_to_string(140, 40);
        assert!(
            rendered
                .contains("next: press A or Enter to restore the latest exact apply_patch task."),
            "{rendered}"
        );
        assert!(
            rendered.contains("A or Enter reverts. Esc closes."),
            "{rendered}"
        );
    }

    #[test]
    fn revert_overlay_keeps_focus_when_latest_task_is_not_auto_revertable() {
        let mut app = AppShell::new_for_tests();
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_revert_limited"),
            profile_name: String::from("psionic-qwen35-2b-q8-registry"),
            model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: Some(TaskWorkspaceSummary {
                task_start_turn_index: 9,
                status: TaskWorkspaceSummaryStatus::Changed,
                changed_files: vec![String::from("example.md")],
                touched_but_unchanged_files: Vec::new(),
                preexisting_dirty_files: Vec::new(),
                outside_tracking_dirty_files: Vec::new(),
                repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
                change_accounting_limited: false,
                checkpoint: checkpoint(
                    TaskCheckpointStatus::Captured,
                    "Probe captured a pre-edit checkpoint before changes landed in example.md.",
                ),
                revertibility: revertibility(
                    TaskRevertibilityStatus::Limited,
                    "The latest task may have created `example.md`, so Probe will not auto-delete it yet.",
                ),
                diff_previews: Vec::new(),
                summary_text: String::from("This task changed 1 file(s): example.md."),
            }),
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });

        app.dispatch(UiEvent::ComposerPaste(String::from("/revert")));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.active_screen_id(), ScreenId::RevertOverlay);
        assert_eq!(
            app.last_status(),
            "The latest task may have created `example.md`, so Probe will not auto-delete it yet."
        );
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
            rendered.contains("enabled: 1 · attached now: 0"),
            "{rendered}"
        );
        assert!(
            rendered.contains("selected: Saved MCP servers"),
            "{rendered}"
        );
        assert!(rendered.contains("configured: 1"), "{rendered}");
        assert_eq!(
            app.last_status(),
            "saved MCP recipe shadcn MCP; complete setup to make it runnable"
        );

        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::McpServersOverlay);
        let rendered = app.render_to_string(120, 40);
        assert!(rendered.contains("selected: shadcn MCP"), "{rendered}");
        assert!(rendered.contains("client hint: codex"), "{rendered}");
    }

    #[test]
    fn saved_mcp_servers_overlay_marks_entries_attached_to_current_session() {
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
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_mcp_attached"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            mcp_state: Some(SessionMcpState {
                load_error: None,
                servers: vec![SessionMcpServer {
                    id: String::from("local-files"),
                    name: String::from("Local Files"),
                    enabled: true,
                    source: SessionMcpServerSource::ManualLaunch,
                    transport: Some(SessionMcpServerTransport::Stdio),
                    target: Some(String::from(
                        "npx -y @modelcontextprotocol/server-filesystem .",
                    )),
                    provider_setup_command: None,
                    provider_hint: None,
                    client_hint: None,
                    connection_status: Some(SessionMcpConnectionStatus::Connected),
                    connection_note: Some(String::from(
                        "Attached at session start and discovered 1 tool(s).",
                    )),
                    discovered_tools: vec![probe_protocol::session::SessionMcpTool {
                        name: String::from("filesystem/read"),
                        description: None,
                        input_schema: None,
                    }],
                }],
            }),
            recovery_note: None,
        });

        app.dispatch(UiEvent::ComposerPaste(String::from("/mcp")));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerHistoryNext);
        app.dispatch(UiEvent::ComposerSubmit);

        let rendered = app.render_to_string(120, 40);
        assert!(
            rendered.contains("Local Files  connected now · 1 tools"),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "session: connected now in this runtime session with 1 discovered tool(s)"
            ),
            "{rendered}"
        );
        assert!(rendered.contains("tools: filesystem/read"), "{rendered}");
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
        assert_eq!(
            app.last_status(),
            "disabled MCP server Local Files; next turn starts a fresh runtime session"
        );
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
        assert_eq!(
            app.last_status(),
            "enabled MCP server Local Files; next turn starts a fresh runtime session"
        );
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
        assert_eq!(
            app.last_status(),
            "removed MCP server Local Files; next turn starts a fresh runtime session"
        );
    }

    #[test]
    fn provider_recipe_entry_opens_conversion_flow_and_updates_in_place() {
        let probe_home = tempdir().expect("temp probe home");
        let mut app =
            AppShell::new_for_tests_with_chat_config(AppShell::build_chat_runtime_config(
                Some(probe_home.path().to_path_buf()),
                openai_codex_subscription(),
            ));

        app.dispatch(UiEvent::ComposerPaste(String::from("/mcp")));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerPaste(String::from(
            "pnpm dlx shadcn@latest mcp init --client codex",
        )));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerSubmit);

        let rendered = app.render_to_string(120, 42);
        assert!(
            rendered.contains("saved recipe · needs conversion"),
            "{rendered}"
        );
        assert!(
            rendered.contains("next: Enter completes setup"),
            "{rendered}"
        );

        app.dispatch(UiEvent::ComposerSubmit);
        assert_eq!(app.active_screen_id(), ScreenId::McpEditorOverlay);
        let rendered = app.render_to_string(120, 42);
        assert!(rendered.contains("Convert MCP Recipe"), "{rendered}");
        assert!(rendered.contains("provider: shadcn"), "{rendered}");
        assert!(
            rendered.contains("provider command reference:"),
            "{rendered}"
        );
        assert!(
            rendered.contains("recommended runtime command: npx shadcn@latest mcp"),
            "{rendered}"
        );
        assert!(
            rendered.contains("launch command or URL: npx shadcn@latest mcp"),
            "{rendered}"
        );

        app.dispatch(UiEvent::ComposerPaste(String::from(
            "npx -y @modelcontextprotocol/server-filesystem .",
        )));
        app.dispatch(UiEvent::ComposerSubmit);

        assert_eq!(app.mcp_registry.servers.len(), 1);
        let server = &app.mcp_registry.servers[0];
        assert_eq!(server.source, McpServerSource::ManualLaunch);
        assert_eq!(server.name, "shadcn MCP");
        assert_eq!(
            server.target.as_deref(),
            Some("npx -y @modelcontextprotocol/server-filesystem .")
        );
        assert_eq!(
            server.provider_setup_command.as_deref(),
            Some("pnpm dlx shadcn@latest mcp init --client codex")
        );
        assert_eq!(
            app.last_status(),
            "completed runtime setup for shadcn MCP; start a turn to test it"
        );
    }

    #[test]
    fn status_and_doctor_surfaces_saved_recipe_conversion_language() {
        let probe_home = tempdir().expect("temp probe home");
        let mut app =
            AppShell::new_for_tests_with_chat_config(AppShell::build_chat_runtime_config(
                Some(probe_home.path().to_path_buf()),
                openai_codex_subscription(),
            ));

        app.dispatch(UiEvent::ComposerPaste(String::from("/mcp")));
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerSubmit);
        app.dispatch(UiEvent::ComposerPaste(String::from(
            "pnpm dlx shadcn@latest mcp init --client codex",
        )));
        app.dispatch(UiEvent::ComposerSubmit);

        app.dispatch(UiEvent::Dismiss);
        app.dispatch(UiEvent::ComposerPaste(String::from("/status")));
        app.dispatch(UiEvent::ComposerSubmit);
        let rendered = app.render_to_string(140, 44);
        assert!(
            rendered.contains("mcp config: 1 saved recipe(s) still need conversion"),
            "{rendered}"
        );

        app.dispatch(UiEvent::Dismiss);
        app.dispatch(UiEvent::ComposerPaste(String::from("/doctor")));
        app.dispatch(UiEvent::ComposerSubmit);
        let rendered = app.render_to_string(140, 44);
        assert!(
            rendered.contains(
                "mcp config: action - 1 enabled saved recipe(s) still need conversion before Probe can run them"
            ),
            "{rendered}"
        );
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
            mcp_state: None,
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

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut saw_overlay = false;
        while Instant::now() < deadline {
            app.poll_background_messages();
            if app.active_screen_id() == ScreenId::ApprovalOverlay {
                saw_overlay = true;
                break;
            }
            if app.last_status() == "pending approvals cleared" {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        if saw_overlay {
            let rendered = app.render_to_string(120, 32);
            assert!(rendered.contains("Review Changes"));
            assert!(rendered.contains("tool: apply_patch"));
            assert!(rendered.contains("call: call_patch_1"));
            assert!(rendered.contains("risk: write"));
            assert!(rendered.contains("hello.txt"));
            assert!(rendered.contains("proposed patch"));
            app.dispatch(UiEvent::ComposerSubmit);
        }

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut cleared = false;
        while Instant::now() < deadline {
            app.poll_background_messages();
            if app.last_status() == "pending approvals cleared" {
                cleared = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            cleared,
            "timed out waiting for cleared approval state; last_status={}; frame=\n{}",
            app.last_status(),
            app.render_to_string(120, 32)
        );

        let rendered = app.render_to_string(160, 48);
        assert_eq!(app.active_screen_id(), ScreenId::Chat);
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
            mcp_state: None,
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
        assert!(rendered.contains("Backend Unavailable"));
        assert!(rendered.contains("lane: Qwen"));
        assert!(rendered.contains("target: 127.0.0.1:8080"));
        assert!(rendered.contains("Probe could not reach the active backend."));
        assert!(rendered.contains("next: Start the local backend, or switch lanes with Tab"));
        assert!(app.last_status().contains("backend request failed on Qwen"));
    }

    #[test]
    fn usage_limit_failures_get_specific_next_step_copy() {
        let mut app = AppShell::new_for_tests();
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_codex_usage_limit"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });
        app.apply_message(AppMessage::ProbeRuntimeEvent {
            event: RuntimeEvent::ModelRequestFailed {
                session_id: SessionId::new("sess_codex_usage_limit"),
                round_trip: 1,
                backend_kind: BackendKind::OpenAiCodexSubscription,
                error: String::from(
                    r#"backend returned http 429: {"error":{"type":"usage_limit_reached","resets_in_seconds":12525}}"#,
                ),
            },
        });

        let rendered = app.render_to_string(120, 32);
        assert!(rendered.contains("Usage Limit Reached"));
        assert!(rendered.contains("activity: usage limit reached"));
        assert!(rendered.contains("next: Wait about 3h 28m, or switch backend/model"));
    }

    #[test]
    fn compact_runtime_status_uses_steady_stream_copy() {
        let mut app = AppShell::new_for_tests();
        app.apply_message(AppMessage::ProbeRuntimeSessionReady {
            session_id: String::from("sess_stream_status"),
            profile_name: String::from("openai-codex-subscription"),
            model_id: String::from("gpt-5.4"),
            cwd: String::from("/tmp/probe-workspace"),
            runtime_activity: None,
            latest_task_workspace_summary: None,
            latest_task_receipt: None,
            mcp_state: None,
            recovery_note: None,
        });
        app.apply_message(AppMessage::AssistantStreamStarted {
            session_id: String::from("sess_stream_status"),
            round_trip: 1,
            response_id: String::from("resp_stream_status"),
            response_model: String::from("gpt-5.4"),
        });
        app.apply_message(AppMessage::AssistantDeltaAppended {
            session_id: String::from("sess_stream_status"),
            round_trip: 1,
            delta: String::from("hello world"),
        });

        let runtime = app.base_screen().compact_runtime_status();
        assert!(runtime.contains("stream: receiving reply"));
        assert!(!runtime.contains("chars:"));
        assert!(!runtime.contains("ttft_ms:"));
        assert!(!runtime.contains("tool_deltas:"));
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
                .any(|request| request.contains("GET /health HTTP/"))
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
