use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use probe_core::backend_profiles::resolved_reasoning_level_for_backend;
use probe_core::provider::{normalize_openai_assistant_text, normalize_openai_stream_display_text};
use probe_core::runtime::{RuntimeEvent, StreamedToolCallDelta};
use probe_core::server_control::ServerOperatorSummary;
use probe_core::tools::tool_result_model_text;
use probe_core::tools::{ToolDeniedAction, ToolLoopConfig};
use probe_openai_auth::OpenAiCodexAuthStore;
use probe_protocol::runtime::{RuntimeActivity, RuntimeActivityKind};
use probe_protocol::session::{
    PendingToolApproval, SessionBranchState, SessionDeliveryState, SessionDeliveryStatus,
    SessionMcpConnectionStatus, SessionMcpState, SessionWorkspaceBootMode, SessionWorkspaceState,
    TaskCheckpointStatus, TaskFinalReceipt, TaskReceiptDisposition, TaskRevertibilityStatus,
    TaskVerificationCommandStatus, TaskVerificationStatus, TaskWorkspaceSummary,
    TaskWorkspaceSummaryStatus, ToolApprovalResolution, ToolRiskClass,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Wrap};

use crate::bottom_pane::ComposerSubmission;
use crate::event::UiEvent;
use crate::failure::classify_runtime_failure;
use crate::memory::{MemoryScope, ProbeMemoryStack};
use crate::message::{
    AppMessage, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
    AppleFmFailureSummary, AppleFmUsageSummary, ProbeRuntimeTurnConfig, SessionUsageSummary,
    UsageCountsSummary,
};
use crate::transcript::{
    ActiveTurn, RetainedTranscript, TranscriptEntry, TranscriptMode, TranscriptRole,
};
use crate::widgets::{InfoPanel, ModalCard, TabStrip};

const MAX_EVENT_LOG: usize = 16;
const LINE_SCROLL_STEP: u16 = 3;
const PAGE_SCROLL_STEP: u16 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenId {
    Chat,
    Help,
    StatusOverlay,
    DoctorOverlay,
    RecipesOverlay,
    GitOverlay,
    BranchOverlay,
    StageOverlay,
    CommitOverlay,
    PushOverlay,
    PrOverlay,
    PrCommentsOverlay,
    SetupOverlay,
    ApprovalOverlay,
    PlanModeOverlay,
    BackgroundModeOverlay,
    ModelPickerOverlay,
    ReasoningPickerOverlay,
    ReviewModeOverlay,
    MemoryOverlay,
    MemoryEditorOverlay,
    DiffOverlay,
    CheckpointOverlay,
    RevertOverlay,
    WorkspaceOverlay,
    ResumeOverlay,
    UsageOverlay,
    McpOverlay,
    McpServersOverlay,
    McpAddOverlay,
    McpProviderCommandOverlay,
    McpEditorOverlay,
    ConfirmationOverlay,
}

impl ScreenId {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Help => "help modal",
            Self::StatusOverlay => "status overlay",
            Self::DoctorOverlay => "doctor overlay",
            Self::RecipesOverlay => "recipes overlay",
            Self::GitOverlay => "git overlay",
            Self::BranchOverlay => "branch overlay",
            Self::StageOverlay => "stage overlay",
            Self::CommitOverlay => "commit overlay",
            Self::PushOverlay => "push overlay",
            Self::PrOverlay => "pr overlay",
            Self::PrCommentsOverlay => "pr comments overlay",
            Self::SetupOverlay => "backend overlay",
            Self::ApprovalOverlay => "approval overlay",
            Self::PlanModeOverlay => "plan mode picker",
            Self::BackgroundModeOverlay => "launch mode picker",
            Self::ModelPickerOverlay => "model picker",
            Self::ReasoningPickerOverlay => "reasoning picker",
            Self::ReviewModeOverlay => "review mode picker",
            Self::MemoryOverlay => "memory overlay",
            Self::MemoryEditorOverlay => "memory editor overlay",
            Self::DiffOverlay => "diff overlay",
            Self::CheckpointOverlay => "checkpoint overlay",
            Self::RevertOverlay => "revert overlay",
            Self::WorkspaceOverlay => "workspace overlay",
            Self::ResumeOverlay => "tasks overlay",
            Self::UsageOverlay => "usage overlay",
            Self::McpOverlay => "mcp overlay",
            Self::McpServersOverlay => "mcp servers overlay",
            Self::McpAddOverlay => "mcp add menu",
            Self::McpProviderCommandOverlay => "mcp provider command overlay",
            Self::McpEditorOverlay => "mcp manual setup overlay",
            Self::ConfirmationOverlay => "confirmation overlay",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Primary,
    Secondary,
    Tertiary,
}

impl ActiveTab {
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Primary => 0,
            Self::Secondary => 1,
            Self::Tertiary => 2,
        }
    }

    pub(crate) const fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Primary,
            1 => Self::Secondary,
            _ => Self::Tertiary,
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Primary => Self::Secondary,
            Self::Secondary => Self::Tertiary,
            Self::Tertiary => Self::Primary,
        }
    }

    pub(crate) fn previous(self) -> Self {
        match self {
            Self::Primary => Self::Tertiary,
            Self::Secondary => Self::Primary,
            Self::Tertiary => Self::Secondary,
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
    workspace_state: Option<SessionWorkspaceState>,
    branch_state: Option<SessionBranchState>,
    delivery_state: Option<SessionDeliveryState>,
    backend_kind: Option<String>,
    activity: Option<RuntimeActivity>,
    round_trip: Option<usize>,
    active_tool: Option<String>,
    active_tool_targets: Vec<String>,
    pending_approvals: Vec<PendingToolApproval>,
    latest_task_workspace_summary: Option<TaskWorkspaceSummary>,
    latest_task_receipt: Option<TaskFinalReceipt>,
    mcp_state: Option<SessionMcpState>,
    recovery_note: Option<String>,
    usage: SessionUsageSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RevertAvailability {
    PendingReview,
    Exact,
    Limited,
    Unavailable,
}

impl RevertAvailability {
    fn label(self) -> &'static str {
        match self {
            Self::PendingReview => "reject before apply",
            Self::Exact => "available",
            Self::Limited => "limited",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssistantStreamMode {
    Delta,
    Snapshot,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StreamToolCallState {
    tool_index: usize,
    call_id: Option<String>,
    tool_name: Option<String>,
    arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AssistantStreamState {
    round_trip: usize,
    response_id: String,
    response_model: String,
    mode: AssistantStreamMode,
    backend_kind: Option<String>,
    first_chunk_ms: Option<u64>,
    assistant_text: String,
    tool_calls: Vec<StreamToolCallState>,
    finish_reason: Option<String>,
    failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenAction {
    None,
    OpenHelp,
    OpenStatusOverlay,
    OpenDoctorOverlay,
    OpenSetupOverlay,
    OpenApprovalOverlay,
    OpenGitOverlay,
    OpenRecipesOverlay,
    OpenTasksOverlay,
    OpenMcpAddOverlay,
    OpenMcpServersOverlay,
    CloseModal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenCommand {
    RunAppleFmSetup,
    SetActivePlanMode {
        enabled: bool,
    },
    SetActiveLaunchMode {
        mode_label: String,
    },
    SetActiveReviewMode {
        mode_label: String,
    },
    SelectActiveBackendModel {
        model_id: String,
    },
    SelectActiveReasoningLevel {
        level: String,
    },
    SetActiveWorkspace {
        cwd: String,
    },
    ResumeDetachedSession {
        session_id: String,
    },
    ToggleMcpServerEnabled {
        server_id: String,
    },
    RemoveMcpServer {
        server_id: String,
    },
    CreateOrSwitchBranch {
        repo_root: PathBuf,
        name: String,
    },
    StageCurrentRepo {
        repo_root: PathBuf,
    },
    CommitCurrentRepo {
        repo_root: PathBuf,
        message: String,
    },
    PushCurrentBranch {
        repo_root: PathBuf,
        branch_name: String,
        set_upstream: bool,
    },
    CreateDraftPullRequest {
        repo_root: PathBuf,
        title: String,
        base_branch: String,
        head_branch: String,
    },
    SeedComposerDraft {
        text: String,
    },
    OpenMcpProviderCommandOverlay,
    OpenMcpManualEditorOverlay {
        server_id: Option<String>,
    },
    OpenMemoryEditor {
        label: String,
        path: PathBuf,
    },
    ImportMcpProviderCommand {
        command: String,
    },
    SaveMcpServer {
        server_id: Option<String>,
        name: String,
        transport: McpServerTransportDraft,
        target: String,
    },
    SaveMemoryFile {
        label: String,
        path: PathBuf,
        body: String,
    },
    RevertLastTask {
        session_id: String,
    },
    ConfirmClearActiveContext,
    ConfirmCompactActiveContext,
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
    Status(StatusOverlay),
    Doctor(DoctorOverlay),
    Recipes(RecipesOverlay),
    Git(GitOverlay),
    Branch(BranchOverlay),
    Stage(StageOverlay),
    Commit(CommitOverlay),
    Push(PushOverlay),
    Pr(PrOverlay),
    PrComments(PrCommentsOverlay),
    Setup(SetupOverlay),
    Approval(ApprovalOverlay),
    PlanMode(PlanModeOverlay),
    BackgroundMode(BackgroundModeOverlay),
    ModelPicker(ModelPickerOverlay),
    ReasoningPicker(ReasoningPickerOverlay),
    ReviewMode(ReviewModeOverlay),
    Memory(MemoryOverlay),
    MemoryEditor(MemoryEditorOverlay),
    Diff(DiffOverlay),
    Checkpoint(CheckpointOverlay),
    Revert(RevertOverlay),
    Workspace(WorkspaceOverlay),
    Resume(ResumeOverlay),
    Usage(UsageOverlay),
    Mcp(McpOverlay),
    McpServers(McpServersOverlay),
    McpAdd(McpAddOverlay),
    McpProviderCommand(McpProviderCommandOverlay),
    McpEditor(McpEditorOverlay),
    Confirmation(ConfirmationOverlay),
}

impl ScreenState {
    pub const fn id(&self) -> ScreenId {
        match self {
            Self::Chat(_) => ScreenId::Chat,
            Self::Help(_) => ScreenId::Help,
            Self::Status(_) => ScreenId::StatusOverlay,
            Self::Doctor(_) => ScreenId::DoctorOverlay,
            Self::Recipes(_) => ScreenId::RecipesOverlay,
            Self::Git(_) => ScreenId::GitOverlay,
            Self::Branch(_) => ScreenId::BranchOverlay,
            Self::Stage(_) => ScreenId::StageOverlay,
            Self::Commit(_) => ScreenId::CommitOverlay,
            Self::Push(_) => ScreenId::PushOverlay,
            Self::Pr(_) => ScreenId::PrOverlay,
            Self::PrComments(_) => ScreenId::PrCommentsOverlay,
            Self::Setup(_) => ScreenId::SetupOverlay,
            Self::Approval(_) => ScreenId::ApprovalOverlay,
            Self::PlanMode(_) => ScreenId::PlanModeOverlay,
            Self::BackgroundMode(_) => ScreenId::BackgroundModeOverlay,
            Self::ModelPicker(_) => ScreenId::ModelPickerOverlay,
            Self::ReasoningPicker(_) => ScreenId::ReasoningPickerOverlay,
            Self::ReviewMode(_) => ScreenId::ReviewModeOverlay,
            Self::Memory(_) => ScreenId::MemoryOverlay,
            Self::MemoryEditor(_) => ScreenId::MemoryEditorOverlay,
            Self::Diff(_) => ScreenId::DiffOverlay,
            Self::Checkpoint(_) => ScreenId::CheckpointOverlay,
            Self::Revert(_) => ScreenId::RevertOverlay,
            Self::Workspace(_) => ScreenId::WorkspaceOverlay,
            Self::Resume(_) => ScreenId::ResumeOverlay,
            Self::Usage(_) => ScreenId::UsageOverlay,
            Self::Mcp(_) => ScreenId::McpOverlay,
            Self::McpServers(_) => ScreenId::McpServersOverlay,
            Self::McpAdd(_) => ScreenId::McpAddOverlay,
            Self::McpProviderCommand(_) => ScreenId::McpProviderCommandOverlay,
            Self::McpEditor(_) => ScreenId::McpEditorOverlay,
            Self::Confirmation(_) => ScreenId::ConfirmationOverlay,
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
            Self::Status(screen) => screen.handle_event(event),
            Self::Doctor(screen) => screen.handle_event(event),
            Self::Recipes(screen) => screen.handle_event(event),
            Self::Git(screen) => screen.handle_event(event),
            Self::Branch(screen) => screen.handle_event(event),
            Self::Stage(screen) => screen.handle_event(event),
            Self::Commit(screen) => screen.handle_event(event),
            Self::Push(screen) => screen.handle_event(event),
            Self::Pr(screen) => screen.handle_event(event),
            Self::PrComments(screen) => screen.handle_event(event),
            Self::Setup(screen) => screen.handle_event(event),
            Self::Approval(screen) => screen.handle_event(event),
            Self::PlanMode(screen) => screen.handle_event(event),
            Self::BackgroundMode(screen) => screen.handle_event(event),
            Self::ModelPicker(screen) => screen.handle_event(event),
            Self::ReasoningPicker(screen) => screen.handle_event(event),
            Self::ReviewMode(screen) => screen.handle_event(event),
            Self::Memory(screen) => screen.handle_event(event),
            Self::MemoryEditor(screen) => screen.handle_event(event),
            Self::Diff(screen) => screen.handle_event(event),
            Self::Checkpoint(screen) => screen.handle_event(event),
            Self::Revert(screen) => screen.handle_event(event),
            Self::Workspace(screen) => screen.handle_event(event),
            Self::Resume(screen) => screen.handle_event(event),
            Self::Usage(screen) => screen.handle_event(event),
            Self::Mcp(screen) => screen.handle_event(event),
            Self::McpServers(screen) => screen.handle_event(event),
            Self::McpAdd(screen) => screen.handle_event(event),
            Self::McpProviderCommand(screen) => screen.handle_event(event),
            Self::McpEditor(screen) => screen.handle_event(event),
            Self::Confirmation(screen) => screen.handle_event(event),
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
            Self::Status(screen) => screen.render(frame, area, stack_depth, base_screen),
            Self::Doctor(screen) => screen.render(frame, area, stack_depth, base_screen),
            Self::Recipes(screen) => screen.render(frame, area, stack_depth),
            Self::Git(screen) => screen.render(frame, area, stack_depth, base_screen),
            Self::Branch(screen) => screen.render(frame, area, stack_depth),
            Self::Stage(screen) => screen.render(frame, area, stack_depth),
            Self::Commit(screen) => screen.render(frame, area, stack_depth),
            Self::Push(screen) => screen.render(frame, area, stack_depth),
            Self::Pr(screen) => screen.render(frame, area, stack_depth),
            Self::PrComments(screen) => screen.render(frame, area, stack_depth),
            Self::Setup(screen) => screen.render(frame, area, stack_depth, base_screen),
            Self::Approval(screen) => screen.render(frame, area, stack_depth),
            Self::PlanMode(screen) => screen.render(frame, area, stack_depth),
            Self::BackgroundMode(screen) => screen.render(frame, area, stack_depth),
            Self::ModelPicker(screen) => screen.render(frame, area, stack_depth),
            Self::ReasoningPicker(screen) => screen.render(frame, area, stack_depth),
            Self::ReviewMode(screen) => screen.render(frame, area, stack_depth),
            Self::Memory(screen) => screen.render(frame, area, stack_depth),
            Self::MemoryEditor(screen) => screen.render(frame, area, stack_depth),
            Self::Diff(screen) => screen.render(frame, area, stack_depth),
            Self::Checkpoint(screen) => screen.render(frame, area, stack_depth, base_screen),
            Self::Revert(screen) => screen.render(frame, area, stack_depth, base_screen),
            Self::Workspace(screen) => screen.render(frame, area, stack_depth),
            Self::Resume(screen) => screen.render(frame, area, stack_depth),
            Self::Usage(screen) => screen.render(frame, area, stack_depth, base_screen),
            Self::Mcp(screen) => screen.render(frame, area, stack_depth, base_screen),
            Self::McpServers(screen) => screen.render(frame, area, stack_depth),
            Self::McpAdd(screen) => screen.render(frame, area, stack_depth),
            Self::McpProviderCommand(screen) => screen.render(frame, area, stack_depth),
            Self::McpEditor(screen) => screen.render(frame, area, stack_depth),
            Self::Confirmation(screen) => screen.render(frame, area, stack_depth),
        }
    }

    pub fn chat_mut(&mut self) -> Option<&mut ChatScreen> {
        match self {
            Self::Chat(screen) => Some(screen),
            Self::Help(_)
            | Self::Status(_)
            | Self::Doctor(_)
            | Self::Recipes(_)
            | Self::Git(_)
            | Self::Branch(_)
            | Self::Stage(_)
            | Self::Commit(_)
            | Self::Push(_)
            | Self::Pr(_)
            | Self::PrComments(_)
            | Self::Setup(_)
            | Self::Approval(_)
            | Self::PlanMode(_)
            | Self::BackgroundMode(_)
            | Self::ModelPicker(_)
            | Self::ReasoningPicker(_)
            | Self::ReviewMode(_)
            | Self::Memory(_)
            | Self::MemoryEditor(_)
            | Self::Diff(_)
            | Self::Checkpoint(_)
            | Self::Revert(_)
            | Self::Workspace(_)
            | Self::Resume(_)
            | Self::Usage(_)
            | Self::Mcp(_)
            | Self::McpServers(_)
            | Self::McpAdd(_)
            | Self::McpProviderCommand(_)
            | Self::McpEditor(_)
            | Self::Confirmation(_) => None,
        }
    }

    pub fn chat(&self) -> Option<&ChatScreen> {
        match self {
            Self::Chat(screen) => Some(screen),
            Self::Help(_)
            | Self::Status(_)
            | Self::Doctor(_)
            | Self::Recipes(_)
            | Self::Git(_)
            | Self::Branch(_)
            | Self::Stage(_)
            | Self::Commit(_)
            | Self::Push(_)
            | Self::Pr(_)
            | Self::PrComments(_)
            | Self::Setup(_)
            | Self::Approval(_)
            | Self::PlanMode(_)
            | Self::BackgroundMode(_)
            | Self::ModelPicker(_)
            | Self::ReasoningPicker(_)
            | Self::ReviewMode(_)
            | Self::Memory(_)
            | Self::MemoryEditor(_)
            | Self::Diff(_)
            | Self::Checkpoint(_)
            | Self::Revert(_)
            | Self::Workspace(_)
            | Self::Resume(_)
            | Self::Usage(_)
            | Self::Mcp(_)
            | Self::McpServers(_)
            | Self::McpAdd(_)
            | Self::McpProviderCommand(_)
            | Self::McpEditor(_)
            | Self::Confirmation(_) => None,
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
    tab_labels: Vec<String>,
    emphasized_copy: bool,
    lane_label: Option<String>,
    approval_posture_lines: Vec<String>,
    recent_events: VecDeque<String>,
    task_events: VecDeque<String>,
    transcript: RetainedTranscript,
    transcript_scroll_from_bottom: u16,
    transcript_follow_latest: bool,
    transcript_line_count: usize,
    runtime: ProbeRuntimeState,
    stream: Option<AssistantStreamState>,
    operator_backend: Option<ServerOperatorSummary>,
    probe_home: Option<PathBuf>,
    operator_mode_label: String,
    launch_mode_label: String,
    review_mode_label: String,
    transcript_mode: TranscriptMode,
    memory_stack: ProbeMemoryStack,
    carry_forward_summary: Option<String>,
    local_action_notice: Option<String>,
    setup: AppleFmSetupState,
}

impl Default for ChatScreen {
    fn default() -> Self {
        let mut screen = Self {
            active_tab: ActiveTab::Primary,
            tab_labels: vec![
                String::from("Codex"),
                String::from("Qwen"),
                String::from("Apple FM"),
            ],
            emphasized_copy: false,
            lane_label: None,
            approval_posture_lines: Vec::new(),
            recent_events: VecDeque::new(),
            task_events: VecDeque::new(),
            transcript: RetainedTranscript::new(),
            transcript_scroll_from_bottom: 0,
            transcript_follow_latest: true,
            transcript_line_count: 0,
            runtime: ProbeRuntimeState::default(),
            stream: None,
            operator_backend: None,
            probe_home: None,
            operator_mode_label: String::from("coding"),
            launch_mode_label: String::from("foreground"),
            review_mode_label: String::from("auto-safe"),
            transcript_mode: TranscriptMode::Conversation,
            memory_stack: ProbeMemoryStack::default(),
            carry_forward_summary: None,
            local_action_notice: None,
            setup: AppleFmSetupState::default(),
        };
        screen.record_event("probe tui ready");
        screen.record_event("press Ctrl+R to rerun backend check when supported");
        screen.record_event("press Ctrl+S to inspect backend status");
        screen.record_event("press F1 for help");
        screen.record_event("press Tab to switch backends");
        screen.record_event("press Shift+Tab for Codex reasoning or previous backend");
        screen
    }
}

impl ChatScreen {
    pub fn active_tab(&self) -> ActiveTab {
        self.active_tab
    }

    pub fn set_backend_selector(&mut self, labels: Vec<String>, active_tab: ActiveTab) {
        self.tab_labels = labels;
        self.active_tab = active_tab;
    }

    pub fn set_probe_home(&mut self, probe_home: Option<PathBuf>) {
        self.probe_home = probe_home;
    }

    pub fn set_runtime_context(&mut self, lane_label: String, config: &ProbeRuntimeTurnConfig) {
        self.lane_label = Some(lane_label);
        self.runtime.profile_name = Some(config.profile.name.clone());
        self.runtime.model_id = Some(config.profile.model.clone());
        self.runtime.cwd = Some(config.cwd.display().to_string());
        self.approval_posture_lines = render_approval_posture_lines(config.tool_loop.as_ref());
    }

    pub fn set_operator_controls(
        &mut self,
        mode_label: impl Into<String>,
        launch_mode_label: impl Into<String>,
        review_mode_label: impl Into<String>,
        carry_forward_summary: Option<&str>,
    ) {
        self.operator_mode_label = mode_label.into();
        self.launch_mode_label = launch_mode_label.into();
        self.review_mode_label = review_mode_label.into();
        self.carry_forward_summary = carry_forward_summary
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
    }

    pub fn set_transcript_mode(&mut self, mode: TranscriptMode) {
        self.transcript_mode = mode;
    }

    pub fn set_memory_stack(&mut self, stack: ProbeMemoryStack) {
        self.memory_stack = stack;
    }

    pub fn set_git_workspace_state(
        &mut self,
        workspace_state: Option<SessionWorkspaceState>,
        branch_state: Option<SessionBranchState>,
        delivery_state: Option<SessionDeliveryState>,
    ) {
        self.runtime.workspace_state = workspace_state;
        self.runtime.branch_state = branch_state;
        self.runtime.delivery_state = delivery_state;
    }

    pub fn set_local_action_notice(&mut self, notice: Option<String>) {
        self.local_action_notice = notice
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }

    pub fn apply_runtime_config_change(
        &mut self,
        lane_label: String,
        config: &ProbeRuntimeTurnConfig,
        summary: ServerOperatorSummary,
        mode_label: &str,
        launch_mode_label: &str,
        review_mode_label: &str,
        transcript_mode: TranscriptMode,
        carry_forward_summary: Option<&str>,
        note: impl Into<String>,
    ) {
        self.lane_label = Some(lane_label);
        self.runtime.profile_name = Some(config.profile.name.clone());
        self.runtime.model_id = Some(config.profile.model.clone());
        self.runtime.cwd = Some(config.cwd.display().to_string());
        self.runtime.backend_kind = Some(render_backend_kind(summary.backend_kind).to_string());
        self.runtime.session_id = None;
        self.runtime.activity = None;
        self.runtime.round_trip = None;
        self.runtime.active_tool = None;
        self.runtime.active_tool_targets.clear();
        self.runtime.pending_approvals.clear();
        self.runtime.mcp_state = None;
        self.runtime.recovery_note = None;
        self.approval_posture_lines = render_approval_posture_lines(config.tool_loop.as_ref());
        self.operator_backend = Some(summary);
        self.set_operator_controls(
            mode_label.to_string(),
            launch_mode_label.to_string(),
            review_mode_label.to_string(),
            carry_forward_summary,
        );
        self.set_transcript_mode(transcript_mode);
        self.memory_stack = ProbeMemoryStack::default();
        self.clear_stream();
        self.record_event(note);
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

    pub fn latest_task_workspace_summary(&self) -> Option<&TaskWorkspaceSummary> {
        self.runtime
            .latest_task_receipt
            .as_ref()
            .map(|receipt| &receipt.workspace)
            .or(self.runtime.latest_task_workspace_summary.as_ref())
    }

    pub fn latest_task_receipt(&self) -> Option<&TaskFinalReceipt> {
        self.runtime.latest_task_receipt.as_ref()
    }

    pub fn memory_stack(&self) -> &ProbeMemoryStack {
        &self.memory_stack
    }

    pub fn runtime_mcp_state(&self) -> Option<&SessionMcpState> {
        self.runtime.mcp_state.as_ref()
    }

    pub fn can_execute_revert(&self) -> bool {
        matches!(self.revert_availability(), RevertAvailability::Exact)
            && self.runtime.session_id.is_some()
    }

    pub fn carries_compacted_context(&self) -> bool {
        self.carry_forward_summary.is_some()
    }

    pub fn compact_summary_text(&self) -> String {
        let mut sections = Vec::new();
        sections.push(format!("Mode: {}.", self.operator_mode_label));
        sections.push(format!("Launch: {}.", self.launch_mode_label));
        if let Some(receipt) = self.runtime.latest_task_receipt.as_ref() {
            sections.push(format!("Latest task receipt: {}", receipt.summary_text));
        } else if let Some(summary) = self.runtime.latest_task_workspace_summary.as_ref() {
            sections.push(format!("Latest task workspace: {}", summary.summary_text));
        }

        let transcript_entries = self.transcript.entries();
        let recent_messages = transcript_entries
            .iter()
            .rev()
            .filter(|entry| {
                matches!(
                    entry.role(),
                    TranscriptRole::User | TranscriptRole::Assistant | TranscriptRole::Status
                )
            })
            .take(4)
            .collect::<Vec<_>>();
        if !recent_messages.is_empty() {
            let mut lines = Vec::new();
            for entry in recent_messages.into_iter().rev() {
                let body = entry
                    .body()
                    .iter()
                    .take(2)
                    .map(|line| preview(line, 120))
                    .collect::<Vec<_>>()
                    .join(" / ");
                lines.push(format!(
                    "{}: {}",
                    entry.label(),
                    preview(body.as_str(), 140)
                ));
            }
            sections.push(format!("Recent conversation: {}.", lines.join(" | ")));
        }
        if sections.len() == 1 {
            sections.push(String::from(
                "No significant prior conversation was available to compact.",
            ));
        }
        sections.join(" ")
    }

    pub fn current_workspace_label(&self) -> String {
        self.workspace_label()
    }

    pub fn committed_transcript_entry_count(&self) -> usize {
        self.transcript.entries().len()
    }

    pub fn has_in_flight_runtime_activity(&self) -> bool {
        matches!(
            self.runtime_activity_kind(),
            Some(
                RuntimeActivityKind::Queued
                    | RuntimeActivityKind::Starting
                    | RuntimeActivityKind::WaitingForBackend
                    | RuntimeActivityKind::StreamingReply
                    | RuntimeActivityKind::UpdatingReply
                    | RuntimeActivityKind::PlanningTool
                    | RuntimeActivityKind::Reading
                    | RuntimeActivityKind::Editing
                    | RuntimeActivityKind::Validating
                    | RuntimeActivityKind::RunningTool
                    | RuntimeActivityKind::Finalizing
            )
        )
    }

    pub fn has_pending_tool_approvals(&self) -> bool {
        !self.runtime.pending_approvals.is_empty()
    }

    pub fn current_pending_tool_approval(&self) -> Option<&PendingToolApproval> {
        self.runtime.pending_approvals.first()
    }

    pub fn set_operator_backend(&mut self, summary: ServerOperatorSummary) {
        self.runtime.backend_kind = Some(render_backend_kind(summary.backend_kind).to_string());
        self.runtime.model_id = summary.model_id.clone();
        self.operator_backend = Some(summary.clone());
        self.record_event(format!(
            "backend target: {} {} {} model={}",
            render_backend_kind(summary.backend_kind),
            summary.attach_mode_label(),
            summary.endpoint_label(),
            summary.model_id.as_deref().unwrap_or("unknown")
        ));
        self.record_event(format!(
            "backend transport: {} ({})",
            summary.target_kind_label(),
            summary.base_url
        ));
        if summary.is_remote_target() {
            self.record_event(
                "remote inference only: tools, approvals, transcripts, and UI stay local",
            );
        } else if summary.target_kind_label() == "loopback_or_ssh_forward" {
            self.record_event(
                "loopback attach may be local or an SSH-forwarded remote Psionic target",
            );
        }
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
        self.transcript
            .push_entry(TranscriptEntry::new(TranscriptRole::User, "You", body));
        self.snap_transcript_to_latest();
        self.record_event(format!(
            "submitted chat turn ({} chars)",
            submission.text.chars().count()
        ));
        self.local_action_notice = None;
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

    fn uses_apple_fm_backend(&self) -> bool {
        self.operator_backend.as_ref().is_some_and(|summary| {
            summary.backend_kind == probe_protocol::backend::BackendKind::AppleFmBridge
        })
    }

    pub fn compact_runtime_status(&self) -> String {
        let backend = self
            .runtime
            .backend_kind
            .as_deref()
            .or_else(|| {
                self.operator_backend
                    .as_ref()
                    .map(|summary| render_backend_kind(summary.backend_kind))
            })
            .unwrap_or("pending");
        let activity = self.operator_activity_label();
        let target = self
            .operator_backend
            .as_ref()
            .map(ServerOperatorSummary::endpoint_label)
            .unwrap_or_else(|| String::from("pending"));
        let model = self
            .runtime
            .model_id
            .as_deref()
            .or_else(|| {
                self.operator_backend
                    .as_ref()
                    .and_then(|summary| summary.model_id.as_deref())
            })
            .unwrap_or("pending");

        if let Some(stream) = &self.stream {
            let mut parts = vec![
                format!("backend: {backend}"),
                format!("target: {target}"),
                format!("model: {}", preview(model, 28)),
                format!("activity: {activity}"),
                format!("round: {}", stream.round_trip),
            ];
            let stream_state = if stream.failure.is_some() {
                "failed"
            } else if !stream.tool_calls.is_empty() {
                "planning tool call"
            } else if stream.assistant_text.trim().is_empty() {
                "waiting for first reply"
            } else {
                "receiving reply"
            };
            parts.push(format!("stream: {stream_state}"));
            if let Some(finish_reason) = stream.finish_reason.as_deref() {
                parts.push(format!("finish: {finish_reason}"));
            }
            if stream.failure.is_some() {
                parts.push(String::from("state: failed"));
            }
            return parts.join(" | ");
        }

        let mut parts = vec![
            format!("backend: {backend}"),
            format!("target: {target}"),
            format!("model: {}", preview(model, 28)),
            format!("activity: {activity}"),
        ];
        if let Some(round_trip) = self.runtime.round_trip {
            parts.push(format!("round: {round_trip}"));
        }
        if let Some(tool) = self.runtime.active_tool.as_deref() {
            parts.push(format!("tool: {tool}"));
        }
        if !self.runtime.active_tool_targets.is_empty() {
            parts.push(format!(
                "updating: {}",
                summarize_inline_paths(self.runtime.active_tool_targets.as_slice(), 2)
            ));
        }
        parts.join(" | ")
    }

    fn clear_stream(&mut self) {
        self.stream = None;
    }

    fn start_stream(
        &mut self,
        session_id: &str,
        round_trip: usize,
        response_id: String,
        response_model: String,
        mode: AssistantStreamMode,
    ) {
        self.runtime.session_id = Some(session_id.to_string());
        self.runtime.activity = Some(match mode {
            AssistantStreamMode::Delta => {
                RuntimeActivity::new(RuntimeActivityKind::StreamingReply, "streaming reply")
            }
            AssistantStreamMode::Snapshot => {
                RuntimeActivity::new(RuntimeActivityKind::UpdatingReply, "updating reply")
            }
        });
        self.runtime.round_trip = Some(round_trip);
        self.runtime.active_tool = None;
        self.runtime.active_tool_targets.clear();
        self.stream = Some(AssistantStreamState {
            round_trip,
            response_id,
            response_model,
            mode,
            backend_kind: self.runtime.backend_kind.clone(),
            first_chunk_ms: None,
            assistant_text: String::new(),
            tool_calls: Vec::new(),
            finish_reason: None,
            failure: None,
        });
        self.sync_stream_active_turn();
    }

    fn note_first_stream_chunk(&mut self, session_id: &str, round_trip: usize, milliseconds: u64) {
        self.runtime.session_id = Some(session_id.to_string());
        self.runtime.activity = Some(RuntimeActivity::new(
            RuntimeActivityKind::StreamingReply,
            "streaming reply",
        ));
        self.runtime.round_trip = Some(round_trip);
        if let Some(stream) = self.stream.as_mut() {
            stream.first_chunk_ms = Some(milliseconds);
            stream.failure = None;
        }
        self.sync_stream_active_turn();
    }

    fn append_stream_delta(&mut self, session_id: &str, round_trip: usize, delta: &str) {
        self.runtime.session_id = Some(session_id.to_string());
        self.runtime.activity = Some(RuntimeActivity::new(
            RuntimeActivityKind::StreamingReply,
            "streaming reply",
        ));
        self.runtime.round_trip = Some(round_trip);
        let stream = self.stream.get_or_insert_with(|| AssistantStreamState {
            round_trip,
            response_id: String::from("pending"),
            response_model: String::from("pending"),
            mode: AssistantStreamMode::Delta,
            backend_kind: self.runtime.backend_kind.clone(),
            first_chunk_ms: None,
            assistant_text: String::new(),
            tool_calls: Vec::new(),
            finish_reason: None,
            failure: None,
        });
        stream.mode = AssistantStreamMode::Delta;
        stream.assistant_text.push_str(delta);
        stream.failure = None;
        self.sync_stream_active_turn();
    }

    fn update_stream_snapshot(&mut self, session_id: &str, round_trip: usize, snapshot: &str) {
        self.runtime.session_id = Some(session_id.to_string());
        self.runtime.activity = Some(RuntimeActivity::new(
            RuntimeActivityKind::UpdatingReply,
            "updating reply",
        ));
        self.runtime.round_trip = Some(round_trip);
        let stream = self.stream.get_or_insert_with(|| AssistantStreamState {
            round_trip,
            response_id: String::from("pending"),
            response_model: String::from("pending"),
            mode: AssistantStreamMode::Snapshot,
            backend_kind: self.runtime.backend_kind.clone(),
            first_chunk_ms: None,
            assistant_text: String::new(),
            tool_calls: Vec::new(),
            finish_reason: None,
            failure: None,
        });
        stream.mode = AssistantStreamMode::Snapshot;
        stream.assistant_text = snapshot.to_string();
        stream.failure = None;
        self.sync_stream_active_turn();
    }

    fn update_stream_tool_calls(
        &mut self,
        session_id: &str,
        round_trip: usize,
        deltas: &[StreamedToolCallDelta],
    ) {
        self.runtime.session_id = Some(session_id.to_string());
        self.runtime.activity = Some(RuntimeActivity::new(
            RuntimeActivityKind::PlanningTool,
            "planning tool call",
        ));
        self.runtime.round_trip = Some(round_trip);
        let stream = self.stream.get_or_insert_with(|| AssistantStreamState {
            round_trip,
            response_id: String::from("pending"),
            response_model: String::from("pending"),
            mode: AssistantStreamMode::Delta,
            backend_kind: self.runtime.backend_kind.clone(),
            first_chunk_ms: None,
            assistant_text: String::new(),
            tool_calls: Vec::new(),
            finish_reason: None,
            failure: None,
        });
        for delta in deltas {
            if let Some(existing) = stream
                .tool_calls
                .iter_mut()
                .find(|tool| tool.tool_index == delta.tool_index)
            {
                if let Some(call_id) = delta.call_id.as_ref() {
                    existing.call_id = Some(call_id.clone());
                }
                if let Some(tool_name) = delta.tool_name.as_ref() {
                    existing.tool_name = Some(tool_name.clone());
                }
                if let Some(arguments_delta) = delta.arguments_delta.as_ref() {
                    existing.arguments.push_str(arguments_delta);
                }
            } else {
                stream.tool_calls.push(StreamToolCallState {
                    tool_index: delta.tool_index,
                    call_id: delta.call_id.clone(),
                    tool_name: delta.tool_name.clone(),
                    arguments: delta.arguments_delta.clone().unwrap_or_default(),
                });
                stream.tool_calls.sort_by_key(|tool| tool.tool_index);
            }
        }
        stream.failure = None;
        self.sync_stream_active_turn();
    }

    fn finish_stream(
        &mut self,
        session_id: &str,
        round_trip: usize,
        response_id: String,
        response_model: String,
        finish_reason: Option<String>,
    ) {
        self.runtime.session_id = Some(session_id.to_string());
        self.runtime.activity = Some(RuntimeActivity::new(
            RuntimeActivityKind::Finalizing,
            "finalizing reply",
        ));
        self.runtime.round_trip = Some(round_trip);
        self.runtime.active_tool = None;
        self.runtime.active_tool_targets.clear();
        if let Some(stream) = self.stream.as_mut() {
            stream.round_trip = round_trip;
            stream.response_id = response_id;
            stream.response_model = response_model;
            stream.finish_reason = finish_reason;
            stream.failure = None;
        }
        self.sync_stream_active_turn();
    }

    fn fail_stream(
        &mut self,
        session_id: &str,
        round_trip: usize,
        backend_kind: &str,
        error: &str,
    ) {
        self.runtime.session_id = Some(session_id.to_string());
        self.runtime.activity = Some(RuntimeActivity::new(RuntimeActivityKind::Failed, "failed"));
        self.runtime.round_trip = Some(round_trip);
        self.runtime.active_tool = None;
        self.runtime.active_tool_targets.clear();
        self.runtime.backend_kind = Some(backend_kind.to_string());
        let stream = self.stream.get_or_insert_with(|| AssistantStreamState {
            round_trip,
            response_id: String::from("failed"),
            response_model: String::from("failed"),
            mode: AssistantStreamMode::Delta,
            backend_kind: Some(backend_kind.to_string()),
            first_chunk_ms: None,
            assistant_text: String::new(),
            tool_calls: Vec::new(),
            finish_reason: None,
            failure: None,
        });
        stream.backend_kind = Some(backend_kind.to_string());
        stream.failure = Some(error.to_string());
        self.sync_stream_active_turn();
    }

    fn sync_stream_active_turn(&mut self) {
        let Some(stream) = self.stream.as_ref() else {
            return;
        };
        let action_hint = self.current_action_hint();
        self.transcript.set_active_turn(render_stream_active_turn(
            stream,
            self.active_lane_label(),
            self.operator_backend.as_ref(),
            self.runtime.activity.as_ref(),
            action_hint.as_str(),
        ));
        self.snap_transcript_to_latest();
    }

    fn apply_runtime_event(&mut self, event: RuntimeEvent) -> String {
        match event {
            RuntimeEvent::ActivityUpdated {
                session_id,
                activity,
            } => {
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.activity = Some(activity.clone());
                self.record_worker_event(format!("activity updated: {}", activity.label));
                format!("activity updated: {}", activity.label)
            }
            RuntimeEvent::TurnStarted {
                session_id,
                profile_name,
                prompt,
                tool_loop_enabled,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.profile_name = Some(profile_name.clone());
                self.runtime.backend_kind = None;
                self.runtime.activity = Some(RuntimeActivity::new(
                    RuntimeActivityKind::Starting,
                    "starting turn",
                ));
                self.runtime.round_trip = None;
                self.runtime.active_tool = None;
                self.runtime.active_tool_targets.clear();
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Assistant,
                    "Starting Turn",
                    vec![
                        format!("profile: {profile_name}"),
                        format!(
                            "tool_loop: {}",
                            if tool_loop_enabled {
                                "enabled"
                            } else {
                                "disabled"
                            }
                        ),
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
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.backend_kind = Some(render_backend_kind(backend_kind).to_string());
                self.runtime.activity = Some(RuntimeActivity::new(
                    RuntimeActivityKind::WaitingForBackend,
                    "waiting for backend",
                ));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = None;
                self.runtime.active_tool_targets.clear();
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Assistant,
                    "Waiting for Backend",
                    Vec::new(),
                ));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("model request started: round {round_trip}"));
                format!("model request started for round {round_trip}")
            }
            RuntimeEvent::AssistantStreamStarted {
                session_id,
                round_trip,
                response_id,
                response_model,
            } => {
                self.start_stream(
                    session_id.as_str(),
                    round_trip,
                    response_id,
                    response_model,
                    AssistantStreamMode::Delta,
                );
                self.record_worker_event(format!("assistant stream started: round {round_trip}"));
                format!("assistant stream started for round {round_trip}")
            }
            RuntimeEvent::TimeToFirstTokenObserved {
                session_id,
                round_trip,
                milliseconds,
            } => {
                self.note_first_stream_chunk(session_id.as_str(), round_trip, milliseconds);
                self.record_worker_event(format!(
                    "first streamed chunk observed after {milliseconds}ms"
                ));
                format!("time to first token observed for round {round_trip}")
            }
            RuntimeEvent::AssistantDelta {
                session_id,
                round_trip,
                delta,
            } => {
                self.append_stream_delta(session_id.as_str(), round_trip, delta.as_str());
                self.record_worker_event(format!(
                    "assistant delta appended (+{} chars)",
                    delta.chars().count()
                ));
                format!("assistant delta appended for round {round_trip}")
            }
            RuntimeEvent::AssistantSnapshot {
                session_id,
                round_trip,
                snapshot,
            } => {
                self.update_stream_snapshot(session_id.as_str(), round_trip, snapshot.as_str());
                self.record_worker_event(format!(
                    "assistant snapshot updated ({} chars)",
                    snapshot.chars().count()
                ));
                format!("assistant snapshot updated for round {round_trip}")
            }
            RuntimeEvent::ToolCallDelta {
                session_id,
                round_trip,
                deltas,
            } => {
                self.update_stream_tool_calls(session_id.as_str(), round_trip, &deltas);
                self.record_worker_event(format!("streamed {} tool call delta(s)", deltas.len()));
                String::from("tool call delta received")
            }
            RuntimeEvent::ToolCallRequested {
                session_id,
                round_trip,
                call_id: _call_id,
                tool_name,
                arguments,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.activity = Some(RuntimeActivity::new(
                    RuntimeActivityKind::PlanningTool,
                    "planning tool call",
                ));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool_name.clone());
                self.runtime.active_tool_targets =
                    runtime_tool_target_paths(tool_name.as_str(), &arguments, None);
                self.transcript.clear_active_turn();
                self.transcript
                    .push_live_entry(runtime_tool_call_entry(tool_name.as_str(), &arguments));
                self.snap_transcript_to_latest();
                let display_name = display_runtime_tool_name(tool_name.as_str());
                self.record_worker_event(format!("tool call requested: {display_name}"));
                format!("tool call requested: {display_name}")
            }
            RuntimeEvent::ToolExecutionStarted {
                session_id,
                round_trip,
                call_id: _call_id,
                tool_name,
                risk_class,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                if self.runtime.activity.is_none() {
                    self.runtime.activity = Some(RuntimeActivity::new(
                        RuntimeActivityKind::RunningTool,
                        format!("running {tool_name}"),
                    ));
                }
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool_name.clone());
                let mut body = vec![format!("risk: {}", render_runtime_risk_class(risk_class))];
                if !self.runtime.active_tool_targets.is_empty() {
                    body.push(format!(
                        "updating: {}",
                        summarize_inline_paths(self.runtime.active_tool_targets.as_slice(), 3)
                    ));
                }
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Status,
                    runtime_activity_title(self.runtime.activity.as_ref(), tool_name.as_str()),
                    body,
                ));
                self.snap_transcript_to_latest();
                let display_name = display_runtime_tool_name(tool_name.as_str());
                self.record_worker_event(format!("tool execution started: {display_name}"));
                format!("tool execution started: {display_name}")
            }
            RuntimeEvent::ToolExecutionCompleted {
                session_id,
                round_trip,
                tool,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool.name.clone());
                self.runtime.active_tool_targets =
                    runtime_tool_target_paths(tool.name.as_str(), &tool.arguments, Some(&tool));
                self.transcript.clear_active_turn();
                self.transcript
                    .push_live_entry(runtime_tool_result_entry(round_trip, &tool));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("tool execution completed: {}", tool.name));
                format!("tool execution completed: {}", tool.name)
            }
            RuntimeEvent::ToolRefused {
                session_id,
                round_trip,
                tool,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.activity = Some(RuntimeActivity::new(
                    RuntimeActivityKind::PlanningTool,
                    format!("{} refused", tool.name),
                ));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool.name.clone());
                self.runtime.active_tool_targets =
                    runtime_tool_target_paths(tool.name.as_str(), &tool.arguments, Some(&tool));
                self.transcript.clear_active_turn();
                self.transcript
                    .push_live_entry(runtime_tool_result_entry(round_trip, &tool));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("tool refused: {}", tool.name));
                format!("tool refused: {}", tool.name)
            }
            RuntimeEvent::ToolPaused {
                session_id,
                round_trip,
                tool,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.activity = Some(RuntimeActivity::new(
                    RuntimeActivityKind::WaitingForApproval,
                    format!("waiting for approval: {}", tool.name),
                ));
                self.runtime.round_trip = Some(round_trip);
                self.runtime.active_tool = Some(tool.name.clone());
                self.runtime.active_tool_targets =
                    runtime_tool_target_paths(tool.name.as_str(), &tool.arguments, Some(&tool));
                self.transcript.clear_active_turn();
                self.transcript
                    .push_live_entry(runtime_tool_result_entry(round_trip, &tool));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("tool paused for approval: {}", tool.name));
                format!("tool paused for approval: {}", tool.name)
            }
            RuntimeEvent::AssistantStreamFinished {
                session_id,
                round_trip,
                response_id,
                response_model,
                finish_reason,
            } => {
                self.finish_stream(
                    session_id.as_str(),
                    round_trip,
                    response_id,
                    response_model,
                    finish_reason,
                );
                self.record_worker_event(String::from("assistant stream finished"));
                String::from("assistant stream finished")
            }
            RuntimeEvent::ModelRequestFailed {
                session_id,
                round_trip,
                backend_kind,
                error,
            } => {
                let backend_kind = render_backend_kind(backend_kind).to_string();
                self.fail_stream(
                    session_id.as_str(),
                    round_trip,
                    backend_kind.as_str(),
                    error.as_str(),
                );
                let target = self.operator_target_label();
                self.record_worker_event(format!(
                    "model request failed on {} @ {}",
                    self.active_lane_label(),
                    target
                ));
                format!(
                    "backend request failed on {} @ {}",
                    self.active_lane_label(),
                    target
                )
            }
            RuntimeEvent::AssistantTurnCommitted {
                session_id,
                response_id: _response_id,
                response_model: _response_model,
                assistant_text,
            } => {
                self.clear_stream();
                self.runtime.session_id = Some(session_id.as_str().to_string());
                self.runtime.activity = Some(RuntimeActivity::new(
                    RuntimeActivityKind::Completed,
                    "completed",
                ));
                self.runtime.active_tool = None;
                self.runtime.active_tool_targets.clear();
                self.transcript.set_active_turn(ActiveTurn::new(
                    TranscriptRole::Assistant,
                    "Probe",
                    assistant_closeout_lines(assistant_text.as_str()),
                ));
                self.snap_transcript_to_latest();
                self.record_worker_event(String::from("assistant turn committed"));
                String::from("assistant turn committed")
            }
        }
    }

    pub fn apply_message(&mut self, message: AppMessage) -> String {
        match message {
            AppMessage::AssistantStreamStarted {
                session_id,
                round_trip,
                response_id,
                response_model,
            } => {
                self.start_stream(
                    session_id.as_str(),
                    round_trip,
                    response_id,
                    response_model,
                    AssistantStreamMode::Delta,
                );
                self.record_worker_event(format!("assistant stream started: round {round_trip}"));
                format!("assistant stream started for round {round_trip}")
            }
            AppMessage::AssistantFirstChunkObserved {
                session_id,
                round_trip,
                milliseconds,
            } => {
                self.note_first_stream_chunk(session_id.as_str(), round_trip, milliseconds);
                self.record_worker_event(format!(
                    "first streamed chunk observed after {milliseconds}ms"
                ));
                format!("time to first token observed for round {round_trip}")
            }
            AppMessage::AssistantDeltaAppended {
                session_id,
                round_trip,
                delta,
            } => {
                self.append_stream_delta(session_id.as_str(), round_trip, delta.as_str());
                self.record_worker_event(format!(
                    "assistant delta appended (+{} chars)",
                    delta.chars().count()
                ));
                format!("assistant delta appended for round {round_trip}")
            }
            AppMessage::AssistantSnapshotUpdated {
                session_id,
                round_trip,
                snapshot,
            } => {
                self.update_stream_snapshot(session_id.as_str(), round_trip, snapshot.as_str());
                self.record_worker_event(format!(
                    "assistant snapshot updated ({} chars)",
                    snapshot.chars().count()
                ));
                format!("assistant snapshot updated for round {round_trip}")
            }
            AppMessage::AssistantToolCallDeltaUpdated {
                session_id,
                round_trip,
                deltas,
            } => {
                self.update_stream_tool_calls(session_id.as_str(), round_trip, deltas.as_slice());
                self.record_worker_event(format!("streamed {} tool call delta(s)", deltas.len()));
                format!("tool call delta updated for round {round_trip}")
            }
            AppMessage::AssistantStreamFinished {
                session_id,
                round_trip,
                response_id,
                response_model,
                finish_reason,
            } => {
                self.finish_stream(
                    session_id.as_str(),
                    round_trip,
                    response_id,
                    response_model,
                    finish_reason,
                );
                self.record_worker_event(String::from("assistant stream finished"));
                String::from("assistant stream finished")
            }
            AppMessage::AssistantStreamFailed {
                session_id,
                round_trip,
                backend_kind,
                error,
            } => {
                let backend_kind = render_backend_kind(backend_kind).to_string();
                self.fail_stream(
                    session_id.as_str(),
                    round_trip,
                    backend_kind.as_str(),
                    error.as_str(),
                );
                let target = self.operator_target_label();
                self.record_worker_event(format!(
                    "model request failed on {} @ {}",
                    self.active_lane_label(),
                    target
                ));
                format!(
                    "backend request failed on {} @ {}",
                    self.active_lane_label(),
                    target
                )
            }
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
                self.clear_stream();
                self.transcript.clear_active_turn();
                self.transcript.clear_live_entries();
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
                self.clear_stream();
                self.transcript.clear_active_turn();
                self.transcript.commit_live_entries();
                self.transcript.push_entry(entry);
                self.snap_transcript_to_latest();
                self.record_worker_event(format!("committed {label} row: {title}"));
                format!("committed {label} row")
            }
            AppMessage::BackgroundTaskQueued {
                session_id,
                title,
                cwd,
                status,
                parent_title,
            } => {
                self.clear_stream();
                self.transcript.clear_active_turn();
                self.transcript.commit_live_entries();
                let heading = if parent_title.is_some() {
                    "Delegated Task"
                } else {
                    "Background Task"
                };
                let mut body = vec![
                    format!("Queued `{title}` to run in the background."),
                    format!("task: {} · status: {status}", short_session_id(&session_id)),
                    format!("cwd: {cwd}"),
                ];
                if let Some(parent_title) = parent_title {
                    body.push(format!("delegated from: {parent_title}"));
                    body.push(String::from(
                        "next: use /tasks to reopen this child task or return to the parent lane.",
                    ));
                    self.local_action_notice = Some(String::from("delegated task queued"));
                } else {
                    body.push(String::from(
                        "next: use /tasks to reopen it when you want to check in.",
                    ));
                    self.local_action_notice = Some(String::from("background task queued"));
                }
                self.transcript.push_entry(TranscriptEntry::new(
                    TranscriptRole::Status,
                    heading,
                    body,
                ));
                self.snap_transcript_to_latest();
                self.record_worker_event(format!(
                    "queued background task {} ({status})",
                    short_session_id(&session_id)
                ));
                format!("queued background task {}", short_session_id(&session_id))
            }
            AppMessage::ProbeRuntimeSessionReady {
                session_id,
                profile_name,
                model_id,
                cwd,
                runtime_activity,
                latest_task_workspace_summary,
                latest_task_receipt,
                mcp_state,
                recovery_note,
            } => {
                self.runtime = ProbeRuntimeState {
                    session_id: Some(session_id.clone()),
                    profile_name: Some(profile_name.clone()),
                    model_id: Some(model_id.clone()),
                    cwd: Some(cwd),
                    workspace_state: None,
                    branch_state: None,
                    delivery_state: None,
                    backend_kind: self.runtime.backend_kind.clone(),
                    activity: runtime_activity.or_else(|| self.runtime.activity.clone()),
                    round_trip: self.runtime.round_trip,
                    active_tool: self.runtime.active_tool.clone(),
                    active_tool_targets: self.runtime.active_tool_targets.clone(),
                    pending_approvals: self.runtime.pending_approvals.clone(),
                    latest_task_workspace_summary,
                    latest_task_receipt,
                    mcp_state,
                    recovery_note,
                    usage: self.runtime.usage.clone(),
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
            AppMessage::ProbeRuntimeWorkspaceStateUpdated {
                session_id,
                workspace_state,
                branch_state,
                delivery_state,
            } => {
                self.runtime.session_id = Some(session_id.clone());
                self.runtime.workspace_state = workspace_state;
                self.runtime.branch_state = branch_state;
                self.runtime.delivery_state = delivery_state;
                self.record_worker_event(format!(
                    "updated git/workspace state for {}",
                    short_session_id(session_id.as_str())
                ));
                String::from("updated git/workspace state")
            }
            AppMessage::SessionUsageUpdated { session_id, usage } => {
                self.runtime.session_id = Some(session_id);
                self.runtime.usage = usage;
                self.record_worker_event(String::from("session usage updated"));
                String::from("session usage updated")
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
            UiEvent::NextView => ScreenOutcome::idle(),
            UiEvent::PreviousView => ScreenOutcome::idle(),
            UiEvent::ToggleBody => {
                self.emphasized_copy = !self.emphasized_copy;
                self.resume_transcript_follow();
                self.sync_transcript_scroll_after_update();
                let status = if self.emphasized_copy {
                    String::from("showing operator notes instead of live response detail")
                } else {
                    String::from("restored live Apple FM detail view")
                };
                self.record_event(status.clone());
                ScreenOutcome::with_status(ScreenAction::None, status)
            }
            UiEvent::ScrollUp => {
                self.scroll_transcript_up(LINE_SCROLL_STEP);
                ScreenOutcome::idle()
            }
            UiEvent::ScrollDown => {
                self.scroll_transcript_down(LINE_SCROLL_STEP);
                ScreenOutcome::idle()
            }
            UiEvent::PageUp => {
                self.scroll_transcript_up(PAGE_SCROLL_STEP);
                ScreenOutcome::idle()
            }
            UiEvent::PageDown => {
                self.scroll_transcript_down(PAGE_SCROLL_STEP);
                ScreenOutcome::idle()
            }
            UiEvent::RunBackgroundTask => {
                if self.uses_apple_fm_backend() {
                    self.record_event(String::from("requested Apple FM backend check rerun"));
                    ScreenOutcome::with_command(
                        String::from("queued Apple FM backend check and opened backend overlay"),
                        ScreenCommand::RunAppleFmSetup,
                    )
                } else {
                    self.record_event(String::from(
                        "current backend is prepared on launch; opened backend overlay",
                    ));
                    ScreenOutcome::with_status(
                        ScreenAction::OpenSetupOverlay,
                        String::from(
                            "current backend is prepared on launch; opened backend overlay",
                        ),
                    )
                }
            }
            UiEvent::OpenStatusOverlay => ScreenOutcome::with_status(
                ScreenAction::OpenStatusOverlay,
                String::from("opened status overlay"),
            ),
            UiEvent::OpenDoctorOverlay => ScreenOutcome::with_status(
                ScreenAction::OpenDoctorOverlay,
                String::from("opened doctor overlay"),
            ),
            UiEvent::OpenSetupOverlay => ScreenOutcome::with_status(
                ScreenAction::OpenSetupOverlay,
                String::from("opened backend overlay"),
            ),
            UiEvent::OpenApprovalOverlay => ScreenOutcome::with_status(
                ScreenAction::OpenApprovalOverlay,
                String::from("opened approval overlay"),
            ),
            UiEvent::OpenGitOverlay => ScreenOutcome::with_status(
                ScreenAction::OpenGitOverlay,
                String::from("opened git overlay"),
            ),
            UiEvent::OpenRecipesOverlay => ScreenOutcome::with_status(
                ScreenAction::OpenRecipesOverlay,
                String::from("opened recipes overlay"),
            ),
            UiEvent::OpenTasksOverlay => ScreenOutcome::with_status(
                ScreenAction::OpenTasksOverlay,
                String::from("opened task list"),
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
            | UiEvent::PasteSystemClipboard
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
        TabStrip::new(self.tab_labels.clone(), self.active_tab.index()).render(frame, sections[0]);
        self.render_chat_shell(frame, sections[1], stack_depth);
    }

    fn render_chat_shell(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let body = self.render_primary_body();
        if area.width < 96 {
            let compact_height = if area.height >= 16 { 8 } else { 6 };
            let sections =
                Layout::vertical([Constraint::Length(compact_height), Constraint::Min(0)])
                    .spacing(1)
                    .split(area);
            InfoPanel::new("Session", self.render_compact_session_panel_body())
                .render(frame, sections[0]);
            let scroll_y = self.transcript_scroll_y(body.lines.len(), sections[1].height);
            let title = self.transcript_panel_title();
            InfoPanel::new(title.as_str(), body)
                .with_scroll(scroll_y)
                .render(frame, sections[1]);
            return;
        }

        let session_width = if area.width >= 110 {
            34
        } else if area.width >= 92 {
            30
        } else {
            26
        };
        let sections = Layout::horizontal([Constraint::Min(0), Constraint::Length(session_width)])
            .spacing(1)
            .split(area);
        let scroll_y = self.transcript_scroll_y(body.lines.len(), sections[0].height);
        let title = self.transcript_panel_title();
        InfoPanel::new(title.as_str(), body)
            .with_scroll(scroll_y)
            .render(frame, sections[0]);
        let session_title = self.session_panel_title();
        InfoPanel::new(session_title.as_str(), self.render_session_panel_body())
            .render(frame, sections[1]);
    }

    fn render_compact_session_panel_body(&self) -> Text<'static> {
        let mut lines = vec![
            Line::from(format!("activity: {}", self.operator_activity_label())),
            Line::from(format!("next: {}", self.current_action_hint())),
            Line::from(self.render_workspace_summary_line()),
            Line::from(format!("lane: {}", self.active_lane_label())),
            Line::from(self.render_safety_line()),
        ];
        if let Some(notice) = self.local_action_notice.as_deref() {
            lines.push(Line::from(format!("applied: {notice}")));
        } else if let Some(line) = self.render_active_tool_targets_line() {
            lines.push(Line::from(line));
        } else if let Some(line) = self.render_git_status_line() {
            lines.push(Line::from(line));
        }
        if self.show_launch_line() {
            lines.push(Line::from(format!("launch: {}", self.launch_mode_label)));
        } else if self.show_view_line() {
            lines.push(Line::from(format!(
                "view: {}",
                self.transcript_mode.label()
            )));
        } else if self.carries_compacted_context() {
            lines.push(Line::from(String::from("context: compact summary")));
        }
        lines.push(Line::from("details: /status · /doctor · /git · /tasks"));
        Text::from(lines)
    }

    fn render_setup_overlay_text(&self, _stack_depth: usize) -> Text<'static> {
        if !self.uses_apple_fm_backend() {
            return self.render_remote_backend_overlay_text();
        }
        let mut lines = self
            .render_setup_body()
            .lines
            .iter()
            .map(ToString::to_string)
            .map(Line::from)
            .collect::<Vec<_>>();
        lines.push(Line::from(""));
        lines.push(Line::from(format!("status: {}", self.render_phase_label())));
        if let Some(backend) = self.setup.backend.as_ref() {
            lines.push(Line::from(format!("backend: {}", backend.profile_name)));
        }
        for line in self.render_backend_lines() {
            lines.push(Line::from(line));
        }
        for line in self.render_availability_lines() {
            lines.push(Line::from(line));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(format!("next: {}", self.setup_overlay_hint())));
        Text::from(lines)
    }

    fn render_remote_backend_overlay_text(&self) -> Text<'static> {
        let Some(summary) = self.operator_backend.as_ref() else {
            return Text::from(vec![
                Line::from("Probe does not have a prepared backend summary yet."),
                Line::from(""),
                Line::from("Restart the TUI from `probe tui` after selecting a backend target."),
            ]);
        };

        let headline = match summary.backend_kind {
            probe_protocol::backend::BackendKind::OpenAiCodexSubscription => {
                "Probe is attached to the hosted Codex subscription backend."
            }
            probe_protocol::backend::BackendKind::OpenAiChatCompletions
            | probe_protocol::backend::BackendKind::AppleFmBridge => {
                "Probe is attached to a prepared backend target."
            }
        };

        let mut lines = vec![
            Line::from(headline),
            Line::from(""),
            Line::from(format!("lane: {}", self.active_lane_label())),
            Line::from(format!(
                "backend: {}",
                render_backend_kind(summary.backend_kind)
            )),
            Line::from(format!("target: {}", summary.endpoint_label())),
            Line::from(format!("transport: {}", summary.target_kind_label())),
            Line::from(format!(
                "model: {}",
                summary.model_id.as_deref().unwrap_or("unknown")
            )),
            Line::from(format!(
                "reasoning: {}",
                summary
                    .reasoning_level
                    .as_deref()
                    .or_else(|| {
                        resolved_reasoning_level_for_backend(summary.backend_kind, None)
                    })
                    .unwrap_or("none")
            )),
            Line::from(format!("attach: {}", summary.attach_mode_label())),
            Line::from(format!("base_url: {}", summary.base_url)),
            Line::from(format!("activity: {}", self.operator_activity_label())),
            Line::from(format!(
                "next: {}",
                self.remote_backend_overlay_hint(summary.backend_kind)
            )),
        ];
        let contract_lines = render_remote_contract_lines(summary);
        if !contract_lines.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from("contract"));
            for line in contract_lines {
                lines.push(Line::from(format!("  {line}")));
            }
        }
        if let Some(round_trip) = self.runtime.round_trip {
            lines.push(Line::from(format!("round_trip: {round_trip}")));
        }
        if let Some(tool) = self.runtime.active_tool.as_deref() {
            lines.push(Line::from(format!(
                "active_tool: {}",
                display_runtime_tool_name(tool)
            )));
        }
        if summary.backend_kind == probe_protocol::backend::BackendKind::OpenAiCodexSubscription {
            lines.push(Line::from(""));
            lines.push(Line::from("auth"));
            for line in self.render_codex_auth_lines() {
                lines.push(Line::from(format!("  {line}")));
            }
        }
        Text::from(lines)
    }

    fn render_usage_overlay_text(&self) -> Text<'static> {
        let usage = &self.runtime.usage;
        let mut lines = vec![
            Line::from("Inspect token usage for the active lane."),
            Line::from(format!(
                "lane: {} · mode: {}",
                self.active_lane_label(),
                self.operator_mode_label
            )),
            Line::from(format!("turns_with_usage: {}", usage.turns_with_usage)),
        ];
        lines.push(Line::from("latest turn"));
        if let Some(latest) = usage.latest_turn.as_ref() {
            lines.extend(
                render_usage_counts_lines(latest)
                    .into_iter()
                    .map(Line::from),
            );
        } else {
            lines.push(Line::from("  none yet"));
        }
        lines.push(Line::from("session aggregate"));
        lines.extend(
            render_usage_counts_lines(&usage.aggregate)
                .into_iter()
                .map(Line::from),
        );
        let recommendation = if usage
            .aggregate
            .total_tokens
            .is_some_and(|value| value >= 20_000)
        {
            Some("/compact")
        } else if usage
            .aggregate
            .total_tokens
            .is_some_and(|value| value >= 10_000)
        {
            Some("consider /compact")
        } else {
            None
        };
        if let Some(recommendation) = recommendation {
            lines.push(Line::from(format!("next: {recommendation}")));
        }
        lines.push(Line::from("Esc closes."));
        Text::from(lines)
    }

    fn render_status_overlay_text(
        &self,
        configured_mcp_count: usize,
        enabled_mcp_count: usize,
        mcp_summary_lines: &[String],
    ) -> Text<'static> {
        let usage = &self.runtime.usage.aggregate;
        let mut lines = vec![
            Line::from("Inspect the current operator state for the active lane."),
            Line::from(""),
            Line::from(format!("lane: {}", self.active_lane_label())),
            Line::from(format!("activity: {}", self.operator_activity_label())),
            Line::from(format!("next: {}", self.current_action_hint())),
            Line::from(""),
            Line::from(format!(
                "backend: {}",
                self.runtime
                    .backend_kind
                    .as_deref()
                    .or_else(|| {
                        self.operator_backend
                            .as_ref()
                            .map(|summary| render_backend_kind(summary.backend_kind))
                    })
                    .unwrap_or("pending")
            )),
            Line::from(format!(
                "model: {}",
                self.runtime
                    .model_id
                    .as_deref()
                    .or_else(|| {
                        self.operator_backend
                            .as_ref()
                            .and_then(|summary| summary.model_id.as_deref())
                    })
                    .unwrap_or("pending")
            )),
            Line::from(format!("reasoning: {}", self.operator_reasoning_label())),
            Line::from(format!("target: {}", self.operator_target_label())),
            Line::from(format!("cwd: {}", self.current_workspace_label())),
            Line::from(format!("mode: {}", self.operator_mode_label)),
            Line::from(format!("review: {}", self.review_mode_label)),
            Line::from(format!("view: {}", self.transcript_mode.label())),
            Line::from(self.render_memory_line()),
            Line::from(format!(
                "mcp: {enabled_mcp_count} enabled / {configured_mcp_count} configured"
            )),
        ];
        if let Some(line) = self.render_git_status_line() {
            lines.push(Line::from(line));
        }
        for line in mcp_summary_lines {
            lines.push(Line::from(line.clone()));
        }
        if let Some(line) = self.render_session_mcp_line() {
            lines.push(Line::from(line));
        }
        if let Some(line) = self.render_git_repo_root_line() {
            lines.push(Line::from(line));
        }
        if let Some(line) = self.render_delivery_status_line() {
            lines.push(Line::from(line));
        }
        if let Some(line) = self.render_workspace_boot_line() {
            lines.push(Line::from(line));
        }
        if let Some(session_id) = self.runtime_session_id() {
            lines.push(Line::from(format!("session: {}", preview(session_id, 36))));
        } else {
            lines.push(Line::from(
                "session: next turn will attach a runtime session",
            ));
        }
        if self.runtime.usage.turns_with_usage > 0 {
            lines.push(Line::from(format!(
                "usage: {} turns, total {}",
                self.runtime.usage.turns_with_usage,
                render_usage_value(usage.total_tokens, usage.total_truth.as_deref())
            )));
        }
        if let Some(line) = self.render_memory_issue_line() {
            lines.push(Line::from(line));
        }
        if let Some(line) = self.render_recovery_line() {
            lines.push(Line::from(line));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(
            "next controls: /doctor · /git · /model · /cwd · /usage · /memory · /mcp",
        ));
        lines.push(Line::from("Esc closes."));
        Text::from(lines)
    }

    fn render_doctor_overlay_text(
        &self,
        configured_mcp_count: usize,
        enabled_mcp_count: usize,
        mcp_summary_lines: &[String],
    ) -> Text<'static> {
        let mut lines = vec![
            Line::from("Run a quick health check for the active lane."),
            Line::from(""),
            Line::from(format!("lane: {}", self.active_lane_label())),
            Line::from(""),
        ];

        if let Some(summary) = self.operator_backend.as_ref() {
            lines.push(Line::from(format!(
                "backend: ok - {} via {}",
                render_backend_kind(summary.backend_kind),
                summary.endpoint_label()
            )));
        } else {
            lines.push(Line::from(
                "backend: action - Probe does not have an operator backend summary yet.",
            ));
        }

        match self.runtime.cwd.as_deref() {
            Some(cwd) if Path::new(cwd).is_dir() => {
                lines.push(Line::from(format!("workspace: ok - {}", cwd)));
            }
            Some(cwd) if Path::new(cwd).exists() => {
                lines.push(Line::from(format!(
                    "workspace: action - {} exists but is not a directory",
                    cwd
                )));
            }
            Some(cwd) => {
                lines.push(Line::from(format!(
                    "workspace: action - {} does not exist",
                    cwd
                )));
            }
            None => lines.push(Line::from(
                "workspace: action - no active workspace is attached yet",
            )),
        }
        lines.push(Line::from(self.git_doctor_line()));
        if let Some(line) = self.delivery_doctor_line() {
            lines.push(Line::from(line));
        }
        if let Some(line) = self.workspace_boot_doctor_line() {
            lines.push(Line::from(line));
        }

        match self.probe_home.as_ref() {
            Some(path) => lines.push(Line::from(format!("probe_home: ok - {}", path.display()))),
            None => lines.push(Line::from(
                "probe_home: action - user memory and persisted integrations are limited",
            )),
        }

        if self.memory_stack.first_issue_line().is_some() {
            lines.push(Line::from(format!(
                "memory: action - {}",
                self.memory_stack
                    .first_issue_line()
                    .unwrap_or("unknown memory issue")
            )));
        } else {
            lines.push(Line::from(format!(
                "memory: ok - {}",
                self.memory_stack.active_label()
            )));
        }

        if self.runtime.pending_approvals.is_empty() {
            lines.push(Line::from("approvals: ok - no pending approvals"));
        } else if self.has_pending_review_changes() {
            lines.push(Line::from(format!(
                "approvals: action - {} proposed change(s) waiting for review",
                self.runtime.pending_approvals.len()
            )));
        } else {
            lines.push(Line::from(format!(
                "approvals: action - {} tool approval(s) waiting",
                self.runtime.pending_approvals.len()
            )));
        }

        match self.runtime_session_id() {
            Some(session_id) => lines.push(Line::from(format!(
                "session: ok - attached ({})",
                preview(session_id, 36)
            ))),
            None => lines.push(Line::from(
                "session: info - the next turn will create a runtime session",
            )),
        }

        if configured_mcp_count == 0 {
            lines.push(Line::from(
                "mcp: info - no saved MCP servers yet; /mcp adds one",
            ));
        } else if enabled_mcp_count == 0 {
            lines.push(Line::from(format!(
                "mcp: action - {configured_mcp_count} saved, but none are enabled"
            )));
        } else {
            lines.push(Line::from(format!(
                "mcp: ok - {enabled_mcp_count} enabled / {configured_mcp_count} configured"
            )));
        }
        if let Some(state) = self.runtime.mcp_state.as_ref() {
            if let Some(error) = state.load_error.as_deref() {
                lines.push(Line::from(format!(
                    "mcp runtime: action - {}",
                    preview(error, 120)
                )));
            } else {
                let connected = state
                    .servers
                    .iter()
                    .filter(|server| {
                        server.connection_status == Some(SessionMcpConnectionStatus::Connected)
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
                        server.connection_status == Some(SessionMcpConnectionStatus::Unsupported)
                    })
                    .count();
                let discovered_tools = state
                    .servers
                    .iter()
                    .map(|server| server.discovered_tools.len())
                    .sum::<usize>();
                lines.push(Line::from(format!(
                    "mcp runtime: {} attached, {} connected, {} failed, {} unsupported, {} tool(s) discovered",
                    state.servers.len(),
                    connected,
                    failed,
                    unsupported,
                    discovered_tools
                )));
            }
        } else {
            lines.push(Line::from(
                "mcp runtime: info - the next runtime session will snapshot enabled MCP entries",
            ));
        }
        for line in mcp_summary_lines {
            lines.push(Line::from(line.clone()));
        }

        if self.operator_backend.as_ref().is_some_and(|summary| {
            summary.backend_kind == probe_protocol::backend::BackendKind::OpenAiCodexSubscription
        }) {
            lines.push(Line::from(format!(
                "codex auth: {}",
                self.codex_auth_doctor_line()
            )));
        }

        let total_tokens = render_usage_value(
            self.runtime.usage.aggregate.total_tokens,
            self.runtime.usage.aggregate.total_truth.as_deref(),
        );
        if self.runtime.usage.turns_with_usage == 0 {
            lines.push(Line::from("usage: info - no token usage recorded yet"));
        } else {
            lines.push(Line::from(format!(
                "usage: ok - {} turn(s) recorded, total {}",
                self.runtime.usage.turns_with_usage, total_tokens
            )));
        }

        if let Some(note) = self.runtime.recovery_note.as_deref() {
            lines.push(Line::from(format!(
                "recovery: action - {}",
                compact_recovery_note(note)
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from("next: /status for the calmer summary, /git for branch details, /backend for target details, /cwd to switch workspace, /memory to adjust rules, /mcp to manage integrations."));
        lines.push(Line::from("Esc closes."));
        Text::from(lines)
    }

    fn render_git_overlay_text(&self) -> Text<'static> {
        let mut lines = vec![
            Line::from("Inspect branch, delivery, and workspace boot state for the active lane."),
            Line::from(""),
            Line::from(format!("lane: {}", self.active_lane_label())),
            Line::from(format!("cwd: {}", self.current_workspace_label())),
        ];

        if let Some(branch_state) = self.runtime.branch_state.as_ref() {
            lines.push(Line::from(format!(
                "repo root: {}",
                branch_state.repo_root.display()
            )));
            lines.push(Line::from(format!(
                "branch: {}",
                self.git_branch_label(branch_state)
            )));
            lines.push(Line::from(format!(
                "head: {}",
                preview(branch_state.head_commit.as_str(), 12)
            )));
            lines.push(Line::from(format!(
                "working tree: {}",
                if branch_state.working_tree_dirty {
                    "dirty"
                } else {
                    "clean"
                }
            )));
            if let Some(upstream_ref) = branch_state.upstream_ref.as_deref() {
                lines.push(Line::from(format!("upstream: {upstream_ref}")));
            } else {
                lines.push(Line::from("upstream: none"));
            }
            lines.push(Line::from(format!(
                "sync: {}",
                self.git_sync_summary(branch_state)
            )));
        } else if self.runtime.session_id.is_some() {
            lines.push(Line::from(
                "git: Probe has not detected a git repo for the current workspace.",
            ));
        } else {
            lines.push(Line::from(
                "git: start a turn to capture branch and delivery state for this lane.",
            ));
        }

        if let Some(line) = self.render_delivery_status_line() {
            lines.push(Line::from(line));
        }
        if let Some(workspace_state) = self.runtime.workspace_state.as_ref() {
            lines.push(Line::from(format!(
                "workspace boot: {}",
                self.workspace_boot_mode_label(workspace_state.boot_mode)
            )));
            if let Some(execution_host) = workspace_state.execution_host.as_ref() {
                let host = execution_host
                    .display_name
                    .as_deref()
                    .unwrap_or(execution_host.host_id.as_str());
                lines.push(Line::from(format!("execution host: {}", preview(host, 48))));
            }
            if let Some(note) = workspace_state.provenance_note.as_deref() {
                lines.push(Line::from(format!("workspace note: {}", preview(note, 88))));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(
            "next: /branch to create or switch work branches, /stage to stage repo changes, /commit to record staged work, /push to deliver the current branch, /pr to open a draft PR, /status for the calm summary, /doctor for recovery help.",
        ));
        lines.push(Line::from("Esc closes."));
        Text::from(lines)
    }

    fn render_checkpoint_overlay_text(&self) -> Text<'static> {
        let mut lines = vec![
            Line::from("Inspect the latest checkpoint coverage for this lane."),
            Line::from(""),
            Line::from(format!("lane: {}", self.active_lane_label())),
            Line::from(format!("workspace: {}", self.current_workspace_label())),
            Line::from(self.render_last_task_line()),
            Line::from(self.render_task_checkpoint_line()),
            Line::from(self.render_revert_line()),
            Line::from(""),
        ];

        if self.has_pending_review_changes() {
            lines.push(Line::from(
                "Current changes are still proposed, so no applied-task checkpoint exists yet.",
            ));
            lines.push(Line::from(format!(
                "paths: {}",
                summarize_inline_paths(
                    self.pending_review_paths().unwrap_or_default().as_slice(),
                    4
                )
            )));
            lines.push(Line::from(
                "next: review /diff, then A applies or R rejects.",
            ));
            lines.push(Line::from("Enter or Esc closes."));
            return Text::from(lines);
        }

        let Some(summary) = self.latest_task_workspace_summary() else {
            lines.push(Line::from(
                "No task checkpoint is available on this lane yet because no recent task receipt was retained.",
            ));
            lines.push(Line::from("Enter or Esc closes."));
            return Text::from(lines);
        };

        lines.push(Line::from(summary.checkpoint.summary_text.clone()));
        if !summary.changed_files.is_empty() {
            lines.push(Line::from(format!(
                "changed: {}",
                summarize_inline_paths(summary.changed_files.as_slice(), 4)
            )));
        }
        if !summary.preexisting_dirty_files.is_empty() {
            lines.push(Line::from(format!(
                "dirty before: {}",
                summarize_inline_paths(summary.preexisting_dirty_files.as_slice(), 4)
            )));
        }
        if !summary.outside_tracking_dirty_files.is_empty() {
            lines.push(Line::from(format!(
                "dirty now: {}",
                summarize_inline_paths(summary.outside_tracking_dirty_files.as_slice(), 4)
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "restore path: {}",
            summary.revertibility.summary_text
        )));
        lines.push(Line::from("Enter or Esc closes."));
        Text::from(lines)
    }

    fn render_revert_overlay_text(&self) -> Text<'static> {
        let mut lines = vec![
            Line::from("Inspect how reversible the latest task is on this lane."),
            Line::from(""),
            Line::from(format!("lane: {}", self.active_lane_label())),
            Line::from(format!("workspace: {}", self.current_workspace_label())),
            Line::from(self.render_last_task_line()),
            Line::from(self.render_task_checkpoint_line()),
            Line::from(self.render_revert_line()),
            Line::from(""),
        ];

        if self.has_pending_review_changes() {
            lines.push(Line::from(
                "There is nothing applied to revert yet because the current write is still waiting for approval.",
            ));
            lines.push(Line::from(
                "next: inspect /diff, then A applies or R rejects.",
            ));
            lines.push(Line::from("Enter or Esc closes."));
            return Text::from(lines);
        }

        let Some(summary) = self.latest_task_workspace_summary() else {
            lines.push(Line::from(
                "No recent applied task is available to revert on this lane yet.",
            ));
            lines.push(Line::from("Enter or Esc closes."));
            return Text::from(lines);
        };

        if summary.changed_files.is_empty()
            && summary.status != TaskWorkspaceSummaryStatus::ChangeAccountingLimited
        {
            lines.push(Line::from(
                "No repo edits from the latest task are available to revert.",
            ));
            lines.push(Line::from("Enter or Esc closes."));
            return Text::from(lines);
        }

        lines.push(Line::from(summary.summary_text.clone()));
        if !summary.changed_files.is_empty() {
            lines.push(Line::from(format!(
                "scope: {}",
                summarize_inline_paths(summary.changed_files.as_slice(), 4)
            )));
        }
        if let Some(reason) = self
            .latest_task_receipt()
            .and_then(|receipt| receipt.uncertainty_reasons.first())
        {
            lines.push(Line::from(format!("watch: {}", preview(reason, 120))));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(summary.revertibility.summary_text.clone()));
        if self.can_execute_revert() {
            lines.push(Line::from(
                "next: press A or Enter to restore the latest exact apply_patch task.",
            ));
            lines.push(Line::from("A or Enter reverts. Esc closes."));
        } else {
            lines.push(Line::from(
                "For now: use /diff to inspect the task and revert manually in git or your editor if needed.",
            ));
            lines.push(Line::from("Enter or Esc closes."));
        }
        Text::from(lines)
    }

    fn active_lane_label(&self) -> &str {
        self.lane_label
            .as_deref()
            .or_else(|| {
                self.tab_labels
                    .get(self.active_tab.index())
                    .map(String::as_str)
            })
            .unwrap_or("pending")
    }

    fn operator_target_label(&self) -> String {
        self.operator_backend
            .as_ref()
            .map(|summary| {
                format!(
                    "{} {}",
                    summary.target_kind_label(),
                    summary.endpoint_label()
                )
            })
            .unwrap_or_else(|| String::from("pending"))
    }

    fn workspace_label(&self) -> String {
        self.runtime
            .cwd
            .as_deref()
            .map(|cwd| shorten_path_for_display(cwd, 28))
            .unwrap_or_else(|| String::from("pending"))
    }

    fn runtime_activity_kind(&self) -> Option<RuntimeActivityKind> {
        self.runtime.activity.as_ref().map(|activity| activity.kind)
    }

    fn operator_activity_label(&self) -> String {
        if !self.runtime.pending_approvals.is_empty() {
            if let Some(paths) = self.pending_review_paths()
                && !paths.is_empty()
            {
                return format!(
                    "review changes: {}",
                    summarize_inline_paths(paths.as_slice(), 2)
                );
            }
            if let Some(approval) = self.runtime.pending_approvals.first() {
                return format!("action needed: approve {}", approval.tool_name);
            }
            return String::from("action needed: review approval");
        }

        if matches!(
            self.runtime_activity_kind(),
            Some(RuntimeActivityKind::Failed)
        ) && let Some(error) = self
            .stream
            .as_ref()
            .and_then(|stream| stream.failure.as_deref())
        {
            return classify_runtime_failure(error).title.to_ascii_lowercase();
        }

        if let Some(activity) = self.runtime.activity.as_ref() {
            if activity.kind == RuntimeActivityKind::WaitingForApproval {
                return action_needed_label(activity.label.as_str());
            }
            return activity.label.clone();
        }

        if self.uses_apple_fm_backend() {
            return self.render_phase_label().to_string();
        }

        String::from("ready")
    }

    fn current_action_hint(&self) -> String {
        if self.operator_mode_label == "plan" {
            return String::from("Plan only; no edits");
        }
        if self.launch_mode_label == "background" {
            return String::from("Enter queues the next task in background · /tasks reopens it");
        }
        if self.launch_mode_label == "delegate" {
            return String::from(
                "Enter delegates the next task as a child · /tasks shows parent and child work",
            );
        }
        if let Some(paths) = self.pending_review_paths()
            && !paths.is_empty()
        {
            return format!(
                "/diff previews {} · A applies · R rejects",
                summarize_inline_paths(paths.as_slice(), 1)
            );
        }
        if !self.runtime.pending_approvals.is_empty()
            || matches!(
                self.runtime_activity_kind(),
                Some(RuntimeActivityKind::WaitingForApproval)
            )
        {
            return String::from("Ctrl+A opens approval · Enter decides");
        }
        if matches!(
            self.runtime_activity_kind(),
            Some(
                RuntimeActivityKind::Queued
                    | RuntimeActivityKind::Starting
                    | RuntimeActivityKind::WaitingForBackend
                    | RuntimeActivityKind::StreamingReply
                    | RuntimeActivityKind::UpdatingReply
                    | RuntimeActivityKind::PlanningTool
                    | RuntimeActivityKind::Reading
                    | RuntimeActivityKind::Editing
                    | RuntimeActivityKind::Validating
                    | RuntimeActivityKind::RunningTool
                    | RuntimeActivityKind::Finalizing
            )
        ) {
            return String::from("Working now; PgUp scrolls without losing your place");
        }
        if matches!(
            self.runtime_activity_kind(),
            Some(RuntimeActivityKind::Failed)
        ) && let Some(error) = self
            .stream
            .as_ref()
            .and_then(|stream| stream.failure.as_deref())
        {
            return stream_failure_summary(error, self.operator_backend.as_ref()).next_step;
        }
        if matches!(
            self.runtime_activity_kind(),
            Some(RuntimeActivityKind::Failed | RuntimeActivityKind::Stopped)
        ) {
            let changed_files = self
                .runtime
                .latest_task_receipt
                .as_ref()
                .map(|receipt| receipt.workspace.changed_files.len())
                .or_else(|| {
                    self.runtime
                        .latest_task_workspace_summary
                        .as_ref()
                        .map(|summary| summary.changed_files.len())
                })
                .unwrap_or(0);
            if changed_files > 0 {
                return String::from("Review landed edits, then continue or rerun");
            }
            return String::from("Retry the turn or switch lanes with Tab");
        }
        if self.uses_apple_fm_backend()
            && matches!(
                self.setup.phase,
                TaskPhase::Idle | TaskPhase::Unavailable | TaskPhase::Failed
            )
        {
            return String::from("Ctrl+R checks Apple FM before you send a task");
        }
        if self.transcript.is_empty() {
            return String::from("Describe the change you want");
        }
        String::from("Ask for the next change or a follow-up")
    }

    fn render_task_workspace_lines(&self) -> Vec<String> {
        if let Some(paths) = self.pending_review_paths()
            && !paths.is_empty()
        {
            return vec![format!(
                "edits: proposed -> {}",
                summarize_inline_paths(paths.as_slice(), 2)
            )];
        }
        let Some(summary) = self
            .runtime
            .latest_task_receipt
            .as_ref()
            .map(|receipt| &receipt.workspace)
            .or(self.runtime.latest_task_workspace_summary.as_ref())
        else {
            return vec![String::from("edits: none yet")];
        };

        let stopped = self
            .runtime
            .latest_task_receipt
            .as_ref()
            .is_some_and(|receipt| receipt.disposition == TaskReceiptDisposition::Stopped);

        let mut lines = match summary.status {
            TaskWorkspaceSummaryStatus::NoRepoChanges if stopped => {
                vec![String::from("edits: stopped before changes landed")]
            }
            TaskWorkspaceSummaryStatus::NoRepoChanges => vec![String::from("edits: none")],
            TaskWorkspaceSummaryStatus::Changed => vec![format!(
                "edits: {}",
                summarize_inline_paths(summary.changed_files.as_slice(), 2)
            )],
            TaskWorkspaceSummaryStatus::Reverted => vec![format!(
                "edits: reverted -> {}",
                summarize_inline_paths(summary.changed_files.as_slice(), 2)
            )],
            TaskWorkspaceSummaryStatus::PartialChangesBeforeFailure if stopped => {
                if summary.changed_files.is_empty() {
                    vec![String::from("edits: stopped before changes landed")]
                } else {
                    vec![format!(
                        "edits: stopped -> {}",
                        summarize_inline_paths(summary.changed_files.as_slice(), 2)
                    )]
                }
            }
            TaskWorkspaceSummaryStatus::PartialChangesBeforeFailure => vec![format!(
                "edits: partial -> {}",
                summarize_inline_paths(summary.changed_files.as_slice(), 2)
            )],
            TaskWorkspaceSummaryStatus::PendingApproval => {
                if summary.changed_files.is_empty() {
                    vec![String::from("edits: waiting, none yet")]
                } else {
                    vec![format!(
                        "edits: landed -> {}",
                        summarize_inline_paths(summary.changed_files.as_slice(), 2)
                    )]
                }
            }
            TaskWorkspaceSummaryStatus::ChangeAccountingLimited => {
                if stopped {
                    vec![String::from("edits: stopped, shell writes unknown")]
                } else {
                    vec![String::from("edits: unknown (shell writes)")]
                }
            }
        };

        if !summary.preexisting_dirty_files.is_empty() {
            lines.push(format!(
                "dirty_before: {}",
                summarize_inline_paths(summary.preexisting_dirty_files.as_slice(), 2)
            ));
        }
        if !summary.outside_tracking_dirty_files.is_empty() {
            lines.push(format!(
                "dirty now: {}",
                summarize_inline_paths(summary.outside_tracking_dirty_files.as_slice(), 2)
            ));
        }
        if summary.change_accounting_limited
            && summary.status != TaskWorkspaceSummaryStatus::ChangeAccountingLimited
        {
            lines.push(String::from("edits: shell write accounting limited"));
        }
        lines
    }

    fn render_task_receipt_lines(&self) -> Vec<String> {
        if self.has_pending_review_changes() {
            return vec![String::from("verify: pending apply")];
        }
        let Some(receipt) = self.runtime.latest_task_receipt.as_ref() else {
            return vec![String::from("verify: none yet")];
        };
        let mut lines = vec![format!("verify: {}", compact_verification_line(receipt))];
        if let Some(reason) = receipt.uncertainty_reasons.first() {
            lines.push(format!("risk: {}", preview(reason, 72)));
        }
        lines
    }

    fn render_last_task_line(&self) -> String {
        if self.has_pending_review_changes() {
            return String::from("last task: proposed");
        }
        if let Some(receipt) = self.runtime.latest_task_receipt.as_ref() {
            let label = match (receipt.disposition, receipt.workspace.status) {
                (TaskReceiptDisposition::Succeeded, TaskWorkspaceSummaryStatus::NoRepoChanges) => {
                    "no repo changes"
                }
                (TaskReceiptDisposition::Succeeded, TaskWorkspaceSummaryStatus::Changed) => {
                    "applied"
                }
                (TaskReceiptDisposition::Succeeded, TaskWorkspaceSummaryStatus::Reverted) => {
                    "reverted"
                }
                (
                    TaskReceiptDisposition::Succeeded,
                    TaskWorkspaceSummaryStatus::PartialChangesBeforeFailure,
                ) => "partial edits",
                (
                    TaskReceiptDisposition::Succeeded,
                    TaskWorkspaceSummaryStatus::PendingApproval,
                ) => "waiting approval",
                (
                    TaskReceiptDisposition::Succeeded,
                    TaskWorkspaceSummaryStatus::ChangeAccountingLimited,
                ) => "limited visibility",
                (TaskReceiptDisposition::PendingApproval, _) => "waiting approval",
                (TaskReceiptDisposition::Failed, TaskWorkspaceSummaryStatus::NoRepoChanges) => {
                    "failed before edits"
                }
                (TaskReceiptDisposition::Failed, _) => "partial edits",
                (TaskReceiptDisposition::Stopped, TaskWorkspaceSummaryStatus::NoRepoChanges) => {
                    "stopped before edits"
                }
                (TaskReceiptDisposition::Stopped, _) => "stopped after edits",
            };
            return format!("last task: {label}");
        }
        if let Some(summary) = self.runtime.latest_task_workspace_summary.as_ref() {
            let label = match summary.status {
                TaskWorkspaceSummaryStatus::NoRepoChanges => "no repo changes",
                TaskWorkspaceSummaryStatus::Changed => "applied",
                TaskWorkspaceSummaryStatus::Reverted => "reverted",
                TaskWorkspaceSummaryStatus::PartialChangesBeforeFailure => "partial edits",
                TaskWorkspaceSummaryStatus::PendingApproval => "waiting approval",
                TaskWorkspaceSummaryStatus::ChangeAccountingLimited => "limited visibility",
            };
            return format!("last task: {label}");
        }
        String::from("last task: none yet")
    }

    fn render_task_checkpoint_line(&self) -> String {
        if self.has_pending_review_changes() {
            return String::from("checkpoint: pending review");
        }
        let Some(summary) = self.latest_task_workspace_summary() else {
            return String::from("checkpoint: none yet");
        };
        let label = match summary.checkpoint.status {
            TaskCheckpointStatus::NotCaptured => {
                if summary.status == TaskWorkspaceSummaryStatus::NoRepoChanges {
                    "not needed"
                } else {
                    "none"
                }
            }
            TaskCheckpointStatus::Captured => "captured",
            TaskCheckpointStatus::Limited => "limited",
        };
        format!("checkpoint: {label}")
    }

    fn revert_availability(&self) -> RevertAvailability {
        if self.has_pending_review_changes() {
            return RevertAvailability::PendingReview;
        }
        let Some(summary) = self.latest_task_workspace_summary() else {
            return RevertAvailability::Unavailable;
        };
        match summary.revertibility.status {
            TaskRevertibilityStatus::Exact => RevertAvailability::Exact,
            TaskRevertibilityStatus::Limited => RevertAvailability::Limited,
            TaskRevertibilityStatus::Unavailable => RevertAvailability::Unavailable,
        }
    }

    fn render_revert_line(&self) -> String {
        format!("revert: {}", self.revert_availability().label())
    }

    fn git_branch_label(&self, branch_state: &SessionBranchState) -> String {
        if branch_state.detached_head {
            format!(
                "detached @ {}",
                preview(branch_state.head_commit.as_str(), 12)
            )
        } else {
            branch_state.head_ref.clone()
        }
    }

    fn git_sync_summary(&self, branch_state: &SessionBranchState) -> String {
        let ahead = branch_state.ahead_by.unwrap_or(0);
        let behind = branch_state.behind_by.unwrap_or(0);
        match (ahead, behind) {
            (0, 0) => String::from("up to date"),
            (ahead, 0) => format!("ahead {ahead}"),
            (0, behind) => format!("behind {behind}"),
            (ahead, behind) => format!("ahead {ahead} · behind {behind}"),
        }
    }

    fn delivery_status_label(&self, status: SessionDeliveryStatus) -> &'static str {
        match status {
            SessionDeliveryStatus::NeedsCommit => "needs commit",
            SessionDeliveryStatus::LocalOnly => "local only",
            SessionDeliveryStatus::NeedsPush => "needs push",
            SessionDeliveryStatus::Synced => "synced",
            SessionDeliveryStatus::Diverged => "diverged",
        }
    }

    fn workspace_boot_mode_label(&self, boot_mode: SessionWorkspaceBootMode) -> &'static str {
        match boot_mode {
            SessionWorkspaceBootMode::Fresh => "fresh",
            SessionWorkspaceBootMode::PreparedBaseline => "prepared baseline",
            SessionWorkspaceBootMode::SnapshotRestore => "snapshot restore",
        }
    }

    fn render_git_status_line(&self) -> Option<String> {
        if let Some(branch_state) = self.runtime.branch_state.as_ref() {
            let mut parts = vec![self.git_branch_label(branch_state)];
            parts.push(String::from(if branch_state.working_tree_dirty {
                "dirty"
            } else {
                "clean"
            }));
            if let Some(delivery_state) = self.runtime.delivery_state.as_ref() {
                parts.push(String::from(
                    self.delivery_status_label(delivery_state.status),
                ));
            }
            let ahead = branch_state.ahead_by.unwrap_or(0);
            let behind = branch_state.behind_by.unwrap_or(0);
            if ahead > 0 {
                parts.push(format!("ahead {ahead}"));
            }
            if behind > 0 {
                parts.push(format!("behind {behind}"));
            }
            return Some(format!("git: {}", parts.join(" · ")));
        }
        if self.runtime.workspace_state.is_some() || self.runtime.delivery_state.is_some() {
            return Some(String::from("git: no repo"));
        }
        None
    }

    fn render_git_repo_root_line(&self) -> Option<String> {
        self.runtime
            .branch_state
            .as_ref()
            .map(|branch_state| format!("repo: {}", branch_state.repo_root.display()))
    }

    fn render_delivery_status_line(&self) -> Option<String> {
        let delivery_state = self.runtime.delivery_state.as_ref()?;
        let mut line = format!(
            "delivery: {}",
            self.delivery_status_label(delivery_state.status)
        );
        if let Some(compare_ref) = delivery_state.compare_ref.as_deref() {
            line.push_str(&format!(" · {}", preview(compare_ref, 42)));
        } else if let Some(remote) = delivery_state.remote_tracking_ref.as_deref() {
            line.push_str(&format!(" · {}", preview(remote, 42)));
        }
        Some(line)
    }

    fn render_workspace_boot_line(&self) -> Option<String> {
        self.runtime
            .workspace_state
            .as_ref()
            .map(|workspace_state| {
                format!(
                    "workspace boot: {}",
                    self.workspace_boot_mode_label(workspace_state.boot_mode)
                )
            })
    }

    fn git_doctor_line(&self) -> String {
        let Some(branch_state) = self.runtime.branch_state.as_ref() else {
            if self.runtime.session_id.is_some() {
                return String::from("git: info - no git repo detected in the current workspace");
            }
            return String::from("git: info - start a turn to capture branch state");
        };
        let dirty = if branch_state.working_tree_dirty {
            "dirty working tree"
        } else {
            "clean working tree"
        };
        format!(
            "git: ok - {} ({dirty})",
            self.git_branch_label(branch_state)
        )
    }

    fn delivery_doctor_line(&self) -> Option<String> {
        let delivery_state = self.runtime.delivery_state.as_ref()?;
        let line = match delivery_state.status {
            SessionDeliveryStatus::NeedsCommit => {
                "delivery: action - repo changes need a commit".to_string()
            }
            SessionDeliveryStatus::LocalOnly => {
                "delivery: action - branch has local commits without a tracked remote".to_string()
            }
            SessionDeliveryStatus::NeedsPush => {
                "delivery: action - local commits are ready to push".to_string()
            }
            SessionDeliveryStatus::Synced => {
                "delivery: ok - branch is synced with its tracked remote".to_string()
            }
            SessionDeliveryStatus::Diverged => {
                "delivery: action - local and remote history have diverged".to_string()
            }
        };
        Some(line)
    }

    fn workspace_boot_doctor_line(&self) -> Option<String> {
        let workspace_state = self.runtime.workspace_state.as_ref()?;
        Some(format!(
            "workspace boot: ok - {}",
            self.workspace_boot_mode_label(workspace_state.boot_mode)
        ))
    }

    fn render_memory_line(&self) -> String {
        format!("memory: {}", self.memory_stack.active_label())
    }

    fn workspace_summary_label(&self) -> String {
        let cwd = self.current_workspace_label();
        Path::new(cwd.as_str())
            .file_name()
            .and_then(|segment| segment.to_str())
            .filter(|segment| !segment.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or(cwd)
    }

    fn render_workspace_summary_line(&self) -> String {
        format!("workspace: {}", self.workspace_summary_label())
    }

    fn render_safety_line(&self) -> String {
        if self.has_pending_review_changes()
            || !self.runtime.pending_approvals.is_empty()
            || matches!(
                self.runtime_activity_kind(),
                Some(RuntimeActivityKind::WaitingForApproval)
            )
        {
            return String::from("safety: approval required");
        }
        format!("safety: {}", self.review_mode_label)
    }

    fn show_lane_line(&self) -> bool {
        true
    }

    fn show_git_summary_line(&self) -> bool {
        self.runtime
            .branch_state
            .as_ref()
            .is_some_and(|branch_state| {
                branch_state.working_tree_dirty
                    || branch_state.ahead_by.unwrap_or(0) > 0
                    || branch_state.behind_by.unwrap_or(0) > 0
            })
            || self
                .runtime
                .delivery_state
                .as_ref()
                .is_some_and(|delivery_state| {
                    delivery_state.status != SessionDeliveryStatus::Synced
                })
    }

    fn show_mode_line(&self) -> bool {
        self.operator_mode_label != "coding"
    }

    fn show_launch_line(&self) -> bool {
        self.launch_mode_label != "foreground"
    }

    fn show_view_line(&self) -> bool {
        self.transcript_mode != TranscriptMode::Conversation
    }

    fn show_memory_line(&self) -> bool {
        !self.memory_stack.layers.is_empty()
    }

    fn show_session_mcp_line(&self) -> bool {
        self.runtime
            .mcp_state
            .as_ref()
            .is_some_and(|state| state.load_error.is_some() || !state.servers.is_empty())
    }

    fn render_session_mcp_line(&self) -> Option<String> {
        let state = self.runtime.mcp_state.as_ref()?;
        if let Some(error) = state.load_error.as_deref() {
            return Some(format!("mcp session: error - {}", preview(error, 64)));
        }
        let connected = state
            .servers
            .iter()
            .filter(|server| {
                server.connection_status == Some(SessionMcpConnectionStatus::Connected)
            })
            .count();
        let failed = state
            .servers
            .iter()
            .filter(|server| server.connection_status == Some(SessionMcpConnectionStatus::Failed))
            .count();
        let discovered_tools = state
            .servers
            .iter()
            .map(|server| server.discovered_tools.len())
            .sum::<usize>();
        Some(format!(
            "mcp session: {} attached · {} connected · {} failed · {} tools",
            state.servers.len(),
            connected,
            failed,
            discovered_tools
        ))
    }

    fn render_memory_issue_line(&self) -> Option<String> {
        self.memory_stack
            .first_issue_line()
            .map(|issue| format!("memory issue: {}", preview(issue, 72)))
    }

    fn operator_reasoning_label(&self) -> &str {
        self.operator_backend
            .as_ref()
            .and_then(|summary| {
                summary
                    .reasoning_level
                    .as_deref()
                    .or_else(|| resolved_reasoning_level_for_backend(summary.backend_kind, None))
            })
            .unwrap_or("none")
    }

    fn render_recovery_line(&self) -> Option<String> {
        self.runtime
            .recovery_note
            .as_deref()
            .map(|note| format!("resume: {}", compact_recovery_note(note)))
    }

    fn render_active_tool_targets_line(&self) -> Option<String> {
        let activity_kind = self.runtime_activity_kind()?;
        if !matches!(
            activity_kind,
            RuntimeActivityKind::PlanningTool
                | RuntimeActivityKind::Editing
                | RuntimeActivityKind::RunningTool
                | RuntimeActivityKind::WaitingForApproval
        ) {
            return None;
        }
        if self.runtime.active_tool_targets.is_empty() {
            return None;
        }
        Some(format!(
            "updating: {}",
            summarize_inline_paths(self.runtime.active_tool_targets.as_slice(), 2)
        ))
    }

    fn render_session_panel_body(&self) -> Text<'static> {
        let mut lines = vec![
            Line::from(format!("activity: {}", self.operator_activity_label())),
            Line::from(format!("next: {}", self.current_action_hint())),
        ];
        if let Some(line) = self.render_active_tool_targets_line() {
            lines.push(Line::from(line));
        }
        if let Some(notice) = self.local_action_notice.as_deref() {
            lines.push(Line::from(format!("applied: {notice}")));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(self.render_workspace_summary_line()));
        if self.show_lane_line() {
            lines.push(Line::from(format!("lane: {}", self.active_lane_label())));
        }
        if self.show_git_summary_line()
            && let Some(line) = self.render_git_status_line()
        {
            lines.push(Line::from(line));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(self.render_safety_line()));
        if self.show_mode_line() {
            lines.push(Line::from(format!("mode: {}", self.operator_mode_label)));
        }
        if self.show_launch_line() {
            lines.push(Line::from(format!("launch: {}", self.launch_mode_label)));
        }
        if self.show_view_line() {
            lines.push(Line::from(format!(
                "view: {}",
                self.transcript_mode.label()
            )));
        }
        if self.show_memory_line() {
            lines.push(Line::from(self.render_memory_line()));
        }
        if self.show_session_mcp_line()
            && let Some(line) = self.render_session_mcp_line()
        {
            lines.push(Line::from(line));
        }
        if self.has_pending_review_changes()
            || self.runtime.latest_task_receipt.is_some()
            || self.runtime.latest_task_workspace_summary.is_some()
        {
            lines.push(Line::from(""));
            lines.push(Line::from(self.render_last_task_line()));
            lines.push(Line::from(self.render_task_checkpoint_line()));
            lines.push(Line::from(self.render_revert_line()));
            lines.extend(
                self.render_task_workspace_lines()
                    .into_iter()
                    .map(Line::from),
            );
            lines.extend(self.render_task_receipt_lines().into_iter().map(Line::from));
        }
        if let Some(line) = self.render_memory_issue_line() {
            lines.push(Line::from(line));
        }
        if self.carries_compacted_context() {
            lines.push(Line::from(String::from("context: compact summary")));
        }
        if let Some(line) = self.render_recovery_line() {
            lines.push(Line::from(line));
        }
        Text::from(lines)
    }

    fn render_empty_state_body(&self) -> Text<'static> {
        let mut lines = vec![
            Line::from("Tell Probe what you want changed."),
            Line::from(""),
        ];
        if self.uses_apple_fm_backend()
            && matches!(
                self.setup.phase,
                TaskPhase::Idle | TaskPhase::Unavailable | TaskPhase::Failed
            )
        {
            lines.extend([
                Line::from("This lane needs a backend check before coding."),
                Line::from("Press Ctrl+R to verify the bridge, then send a task."),
                Line::from(""),
            ]);
        } else {
            lines.extend([
                Line::from("Press Enter after a repo question or code request."),
                Line::from("Probe will inspect the workspace before making claims or edits."),
                Line::from(""),
            ]);
        }
        lines.extend([
            Line::from("Try:"),
            Line::from("- explain how runtime events reach the TUI"),
            Line::from("- add a focused README note"),
            Line::from("- search for approval handling and summarize it"),
            Line::from(""),
            Line::from("Keys: F2 status · F4 tasks · Ctrl+G git"),
        ]);
        Text::from(lines)
    }

    fn render_primary_body(&self) -> Text<'static> {
        if self.transcript.is_empty() {
            return self.render_empty_state_body();
        }
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
        self.transcript.as_text_for_mode(self.transcript_mode)
    }

    fn render_setup_body(&self) -> Text<'static> {
        if !self.uses_apple_fm_backend() {
            return Text::from(vec![
                Line::from("This lane already uses a prepared remote backend."),
                Line::from(""),
                Line::from("Esc returns to chat. Tab switches lanes there."),
            ]);
        }
        if self.emphasized_copy {
            return Text::from(vec![
                Line::from("Apple FM setup is now a secondary Probe surface."),
                Line::from(""),
                Line::from("The primary home screen is the chat shell."),
                Line::from("This overlay keeps the backend proof close by when you need it."),
            ]);
        }

        if let Some(failure) = &self.setup.failure {
            return Text::from(vec![
                Line::from(format!("Setup failed during {}.", failure.stage)),
                Line::from(""),
                Line::from(format!("why: {}", failure.detail)),
                Line::from(format!(
                    "code: {}",
                    failure
                        .reason_code
                        .clone()
                        .unwrap_or_else(|| String::from("none"))
                )),
                Line::from(format!(
                    "detail: {}",
                    failure
                        .failure_reason
                        .clone()
                        .unwrap_or_else(|| String::from("none"))
                )),
                Line::from(format!(
                    "fix: {}",
                    failure
                        .recovery_suggestion
                        .clone()
                        .unwrap_or_else(|| String::from("Press Ctrl+R to try again"))
                )),
                Line::from(""),
                Line::from("Press Ctrl+R to rerun the setup flow."),
            ]);
        }

        if let Some(active_call) = &self.setup.active_call {
            let mut lines = Vec::new();
            if let Some(last_call) = self.setup.calls.last() {
                lines.extend([
                    Line::from(format!("Last step: {}", last_call.title)),
                    Line::from(last_call.response_text.clone()),
                    Line::from(""),
                ]);
            }
            lines.extend([
                Line::from(format!(
                    "Working through Apple FM proof {}/{}: {}",
                    active_call.index, active_call.total_calls, active_call.title
                )),
                Line::from(""),
                Line::from(format!("now: {}", active_call.title)),
                Line::from("prompt"),
                Line::from(active_call.prompt.clone()),
                Line::from(""),
                Line::from("reply"),
                Line::from("[waiting for Apple FM reply]"),
            ]);
            return Text::from(lines);
        }

        if let Some(last_call) = self.setup.calls.last() {
            let mut lines = vec![
                Line::from(format!("Latest proof step: {}", last_call.title)),
                Line::from(""),
                Line::from("reply"),
                Line::from(last_call.response_text.clone()),
            ];
            lines.extend(last_call.usage.render_lines());
            return Text::from(lines);
        }

        match self.setup.phase {
            TaskPhase::Queued => Text::from(vec![
                Line::from("Apple FM setup is queued."),
                Line::from(""),
                Line::from("Probe will check availability before issuing any inference."),
            ]),
            TaskPhase::CheckingAvailability => Text::from(vec![
                Line::from("Checking whether Apple FM is available on this machine."),
                Line::from(""),
                Line::from("No inference requests will be issued until this gate passes."),
            ]),
            TaskPhase::Unavailable => Text::from(vec![
                Line::from("Apple FM is not ready on this machine."),
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
                Line::from("Press Ctrl+R to rerun the check after the machine is admitted."),
            ]),
            TaskPhase::Completed => Text::from(vec![
                Line::from("Apple FM setup completed successfully."),
                Line::from(""),
                Line::from("Esc returns to chat. This overlay keeps the proof details nearby."),
            ]),
            TaskPhase::Idle | TaskPhase::Running | TaskPhase::Failed => Text::from(vec![
                Line::from("Run the Apple FM setup proof when you need it."),
                Line::from(""),
                Line::from("Press Ctrl+R to start or rerun the Apple FM setup flow."),
            ]),
        }
    }

    fn render_backend_lines(&self) -> Vec<String> {
        let Some(backend) = &self.setup.backend else {
            return vec![
                String::from("base_url: pending"),
                String::from("model: pending"),
            ];
        };
        vec![
            format!("base_url: {}", backend.base_url),
            format!("model: {}", backend.model_id),
        ]
    }

    fn render_availability_lines(&self) -> Vec<String> {
        let Some(availability) = &self.setup.availability else {
            if let Some(failure) = &self.setup.failure {
                return vec![
                    format!(
                        "availability: {}",
                        failure
                            .reason_code
                            .clone()
                            .unwrap_or_else(|| String::from("transport_or_unknown"))
                    ),
                    format!("message: {}", preview(failure.detail.as_str(), 56)),
                ];
            }
            return vec![
                String::from("availability: pending"),
                String::from("message: waiting for /health"),
            ];
        };
        let mut lines = vec![format!("availability: {}", availability.ready)];
        if let Some(message) = availability.availability_message.as_deref() {
            lines.push(format!("message: {}", preview(message, 56)));
        }
        if !availability.ready {
            lines.push(format!(
                "reason: {}",
                availability
                    .unavailable_reason
                    .clone()
                    .unwrap_or_else(|| String::from("none"))
            ));
        }
        if availability.platform.is_some() || availability.version.is_some() {
            lines.push(format!(
                "platform: {} {}",
                availability.platform.as_deref().unwrap_or("unknown"),
                availability.version.as_deref().unwrap_or("")
            ));
        }
        lines
    }

    fn render_codex_auth_lines(&self) -> Vec<String> {
        let Some(probe_home) = self.probe_home.as_ref() else {
            return vec![
                String::from("status: unavailable"),
                String::from("reason: no probe_home configured for this lane"),
            ];
        };
        let store = OpenAiCodexAuthStore::new(probe_home);
        match store.status() {
            Ok(status) if status.authenticated => vec![
                String::from("status: connected"),
                format!("path: {}", status.path.display()),
                format!(
                    "account: {}",
                    status.account_id.as_deref().unwrap_or("none")
                ),
                format!("expired: {}", status.expired),
                String::from("manage: `probe codex status` / `probe codex logout`"),
            ],
            Ok(status) => vec![
                String::from("status: disconnected"),
                format!("path: {}", status.path.display()),
                String::from("connect: `probe codex login --method browser`"),
            ],
            Err(error) => vec![String::from("status: error"), format!("detail: {error}")],
        }
    }

    fn codex_auth_doctor_line(&self) -> String {
        let Some(probe_home) = self.probe_home.as_ref() else {
            return String::from("action - no probe_home configured for this lane");
        };
        let store = OpenAiCodexAuthStore::new(probe_home);
        match store.status() {
            Ok(status) if status.authenticated && !status.expired => {
                String::from("ok - connected and ready")
            }
            Ok(status) if status.authenticated => {
                String::from("action - authenticated but expired")
            }
            Ok(_) => String::from("action - not authenticated"),
            Err(error) => format!("action - {error}"),
        }
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

    fn session_panel_title(&self) -> String {
        if self.has_pending_review_changes() {
            return String::from("Review Changes");
        }
        if !self.runtime.pending_approvals.is_empty() {
            return String::from("Action Needed");
        }
        match self.runtime_activity_kind() {
            Some(RuntimeActivityKind::Failed | RuntimeActivityKind::Stopped) => {
                String::from("Needs Review")
            }
            Some(
                RuntimeActivityKind::Queued
                | RuntimeActivityKind::Starting
                | RuntimeActivityKind::WaitingForBackend
                | RuntimeActivityKind::StreamingReply
                | RuntimeActivityKind::UpdatingReply
                | RuntimeActivityKind::PlanningTool
                | RuntimeActivityKind::Reading
                | RuntimeActivityKind::Editing
                | RuntimeActivityKind::Validating
                | RuntimeActivityKind::RunningTool
                | RuntimeActivityKind::WaitingForApproval
                | RuntimeActivityKind::Finalizing,
            ) => String::from("Working"),
            _ if self.transcript.is_empty() => String::from("Ready"),
            _ => String::from("Session"),
        }
    }

    fn setup_overlay_hint(&self) -> &'static str {
        match self.setup.phase {
            TaskPhase::Queued | TaskPhase::CheckingAvailability | TaskPhase::Running => {
                "Esc returns to chat while setup keeps running"
            }
            TaskPhase::Unavailable | TaskPhase::Failed => "Ctrl+R reruns setup",
            TaskPhase::Idle | TaskPhase::Completed => "Esc returns to chat",
        }
    }

    fn remote_backend_overlay_hint(
        &self,
        backend_kind: probe_protocol::backend::BackendKind,
    ) -> &'static str {
        match backend_kind {
            probe_protocol::backend::BackendKind::OpenAiCodexSubscription => {
                "Esc returns to chat; Tab changes lanes there"
            }
            probe_protocol::backend::BackendKind::OpenAiChatCompletions
            | probe_protocol::backend::BackendKind::AppleFmBridge => {
                "Esc returns to chat; Ctrl+S reopens this view"
            }
        }
    }

    fn resume_transcript_follow(&mut self) {
        self.transcript_follow_latest = true;
        self.transcript_scroll_from_bottom = 0;
    }

    fn transcript_panel_title(&self) -> String {
        if self.transcript.is_empty() {
            return String::from("Start Here");
        }
        if self.transcript_follow_latest || self.transcript_scroll_from_bottom == 0 {
            if self.has_pending_review_changes() {
                return String::from("Review Changes");
            }
            if !self.runtime.pending_approvals.is_empty() {
                return String::from("Action Needed");
            }
            return match self.runtime_activity_kind() {
                Some(
                    RuntimeActivityKind::Queued
                    | RuntimeActivityKind::Starting
                    | RuntimeActivityKind::WaitingForBackend
                    | RuntimeActivityKind::StreamingReply
                    | RuntimeActivityKind::UpdatingReply
                    | RuntimeActivityKind::PlanningTool
                    | RuntimeActivityKind::Reading
                    | RuntimeActivityKind::Editing
                    | RuntimeActivityKind::Validating
                    | RuntimeActivityKind::RunningTool
                    | RuntimeActivityKind::Finalizing,
                ) => String::from("Working"),
                Some(RuntimeActivityKind::Failed | RuntimeActivityKind::Stopped) => {
                    String::from("Needs Review")
                }
                _ => String::from("Transcript"),
            };
        }
        format!("Transcript · {} below", self.transcript_scroll_from_bottom)
    }

    fn has_pending_review_changes(&self) -> bool {
        self.pending_review_paths()
            .is_some_and(|paths| !paths.is_empty())
    }

    fn pending_review_paths(&self) -> Option<Vec<String>> {
        let approval = self.runtime.pending_approvals.first()?;
        if let Some(proposed) = approval.proposed_edit.as_ref() {
            if proposed.changed_files.is_empty() {
                return None;
            }
            return Some(proposed.changed_files.clone());
        }
        if approval.tool_name != "apply_patch" {
            return None;
        }
        approval
            .arguments
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(|path| vec![path.to_string()])
    }

    fn sync_transcript_scroll_after_update(&mut self) {
        let line_count = self.render_primary_body().lines.len();
        if self.transcript_follow_latest {
            self.transcript_scroll_from_bottom = 0;
        } else if line_count > self.transcript_line_count {
            let added = line_count.saturating_sub(self.transcript_line_count);
            self.transcript_scroll_from_bottom = self
                .transcript_scroll_from_bottom
                .saturating_add(added.min(u16::MAX as usize) as u16)
                .min(self.max_transcript_scroll_from_bottom_for_line_count(line_count));
        } else {
            self.transcript_scroll_from_bottom = self
                .transcript_scroll_from_bottom
                .min(self.max_transcript_scroll_from_bottom_for_line_count(line_count));
        }
        self.transcript_line_count = line_count;
    }

    fn snap_transcript_to_latest(&mut self) {
        self.sync_transcript_scroll_after_update();
    }

    fn scroll_transcript_up(&mut self, amount: u16) {
        let max = self.max_transcript_scroll_from_bottom();
        self.transcript_scroll_from_bottom = self
            .transcript_scroll_from_bottom
            .saturating_add(amount)
            .min(max);
        self.transcript_follow_latest = self.transcript_scroll_from_bottom == 0;
        self.transcript_line_count = self.render_primary_body().lines.len();
    }

    fn scroll_transcript_down(&mut self, amount: u16) {
        self.transcript_scroll_from_bottom =
            self.transcript_scroll_from_bottom.saturating_sub(amount);
        self.transcript_follow_latest = self.transcript_scroll_from_bottom == 0;
        self.transcript_line_count = self.render_primary_body().lines.len();
    }

    fn max_transcript_scroll_from_bottom_for_line_count(&self, line_count: usize) -> u16 {
        line_count.saturating_sub(1).min(u16::MAX as usize) as u16
    }

    fn max_transcript_scroll_from_bottom(&self) -> u16 {
        self.max_transcript_scroll_from_bottom_for_line_count(
            self.render_primary_body().lines.len(),
        )
    }

    fn transcript_scroll_y(&self, line_count: usize, panel_height: u16) -> u16 {
        let viewport_height = panel_height.saturating_sub(2) as usize;
        let max_top_scroll = line_count.saturating_sub(viewport_height);
        let from_bottom = usize::from(
            self.transcript_scroll_from_bottom
                .min(max_top_scroll.min(u16::MAX as usize) as u16),
        );
        max_top_scroll
            .saturating_sub(from_bottom)
            .min(u16::MAX as usize) as u16
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
            Line::from("Move"),
            Line::from(""),
            Line::from("Tab / Shift+Tab   lanes / Codex effort"),
            Line::from("PgUp / PgDn       scroll transcript"),
            Line::from("Up / Down         draft history"),
            Line::from(""),
            Line::from("Act"),
            Line::from(""),
            Line::from("Enter / Ctrl+J    send / newline"),
            Line::from("Ctrl+A            review approval"),
            Line::from("Ctrl+G            git workflow"),
            Line::from("Ctrl+R            run Apple FM check"),
            Line::from("Ctrl+O / Ctrl+T   attachment / notes"),
            Line::from(""),
            Line::from("Inspect"),
            Line::from(""),
            Line::from("F2 / F3           status / doctor"),
            Line::from("F4 / F5           tasks / recipes"),
            Line::from("Ctrl+S            backend details"),
            Line::from("F1 / Esc          close help"),
            Line::from("Ctrl+C            quit"),
            Line::from(""),
            Line::from(format!("stack depth: {stack_depth}")),
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
                String::from("dismissed backend overlay"),
            ),
            UiEvent::RunBackgroundTask => ScreenOutcome::with_command(
                String::from("requested backend check"),
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
        ModalCard::new("Backend", content).render(frame, area);
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
            Self::Approve => "apply",
            Self::Reject => "reject",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalOverlay {
    selected: ApprovalChoice,
    approval: PendingToolApproval,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ApprovalPreview {
    title: &'static str,
    summary: String,
    files: Vec<String>,
    preview_label: &'static str,
    preview_lines: Vec<String>,
    validation_line: String,
    primary_action_label: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPickerOverlay {
    backend_label: String,
    current_model: String,
    models: Vec<String>,
    selected: usize,
}

impl ModelPickerOverlay {
    pub fn new(
        backend_label: impl Into<String>,
        current_model: impl Into<String>,
        models: Vec<String>,
    ) -> Self {
        let backend_label = backend_label.into();
        let current_model = current_model.into();
        let selected = models
            .iter()
            .position(|model| model == &current_model)
            .unwrap_or(0);
        Self {
            backend_label,
            current_model,
            models,
            selected,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView => {
                if self.selected == 0 {
                    self.selected = self.models.len().saturating_sub(1);
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected model {}", self.models[self.selected]),
                )
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView => {
                self.selected = (self.selected + 1) % self.models.len();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected model {}", self.models[self.selected]),
                )
            }
            UiEvent::ComposerSubmit => ScreenOutcome::with_command(
                format!("selected model {}", self.models[self.selected]),
                ScreenCommand::SelectActiveBackendModel {
                    model_id: self.models[self.selected].clone(),
                },
            ),
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed model picker"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = vec![
            Line::from("Choose the model for the active backend."),
            Line::from(""),
            Line::from(format!("backend: {}", self.backend_label)),
            Line::from(format!("current: {}", self.current_model)),
            Line::from("next turn: starts with the selected model"),
            Line::from(""),
            Line::from("Use Up/Down to choose. Enter applies. Esc closes."),
            Line::from(""),
        ];
        lines.extend(self.models.iter().enumerate().map(|(index, model)| {
            let selected = index == self.selected;
            let marker = if selected { ">" } else { " " };
            let suffix = if model == &self.current_model {
                "  current"
            } else {
                ""
            };
            Line::from(format!("{marker} {model}{suffix}"))
        }));
        let content = Paragraph::new(Text::from(lines));
        ModalCard::new("Model", content).render(frame, area);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanModeChoice {
    Coding,
    Plan,
}

impl PlanModeChoice {
    fn next(self) -> Self {
        match self {
            Self::Coding => Self::Plan,
            Self::Plan => Self::Coding,
        }
    }

    fn previous(self) -> Self {
        self.next()
    }

    fn label(self) -> &'static str {
        match self {
            Self::Coding => "coding",
            Self::Plan => "plan",
        }
    }

    fn enabled(self) -> bool {
        matches!(self, Self::Plan)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanModeOverlay {
    current: PlanModeChoice,
    selected: PlanModeChoice,
}

impl PlanModeOverlay {
    pub fn new(current_plan_enabled: bool) -> Self {
        let current = if current_plan_enabled {
            PlanModeChoice::Plan
        } else {
            PlanModeChoice::Coding
        };
        Self {
            current,
            selected: current,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView => {
                self.selected = self.selected.previous();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {} mode", self.selected.label()),
                )
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView => {
                self.selected = self.selected.next();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {} mode", self.selected.label()),
                )
            }
            UiEvent::ComposerSubmit => ScreenOutcome::with_command(
                format!("selected {} mode", self.selected.label()),
                ScreenCommand::SetActivePlanMode {
                    enabled: self.selected.enabled(),
                },
            ),
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed plan mode picker"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let coding_marker = if self.selected == PlanModeChoice::Coding {
            ">"
        } else {
            " "
        };
        let plan_marker = if self.selected == PlanModeChoice::Plan {
            ">"
        } else {
            " "
        };
        let current = self.current.label();
        let next = match self.selected {
            PlanModeChoice::Coding => {
                "Normal coding mode. The next turn can edit files and use the usual coding tool posture."
            }
            PlanModeChoice::Plan => {
                "Plan-first mode. The next turn should focus on sequencing, risks, and implementation strategy without implying edits already landed."
            }
        };
        let content = Paragraph::new(Text::from(vec![
            Line::from("Choose how Probe should behave on the next turn."),
            Line::from(""),
            Line::from(format!("current mode: {current}")),
            Line::from(""),
            Line::from(format!("{coding_marker} coding")),
            Line::from("  best when you want Probe to make or verify real repo changes"),
            Line::from(format!("{plan_marker} plan")),
            Line::from("  best when you want Probe to think first and avoid edit-like claims"),
            Line::from(""),
            Line::from(format!("next: {next}")),
            Line::from("Use Up/Down to choose. Enter applies. Esc closes."),
        ]))
        .wrap(Wrap { trim: false });
        ModalCard::new("Plan Mode", content).render(frame, area);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackgroundModeChoice {
    Foreground,
    Background,
    Delegate,
}

impl BackgroundModeChoice {
    fn next(self) -> Self {
        match self {
            Self::Foreground => Self::Background,
            Self::Background => Self::Delegate,
            Self::Delegate => Self::Foreground,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Foreground => Self::Delegate,
            Self::Background => Self::Foreground,
            Self::Delegate => Self::Background,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::Background => "background",
            Self::Delegate => "delegate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundModeOverlay {
    current: BackgroundModeChoice,
    selected: BackgroundModeChoice,
}

impl BackgroundModeOverlay {
    pub fn new(current: &str) -> Self {
        let current = match current {
            "background" => BackgroundModeChoice::Background,
            "delegate" => BackgroundModeChoice::Delegate,
            _ => BackgroundModeChoice::Foreground,
        };
        Self {
            current,
            selected: current,
        }
    }

    pub fn with_selected(current: &str, selected: &str) -> Self {
        let mut overlay = Self::new(current);
        overlay.selected = match selected {
            "background" => BackgroundModeChoice::Background,
            "delegate" => BackgroundModeChoice::Delegate,
            _ => BackgroundModeChoice::Foreground,
        };
        overlay
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView => {
                self.selected = self.selected.previous();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {} launch mode", self.selected.label()),
                )
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView => {
                self.selected = self.selected.next();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {} launch mode", self.selected.label()),
                )
            }
            UiEvent::ComposerSubmit => ScreenOutcome::with_command(
                format!("selected {} launch mode", self.selected.label()),
                ScreenCommand::SetActiveLaunchMode {
                    mode_label: self.selected.label().to_string(),
                },
            ),
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed background mode picker"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let foreground_marker = if self.selected == BackgroundModeChoice::Foreground {
            ">"
        } else {
            " "
        };
        let background_marker = if self.selected == BackgroundModeChoice::Background {
            ">"
        } else {
            " "
        };
        let delegate_marker = if self.selected == BackgroundModeChoice::Delegate {
            ">"
        } else {
            " "
        };
        let next = match self.selected {
            BackgroundModeChoice::Foreground => {
                "Next turns stay on this lane and stream their progress here."
            }
            BackgroundModeChoice::Background => {
                "Next turns start as detached tasks. Probe will hand you back a task receipt and /tasks will reopen it later."
            }
            BackgroundModeChoice::Delegate => {
                "Next turns become child tasks of this lane's current session. Use this when you want a subtask to run in parallel but keep the parent/child relationship visible."
            }
        };
        let content = Paragraph::new(Text::from(vec![
            Line::from("Choose how the next turn should run on this lane."),
            Line::from(""),
            Line::from(format!("current launch: {}", self.current.label())),
            Line::from(""),
            Line::from(format!("{foreground_marker} foreground")),
            Line::from("  best when you want to watch the turn live in this shell"),
            Line::from(format!("{background_marker} background")),
            Line::from("  best when you want Probe to queue detached work and hand control back right away"),
            Line::from(format!("{delegate_marker} delegate")),
            Line::from("  best when you want a child task linked to the current session"),
            Line::from(""),
            Line::from(format!("next: {next}")),
            Line::from("Use Up/Down to choose. Enter applies. Esc closes."),
        ]))
        .wrap(Wrap { trim: false });
        ModalCard::new("Launch Mode", content).render(frame, area);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewModeChoice {
    AutoSafe,
    ReviewRisky,
    ReviewAll,
}

impl ReviewModeChoice {
    fn next(self) -> Self {
        match self {
            Self::AutoSafe => Self::ReviewRisky,
            Self::ReviewRisky => Self::ReviewAll,
            Self::ReviewAll => Self::AutoSafe,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::AutoSafe => Self::ReviewAll,
            Self::ReviewRisky => Self::AutoSafe,
            Self::ReviewAll => Self::ReviewRisky,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::AutoSafe => "auto-safe",
            Self::ReviewRisky => "review-risky",
            Self::ReviewAll => "review-all",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::AutoSafe => {
                "Fastest path. Probe can apply write-capable tools immediately when its local approval posture allows them."
            }
            Self::ReviewRisky => {
                "Current Probe behavior: write, network, and destructive actions pause for approval before they land."
            }
            Self::ReviewAll => {
                "Strictest current Probe behavior: all write-capable changes pause for approval, even when the task looks routine."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewModeOverlay {
    current: ReviewModeChoice,
    selected: ReviewModeChoice,
}

impl ReviewModeOverlay {
    pub fn new(current: &str) -> Self {
        let current = match current {
            "review-risky" => ReviewModeChoice::ReviewRisky,
            "review-all" => ReviewModeChoice::ReviewAll,
            _ => ReviewModeChoice::AutoSafe,
        };
        Self {
            current,
            selected: current,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView => {
                self.selected = self.selected.previous();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {} review mode", self.selected.label()),
                )
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView => {
                self.selected = self.selected.next();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {} review mode", self.selected.label()),
                )
            }
            UiEvent::ComposerSubmit => ScreenOutcome::with_command(
                format!("selected {} review mode", self.selected.label()),
                ScreenCommand::SetActiveReviewMode {
                    mode_label: self.selected.label().to_string(),
                },
            ),
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed review mode picker"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = vec![
            Line::from("Choose how Probe should treat write-capable work on this lane."),
            Line::from(""),
            Line::from(format!("current mode: {}", self.current.label())),
            Line::from(""),
        ];
        for choice in [
            ReviewModeChoice::AutoSafe,
            ReviewModeChoice::ReviewRisky,
            ReviewModeChoice::ReviewAll,
        ] {
            let marker = if self.selected == choice { ">" } else { " " };
            let suffix = if self.current == choice {
                "  current"
            } else {
                ""
            };
            lines.push(Line::from(format!("{marker} {}{suffix}", choice.label())));
            lines.push(Line::from(format!("  {}", choice.detail())));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(
            "Use Up/Down to choose. Enter applies. Esc closes.",
        ));
        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new("Review Mode", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningPickerOverlay {
    backend_label: String,
    current_level: String,
    levels: Vec<String>,
    selected: usize,
}

impl ReasoningPickerOverlay {
    pub fn new(
        backend_label: impl Into<String>,
        current_level: impl Into<String>,
        levels: Vec<String>,
    ) -> Self {
        let backend_label = backend_label.into();
        let current_level = current_level.into();
        let selected = levels
            .iter()
            .position(|level| level == &current_level)
            .unwrap_or(0);
        Self {
            backend_label,
            current_level,
            levels,
            selected,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView => {
                if self.selected == 0 {
                    self.selected = self.levels.len().saturating_sub(1);
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected reasoning {}", self.levels[self.selected]),
                )
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView => {
                self.selected = (self.selected + 1) % self.levels.len();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected reasoning {}", self.levels[self.selected]),
                )
            }
            UiEvent::ComposerSubmit => ScreenOutcome::with_command(
                format!("selected reasoning {}", self.levels[self.selected]),
                ScreenCommand::SelectActiveReasoningLevel {
                    level: self.levels[self.selected].clone(),
                },
            ),
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed reasoning picker"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = vec![
            Line::from("Choose the reasoning effort for the active backend."),
            Line::from(""),
            Line::from(format!("backend: {}", self.backend_label)),
            Line::from(format!("current: {}", self.current_level)),
            Line::from("next turn: starts with the selected reasoning level"),
            Line::from(""),
            Line::from("Use Up/Down to choose. Enter applies. Esc closes."),
            Line::from(""),
        ];
        lines.extend(self.levels.iter().enumerate().map(|(index, level)| {
            let selected = index == self.selected;
            let marker = if selected { ">" } else { " " };
            let suffix = if level == &self.current_level {
                "  current"
            } else {
                ""
            };
            Line::from(format!("{marker} {level}{suffix}"))
        }));
        let content = Paragraph::new(Text::from(lines));
        ModalCard::new("Reasoning", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MemoryMenuEntry {
    label: String,
    detail: String,
    preview_title: String,
    preview_path: Option<PathBuf>,
    preview_lines: Vec<String>,
    command: Option<ScreenCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryOverlay {
    stack: ProbeMemoryStack,
    selected: usize,
    preview_scroll: usize,
}

impl MemoryOverlay {
    pub fn new(stack: ProbeMemoryStack) -> Self {
        Self {
            stack,
            selected: 0,
            preview_scroll: 0,
        }
    }

    fn menu_entries(&self) -> Vec<MemoryMenuEntry> {
        let mut entries = vec![
            self.scope_entry(MemoryScope::User),
            self.scope_entry(MemoryScope::Repo),
            self.scope_entry(MemoryScope::Directory),
        ];
        entries.extend(self.stack.layers.iter().map(|layer| {
            let mut preview_lines = preview_block_lines(layer.body.as_str(), 14);
            let suffix = if layer.truncated {
                " (truncated preview)"
            } else {
                ""
            };
            MemoryMenuEntry {
                label: format!("Edit loaded layer: {}{}", layer.label, suffix),
                detail: layer.path.display().to_string(),
                preview_title: format!("loaded layer preview: {}", layer.label),
                preview_path: Some(layer.path.clone()),
                preview_lines: {
                    if let Some(recovery) = self.recovery_hint_for_layer_path(layer.path.as_path())
                    {
                        preview_lines.push(String::new());
                        preview_lines.push(format!("recovery: {recovery}"));
                    }
                    preview_lines
                },
                command: Some(ScreenCommand::OpenMemoryEditor {
                    label: layer.label.clone(),
                    path: layer.path.clone(),
                }),
            }
        }));
        entries
    }

    fn recovery_hint_for_layer_path(&self, path: &Path) -> Option<String> {
        if self
            .stack
            .suggested_user_path
            .as_ref()
            .is_some_and(|candidate| candidate == path)
        {
            return self.stack.recovery_hint_for_scope(MemoryScope::User);
        }
        if self
            .stack
            .suggested_repo_path
            .as_ref()
            .is_some_and(|candidate| candidate == path)
        {
            return self.stack.recovery_hint_for_scope(MemoryScope::Repo);
        }
        if self
            .stack
            .suggested_directory_path
            .as_ref()
            .is_some_and(|candidate| candidate == path)
        {
            return self.stack.recovery_hint_for_scope(MemoryScope::Directory);
        }
        None
    }

    fn scope_entry(&self, scope: MemoryScope) -> MemoryMenuEntry {
        let existing_layer = self.stack.layer_for_scope(scope);
        let editable_path = self.stack.editable_path_for_scope(scope);
        let exists =
            existing_layer.is_some() || editable_path.as_ref().is_some_and(|path| path.exists());
        let label = if exists {
            format!("Edit {}", scope.label())
        } else {
            format!("Create {}", scope.label())
        };
        let detail = editable_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| match scope {
                MemoryScope::User => {
                    String::from("Probe needs a probe_home before user memory can be managed here.")
                }
                MemoryScope::Repo => {
                    String::from("Repo memory is only available inside a git workspace.")
                }
                MemoryScope::Directory => String::from(
                    "Folder memory is only available inside a subdirectory of the active repo.",
                ),
            });
        let mut preview_lines = if let Some(layer) = existing_layer {
            preview_block_lines(layer.body.as_str(), 14)
        } else if let Some(path) = editable_path.as_ref() {
            vec![format!(
                "Probe will create {} at {}.",
                scope.label(),
                path.display()
            )]
        } else {
            vec![detail.clone()]
        };
        if let Some(recovery) = self.stack.recovery_hint_for_scope(scope) {
            preview_lines.push(String::new());
            preview_lines.push(format!("recovery: {recovery}"));
        }
        MemoryMenuEntry {
            label,
            detail,
            preview_title: if exists {
                format!("current {}", scope.label())
            } else {
                format!("new {}", scope.label())
            },
            preview_path: editable_path.clone(),
            preview_lines,
            command: editable_path.map(|path| ScreenCommand::OpenMemoryEditor {
                label: scope.label().to_string(),
                path,
            }),
        }
    }

    fn selected_entry(&self) -> Option<MemoryMenuEntry> {
        let entries = self.menu_entries();
        entries
            .get(self.selected.min(entries.len().saturating_sub(1)))
            .cloned()
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        let entry_count = self.menu_entries().len();
        match event {
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView => {
                if entry_count == 0 {
                    return ScreenOutcome::idle();
                }
                if self.selected == 0 {
                    self.selected = entry_count.saturating_sub(1);
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
                self.preview_scroll = 0;
                let label = self
                    .selected_entry()
                    .map(|entry| entry.label)
                    .unwrap_or_else(|| String::from("memory entry"));
                ScreenOutcome::with_status(ScreenAction::None, format!("selected {label}"))
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView => {
                if entry_count == 0 {
                    return ScreenOutcome::idle();
                }
                self.selected = (self.selected + 1) % entry_count;
                self.preview_scroll = 0;
                let label = self
                    .selected_entry()
                    .map(|entry| entry.label)
                    .unwrap_or_else(|| String::from("memory entry"));
                ScreenOutcome::with_status(ScreenAction::None, format!("selected {label}"))
            }
            UiEvent::ScrollUp => {
                self.preview_scroll = self
                    .preview_scroll
                    .saturating_sub(LINE_SCROLL_STEP as usize);
                ScreenOutcome::idle()
            }
            UiEvent::ScrollDown => {
                self.preview_scroll = self
                    .preview_scroll
                    .saturating_add(LINE_SCROLL_STEP as usize);
                ScreenOutcome::idle()
            }
            UiEvent::PageUp => {
                self.preview_scroll = self
                    .preview_scroll
                    .saturating_sub(PAGE_SCROLL_STEP as usize);
                ScreenOutcome::idle()
            }
            UiEvent::PageDown => {
                self.preview_scroll = self
                    .preview_scroll
                    .saturating_add(PAGE_SCROLL_STEP as usize);
                ScreenOutcome::idle()
            }
            UiEvent::ComposerSubmit => {
                let Some(entry) = self.selected_entry() else {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("there is no memory entry to open yet"),
                    );
                };
                let Some(command) = entry.command else {
                    return ScreenOutcome::with_status(ScreenAction::None, entry.detail);
                };
                ScreenOutcome::with_command(format!("opening {}", entry.label), command)
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed memory overlay"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let entries = self.menu_entries();
        let selected_index = self.selected.min(entries.len().saturating_sub(1));
        let selected_entry = entries.get(selected_index);
        let mut lines = vec![
            Line::from(
                "Inspect and manage the memory and rules Probe will carry into the next turn.",
            ),
            Line::from(
                "precedence: folder memory overrides repo memory, and repo memory overrides user memory.",
            ),
            Line::from(""),
            Line::from(format!("active memory: {}", self.stack.active_label())),
        ];
        if let Some(issue) = self.stack.first_issue_line() {
            lines.push(Line::from(format!("issue: {}", preview(issue, 120))));
            lines.push(Line::from(
                "recovery: open the relevant memory file here and save valid UTF-8 markdown text.",
            ));
        }

        lines.push(Line::from(""));
        lines.push(Line::from("actions"));
        for (index, entry) in entries.iter().take(3).enumerate() {
            let marker = if index == selected_index { ">" } else { " " };
            lines.push(Line::from(format!(
                "{marker} {}",
                preview(entry.label.as_str(), 100)
            )));
            lines.push(Line::from(format!(
                "    {}",
                preview(entry.detail.as_str(), 110)
            )));
        }

        if entries.len() > 3 {
            lines.push(Line::from(""));
            lines.push(Line::from("loaded layers"));
            for (offset, entry) in entries.iter().enumerate().skip(3) {
                let marker = if offset == selected_index { ">" } else { " " };
                lines.push(Line::from(format!(
                    "{marker} {}",
                    preview(entry.label.as_str(), 100)
                )));
            }
        }

        let reserved_rows = entries.len().min(9) + 11;
        let visible_body_lines =
            usize::from(area.height.saturating_sub(reserved_rows as u16)).max(6);
        if let Some(entry) = selected_entry {
            let total_lines = entry.preview_lines.len();
            let start = self
                .preview_scroll
                .min(total_lines.saturating_sub(visible_body_lines));
            let end = (start + visible_body_lines).min(total_lines);
            lines.push(Line::from(""));
            lines.push(Line::from(entry.preview_title.clone()));
            if let Some(path) = entry.preview_path.as_ref() {
                lines.push(Line::from(format!("path: {}", path.display())));
            }
            if total_lines > visible_body_lines {
                lines.push(Line::from(format!(
                    "scroll: {}-{} of {}",
                    start + 1,
                    end,
                    total_lines
                )));
            }
            lines.push(Line::from(""));
            if entry.preview_lines.is_empty() {
                lines.push(Line::from("[no preview available]"));
            } else {
                for line in entry.preview_lines[start..end].iter() {
                    lines.push(Line::from(line.clone()));
                }
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(
            "Use Up/Down to choose an action or layer. Enter opens it. PgUp/PgDn scroll. Esc closes.",
        ));

        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new("Memory", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEditorOverlay {
    label: String,
    path: PathBuf,
    body: String,
    cursor: usize,
    scroll: usize,
    existed: bool,
    load_note: Option<String>,
}

impl MemoryEditorOverlay {
    pub fn new(
        label: impl Into<String>,
        path: PathBuf,
        body: String,
        existed: bool,
        load_note: Option<String>,
    ) -> Self {
        let cursor = body.len();
        Self {
            label: label.into(),
            path,
            body,
            cursor,
            scroll: 0,
            existed,
            load_note,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerInsert(ch) => {
                self.body.insert(self.cursor, ch);
                self.cursor += ch.len_utf8();
                self.ensure_cursor_visible();
                ScreenOutcome::idle()
            }
            UiEvent::ComposerPaste(payload) => {
                self.body.insert_str(self.cursor, payload.as_str());
                self.cursor += payload.len();
                self.ensure_cursor_visible();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!(
                        "pasted {} chars into {}",
                        payload.chars().count(),
                        self.label
                    ),
                )
            }
            UiEvent::ComposerNewline => {
                self.body.insert(self.cursor, '\n');
                self.cursor += 1;
                self.ensure_cursor_visible();
                ScreenOutcome::idle()
            }
            UiEvent::ComposerBackspace => {
                if let Some(previous) = previous_char_boundary(self.body.as_str(), self.cursor) {
                    self.body.drain(previous..self.cursor);
                    self.cursor = previous;
                    self.ensure_cursor_visible();
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerDelete => {
                if let Some(next) = next_char_boundary(self.body.as_str(), self.cursor) {
                    self.body.drain(self.cursor..next);
                    self.ensure_cursor_visible();
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveLeft => {
                if let Some(previous) = previous_char_boundary(self.body.as_str(), self.cursor) {
                    self.cursor = previous;
                    self.ensure_cursor_visible();
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveRight => {
                if let Some(next) = next_char_boundary(self.body.as_str(), self.cursor) {
                    self.cursor = next;
                    self.ensure_cursor_visible();
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveHome => {
                self.cursor = line_start(self.body.as_str(), self.cursor);
                self.ensure_cursor_visible();
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveEnd => {
                self.cursor = line_end(self.body.as_str(), self.cursor);
                self.ensure_cursor_visible();
                ScreenOutcome::idle()
            }
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView => {
                self.cursor = move_cursor_vertical(self.body.as_str(), self.cursor, -1);
                self.ensure_cursor_visible();
                ScreenOutcome::idle()
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView => {
                self.cursor = move_cursor_vertical(self.body.as_str(), self.cursor, 1);
                self.ensure_cursor_visible();
                ScreenOutcome::idle()
            }
            UiEvent::PageUp => {
                self.scroll = self.scroll.saturating_sub(PAGE_SCROLL_STEP as usize);
                ScreenOutcome::idle()
            }
            UiEvent::PageDown => {
                self.scroll = self.scroll.saturating_add(PAGE_SCROLL_STEP as usize);
                ScreenOutcome::idle()
            }
            UiEvent::ComposerSubmit => {
                if self.body.trim().is_empty() {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("memory files should not be empty; add text or Esc to cancel"),
                    );
                }
                ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    format!("saved {}", self.label),
                    ScreenCommand::SaveMemoryFile {
                        label: self.label.clone(),
                        path: self.path.clone(),
                        body: self.body.clone(),
                    },
                )
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                format!("dismissed {}", self.label),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn ensure_cursor_visible(&mut self) {
        let cursor_line = cursor_line_index(self.body.as_str(), self.cursor);
        if cursor_line < self.scroll {
            self.scroll = cursor_line;
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = vec![
            Line::from(if self.existed {
                "Edit this memory file from inside Probe."
            } else {
                "Create this memory file from inside Probe."
            }),
            Line::from(format!("target: {}", self.label)),
            Line::from(format!("path: {}", self.path.display())),
            Line::from(format!(
                "mode: {}",
                if self.existed {
                    "editing existing file"
                } else {
                    "creating a new file"
                }
            )),
        ];
        if let Some(note) = self.load_note.as_ref() {
            lines.push(Line::from(format!("note: {note}")));
        }
        lines.push(Line::from(
            "Type to edit. Ctrl+J inserts a newline. Left/Right/Home/End move. Up/Down move lines. Enter saves.",
        ));

        let rendered_lines = render_editor_lines(self.body.as_str(), self.cursor);
        let reserved_rows = 8;
        let visible_lines = usize::from(area.height.saturating_sub(reserved_rows)).max(8);
        let cursor_line = cursor_line_index(self.body.as_str(), self.cursor);
        let mut start = self
            .scroll
            .min(rendered_lines.len().saturating_sub(visible_lines));
        if cursor_line < start {
            start = cursor_line;
        } else if cursor_line >= start + visible_lines {
            start = cursor_line.saturating_sub(visible_lines.saturating_sub(1));
        }
        let end = (start + visible_lines).min(rendered_lines.len());

        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "editor: lines {}-{} of {}",
            start + 1,
            end,
            rendered_lines.len()
        )));
        lines.push(Line::from(""));
        for line in rendered_lines[start..end].iter() {
            lines.push(Line::from(line.clone()));
        }

        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new("Memory Editor", content).render(frame, area);
    }
}

fn preview_block_lines(body: &str, max_lines: usize) -> Vec<String> {
    let mut lines = body.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    if lines.is_empty() {
        return vec![String::from("[empty]")];
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        lines.push(String::from("..."));
    }
    lines
}

fn previous_char_boundary(value: &str, cursor: usize) -> Option<usize> {
    value[..cursor]
        .char_indices()
        .last()
        .map(|(index, _)| index)
}

fn next_char_boundary(value: &str, cursor: usize) -> Option<usize> {
    value[cursor..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| cursor + offset)
        .or_else(|| (cursor < value.len()).then_some(value.len()))
}

fn line_start(value: &str, cursor: usize) -> usize {
    value[..cursor]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0)
}

fn line_end(value: &str, cursor: usize) -> usize {
    value[cursor..]
        .find('\n')
        .map(|offset| cursor + offset)
        .unwrap_or(value.len())
}

fn cursor_line_index(value: &str, cursor: usize) -> usize {
    value[..cursor].chars().filter(|ch| *ch == '\n').count()
}

fn move_cursor_vertical(value: &str, cursor: usize, direction: isize) -> usize {
    let current_start = line_start(value, cursor);
    let current_column = value[current_start..cursor].chars().count();
    if direction < 0 {
        if current_start == 0 {
            return cursor;
        }
        let previous_line_end = current_start.saturating_sub(1);
        let previous_line_start = line_start(value, previous_line_end);
        return nth_char_boundary(value, previous_line_start, current_column)
            .min(previous_line_end);
    }
    let current_end = line_end(value, cursor);
    if current_end == value.len() {
        return cursor;
    }
    let next_line_start = current_end + 1;
    let next_line_end = line_end(value, next_line_start);
    nth_char_boundary(value, next_line_start, current_column).min(next_line_end)
}

fn nth_char_boundary(value: &str, start: usize, column: usize) -> usize {
    value[start..]
        .char_indices()
        .nth(column)
        .map(|(offset, _)| start + offset)
        .unwrap_or_else(|| line_end(value, start))
}

fn render_editor_lines(value: &str, cursor: usize) -> Vec<String> {
    let mut rendered = Vec::new();
    let mut line_start_index = 0usize;
    for (index, line) in value.split('\n').enumerate() {
        let cursor_in_line = cursor >= line_start_index && cursor <= line_start_index + line.len();
        let rendered_line = if cursor_in_line {
            let column = value[line_start_index..cursor].chars().count();
            insert_visual_cursor(line, column)
        } else {
            line.to_string()
        };
        rendered.push(format!("{:>3}: {}", index + 1, rendered_line));
        line_start_index += line.len() + 1;
    }
    if rendered.is_empty() {
        rendered.push(String::from("  1: |"));
    }
    rendered
}

fn insert_visual_cursor(line: &str, column: usize) -> String {
    let mut output = String::new();
    let mut inserted = false;
    for (index, ch) in line.chars().enumerate() {
        if index == column {
            output.push('|');
            inserted = true;
        }
        output.push(ch);
    }
    if !inserted {
        output.push('|');
    }
    output
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDiffFileView {
    pub path: String,
    pub diff_lines: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffOverlay {
    title: String,
    summary_lines: Vec<String>,
    files: Vec<TaskDiffFileView>,
    selected: usize,
    diff_scroll: usize,
}

impl DiffOverlay {
    pub fn new(
        title: impl Into<String>,
        summary_lines: Vec<String>,
        files: Vec<TaskDiffFileView>,
    ) -> Self {
        Self {
            title: title.into(),
            summary_lines,
            files,
            selected: 0,
            diff_scroll: 0,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView => {
                if self.files.is_empty() {
                    return ScreenOutcome::idle();
                }
                if self.selected == 0 {
                    self.selected = self.files.len().saturating_sub(1);
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
                self.diff_scroll = 0;
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected diff {}", self.files[self.selected].path),
                )
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView => {
                if self.files.is_empty() {
                    return ScreenOutcome::idle();
                }
                self.selected = (self.selected + 1) % self.files.len();
                self.diff_scroll = 0;
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected diff {}", self.files[self.selected].path),
                )
            }
            UiEvent::ScrollUp => {
                self.diff_scroll = self.diff_scroll.saturating_sub(LINE_SCROLL_STEP as usize);
                ScreenOutcome::idle()
            }
            UiEvent::ScrollDown => {
                self.diff_scroll = self.diff_scroll.saturating_add(LINE_SCROLL_STEP as usize);
                ScreenOutcome::idle()
            }
            UiEvent::PageUp => {
                self.diff_scroll = self.diff_scroll.saturating_sub(PAGE_SCROLL_STEP as usize);
                ScreenOutcome::idle()
            }
            UiEvent::PageDown => {
                self.diff_scroll = self.diff_scroll.saturating_add(PAGE_SCROLL_STEP as usize);
                ScreenOutcome::idle()
            }
            UiEvent::ComposerSubmit => {
                self.diff_scroll = 0;
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    String::from("reset diff preview to the top"),
                )
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed diff overlay"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = self
            .summary_lines
            .iter()
            .cloned()
            .map(Line::from)
            .collect::<Vec<_>>();
        if self.files.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from("No diff is available for the active lane yet."));
            lines.push(Line::from("Esc closes."));
            let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
            ModalCard::new(self.title.as_str(), content).render(frame, area);
            return;
        }

        lines.push(Line::from(""));
        lines.push(Line::from("files"));
        for (index, file) in self.files.iter().enumerate() {
            let marker = if index == self.selected { ">" } else { " " };
            lines.push(Line::from(format!("{marker} {}", file.path)));
        }

        let selected = &self.files[self.selected];
        lines.push(Line::from(""));
        lines.push(Line::from(format!("diff preview: {}", selected.path)));
        if selected.truncated {
            lines.push(Line::from(
                "preview: truncated to keep this overlay readable",
            ));
        }

        let reserved_rows = self.files.len().min(6) + self.summary_lines.len() + 8;
        let visible_diff_lines =
            usize::from(area.height.saturating_sub(reserved_rows as u16)).max(6);
        let total_lines = selected.diff_lines.len();
        let start = self
            .diff_scroll
            .min(total_lines.saturating_sub(visible_diff_lines));
        let end = (start + visible_diff_lines).min(total_lines);

        if total_lines > visible_diff_lines {
            lines.push(Line::from(format!(
                "scroll: {}-{} of {}",
                start + 1,
                end,
                total_lines
            )));
        }

        if selected.diff_lines.is_empty() {
            lines.push(Line::from(
                "No git diff output is available for this file right now.",
            ));
        } else {
            for line in selected.diff_lines[start..end].iter().cloned() {
                lines.push(Line::from(line));
            }
        }

        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new(self.title.as_str(), content).render(frame, area);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationKind {
    FreshSession,
    ClearContext,
    CompactContext,
}

impl ConfirmationKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::FreshSession => "start fresh task",
            Self::ClearContext => "clear context",
            Self::CompactContext => "compact conversation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceOverlay {
    current_cwd: String,
    draft_cwd: String,
}

impl WorkspaceOverlay {
    pub fn new(current_cwd: impl Into<String>) -> Self {
        Self {
            current_cwd: current_cwd.into(),
            draft_cwd: String::new(),
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerInsert(ch) => {
                self.draft_cwd.push(ch);
                ScreenOutcome::idle()
            }
            UiEvent::ComposerPaste(payload) => {
                self.draft_cwd.push_str(payload.as_str());
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!(
                        "pasted {} chars into workspace path",
                        payload.chars().count()
                    ),
                )
            }
            UiEvent::ComposerBackspace => {
                self.draft_cwd.pop();
                ScreenOutcome::idle()
            }
            UiEvent::ComposerSubmit => {
                if self.draft_cwd.trim().is_empty() {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("paste or type a workspace path first"),
                    );
                }
                ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    format!("selected workspace {}", self.draft_cwd.trim()),
                    ScreenCommand::SetActiveWorkspace {
                        cwd: self.draft_cwd.trim().to_string(),
                    },
                )
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed workspace picker"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let draft = if self.draft_cwd.trim().is_empty() {
            "[empty]"
        } else {
            self.draft_cwd.as_str()
        };
        let content = Paragraph::new(Text::from(vec![
            Line::from("Inspect or change the active workspace for this lane."),
            Line::from(""),
            Line::from(format!("current workspace: {}", self.current_cwd)),
            Line::from("next turn: uses the selected workspace path"),
            Line::from(""),
            Line::from("new workspace path:"),
            Line::from(format!("  {draft}")),
            Line::from(""),
            Line::from("Paste or type a directory path, then press Enter to apply."),
            Line::from("Esc closes without changing the workspace."),
        ]))
        .wrap(Wrap { trim: false });
        ModalCard::new("Workspace", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeSessionView {
    pub id: String,
    pub title: String,
    pub backend: String,
    pub cwd: String,
    pub status: String,
    pub detail_lines: Vec<String>,
    pub next_hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeOverlay {
    sessions: Vec<ResumeSessionView>,
    selected: usize,
}

impl ResumeOverlay {
    pub fn new(sessions: Vec<ResumeSessionView>) -> Self {
        Self {
            sessions,
            selected: 0,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView => {
                if self.sessions.is_empty() {
                    return ScreenOutcome::idle();
                }
                if self.selected == 0 {
                    self.selected = self.sessions.len().saturating_sub(1);
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {}", self.sessions[self.selected].title),
                )
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView => {
                if self.sessions.is_empty() {
                    return ScreenOutcome::idle();
                }
                self.selected = (self.selected + 1) % self.sessions.len();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {}", self.sessions[self.selected].title),
                )
            }
            UiEvent::ComposerSubmit => {
                let Some(session) = self.sessions.get(self.selected) else {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("there are no detached tasks to reopen"),
                    );
                };
                ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    format!("reopening {}", session.title),
                    ScreenCommand::ResumeDetachedSession {
                        session_id: session.id.clone(),
                    },
                )
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed task list"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = vec![
            Line::from("Inspect detached Probe tasks and reopen one on the matching lane."),
            Line::from("Use Up/Down to choose. Enter reopens it. Esc closes."),
            Line::from(""),
            Line::from(format!("tasks discovered: {}", self.sessions.len())),
            Line::from(""),
        ];
        if self.sessions.is_empty() {
            lines.push(Line::from(
                "No detached tasks were found for this Probe home.",
            ));
        } else {
            for (index, session) in self.sessions.iter().enumerate() {
                let marker = if index == self.selected { ">" } else { " " };
                lines.push(Line::from(format!(
                    "{marker} {}  {}",
                    session.title, session.status
                )));
            }
            if let Some(session) = self.sessions.get(self.selected) {
                lines.push(Line::from(""));
                lines.push(Line::from(format!("selected: {}", session.title)));
                lines.push(Line::from(format!("  task: {}", session.id)));
                lines.push(Line::from(format!("  backend: {}", session.backend)));
                lines.push(Line::from(format!("  cwd: {}", session.cwd)));
                for line in session.detail_lines.iter().take(5) {
                    lines.push(Line::from(format!("  {}", line)));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(session.next_hint.clone()));
            }
        }
        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new("Tasks", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageOverlay;

impl UsageOverlay {
    pub fn new() -> Self {
        Self
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed usage overlay"),
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
        _stack_depth: usize,
        base_screen: &ChatScreen,
    ) {
        let content = Paragraph::new(base_screen.render_usage_overlay_text());
        ModalCard::new("Usage", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusOverlay {
    configured_mcp_count: usize,
    enabled_mcp_count: usize,
    mcp_summary_lines: Vec<String>,
}

impl StatusOverlay {
    pub fn new(
        configured_mcp_count: usize,
        enabled_mcp_count: usize,
        mcp_summary_lines: Vec<String>,
    ) -> Self {
        Self {
            configured_mcp_count,
            enabled_mcp_count,
            mcp_summary_lines,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed status overlay"),
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
        _stack_depth: usize,
        base_screen: &ChatScreen,
    ) {
        let content = Paragraph::new(base_screen.render_status_overlay_text(
            self.configured_mcp_count,
            self.enabled_mcp_count,
            self.mcp_summary_lines.as_slice(),
        ))
        .wrap(Wrap { trim: false });
        ModalCard::new("Status", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorOverlay {
    configured_mcp_count: usize,
    enabled_mcp_count: usize,
    mcp_summary_lines: Vec<String>,
}

impl DoctorOverlay {
    pub fn new(
        configured_mcp_count: usize,
        enabled_mcp_count: usize,
        mcp_summary_lines: Vec<String>,
    ) -> Self {
        Self {
            configured_mcp_count,
            enabled_mcp_count,
            mcp_summary_lines,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed doctor overlay"),
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
        _stack_depth: usize,
        base_screen: &ChatScreen,
    ) {
        let content = Paragraph::new(base_screen.render_doctor_overlay_text(
            self.configured_mcp_count,
            self.enabled_mcp_count,
            self.mcp_summary_lines.as_slice(),
        ))
        .wrap(Wrap { trim: false });
        ModalCard::new("Doctor", content).render(frame, area);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecipeChoice {
    ReviewRiskyEdit,
    ShipCurrentWork,
    ConvertMcpRecipe,
    DelegateBackgroundTask,
    WorkFromPrFeedback,
}

impl RecipeChoice {
    const ALL: [Self; 5] = [
        Self::ReviewRiskyEdit,
        Self::ShipCurrentWork,
        Self::ConvertMcpRecipe,
        Self::DelegateBackgroundTask,
        Self::WorkFromPrFeedback,
    ];

    fn previous(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    const fn label(self) -> &'static str {
        match self {
            Self::ReviewRiskyEdit => "Review a risky edit",
            Self::ShipCurrentWork => "Ship current work",
            Self::ConvertMcpRecipe => "Convert an MCP recipe",
            Self::DelegateBackgroundTask => "Delegate a background task",
            Self::WorkFromPrFeedback => "Work from PR feedback",
        }
    }

    const fn summary(self) -> &'static str {
        match self {
            Self::ReviewRiskyEdit => {
                "Set review-first editing before you ask Probe to change code."
            }
            Self::ShipCurrentWork => {
                "Move from local changes to a pushed draft PR without leaving Probe."
            }
            Self::ConvertMcpRecipe => "Turn a saved provider recipe into a runnable MCP server.",
            Self::DelegateBackgroundTask => "Queue child work without giving up the current lane.",
            Self::WorkFromPrFeedback => {
                "Pull GitHub review feedback into the composer for the next fix."
            }
        }
    }

    fn steps(self) -> [&'static str; 4] {
        match self {
            Self::ReviewRiskyEdit => [
                "Run /review_mode and choose review-risky or review-all.",
                "Ask Probe for the edit you want.",
                "Use /diff to inspect the proposed patch.",
                "Approve with A or reject with R before anything lands.",
            ],
            Self::ShipCurrentWork => [
                "Run /git to sanity-check branch and dirty state.",
                "Use /stage to collect the current repo changes.",
                "Use /commit, /push, and /pr to ship the branch.",
                "Use /pr_comments later to pull review feedback back in.",
            ],
            Self::ConvertMcpRecipe => [
                "Open /mcp and enter Saved MCP servers.",
                "Select the saved recipe entry and press Enter.",
                "Review the suggested runtime command or paste a better one.",
                "Save it, then start a turn to test runtime attachment.",
            ],
            Self::DelegateBackgroundTask => [
                "Run /delegate to switch the lane into child-task launch mode.",
                "Submit the prompt you want Probe to work on in the background.",
                "Use /tasks to reopen the child task later.",
                "Return to the parent lane while the child keeps running.",
            ],
            Self::WorkFromPrFeedback => [
                "Run /pr_comments on the branch that owns the PR.",
                "Use Up/Down to choose the review item you want to address.",
                "Press Enter to load that feedback into the composer.",
                "Edit, validate, and ship the follow-up change from there.",
            ],
        }
    }

    const fn starter_command(self) -> &'static str {
        match self {
            Self::ReviewRiskyEdit => "/review_mode",
            Self::ShipCurrentWork => "/git",
            Self::ConvertMcpRecipe => "/mcp",
            Self::DelegateBackgroundTask => "/delegate",
            Self::WorkFromPrFeedback => "/pr_comments",
        }
    }

    fn load_status(self) -> String {
        format!("loaded {} into the composer", self.starter_command())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipesOverlay {
    selected: RecipeChoice,
}

impl RecipesOverlay {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            selected: RecipeChoice::ReviewRiskyEdit,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView => {
                self.selected = self.selected.previous();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {}", self.selected.label()),
                )
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView => {
                self.selected = self.selected.next();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {}", self.selected.label()),
                )
            }
            UiEvent::ComposerSubmit => ScreenOutcome::with_action_and_command(
                ScreenAction::CloseModal,
                self.selected.load_status(),
                ScreenCommand::SeedComposerDraft {
                    text: String::from(self.selected.starter_command()),
                },
            ),
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed recipes overlay"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = vec![
            Line::from(
                "Use a guided workflow when you know the job but not the exact command path.",
            ),
            Line::from(
                "Up/Down chooses a workflow. Enter loads the first step into the composer. Esc returns.",
            ),
            Line::from(""),
            Line::from(format!("recipes available: {}", RecipeChoice::ALL.len())),
            Line::from(""),
        ];
        for choice in RecipeChoice::ALL {
            let marker = if choice == self.selected { ">" } else { " " };
            lines.push(Line::from(format!("{marker} {}", choice.label())));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(format!("selected: {}", self.selected.label())));
        lines.push(Line::from(format!("  {}", self.selected.summary())));
        lines.push(Line::from("  steps:"));
        for step in self.selected.steps() {
            lines.push(Line::from(format!("    - {step}")));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "next: Enter loads `{}` into the composer.",
            self.selected.starter_command()
        )));
        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new("Recipes", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOverlay;

impl GitOverlay {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed git overlay"),
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
        _stack_depth: usize,
        base_screen: &ChatScreen,
    ) {
        let content =
            Paragraph::new(base_screen.render_git_overlay_text()).wrap(Wrap { trim: false });
        ModalCard::new("Git", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchOverlay {
    repo_root: Option<PathBuf>,
    current_branch: String,
    message: String,
    cursor: usize,
    replace_message_on_edit: bool,
    working_tree_dirty: bool,
}

impl BranchOverlay {
    pub fn new(
        repo_root: Option<PathBuf>,
        current_branch: impl Into<String>,
        message: String,
        working_tree_dirty: bool,
    ) -> Self {
        let cursor = message.len();
        Self {
            repo_root,
            current_branch: current_branch.into(),
            message,
            cursor,
            replace_message_on_edit: true,
            working_tree_dirty,
        }
    }

    fn can_apply(&self) -> bool {
        self.repo_root.is_some()
    }

    fn replace_or_insert(&mut self, payload: &str) {
        if self.replace_message_on_edit {
            self.message.clear();
            self.cursor = 0;
            self.replace_message_on_edit = false;
        }
        self.message.insert_str(self.cursor, payload);
        self.cursor += payload.len();
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerInsert(ch) => {
                let mut payload = String::new();
                payload.push(ch);
                self.replace_or_insert(payload.as_str());
                ScreenOutcome::idle()
            }
            UiEvent::ComposerPaste(payload) => {
                self.replace_or_insert(payload.as_str());
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("pasted {} chars into branch name", payload.chars().count()),
                )
            }
            UiEvent::ComposerBackspace => {
                self.replace_message_on_edit = false;
                if let Some(previous) = previous_char_boundary(self.message.as_str(), self.cursor) {
                    self.message.drain(previous..self.cursor);
                    self.cursor = previous;
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerDelete => {
                self.replace_message_on_edit = false;
                if let Some(next) = next_char_boundary(self.message.as_str(), self.cursor) {
                    self.message.drain(self.cursor..next);
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveLeft => {
                self.replace_message_on_edit = false;
                if let Some(previous) = previous_char_boundary(self.message.as_str(), self.cursor) {
                    self.cursor = previous;
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveRight => {
                self.replace_message_on_edit = false;
                if let Some(next) = next_char_boundary(self.message.as_str(), self.cursor) {
                    self.cursor = next;
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveHome => {
                self.replace_message_on_edit = false;
                self.cursor = 0;
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveEnd => {
                self.replace_message_on_edit = false;
                self.cursor = self.message.len();
                ScreenOutcome::idle()
            }
            UiEvent::ComposerNewline => ScreenOutcome::with_status(
                ScreenAction::None,
                String::from("branch names are single-line here; Enter applies"),
            ),
            UiEvent::ComposerSubmit => {
                if !self.can_apply() {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("Probe did not detect a git repo for the current workspace"),
                    );
                }
                if self.message.trim().is_empty() {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("enter a branch name before you continue"),
                    );
                }
                ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    format!("branch ready: {}", self.message.trim()),
                    ScreenCommand::CreateOrSwitchBranch {
                        repo_root: self.repo_root.clone().expect("checked in can_apply"),
                        name: self.message.trim().to_string(),
                    },
                )
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed branch overlay"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = vec![
            Line::from("Create a new branch or switch to an existing branch for this workspace."),
            Line::from(""),
            Line::from(format!("current branch: {}", self.current_branch)),
            Line::from(format!(
                "repo: {}",
                self.repo_root
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| String::from("none detected"))
            )),
            Line::from(format!(
                "working tree: {}",
                if self.working_tree_dirty {
                    "dirty"
                } else {
                    "clean"
                }
            )),
            Line::from(""),
        ];
        if self.can_apply() {
            lines.push(Line::from("branch name:"));
            lines.push(Line::from(self.message.clone()));
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Enter switches to this branch. Probe creates it first if it does not already exist.",
            ));
            if self.working_tree_dirty {
                lines.push(Line::from(
                    "Your current repo changes stay with you when Probe creates a new branch from here.",
                ));
            }
        } else {
            lines.push(Line::from(
                "Probe cannot manage branches here because this workspace is not inside a git repo.",
            ));
        }
        lines.push(Line::from("Esc closes."));
        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new("Branch", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageOverlay {
    repo_root: Option<PathBuf>,
    branch_label: String,
    staged_count: usize,
    unstaged_count: usize,
    untracked_count: usize,
    preview_paths: Vec<String>,
}

impl StageOverlay {
    pub fn new(
        repo_root: Option<PathBuf>,
        branch_label: impl Into<String>,
        staged_count: usize,
        unstaged_count: usize,
        untracked_count: usize,
        preview_paths: Vec<String>,
    ) -> Self {
        Self {
            repo_root,
            branch_label: branch_label.into(),
            staged_count,
            unstaged_count,
            untracked_count,
            preview_paths,
        }
    }

    fn can_stage(&self) -> bool {
        self.repo_root.is_some() && (self.unstaged_count > 0 || self.untracked_count > 0)
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerSubmit => {
                if let Some(repo_root) = self.repo_root.as_ref()
                    && self.can_stage()
                {
                    return ScreenOutcome::with_action_and_command(
                        ScreenAction::CloseModal,
                        String::from("staged current repo changes"),
                        ScreenCommand::StageCurrentRepo {
                            repo_root: repo_root.clone(),
                        },
                    );
                }
                let status = if self.repo_root.is_none() {
                    String::from("Probe did not detect a git repo for the current workspace")
                } else if self.staged_count > 0 {
                    String::from("all current changes are already staged; /commit is ready")
                } else {
                    String::from("there are no repo changes to stage right now")
                };
                ScreenOutcome::with_status(ScreenAction::None, status)
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed stage overlay"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = vec![
            Line::from("Stage the current repo changes for the active workspace."),
            Line::from(""),
            Line::from(format!("branch: {}", self.branch_label)),
            Line::from(format!(
                "repo: {}",
                self.repo_root
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| String::from("none detected"))
            )),
            Line::from(format!(
                "current: {} staged · {} unstaged · {} untracked",
                self.staged_count, self.unstaged_count, self.untracked_count
            )),
        ];
        if !self.preview_paths.is_empty() {
            lines.push(Line::from(format!(
                "paths: {}",
                summarize_inline_paths(self.preview_paths.as_slice(), 4)
            )));
        }
        lines.push(Line::from(""));
        if self.can_stage() {
            lines.push(Line::from(
                "Enter stages all current repo changes, including tracked edits and untracked files.",
            ));
        } else if self.repo_root.is_none() {
            lines.push(Line::from(
                "Probe cannot stage here because this workspace is not inside a git repo.",
            ));
        } else if self.staged_count > 0 {
            lines.push(Line::from(
                "Everything is already staged. Use /commit when you are ready to record it.",
            ));
        } else {
            lines.push(Line::from("There are no repo changes to stage right now."));
        }
        lines.push(Line::from("Esc closes."));
        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new("Stage Changes", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOverlay {
    repo_root: Option<PathBuf>,
    branch_label: String,
    staged_count: usize,
    unstaged_count: usize,
    untracked_count: usize,
    preview_paths: Vec<String>,
    message: String,
    cursor: usize,
    replace_message_on_edit: bool,
}

impl CommitOverlay {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repo_root: Option<PathBuf>,
        branch_label: impl Into<String>,
        staged_count: usize,
        unstaged_count: usize,
        untracked_count: usize,
        preview_paths: Vec<String>,
        message: String,
    ) -> Self {
        let cursor = message.len();
        Self {
            repo_root,
            branch_label: branch_label.into(),
            staged_count,
            unstaged_count,
            untracked_count,
            preview_paths,
            message,
            cursor,
            replace_message_on_edit: true,
        }
    }

    fn can_commit(&self) -> bool {
        self.repo_root.is_some() && self.staged_count > 0
    }

    fn replace_or_insert(&mut self, payload: &str) {
        if self.replace_message_on_edit {
            self.message.clear();
            self.cursor = 0;
            self.replace_message_on_edit = false;
        }
        self.message.insert_str(self.cursor, payload);
        self.cursor += payload.len();
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerInsert(ch) => {
                let mut payload = String::new();
                payload.push(ch);
                self.replace_or_insert(payload.as_str());
                ScreenOutcome::idle()
            }
            UiEvent::ComposerPaste(payload) => {
                self.replace_or_insert(payload.as_str());
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!(
                        "pasted {} chars into commit message",
                        payload.chars().count()
                    ),
                )
            }
            UiEvent::ComposerBackspace => {
                self.replace_message_on_edit = false;
                if let Some(previous) = previous_char_boundary(self.message.as_str(), self.cursor) {
                    self.message.drain(previous..self.cursor);
                    self.cursor = previous;
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerDelete => {
                self.replace_message_on_edit = false;
                if let Some(next) = next_char_boundary(self.message.as_str(), self.cursor) {
                    self.message.drain(self.cursor..next);
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveLeft => {
                self.replace_message_on_edit = false;
                if let Some(previous) = previous_char_boundary(self.message.as_str(), self.cursor) {
                    self.cursor = previous;
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveRight => {
                self.replace_message_on_edit = false;
                if let Some(next) = next_char_boundary(self.message.as_str(), self.cursor) {
                    self.cursor = next;
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveHome => {
                self.replace_message_on_edit = false;
                self.cursor = 0;
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveEnd => {
                self.replace_message_on_edit = false;
                self.cursor = self.message.len();
                ScreenOutcome::idle()
            }
            UiEvent::ComposerNewline => ScreenOutcome::with_status(
                ScreenAction::None,
                String::from("commit messages are single-line here; Enter commits"),
            ),
            UiEvent::ComposerSubmit => {
                if !self.can_commit() {
                    let status = if self.repo_root.is_none() {
                        String::from("Probe did not detect a git repo for the current workspace")
                    } else if self.unstaged_count > 0 || self.untracked_count > 0 {
                        String::from("stage the repo changes first, then commit them")
                    } else {
                        String::from("there are no staged repo changes to commit")
                    };
                    return ScreenOutcome::with_status(ScreenAction::None, status);
                }
                if self.message.trim().is_empty() {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("enter a commit message before you commit"),
                    );
                }
                ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    format!("committing on {}", self.branch_label),
                    ScreenCommand::CommitCurrentRepo {
                        repo_root: self.repo_root.clone().expect("checked in can_commit"),
                        message: self.message.trim().to_string(),
                    },
                )
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed commit overlay"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = vec![
            Line::from("Create a git commit for the current staged repo changes."),
            Line::from(""),
            Line::from(format!("branch: {}", self.branch_label)),
            Line::from(format!(
                "repo: {}",
                self.repo_root
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| String::from("none detected"))
            )),
            Line::from(format!(
                "current: {} staged · {} unstaged · {} untracked",
                self.staged_count, self.unstaged_count, self.untracked_count
            )),
        ];
        if !self.preview_paths.is_empty() {
            lines.push(Line::from(format!(
                "staged paths: {}",
                summarize_inline_paths(self.preview_paths.as_slice(), 4)
            )));
        }
        lines.push(Line::from(""));
        if self.can_commit() {
            lines.push(Line::from("commit message:"));
            lines.push(Line::from(self.message.clone()));
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Enter commits with this message. Esc closes without committing.",
            ));
        } else if self.repo_root.is_none() {
            lines.push(Line::from(
                "Probe cannot commit here because this workspace is not inside a git repo.",
            ));
        } else if self.unstaged_count > 0 || self.untracked_count > 0 {
            lines.push(Line::from(
                "Stage the current repo changes first. Use /stage, then come back to /commit.",
            ));
        } else {
            lines.push(Line::from(
                "There are no staged changes to commit right now.",
            ));
        }
        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new("Commit Changes", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushOverlay {
    repo_root: Option<PathBuf>,
    branch_label: String,
    remote_label: String,
    upstream_label: String,
    ahead_by: Option<u64>,
    behind_by: Option<u64>,
    working_tree_dirty: bool,
    can_push: bool,
    set_upstream: bool,
    blocked_reason: String,
}

impl PushOverlay {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repo_root: Option<PathBuf>,
        branch_label: impl Into<String>,
        remote_label: impl Into<String>,
        upstream_label: impl Into<String>,
        ahead_by: Option<u64>,
        behind_by: Option<u64>,
        working_tree_dirty: bool,
        can_push: bool,
        set_upstream: bool,
        blocked_reason: impl Into<String>,
    ) -> Self {
        Self {
            repo_root,
            branch_label: branch_label.into(),
            remote_label: remote_label.into(),
            upstream_label: upstream_label.into(),
            ahead_by,
            behind_by,
            working_tree_dirty,
            can_push,
            set_upstream,
            blocked_reason: blocked_reason.into(),
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerSubmit if self.can_push => ScreenOutcome::with_action_and_command(
                ScreenAction::CloseModal,
                format!("push ready: {}", self.branch_label),
                ScreenCommand::PushCurrentBranch {
                    repo_root: self.repo_root.clone().expect("checked in can_push"),
                    branch_name: self.branch_label.clone(),
                    set_upstream: self.set_upstream,
                },
            ),
            UiEvent::ComposerSubmit => {
                ScreenOutcome::with_status(ScreenAction::None, self.blocked_reason.clone())
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed push overlay"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = vec![
            Line::from("Push the current branch without leaving Probe."),
            Line::from(""),
            Line::from(format!("branch: {}", self.branch_label)),
            Line::from(format!(
                "repo: {}",
                self.repo_root
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| String::from("none detected"))
            )),
            Line::from(format!("remote: {}", self.remote_label)),
            Line::from(format!("upstream: {}", self.upstream_label)),
            Line::from(format!(
                "sync: ahead {} · behind {}",
                self.ahead_by.unwrap_or(0),
                self.behind_by.unwrap_or(0)
            )),
            Line::from(format!(
                "working tree: {}",
                if self.working_tree_dirty {
                    "dirty"
                } else {
                    "clean"
                }
            )),
            Line::from(""),
        ];
        if self.can_push {
            if self.set_upstream {
                lines.push(Line::from(format!(
                    "Enter pushes {} and sets upstream on {}.",
                    self.branch_label, self.remote_label
                )));
            } else {
                lines.push(Line::from(format!(
                    "Enter pushes {} to its tracked remote.",
                    self.branch_label
                )));
            }
            if self.working_tree_dirty {
                lines.push(Line::from(
                    "Uncommitted workspace changes stay local; Probe only pushes committed work.",
                ));
            }
        } else {
            lines.push(Line::from(self.blocked_reason.clone()));
        }
        lines.push(Line::from("Esc closes."));
        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new("Push Branch", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrOverlay {
    repo_root: Option<PathBuf>,
    head_branch: String,
    base_branch: String,
    remote_label: String,
    upstream_label: String,
    title: String,
    cursor: usize,
    replace_title_on_edit: bool,
    can_create: bool,
    blocked_reason: String,
}

impl PrOverlay {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repo_root: Option<PathBuf>,
        head_branch: impl Into<String>,
        base_branch: impl Into<String>,
        remote_label: impl Into<String>,
        upstream_label: impl Into<String>,
        title: String,
        can_create: bool,
        blocked_reason: impl Into<String>,
    ) -> Self {
        let cursor = title.len();
        Self {
            repo_root,
            head_branch: head_branch.into(),
            base_branch: base_branch.into(),
            remote_label: remote_label.into(),
            upstream_label: upstream_label.into(),
            title,
            cursor,
            replace_title_on_edit: true,
            can_create,
            blocked_reason: blocked_reason.into(),
        }
    }

    fn replace_or_insert(&mut self, payload: &str) {
        if self.replace_title_on_edit {
            self.title.clear();
            self.cursor = 0;
            self.replace_title_on_edit = false;
        }
        self.title.insert_str(self.cursor, payload);
        self.cursor += payload.len();
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerInsert(ch) => {
                let mut payload = String::new();
                payload.push(ch);
                self.replace_or_insert(payload.as_str());
                ScreenOutcome::idle()
            }
            UiEvent::ComposerPaste(payload) => {
                self.replace_or_insert(payload.as_str());
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("pasted {} chars into PR title", payload.chars().count()),
                )
            }
            UiEvent::ComposerBackspace => {
                self.replace_title_on_edit = false;
                if let Some(previous) = previous_char_boundary(self.title.as_str(), self.cursor) {
                    self.title.drain(previous..self.cursor);
                    self.cursor = previous;
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerDelete => {
                self.replace_title_on_edit = false;
                if let Some(next) = next_char_boundary(self.title.as_str(), self.cursor) {
                    self.title.drain(self.cursor..next);
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveLeft => {
                self.replace_title_on_edit = false;
                if let Some(previous) = previous_char_boundary(self.title.as_str(), self.cursor) {
                    self.cursor = previous;
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveRight => {
                self.replace_title_on_edit = false;
                if let Some(next) = next_char_boundary(self.title.as_str(), self.cursor) {
                    self.cursor = next;
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveHome => {
                self.replace_title_on_edit = false;
                self.cursor = 0;
                ScreenOutcome::idle()
            }
            UiEvent::ComposerMoveEnd => {
                self.replace_title_on_edit = false;
                self.cursor = self.title.len();
                ScreenOutcome::idle()
            }
            UiEvent::ComposerNewline => ScreenOutcome::with_status(
                ScreenAction::None,
                String::from("PR titles are single-line here; Enter creates the draft PR"),
            ),
            UiEvent::ComposerSubmit if !self.can_create => {
                ScreenOutcome::with_status(ScreenAction::None, self.blocked_reason.clone())
            }
            UiEvent::ComposerSubmit if self.title.trim().is_empty() => ScreenOutcome::with_status(
                ScreenAction::None,
                String::from("enter a PR title before you continue"),
            ),
            UiEvent::ComposerSubmit => ScreenOutcome::with_action_and_command(
                ScreenAction::CloseModal,
                format!("draft PR ready: {}", self.title.trim()),
                ScreenCommand::CreateDraftPullRequest {
                    repo_root: self.repo_root.clone().expect("checked in can_create"),
                    title: self.title.trim().to_string(),
                    base_branch: self.base_branch.clone(),
                    head_branch: self.head_branch.clone(),
                },
            ),
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed PR overlay"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = vec![
            Line::from("Create a draft pull request from the current branch."),
            Line::from(""),
            Line::from(format!("head branch: {}", self.head_branch)),
            Line::from(format!("base branch: {}", self.base_branch)),
            Line::from(format!("remote: {}", self.remote_label)),
            Line::from(format!("upstream: {}", self.upstream_label)),
            Line::from(format!(
                "repo: {}",
                self.repo_root
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| String::from("none detected"))
            )),
            Line::from(""),
        ];
        if self.can_create {
            lines.push(Line::from("draft PR title:"));
            lines.push(Line::from(self.title.clone()));
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Enter creates a draft PR with this title. Esc closes without creating it.",
            ));
        } else {
            lines.push(Line::from(self.blocked_reason.clone()));
        }
        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new("Create Draft PR", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrFeedbackItemView {
    pub label: String,
    pub preview: String,
    pub detail_lines: Vec<String>,
    pub seed_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrCommentsOverlay {
    summary_lines: Vec<String>,
    items: Vec<PrFeedbackItemView>,
    selected: usize,
}

impl PrCommentsOverlay {
    pub fn new(summary_lines: Vec<String>, items: Vec<PrFeedbackItemView>) -> Self {
        Self {
            summary_lines,
            items,
            selected: 0,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView if !self.items.is_empty() => {
                if self.selected == 0 {
                    self.selected = self.items.len().saturating_sub(1);
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {}", self.items[self.selected].label),
                )
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView if !self.items.is_empty() => {
                self.selected = (self.selected + 1) % self.items.len();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {}", self.items[self.selected].label),
                )
            }
            UiEvent::ComposerSubmit => {
                if let Some(item) = self.items.get(self.selected) {
                    ScreenOutcome::with_action_and_command(
                        ScreenAction::CloseModal,
                        format!("loaded {} into the composer", item.label),
                        ScreenCommand::SeedComposerDraft {
                            text: item.seed_text.clone(),
                        },
                    )
                } else {
                    ScreenOutcome::with_status(
                        ScreenAction::CloseModal,
                        String::from("dismissed PR comments overlay"),
                    )
                }
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed PR comments overlay"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = vec![
            Line::from("Inspect the current PR feedback for this branch."),
            if self.items.is_empty() {
                Line::from("Enter or Esc closes. Use /pr_comments again to refresh later.")
            } else {
                Line::from(
                    "Up/Down choose a feedback item. Enter loads it into the composer. Esc returns.",
                )
            },
            Line::from(""),
        ];
        lines.extend(self.summary_lines.iter().cloned().map(Line::from));
        if !self.items.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(format!("feedback items: {}", self.items.len())));
            lines.push(Line::from(""));
            for (index, item) in self.items.iter().enumerate() {
                let marker = if index == self.selected { ">" } else { " " };
                lines.push(Line::from(format!(
                    "{marker} {}  {}",
                    item.label, item.preview
                )));
            }
            if let Some(item) = self.items.get(self.selected) {
                lines.push(Line::from(""));
                lines.push(Line::from(format!("selected: {}", item.label)));
                for line in item.detail_lines.iter().take(6) {
                    lines.push(Line::from(format!("  {line}")));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(
                    "next: Enter loads this feedback into the composer as the next coding task.",
                ));
            }
        }
        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new("PR Comments", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointOverlay;

impl CheckpointOverlay {
    pub fn new() -> Self {
        Self
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerSubmit | UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed checkpoint overlay"),
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
        _stack_depth: usize,
        base_screen: &ChatScreen,
    ) {
        let content =
            Paragraph::new(base_screen.render_checkpoint_overlay_text()).wrap(Wrap { trim: false });
        ModalCard::new("Checkpoint", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertOverlay {
    can_execute: bool,
    session_id: Option<String>,
    blocked_status: String,
}

impl RevertOverlay {
    pub fn new(can_execute: bool, session_id: Option<String>, blocked_status: String) -> Self {
        Self {
            can_execute,
            session_id,
            blocked_status,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerInsert('a') | UiEvent::ComposerInsert('A') if self.can_execute => {
                ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    String::from("queued revert for the latest task"),
                    ScreenCommand::RevertLastTask {
                        session_id: self.session_id.clone().unwrap_or_default(),
                    },
                )
            }
            UiEvent::ComposerInsert('a') | UiEvent::ComposerInsert('A') => {
                ScreenOutcome::with_status(ScreenAction::None, self.blocked_status.clone())
            }
            UiEvent::ComposerSubmit if self.can_execute => ScreenOutcome::with_action_and_command(
                ScreenAction::CloseModal,
                String::from("queued revert for the latest task"),
                ScreenCommand::RevertLastTask {
                    session_id: self.session_id.clone().unwrap_or_default(),
                },
            ),
            UiEvent::ComposerSubmit => {
                ScreenOutcome::with_status(ScreenAction::None, self.blocked_status.clone())
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed revert overlay"),
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
        _stack_depth: usize,
        base_screen: &ChatScreen,
    ) {
        let content =
            Paragraph::new(base_screen.render_revert_overlay_text()).wrap(Wrap { trim: false });
        ModalCard::new("Revert Last Task", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationCardView {
    pub label: String,
    pub status: String,
    pub detail_lines: Vec<String>,
    pub next_step: String,
    pub toggle_server_id: Option<String>,
    pub remove_server_id: Option<String>,
    pub opens_server_list: bool,
    pub opens_editor: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedMcpServerView {
    pub label: String,
    pub enabled: bool,
    pub status: String,
    pub detail_lines: Vec<String>,
    pub toggle_server_id: String,
    pub remove_server_id: String,
    pub edit_server_id: Option<String>,
    pub enter_hint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerTransportDraft {
    Stdio,
    Http,
}

impl McpServerTransportDraft {
    fn label(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOverlay {
    cards: Vec<IntegrationCardView>,
    configured_count: usize,
    enabled_count: usize,
    selected: usize,
}

impl McpOverlay {
    pub fn new(
        cards: Vec<IntegrationCardView>,
        configured_count: usize,
        enabled_count: usize,
    ) -> Self {
        Self {
            cards,
            configured_count,
            enabled_count,
            selected: 0,
        }
    }

    pub fn with_selected(
        cards: Vec<IntegrationCardView>,
        configured_count: usize,
        enabled_count: usize,
        selected: usize,
    ) -> Self {
        let selected = if cards.is_empty() {
            0
        } else {
            selected.min(cards.len().saturating_sub(1))
        };
        Self {
            cards,
            configured_count,
            enabled_count,
            selected,
        }
    }

    pub fn selected_label(&self) -> Option<&str> {
        self.cards
            .get(self.selected)
            .map(|card| card.label.as_str())
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView => {
                if self.cards.is_empty() {
                    return ScreenOutcome::idle();
                }
                if self.selected == 0 {
                    self.selected = self.cards.len().saturating_sub(1);
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {}", self.cards[self.selected].label),
                )
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView => {
                if self.cards.is_empty() {
                    return ScreenOutcome::idle();
                }
                self.selected = (self.selected + 1) % self.cards.len();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {}", self.cards[self.selected].label),
                )
            }
            UiEvent::ComposerInsert('a') => ScreenOutcome::with_status(
                ScreenAction::OpenMcpAddOverlay,
                String::from("opened MCP add menu"),
            ),
            UiEvent::ComposerInsert('d') => {
                let Some(card) = self.cards.get(self.selected) else {
                    return ScreenOutcome::idle();
                };
                if let Some(server_id) = card.remove_server_id.clone() {
                    ScreenOutcome::with_command(
                        format!("removed {}", card.label),
                        ScreenCommand::RemoveMcpServer { server_id },
                    )
                } else {
                    ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("only saved MCP servers can be removed"),
                    )
                }
            }
            UiEvent::ComposerSubmit => {
                let Some(card) = self.cards.get(self.selected) else {
                    return ScreenOutcome::idle();
                };
                if card.opens_editor {
                    ScreenOutcome::with_status(
                        ScreenAction::OpenMcpAddOverlay,
                        String::from("opened MCP add menu"),
                    )
                } else if card.opens_server_list {
                    ScreenOutcome::with_status(
                        ScreenAction::OpenMcpServersOverlay,
                        String::from("opened saved MCP servers"),
                    )
                } else if let Some(server_id) = card.toggle_server_id.clone() {
                    ScreenOutcome::with_command(
                        format!("toggled {}", card.label),
                        ScreenCommand::ToggleMcpServerEnabled { server_id },
                    )
                } else {
                    ScreenOutcome::with_status(
                        ScreenAction::None,
                        format!("selected {}", card.label),
                    )
                }
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed MCP overlay"),
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
        _stack_depth: usize,
        _base_screen: &ChatScreen,
    ) {
        let mut lines = vec![
            Line::from("Inspect Probe's current integration boundary."),
            Line::from(
                "Up/Down choose a card. Enter runs the selected action. A opens add flow. Esc returns.",
            ),
        ];
        lines.push(Line::from(format!(
            "saved MCP servers: {} configured, {} enabled",
            self.configured_count, self.enabled_count
        )));
        lines.push(Line::from(
            "legend: saved recipe = not runnable yet · ready after next turn = enabled manual runtime server · connected now = live in this session",
        ));
        for (index, card) in self.cards.iter().enumerate() {
            let marker = if index == self.selected { ">" } else { " " };
            lines.push(Line::from(format!(
                "{marker} {}  {}",
                card.label, card.status
            )));
        }
        if let Some(card) = self.cards.get(self.selected) {
            lines.push(Line::from(format!("selected: {}", card.label)));
            for line in card.detail_lines.iter().take(2) {
                lines.push(Line::from(format!("  {line}")));
            }
            lines.push(Line::from(format!("next: {}", card.next_step)));
        }
        let content = Paragraph::new(Text::from(lines));
        ModalCard::new("Integrations", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServersOverlay {
    servers: Vec<ManagedMcpServerView>,
    selected: usize,
}

impl McpServersOverlay {
    pub fn new(servers: Vec<ManagedMcpServerView>) -> Self {
        Self {
            servers,
            selected: 0,
        }
    }

    pub fn with_selected(servers: Vec<ManagedMcpServerView>, selected: usize) -> Self {
        let selected = if servers.is_empty() {
            0
        } else {
            selected.min(servers.len().saturating_sub(1))
        };
        Self { servers, selected }
    }

    pub fn selected_label(&self) -> Option<&str> {
        self.servers
            .get(self.selected)
            .map(|server| server.label.as_str())
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView => {
                if self.servers.is_empty() {
                    return ScreenOutcome::idle();
                }
                if self.selected == 0 {
                    self.selected = self.servers.len().saturating_sub(1);
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {}", self.servers[self.selected].label),
                )
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView => {
                if self.servers.is_empty() {
                    return ScreenOutcome::idle();
                }
                self.selected = (self.selected + 1) % self.servers.len();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {}", self.servers[self.selected].label),
                )
            }
            UiEvent::ComposerInsert('a') => ScreenOutcome::with_status(
                ScreenAction::OpenMcpAddOverlay,
                String::from("opened MCP add menu"),
            ),
            UiEvent::ComposerInsert('d') => {
                let Some(server) = self.servers.get(self.selected) else {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("there are no saved MCP servers to disable"),
                    );
                };
                if !server.enabled {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        format!("{} is already disabled", server.label),
                    );
                }
                ScreenOutcome::with_command(
                    format!("disabled {}", server.label),
                    ScreenCommand::ToggleMcpServerEnabled {
                        server_id: server.toggle_server_id.clone(),
                    },
                )
            }
            UiEvent::ComposerInsert('e') => {
                let Some(server) = self.servers.get(self.selected) else {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("there are no saved MCP servers to enable"),
                    );
                };
                if server.enabled {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        format!("{} is already enabled", server.label),
                    );
                };
                ScreenOutcome::with_command(
                    format!("enabled {}", server.label),
                    ScreenCommand::ToggleMcpServerEnabled {
                        server_id: server.toggle_server_id.clone(),
                    },
                )
            }
            UiEvent::ComposerInsert('r') => {
                let Some(server) = self.servers.get(self.selected) else {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("there are no saved MCP servers to remove"),
                    );
                };
                ScreenOutcome::with_command(
                    format!("removed {}", server.label),
                    ScreenCommand::RemoveMcpServer {
                        server_id: server.remove_server_id.clone(),
                    },
                )
            }
            UiEvent::ComposerSubmit => {
                let Some(server) = self.servers.get(self.selected) else {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("there are no saved MCP servers to open"),
                    );
                };
                if let Some(server_id) = server.edit_server_id.clone() {
                    ScreenOutcome::with_action_and_command(
                        ScreenAction::CloseModal,
                        format!("opened MCP setup for {}", server.label),
                        ScreenCommand::OpenMcpManualEditorOverlay {
                            server_id: Some(server_id),
                        },
                    )
                } else {
                    ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from(
                            "use E to enable, D to disable, or R to remove the selected MCP",
                        ),
                    )
                }
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed saved MCP servers"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = vec![
            Line::from("Manage the MCP servers saved in this Probe home."),
            Line::from(
                "Up/Down choose a server. Enter opens setup. E enables. D disables. R removes. A adds. Esc returns.",
            ),
            Line::from(""),
            Line::from(format!("saved MCP servers: {}", self.servers.len())),
            Line::from(""),
        ];
        if self.servers.is_empty() {
            lines.push(Line::from("No saved MCP servers yet."));
            lines.push(Line::from(""));
            lines.push(Line::from("Press A to add one."));
        } else {
            for (index, server) in self.servers.iter().enumerate() {
                let marker = if index == self.selected { ">" } else { " " };
                lines.push(Line::from(format!(
                    "{marker} {}  {}",
                    server.label, server.status
                )));
            }
            if let Some(server) = self.servers.get(self.selected) {
                lines.push(Line::from(""));
                lines.push(Line::from(format!("selected: {}", server.label)));
                for line in server.detail_lines.iter().take(6) {
                    lines.push(Line::from(format!("  {line}")));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(server.enter_hint.clone()));
            }
        }
        let content = Paragraph::new(Text::from(lines));
        ModalCard::new("Saved MCP Servers", content).render(frame, area);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpAddChoice {
    ProviderCommand,
    ManualSetup,
}

impl McpAddChoice {
    fn next(self) -> Self {
        match self {
            Self::ProviderCommand => Self::ManualSetup,
            Self::ManualSetup => Self::ProviderCommand,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::ProviderCommand => Self::ManualSetup,
            Self::ManualSetup => Self::ProviderCommand,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ProviderCommand => "Paste provider setup command",
            Self::ManualSetup => "Manual setup (advanced)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAddOverlay {
    selected: McpAddChoice,
}

impl McpAddOverlay {
    pub fn new() -> Self {
        Self {
            selected: McpAddChoice::ProviderCommand,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView => {
                self.selected = self.selected.previous();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {}", self.selected.label()),
                )
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView => {
                self.selected = self.selected.next();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("selected {}", self.selected.label()),
                )
            }
            UiEvent::ComposerSubmit => match self.selected {
                McpAddChoice::ProviderCommand => ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    String::from("opened MCP provider command setup"),
                    ScreenCommand::OpenMcpProviderCommandOverlay,
                ),
                McpAddChoice::ManualSetup => ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    String::from("opened manual MCP setup"),
                    ScreenCommand::OpenMcpManualEditorOverlay { server_id: None },
                ),
            },
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed MCP add menu"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let provider_marker = if self.selected == McpAddChoice::ProviderCommand {
            ">"
        } else {
            " "
        };
        let manual_marker = if self.selected == McpAddChoice::ManualSetup {
            ">"
        } else {
            " "
        };
        let selected_next = match self.selected {
            McpAddChoice::ProviderCommand => {
                "Paste the command from provider docs. Probe will carry it into setup as reference."
            }
            McpAddChoice::ManualSetup => {
                "Enter the display name, connection type, and launch command or URL yourself."
            }
        };
        let content = Paragraph::new(Text::from(vec![
            Line::from("Choose how you want to add this MCP integration."),
            Line::from(
                "Use the standard provider-command path when docs give you a setup command.",
            ),
            Line::from(""),
            Line::from(format!(
                "{provider_marker} Paste provider setup command  recommended"
            )),
            Line::from("  best for commands like `pnpm dlx ... mcp init --client codex`"),
            Line::from(format!("{manual_marker} Manual setup (advanced)")),
            Line::from("  best when you already know the final launch command or MCP URL"),
            Line::from(""),
            Line::from(format!("next: {selected_next}")),
            Line::from("Enter continues. Esc cancels."),
        ]));
        ModalCard::new("Add MCP", content).render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpProviderCommandOverlay {
    command: String,
    last_paste_summary: Option<String>,
}

impl McpProviderCommandOverlay {
    pub fn new() -> Self {
        Self {
            command: String::new(),
            last_paste_summary: None,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerInsert(ch) => {
                self.command.push(ch);
                self.last_paste_summary = None;
                ScreenOutcome::idle()
            }
            UiEvent::ComposerPaste(payload) => {
                let pasted_chars = payload.chars().count();
                self.command.push_str(payload.as_str());
                self.last_paste_summary = Some(format!("pasted {pasted_chars} chars"));
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("pasted {pasted_chars} chars into provider command"),
                )
            }
            UiEvent::ComposerBackspace => {
                self.command.pop();
                ScreenOutcome::idle()
            }
            UiEvent::ComposerSubmit => {
                if self.command.trim().is_empty() {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("paste a provider setup command first"),
                    );
                }
                ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    String::from("imported provider command into MCP recipes"),
                    ScreenCommand::ImportMcpProviderCommand {
                        command: self.command.trim().to_string(),
                    },
                )
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed MCP provider command setup"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let mut lines = vec![Line::from("Paste the provider command from the MCP docs.")];
        if let Some(summary) = &self.last_paste_summary {
            lines.push(Line::from(format!("clipboard: {summary}")));
        }
        lines.push(Line::from(""));
        lines.push(Line::from("provider setup command preview:"));
        if self.command.is_empty() {
            lines.push(Line::from("  [empty]"));
            lines.push(Line::from(""));
            lines.push(Line::from(
                "example: pnpm dlx shadcn@latest mcp init --client codex",
            ));
        } else {
            lines.push(Line::from(format!("  {}", self.command)));
            lines.push(Line::from(""));
            lines.push(Line::from(format!(
                "chars: {}",
                self.command.chars().count()
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(
            "Use Cmd+V to paste normally. If Terminal does not show it, press Ctrl+V to pull from the system clipboard.",
        ));
        lines.push(Line::from(
            "Probe will save this as an imported MCP recipe entry.",
        ));
        lines.push(Line::from(""));
        lines.push(Line::from("Enter imports. Esc cancels."));
        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new("Provider Command", content).render(frame, area);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpEditorField {
    Name,
    Transport,
    Target,
}

impl McpEditorField {
    fn next(self) -> Self {
        match self {
            Self::Name => Self::Transport,
            Self::Transport => Self::Target,
            Self::Target => Self::Name,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Name => Self::Target,
            Self::Transport => Self::Name,
            Self::Target => Self::Transport,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpEditorOverlay {
    server_id: Option<String>,
    selected: McpEditorField,
    name: String,
    transport: McpServerTransportDraft,
    target: String,
    replace_target_on_edit: bool,
    provider_command: Option<String>,
    provider_hint: Option<String>,
    recommended_target: Option<String>,
    recommendation_note: Option<String>,
}

impl McpEditorOverlay {
    pub fn new() -> Self {
        Self {
            server_id: None,
            selected: McpEditorField::Name,
            name: String::new(),
            transport: McpServerTransportDraft::Stdio,
            target: String::new(),
            replace_target_on_edit: false,
            provider_command: None,
            provider_hint: None,
            recommended_target: None,
            recommendation_note: None,
        }
    }

    pub fn seeded(
        server_id: Option<String>,
        name: String,
        transport: McpServerTransportDraft,
        target: String,
        provider_command: Option<String>,
        provider_hint: Option<String>,
        recommended_target: Option<String>,
        recommendation_note: Option<String>,
    ) -> Self {
        let use_recommended_target = target.trim().is_empty() && recommended_target.is_some();
        let target = if use_recommended_target {
            recommended_target.clone().unwrap_or_default()
        } else {
            target
        };
        Self {
            server_id,
            selected: if provider_command.is_some() {
                McpEditorField::Target
            } else {
                McpEditorField::Name
            },
            name,
            transport,
            target,
            replace_target_on_edit: use_recommended_target,
            provider_command,
            provider_hint,
            recommended_target,
            recommendation_note,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerHistoryPrevious | UiEvent::PreviousView => {
                self.selected = self.selected.previous();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("editing {}", self.selected_label()),
                )
            }
            UiEvent::ComposerHistoryNext | UiEvent::NextView => {
                self.selected = self.selected.next();
                ScreenOutcome::with_status(
                    ScreenAction::None,
                    format!("editing {}", self.selected_label()),
                )
            }
            UiEvent::ComposerInsert(ch) => {
                match self.selected {
                    McpEditorField::Name => self.name.push(ch),
                    McpEditorField::Target => {
                        if self.replace_target_on_edit {
                            self.target.clear();
                            self.replace_target_on_edit = false;
                        }
                        self.target.push(ch);
                    }
                    McpEditorField::Transport => {
                        if matches!(ch, 'h' | 'H') {
                            self.transport = McpServerTransportDraft::Http;
                        } else if matches!(ch, 's' | 'S') {
                            self.transport = McpServerTransportDraft::Stdio;
                        }
                    }
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerPaste(payload) => {
                match self.selected {
                    McpEditorField::Name => self.name.push_str(payload.as_str()),
                    McpEditorField::Target => {
                        if self.replace_target_on_edit {
                            self.target.clear();
                            self.replace_target_on_edit = false;
                        }
                        self.target.push_str(payload.as_str());
                    }
                    McpEditorField::Transport => {}
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerBackspace => {
                match self.selected {
                    McpEditorField::Name => {
                        self.name.pop();
                    }
                    McpEditorField::Target => {
                        if self.replace_target_on_edit {
                            self.target.clear();
                            self.replace_target_on_edit = false;
                        }
                        self.target.pop();
                    }
                    McpEditorField::Transport => {}
                }
                ScreenOutcome::idle()
            }
            UiEvent::ComposerSubmit => {
                if self.name.trim().is_empty() {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("enter a display name first"),
                    );
                }
                if self.target.trim().is_empty() {
                    return ScreenOutcome::with_status(
                        ScreenAction::None,
                        String::from("enter a launch command or URL first"),
                    );
                }
                ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    format!("saved MCP server {}", self.name.trim()),
                    ScreenCommand::SaveMcpServer {
                        server_id: self.server_id.clone(),
                        name: self.name.trim().to_string(),
                        transport: self.transport,
                        target: self.target.trim().to_string(),
                    },
                )
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed MCP add overlay"),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn selected_label(&self) -> &'static str {
        match self.selected {
            McpEditorField::Name => "display name",
            McpEditorField::Transport => "connection type",
            McpEditorField::Target => "launch command or URL",
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let name_marker = if self.selected == McpEditorField::Name {
            ">"
        } else {
            " "
        };
        let transport_marker = if self.selected == McpEditorField::Transport {
            ">"
        } else {
            " "
        };
        let target_marker = if self.selected == McpEditorField::Target {
            ">"
        } else {
            " "
        };
        let target_hint = match self.transport {
            McpServerTransportDraft::Stdio => {
                "full command, e.g. npx -y @modelcontextprotocol/server-filesystem ."
            }
            McpServerTransportDraft::Http => "URL, e.g. http://127.0.0.1:8787/mcp",
        };
        let mut lines = vec![
            Line::from(if self.provider_command.is_some() {
                "Complete runtime setup so this saved recipe becomes a runnable MCP server."
            } else {
                "Add or edit a manual MCP runtime server."
            }),
            Line::from(if self.provider_command.is_some() {
                "Probe saved the provider docs command, but it still needs a real runtime launch command."
            } else {
                "Use this when you know the final launch command or MCP URL."
            }),
            Line::from("Tab or Up/Down changes fields. Enter saves. Esc cancels."),
            Line::from(""),
        ];
        if let Some(command) = &self.provider_command {
            if let Some(provider_hint) = &self.provider_hint {
                lines.push(Line::from(format!("provider: {provider_hint}")));
            }
            lines.push(Line::from("provider command reference:"));
            lines.push(Line::from(format!("  {command}")));
            if let Some(recommended_target) = &self.recommended_target {
                lines.push(Line::from(format!(
                    "recommended runtime command: {recommended_target}"
                )));
            }
            if let Some(note) = &self.recommendation_note {
                lines.push(Line::from(note.clone()));
            }
            lines.push(Line::from(
                "Review the recommended runtime command below, then save or adjust it.",
            ));
            lines.push(Line::from(""));
        }
        lines.push(Line::from(format!(
            "{name_marker} display name: {}",
            self.name
        )));
        lines.push(Line::from(format!(
            "{transport_marker} connection type: {}",
            self.transport.label()
        )));
        lines.push(Line::from(format!(
            "{target_marker} launch command or URL: {}",
            self.target
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(format!("launch help: {target_hint}")));
        lines.push(Line::from(
            if matches!(self.transport, McpServerTransportDraft::Stdio) {
                "Probe can run connected manual stdio MCP servers during turns."
            } else {
                "HTTP MCP entries are saved, but runtime mounting is still limited in Probe today."
            },
        ));
        let content = Paragraph::new(Text::from(lines));
        ModalCard::new(
            if self.provider_command.is_some() {
                "Convert MCP Recipe"
            } else {
                "Manual MCP Setup"
            },
            content,
        )
        .render(frame, area);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmationOverlay {
    kind: ConfirmationKind,
    lane_label: String,
    mode_label: String,
    model_id: String,
    cwd: String,
    summary_preview: Option<String>,
}

impl ConfirmationOverlay {
    pub fn new(
        kind: ConfirmationKind,
        lane_label: impl Into<String>,
        mode_label: impl Into<String>,
        model_id: impl Into<String>,
        cwd: impl Into<String>,
        summary_preview: Option<String>,
    ) -> Self {
        Self {
            kind,
            lane_label: lane_label.into(),
            mode_label: mode_label.into(),
            model_id: model_id.into(),
            cwd: cwd.into(),
            summary_preview,
        }
    }

    fn handle_event(&mut self, event: UiEvent) -> ScreenOutcome {
        match event {
            UiEvent::ComposerSubmit => match self.kind {
                ConfirmationKind::FreshSession => ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    String::from("confirmed fresh task"),
                    ScreenCommand::ConfirmClearActiveContext,
                ),
                ConfirmationKind::ClearContext => ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    String::from("confirmed clear context"),
                    ScreenCommand::ConfirmClearActiveContext,
                ),
                ConfirmationKind::CompactContext => ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    String::from("confirmed compact conversation"),
                    ScreenCommand::ConfirmCompactActiveContext,
                ),
            },
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                format!("dismissed {}", self.kind.label()),
            ),
            UiEvent::OpenHelp => ScreenOutcome::with_status(
                ScreenAction::OpenHelp,
                String::from("opened help modal"),
            ),
            _ => ScreenOutcome::idle(),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, _stack_depth: usize) {
        let (title, description, caution) = match self.kind {
            ConfirmationKind::FreshSession => (
                "Start Fresh Task",
                "Start a fresh runtime session on this lane.",
                "This clears the visible conversation for this lane in the TUI, but it does not touch repo files or backend settings.",
            ),
            ConfirmationKind::ClearContext => (
                "Clear Context",
                "Drop the current conversation context and start fresh on this lane.",
                "This clears the visible conversation for this lane in the TUI, but it does not touch repo files, backend selection, or workspace settings.",
            ),
            ConfirmationKind::CompactContext => (
                "Compact Conversation",
                "Carry forward a Probe summary and start a fresh runtime session.",
                "This clears the visible conversation and seeds the next turn with a compact Probe summary instead of replaying the full transcript.",
            ),
        };
        let mut lines = vec![
            Line::from(description),
            Line::from(""),
            Line::from(format!("lane: {}", self.lane_label)),
            Line::from(format!("mode: {}", self.mode_label)),
            Line::from(format!("model: {}", self.model_id)),
            Line::from(format!("workspace: {}", self.cwd)),
            Line::from(""),
            Line::from(caution),
        ];
        if let Some(summary_preview) = self.summary_preview.as_deref() {
            lines.push(Line::from(""));
            lines.push(Line::from("carry-forward summary preview:"));
            for line in summary_preview.lines() {
                lines.push(Line::from(format!("  {}", line)));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from("Enter confirms. Esc cancels."));
        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new(title, content).render(frame, area);
    }
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
            UiEvent::ComposerInsert('a') | UiEvent::ComposerInsert('A') => {
                ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    format!("queued apply for pending tool {}", self.approval.tool_name),
                    ScreenCommand::ResolvePendingToolApproval {
                        session_id: self.approval.session_id.as_str().to_string(),
                        call_id: self.approval.tool_call_id.clone(),
                        resolution: ToolApprovalResolution::Approved,
                    },
                )
            }
            UiEvent::ComposerInsert('r') | UiEvent::ComposerInsert('R') => {
                ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    format!("queued reject for pending tool {}", self.approval.tool_name),
                    ScreenCommand::ResolvePendingToolApproval {
                        session_id: self.approval.session_id.as_str().to_string(),
                        call_id: self.approval.tool_call_id.clone(),
                        resolution: ToolApprovalResolution::Rejected,
                    },
                )
            }
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
        let review = approval_preview(&self.approval);
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
            Line::from(if review.title == "Review Changes" {
                "Review these proposed changes before they land."
            } else {
                "Review this tool request."
            }),
            Line::from(""),
            Line::from(format!(
                "tool: {}",
                display_runtime_tool_name(self.approval.tool_name.as_str())
            )),
            Line::from(format!(
                "call: {}",
                preview(self.approval.tool_call_id.as_str(), 24)
            )),
            Line::from(format!(
                "risk: {}",
                render_runtime_risk_class(self.approval.risk_class)
            )),
            Line::from(format!(
                "turns: {} -> {}",
                self.approval.tool_call_turn_index, self.approval.paused_result_turn_index
            )),
            Line::from(format!(
                "why: {}",
                self.approval
                    .reason
                    .as_deref()
                    .unwrap_or("pending operator decision")
            )),
            Line::from(""),
            Line::from(format!("summary: {}", review.summary)),
        ];
        if !review.files.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from("files"));
            for path in review.files {
                lines.push(Line::from(format!("  {path}")));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(review.preview_label));
        for line in review.preview_lines {
            lines.push(Line::from(format!("  {line}")));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "validation: {}",
            review.validation_line
        )));
        lines.extend([
            Line::from(""),
            Line::from(format!("{approve_marker} {}", review.primary_action_label)),
            Line::from(format!("{reject_marker} Reject")),
            Line::from(""),
            Line::from("A applies. R rejects. Tab changes selection. Enter decides. Esc closes."),
            Line::from(format!("stack depth: {stack_depth}")),
        ]);
        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new(review.title, content).render(frame, area);
    }
}

fn approval_preview(approval: &PendingToolApproval) -> ApprovalPreview {
    if let Some(proposed) = approval.proposed_edit.as_ref() {
        return ApprovalPreview {
            title: "Review Changes",
            summary: proposed.summary_text.clone(),
            files: proposed.changed_files.clone(),
            preview_label: "proposed patch",
            preview_lines: if proposed.preview_lines.is_empty() {
                vec![String::from("[no text preview available]")]
            } else {
                proposed.preview_lines.clone()
            },
            validation_line: proposed
                .validation_hint
                .clone()
                .unwrap_or_else(|| String::from("not available until the paused turn resumes")),
            primary_action_label: "Apply",
        };
    }
    if approval.tool_name == "apply_patch" {
        let path = approval
            .arguments
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("workspace file")
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
        let summary = if old_text.trim().is_empty() && !new_text.trim().is_empty() {
            format!("Probe wants to create or replace `{path}` before the turn resumes.")
        } else if !old_text.trim().is_empty() && new_text.trim().is_empty() {
            format!("Probe wants to remove text from `{path}` before the turn resumes.")
        } else {
            format!("Probe wants to update `{path}` before the turn resumes.")
        };
        return ApprovalPreview {
            title: "Review Changes",
            summary,
            files: vec![path],
            preview_label: "proposed patch",
            preview_lines: proposed_patch_preview_lines(old_text, new_text),
            validation_line: String::from("not available until the paused turn resumes"),
            primary_action_label: "Apply",
        };
    }

    let preview_lines = if approval.tool_name == "shell" {
        approval
            .arguments
            .get("command")
            .map(render_shell_command_preview_lines)
            .filter(|lines| !lines.is_empty())
            .unwrap_or_else(|| compact_json_lines(&approval.arguments, 6))
    } else {
        compact_json_lines(&approval.arguments, 6)
    };

    let files = approval_overlay_paths(approval);
    let summary = if approval.risk_class == ToolRiskClass::Write {
        if files.is_empty() {
            format!(
                "Probe wants approval before `{}` changes the workspace.",
                approval.tool_name
            )
        } else {
            format!(
                "Probe wants approval before `{}` touches {}.",
                approval.tool_name,
                summarize_inline_paths(files.as_slice(), 2)
            )
        }
    } else {
        format!(
            "Probe wants approval before `{}` continues.",
            approval.tool_name
        )
    };

    ApprovalPreview {
        title: if approval.risk_class == ToolRiskClass::Write {
            "Review Changes"
        } else {
            "Approval"
        },
        summary,
        files,
        preview_label: if approval.tool_name == "shell" {
            "command preview"
        } else {
            "request preview"
        },
        preview_lines,
        validation_line: String::from("not available until the paused turn resumes"),
        primary_action_label: if approval.risk_class == ToolRiskClass::Write {
            "Apply"
        } else {
            "Approve"
        },
    }
}

fn approval_overlay_paths(approval: &PendingToolApproval) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(path) = approval
        .arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
    {
        append_unique_path(&mut paths, path);
    }
    paths
}

fn render_shell_command_preview_lines(value: &serde_json::Value) -> Vec<String> {
    if let Some(command) = value.as_array() {
        let joined = command
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
            .join(" ");
        if !joined.is_empty() {
            return vec![preview(joined.as_str(), 96)];
        }
    }
    if let Some(command) = value.as_str() {
        return vec![preview(command, 96)];
    }
    Vec::new()
}

fn proposed_patch_preview_lines(old_text: &str, new_text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for line in compact_text_lines(old_text, 4) {
        lines.push(format!("- {}", display_patch_line(line.as_str())));
    }
    for line in compact_text_lines(new_text, 4) {
        lines.push(format!("+ {}", display_patch_line(line.as_str())));
    }
    if lines.is_empty() {
        lines.push(String::from("[no text preview available]"));
    }
    if lines.len() > 10 {
        lines.truncate(10);
        lines.push(String::from("..."));
    }
    lines
}

fn display_patch_line(line: &str) -> String {
    if line.is_empty() {
        String::from("[blank]")
    } else {
        preview(line, 96)
    }
}

fn render_stream_active_turn(
    stream: &AssistantStreamState,
    lane_label: &str,
    operator_backend: Option<&ServerOperatorSummary>,
    activity: Option<&RuntimeActivity>,
    action_hint: &str,
) -> ActiveTurn {
    let display_text = normalize_openai_stream_display_text(stream.assistant_text.as_str());
    let is_waiting =
        display_text.is_empty() && stream.tool_calls.is_empty() && stream.failure.is_none();
    let mut body = Vec::new();
    let failure_summary = stream
        .failure
        .as_deref()
        .map(|error| stream_failure_summary(error, operator_backend));
    if let Some(error) = stream.failure.as_deref() {
        body.push(format!("lane: {lane_label}"));
        if let Some(backend_kind) = stream.backend_kind.as_deref() {
            body.push(format!("backend: {backend_kind}"));
        }
        if let Some(summary) = operator_backend {
            body.push(format!(
                "target: {} ({})",
                summary.endpoint_label(),
                summary.target_kind_label()
            ));
        }
        if let Some(summary) = failure_summary.as_ref() {
            body.push(summary.summary.clone());
            if let Some(detail) = summary.detail.as_deref() {
                body.push(format!("detail: {detail}"));
            }
            body.push(format!("next: {}", summary.next_step));
        } else {
            body.push(format!("detail: {error}"));
            body.push(format!("next: {action_hint}"));
        }
    }

    if !stream.tool_calls.is_empty() {
        for tool in &stream.tool_calls {
            body.push(format!(
                "{} {}",
                tool.tool_index + 1,
                tool.tool_name
                    .as_deref()
                    .map(display_runtime_tool_name)
                    .unwrap_or_else(|| String::from("unknown"))
            ));
            if let Some(arguments) = summarize_stream_tool_arguments(tool.arguments.as_str()) {
                body.push(format!("args: {arguments}"));
            }
        }
    }

    if !display_text.is_empty() {
        body.extend(split_text_lines(display_text.as_str()));
    }

    let role = if is_waiting {
        TranscriptRole::Assistant
    } else if display_text.is_empty() && !stream.tool_calls.is_empty() {
        TranscriptRole::Tool
    } else if stream.failure.is_some() {
        TranscriptRole::Status
    } else {
        TranscriptRole::Assistant
    };
    let title = if is_waiting {
        activity
            .map(|activity| runtime_activity_title(Some(activity), "Waiting for Reply"))
            .unwrap_or("Waiting for Reply")
    } else if stream.failure.is_some() {
        failure_summary
            .as_ref()
            .map_or("Backend Request Failed", |summary| summary.title)
    } else if display_text.is_empty() && !stream.tool_calls.is_empty() {
        activity
            .map(|activity| runtime_activity_title(Some(activity), "Streaming Tool Call"))
            .unwrap_or("Streaming Tool Call")
    } else {
        "Probe"
    };
    ActiveTurn::new(role, title, body)
}

fn stream_failure_summary(
    error: &str,
    operator_backend: Option<&ServerOperatorSummary>,
) -> crate::failure::RuntimeFailureSummary {
    let mut summary = classify_runtime_failure(error);
    if summary.title == "Backend Unavailable"
        && operator_backend.is_some_and(|backend| {
            backend.endpoint_label().contains("127.0.0.1")
                || backend.endpoint_label().contains("localhost")
                || backend.target_kind_label() == "localhost"
                || backend.target_kind_label() == "loopback_or_ssh_forward"
        })
    {
        summary.next_step = String::from("Start the local backend, or switch lanes with Tab");
    }
    summary
}

fn summarize_stream_tool_arguments(arguments: &str) -> Option<String> {
    let trimmed = arguments.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(preview(runtime_tool_argument_summary(&value).as_str(), 72));
    }
    let has_signal = trimmed
        .chars()
        .any(|character| character.is_ascii_alphanumeric() || "/._-".contains(character));
    if !has_signal {
        return None;
    }
    Some(preview(trimmed, 72))
}

fn action_needed_label(label: &str) -> String {
    if label.starts_with("action needed: ") {
        label.to_string()
    } else if let Some(tool_name) = label.strip_prefix("waiting for approval: ") {
        format!(
            "action needed: approve {}",
            display_runtime_tool_name(tool_name)
        )
    } else {
        String::from("action needed: review approval")
    }
}

fn compact_recovery_note(note: &str) -> String {
    if note.contains("pending approval is resolved") {
        String::from("resolve approval to continue this turn")
    } else if note.contains("restarted before this running turn completed") {
        String::from("the previous running turn stopped during restart; inspect before resuming")
    } else {
        preview(note, 72).to_string()
    }
}

fn compact_verification_line(receipt: &TaskFinalReceipt) -> String {
    match receipt.verification_status {
        TaskVerificationStatus::NotRun => match receipt.disposition {
            TaskReceiptDisposition::PendingApproval => String::from("not yet"),
            TaskReceiptDisposition::Stopped => String::from("not run (task stopped)"),
            _ => String::from("not run"),
        },
        TaskVerificationStatus::Passed => format!(
            "passed -> {}",
            summarize_receipt_commands(receipt.verification_commands.as_slice(), 2)
        ),
        TaskVerificationStatus::Failed => format!(
            "failed -> {}",
            summarize_receipt_commands(receipt.verification_commands.as_slice(), 2)
        ),
        TaskVerificationStatus::TimedOut => format!(
            "timed out -> {}",
            summarize_receipt_commands(receipt.verification_commands.as_slice(), 2)
        ),
        TaskVerificationStatus::Mixed => format!(
            "mixed -> {}",
            summarize_receipt_commands(receipt.verification_commands.as_slice(), 2)
        ),
    }
}

fn summarize_receipt_commands(
    commands: &[probe_protocol::session::TaskVerificationCommandSummary],
    max_items: usize,
) -> String {
    let mut items = commands
        .iter()
        .take(max_items)
        .map(|command| {
            let status = match command.status {
                TaskVerificationCommandStatus::Passed => "passed",
                TaskVerificationCommandStatus::Failed => "failed",
                TaskVerificationCommandStatus::TimedOut => "timed out",
            };
            let label = preview(command.command.as_str(), 48);
            if command.truncated_output {
                format!("{label} ({status}, truncated)")
            } else {
                format!("{label} ({status})")
            }
        })
        .collect::<Vec<_>>();
    let remaining = commands.len().saturating_sub(items.len());
    if remaining > 0 {
        items.push(format!("and {remaining} more"));
    }
    if items.is_empty() {
        String::from("none")
    } else {
        items.join(", ")
    }
}

fn runtime_activity_title<'a>(activity: Option<&'a RuntimeActivity>, fallback: &'a str) -> &'a str {
    match activity.map(|activity| activity.kind) {
        Some(RuntimeActivityKind::Queued) => "Queued Turn",
        Some(RuntimeActivityKind::Starting) => "Starting Turn",
        Some(RuntimeActivityKind::WaitingForBackend) => "Waiting for Backend",
        Some(RuntimeActivityKind::StreamingReply) => "Streaming Reply",
        Some(RuntimeActivityKind::UpdatingReply) => "Updating Reply",
        Some(RuntimeActivityKind::PlanningTool) => "Planning Tool Call",
        Some(RuntimeActivityKind::Reading) => "Reading Workspace",
        Some(RuntimeActivityKind::Editing) => "Editing Workspace",
        Some(RuntimeActivityKind::Validating) => "Running Validation",
        Some(RuntimeActivityKind::RunningTool) => "Running Tool",
        Some(RuntimeActivityKind::WaitingForApproval) => "Waiting for Approval",
        Some(RuntimeActivityKind::Finalizing) => "Finalizing Reply",
        Some(RuntimeActivityKind::Completed) => "Completed",
        Some(RuntimeActivityKind::Failed) => "Failed",
        Some(RuntimeActivityKind::Stopped) => "Stopped",
        None => fallback,
    }
}

fn display_runtime_tool_name(tool_name: &str) -> String {
    let Some(rest) = tool_name.strip_prefix("mcp__") else {
        return tool_name.to_string();
    };
    let mut parts = rest.splitn(2, "__");
    let Some(server) = parts.next() else {
        return tool_name.to_string();
    };
    let Some(tool) = parts.next() else {
        return tool_name.to_string();
    };
    format!(
        "MCP {} · {}",
        display_mcp_server_segment(server),
        display_mcp_tool_segment(tool)
    )
}

fn display_mcp_server_segment(value: &str) -> String {
    value.replace('_', "-")
}

fn display_mcp_tool_segment(value: &str) -> String {
    value.replace('_', "/")
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

fn render_usage_counts_lines(summary: &UsageCountsSummary) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!(
        "  total: {}",
        render_usage_value(summary.total_tokens, summary.total_truth.as_deref())
    ));
    lines.push(format!(
        "  prompt: {}",
        render_usage_value(summary.prompt_tokens, summary.prompt_truth.as_deref())
    ));
    lines.push(format!(
        "  completion: {}",
        render_usage_value(
            summary.completion_tokens,
            summary.completion_truth.as_deref()
        )
    ));
    lines
}

fn runtime_tool_body_lines(
    round_trip: usize,
    tool: &probe_core::tools::ExecutedToolCall,
) -> Vec<String> {
    let _ = round_trip;
    match tool.tool_execution.policy_decision {
        probe_protocol::session::ToolPolicyDecision::Paused => vec![
            runtime_tool_subject(tool),
            format!(
                "needs approval: {}",
                compact_runtime_policy_reason(
                    tool.tool_execution.reason.as_deref(),
                    tool.name.as_str()
                )
            ),
        ],
        probe_protocol::session::ToolPolicyDecision::Refused => vec![
            runtime_tool_subject(tool),
            format!(
                "blocked: {}",
                compact_runtime_policy_reason(
                    tool.tool_execution.reason.as_deref(),
                    tool.name.as_str()
                )
            ),
        ],
        probe_protocol::session::ToolPolicyDecision::AutoAllow
        | probe_protocol::session::ToolPolicyDecision::Approved => {
            compact_runtime_tool_output_lines(tool)
        }
    }
}

fn runtime_tool_call_entry(tool_name: &str, arguments: &serde_json::Value) -> TranscriptEntry {
    TranscriptEntry::tool_call(
        display_runtime_tool_name(tool_name),
        vec![runtime_tool_argument_summary(arguments)],
    )
}

fn runtime_tool_result_entry(
    round_trip: usize,
    tool: &probe_core::tools::ExecutedToolCall,
) -> TranscriptEntry {
    match tool.tool_execution.policy_decision {
        probe_protocol::session::ToolPolicyDecision::Paused => TranscriptEntry::approval_pending(
            display_runtime_tool_name(tool.name.as_str()),
            runtime_tool_body_lines(round_trip, tool),
        ),
        probe_protocol::session::ToolPolicyDecision::Refused => TranscriptEntry::tool_refused(
            display_runtime_tool_name(tool.name.as_str()),
            runtime_tool_body_lines(round_trip, tool),
        ),
        probe_protocol::session::ToolPolicyDecision::AutoAllow
        | probe_protocol::session::ToolPolicyDecision::Approved => TranscriptEntry::tool_result(
            display_runtime_tool_name(tool.name.as_str()),
            runtime_tool_body_lines(round_trip, tool),
        ),
    }
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

fn compact_text_lines(value: &str, max_lines: usize) -> Vec<String> {
    let mut lines = value.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        lines.push(String::from("..."));
    }
    lines
}

fn runtime_tool_subject(tool: &probe_core::tools::ExecutedToolCall) -> String {
    tool.tool_execution
        .command
        .as_ref()
        .cloned()
        .unwrap_or_else(|| runtime_tool_argument_summary(&tool.arguments))
}

fn runtime_tool_argument_summary(arguments: &serde_json::Value) -> String {
    if let Some(command) = arguments.get("command").and_then(serde_json::Value::as_str) {
        return command.to_string();
    }
    if let Some(path) = arguments.get("path").and_then(serde_json::Value::as_str) {
        if let Some(pattern) = arguments.get("pattern").and_then(serde_json::Value::as_str) {
            return format!("{pattern} in {path}");
        }
        if let Some(start_line) = arguments
            .get("start_line")
            .and_then(serde_json::Value::as_u64)
            && let Some(end_line) = arguments
                .get("end_line")
                .and_then(serde_json::Value::as_u64)
        {
            return format!("{path}:{start_line}-{end_line}");
        }
        return path.to_string();
    }
    if let Some(question) = arguments
        .get("question")
        .and_then(serde_json::Value::as_str)
    {
        return preview(question, 72);
    }
    preview(
        serde_json::to_string(arguments)
            .unwrap_or_else(|_| arguments.to_string())
            .as_str(),
        72,
    )
}

fn compact_runtime_tool_output_lines(tool: &probe_core::tools::ExecutedToolCall) -> Vec<String> {
    if let Some(mut lines) = structured_runtime_tool_output_lines(&tool.output) {
        let subject = runtime_tool_subject(tool);
        if lines
            .first()
            .is_some_and(|first| first.as_str() == subject.as_str())
        {
            lines.remove(0);
        }
        if !tool.tool_execution.files_changed.is_empty() {
            lines.insert(
                0,
                format!(
                    "updated: {}",
                    summarize_inline_paths(tool.tool_execution.files_changed.as_slice(), 3)
                ),
            );
        }
        if !lines.is_empty() {
            return lines;
        }
    }

    let subject = runtime_tool_subject(tool);
    let summary = preview(
        tool_result_model_text(tool.name.as_str(), &tool.output).as_str(),
        120,
    );
    let mut lines = vec![subject];
    if lines
        .last()
        .map_or(true, |existing| existing.as_str() != summary.as_str())
    {
        lines.push(summary);
    }
    if !tool.tool_execution.files_changed.is_empty() {
        lines.push(format!(
            "updated: {}",
            summarize_inline_paths(tool.tool_execution.files_changed.as_slice(), 3)
        ));
    }
    lines
}

fn structured_runtime_tool_output_lines(value: &serde_json::Value) -> Option<Vec<String>> {
    if let (Some(server_name), Some(tool_name)) = (
        value.get("server_name").and_then(serde_json::Value::as_str),
        value.get("tool").and_then(serde_json::Value::as_str),
    ) {
        let mut lines = vec![format!("MCP {server_name} · {tool_name}")];
        if let Some(result) = value.get("result") {
            if let Some(content) = result.get("content").and_then(serde_json::Value::as_array) {
                for item in content {
                    if let Some(text) = item.get("text").and_then(serde_json::Value::as_str) {
                        lines.extend(split_text_lines(text));
                    }
                }
            }
        }
        if lines.len() > 1 {
            return Some(lines);
        }
    }
    if let Some(path) = value.get("path").and_then(serde_json::Value::as_str) {
        let start_line = value.get("start_line").and_then(serde_json::Value::as_u64);
        let end_line = value.get("end_line").and_then(serde_json::Value::as_u64);
        let mut lines = vec![match (start_line, end_line) {
            (Some(start), Some(end)) => format!("{path}:{start}-{end}"),
            _ => path.to_string(),
        }];
        if let Some(content) = value.get("content").and_then(serde_json::Value::as_str) {
            lines.extend(compact_text_lines(content, 4));
        }
        return Some(lines);
    }
    if let Some(command) = value.get("command").and_then(serde_json::Value::as_str) {
        let mut lines = vec![command.to_string()];
        if let Some(stdout) = value.get("stdout").and_then(serde_json::Value::as_str)
            && !stdout.trim().is_empty()
        {
            lines.extend(compact_text_lines(stdout, 4));
            return Some(lines);
        }
        if let Some(stderr) = value.get("stderr").and_then(serde_json::Value::as_str)
            && !stderr.trim().is_empty()
        {
            lines.extend(compact_text_lines(stderr, 4));
            return Some(lines);
        }
        return Some(lines);
    }
    if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
        return Some(vec![format!("error: {}", preview(error, 96))]);
    }
    if let Some(entries) = value.get("entries").and_then(serde_json::Value::as_array) {
        let mut lines = vec![format!("listed {} entries", entries.len())];
        for entry in entries.iter().take(4).filter_map(serde_json::Value::as_str) {
            lines.push(entry.to_string());
        }
        return Some(lines);
    }
    if let Some(matches) = value.get("matches").and_then(serde_json::Value::as_array) {
        let mut lines = vec![format!("found {} matches", matches.len())];
        for summary in matches.iter().take(3).filter_map(|entry| {
            let path = entry.get("path").and_then(serde_json::Value::as_str)?;
            let line = entry.get("line").and_then(serde_json::Value::as_u64)?;
            Some(format!("{path}:{line}"))
        }) {
            lines.push(summary);
        }
        return Some(lines);
    }
    if let Some(answer) = value
        .get("oracle_answer")
        .and_then(serde_json::Value::as_str)
    {
        return Some(compact_text_lines(answer, 4));
    }
    if let Some(analysis) = value.get("analysis").and_then(serde_json::Value::as_str) {
        return Some(compact_text_lines(analysis, 4));
    }
    None
}

fn compact_runtime_policy_reason(reason: Option<&str>, tool_name: &str) -> String {
    let fallback = "approval required";
    let value = reason.unwrap_or(fallback);
    let prefix = format!("tool `{tool_name}` requires ");
    if let Some(stripped) = value.strip_prefix(prefix.as_str()) {
        return stripped.to_string();
    }
    let display_name = display_runtime_tool_name(tool_name);
    let display_prefix = format!("tool `{display_name}` requires ");
    if let Some(stripped) = value.strip_prefix(display_prefix.as_str()) {
        return stripped.to_string();
    }
    if value == "tool execution blocked by local approval policy" {
        return fallback.to_string();
    }
    value.to_string()
}

fn render_approval_posture_lines(tool_loop: Option<&ToolLoopConfig>) -> Vec<String> {
    let Some(tool_loop) = tool_loop else {
        return vec![String::from("tools: off"), String::from("approvals: n/a")];
    };

    let approval = &tool_loop.approval;
    let write = approval_action_label(approval.allow_write_tools, approval.denied_action);
    let network = approval_action_label(approval.allow_network_shell, approval.denied_action);
    let destructive =
        approval_action_label(approval.allow_destructive_shell, approval.denied_action);

    let summary = if write == network && network == destructive {
        format!("approvals: write/network/destructive {write}")
    } else {
        format!("approvals: write {write}; net {network}; destructive {destructive}")
    };

    vec![String::from("tools: on"), summary]
}

fn approval_action_label(allowed: bool, denied_action: ToolDeniedAction) -> &'static str {
    if allowed {
        "auto"
    } else {
        match denied_action {
            ToolDeniedAction::Refuse => "refuse",
            ToolDeniedAction::Pause => "approval",
        }
    }
}

fn render_remote_contract_lines(summary: &ServerOperatorSummary) -> Vec<&'static str> {
    match summary.backend_kind {
        probe_protocol::backend::BackendKind::OpenAiCodexSubscription => vec![
            "model inference is sent to ChatGPT's hosted Codex backend",
            "the local OpenAI subscription token is attached from Probe auth state",
            "tool execution, approvals, transcripts, and the TUI remain local to Probe",
        ],
        probe_protocol::backend::BackendKind::OpenAiChatCompletions
            if matches!(
                summary.control_plane,
                Some(probe_protocol::backend::BackendControlPlaneKind::PsionicInferenceMesh)
            ) =>
        {
            vec![
                "Probe discovers routed inventory from Psionic mesh management state before attach",
                "mesh startup, warmup, and route execution remain owned by Psionic",
                "tool execution, approvals, transcripts, and the TUI remain local to Probe",
            ]
        }
        probe_protocol::backend::BackendKind::OpenAiChatCompletions
        | probe_protocol::backend::BackendKind::AppleFmBridge => vec![
            "only model inference is delegated to the backend target",
            "tool execution, approvals, transcripts, and the TUI remain local to Probe",
            "switch targets through saved backend configs or `probe server`",
        ],
    }
}

fn render_backend_kind(value: probe_protocol::backend::BackendKind) -> &'static str {
    match value {
        probe_protocol::backend::BackendKind::OpenAiChatCompletions => "openai_chat_completions",
        probe_protocol::backend::BackendKind::OpenAiCodexSubscription => {
            "openai_codex_subscription"
        }
        probe_protocol::backend::BackendKind::AppleFmBridge => "apple_fm_bridge",
    }
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

fn split_text_lines(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    value.split('\n').map(ToOwned::to_owned).collect()
}

fn assistant_closeout_lines(value: &str) -> Vec<String> {
    let normalized = normalize_openai_assistant_text(value);
    let lines = split_text_lines(normalized.as_str());
    if lines.is_empty() {
        vec![String::from("[empty assistant reply]")]
    } else {
        lines
    }
}

fn runtime_tool_target_paths(
    tool_name: &str,
    arguments: &serde_json::Value,
    tool: Option<&probe_core::tools::ExecutedToolCall>,
) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(tool) = tool {
        append_unique_paths(&mut paths, tool.tool_execution.files_changed.as_slice());
        if paths.is_empty() && tool_name == "apply_patch" {
            append_unique_paths(&mut paths, tool.tool_execution.files_touched.as_slice());
        }
    }
    if paths.is_empty() && tool_name == "apply_patch" {
        if let Some(path) = arguments.get("path").and_then(serde_json::Value::as_str) {
            append_unique_path(&mut paths, path);
        }
    }
    paths
}

fn append_unique_paths(target: &mut Vec<String>, paths: &[String]) {
    for path in paths {
        append_unique_path(target, path);
    }
}

fn append_unique_path(target: &mut Vec<String>, path: &str) {
    if path.is_empty() || target.iter().any(|existing| existing == path) {
        return;
    }
    target.push(path.to_string());
}

fn summarize_inline_paths(paths: &[String], max_items: usize) -> String {
    let mut items = paths.iter().take(max_items).cloned().collect::<Vec<_>>();
    let remaining = paths.len().saturating_sub(items.len());
    if remaining > 0 {
        items.push(format!("+{remaining} more"));
    }
    items.join(", ")
}

fn shorten_path_for_display(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let suffix = value
        .chars()
        .rev()
        .take(max_chars.saturating_sub(1))
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("…{suffix}")
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
