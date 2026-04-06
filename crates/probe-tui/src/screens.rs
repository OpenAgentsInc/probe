use std::collections::VecDeque;
use std::path::PathBuf;

use probe_core::backend_profiles::resolved_reasoning_level_for_backend;
use probe_core::provider::{normalize_openai_assistant_text, normalize_openai_stream_display_text};
use probe_core::runtime::{RuntimeEvent, StreamedToolCallDelta};
use probe_core::server_control::ServerOperatorSummary;
use probe_core::tools::tool_result_model_text;
use probe_core::tools::{ToolDeniedAction, ToolLoopConfig};
use probe_openai_auth::OpenAiCodexAuthStore;
use probe_protocol::runtime::{RuntimeActivity, RuntimeActivityKind};
use probe_protocol::session::{
    PendingToolApproval, TaskFinalReceipt, TaskReceiptDisposition, TaskVerificationCommandStatus,
    TaskVerificationStatus, TaskWorkspaceSummary, TaskWorkspaceSummaryStatus,
    ToolApprovalResolution,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Wrap};

use crate::bottom_pane::ComposerSubmission;
use crate::event::UiEvent;
use crate::message::{
    AppMessage, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
    AppleFmFailureSummary, AppleFmUsageSummary, ProbeRuntimeTurnConfig, SessionUsageSummary,
    UsageCountsSummary,
};
use crate::transcript::{ActiveTurn, RetainedTranscript, TranscriptEntry, TranscriptRole};
use crate::widgets::{InfoPanel, ModalCard, TabStrip};

const MAX_EVENT_LOG: usize = 16;
const LINE_SCROLL_STEP: u16 = 3;
const PAGE_SCROLL_STEP: u16 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenId {
    Chat,
    Help,
    SetupOverlay,
    ApprovalOverlay,
    PlanModeOverlay,
    ModelPickerOverlay,
    ReasoningPickerOverlay,
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
            Self::SetupOverlay => "backend overlay",
            Self::ApprovalOverlay => "approval overlay",
            Self::PlanModeOverlay => "plan mode picker",
            Self::ModelPickerOverlay => "model picker",
            Self::ReasoningPickerOverlay => "reasoning picker",
            Self::WorkspaceOverlay => "workspace overlay",
            Self::ResumeOverlay => "resume overlay",
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
    backend_kind: Option<String>,
    activity: Option<RuntimeActivity>,
    round_trip: Option<usize>,
    active_tool: Option<String>,
    active_tool_targets: Vec<String>,
    pending_approvals: Vec<PendingToolApproval>,
    latest_task_workspace_summary: Option<TaskWorkspaceSummary>,
    latest_task_receipt: Option<TaskFinalReceipt>,
    recovery_note: Option<String>,
    usage: SessionUsageSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssistantStreamMode {
    Delta,
    Snapshot,
}

impl AssistantStreamMode {
    fn label(self) -> &'static str {
        match self {
            Self::Delta => "delta",
            Self::Snapshot => "snapshot",
        }
    }
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
    OpenSetupOverlay,
    OpenApprovalOverlay,
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
    OpenMcpProviderCommandOverlay,
    OpenMcpManualEditorOverlay,
    ImportMcpProviderCommand {
        command: String,
    },
    SaveMcpServer {
        name: String,
        transport: McpServerTransportDraft,
        target: String,
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
    Setup(SetupOverlay),
    Approval(ApprovalOverlay),
    PlanMode(PlanModeOverlay),
    ModelPicker(ModelPickerOverlay),
    ReasoningPicker(ReasoningPickerOverlay),
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
            Self::Setup(_) => ScreenId::SetupOverlay,
            Self::Approval(_) => ScreenId::ApprovalOverlay,
            Self::PlanMode(_) => ScreenId::PlanModeOverlay,
            Self::ModelPicker(_) => ScreenId::ModelPickerOverlay,
            Self::ReasoningPicker(_) => ScreenId::ReasoningPickerOverlay,
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
            Self::Setup(screen) => screen.handle_event(event),
            Self::Approval(screen) => screen.handle_event(event),
            Self::PlanMode(screen) => screen.handle_event(event),
            Self::ModelPicker(screen) => screen.handle_event(event),
            Self::ReasoningPicker(screen) => screen.handle_event(event),
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
            Self::Setup(screen) => screen.render(frame, area, stack_depth, base_screen),
            Self::Approval(screen) => screen.render(frame, area, stack_depth),
            Self::PlanMode(screen) => screen.render(frame, area, stack_depth),
            Self::ModelPicker(screen) => screen.render(frame, area, stack_depth),
            Self::ReasoningPicker(screen) => screen.render(frame, area, stack_depth),
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
            | Self::Setup(_)
            | Self::Approval(_)
            | Self::PlanMode(_)
            | Self::ModelPicker(_)
            | Self::ReasoningPicker(_)
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
            | Self::Setup(_)
            | Self::Approval(_)
            | Self::PlanMode(_)
            | Self::ModelPicker(_)
            | Self::ReasoningPicker(_)
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
        carry_forward_summary: Option<&str>,
    ) {
        self.operator_mode_label = mode_label.into();
        self.carry_forward_summary = carry_forward_summary
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
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
        self.runtime.recovery_note = None;
        self.approval_posture_lines = render_approval_posture_lines(config.tool_loop.as_ref());
        self.operator_backend = Some(summary);
        self.set_operator_controls(mode_label.to_string(), carry_forward_summary);
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

    pub fn carries_compacted_context(&self) -> bool {
        self.carry_forward_summary.is_some()
    }

    pub fn compact_summary_text(&self) -> String {
        let mut sections = Vec::new();
        sections.push(format!("Mode: {}.", self.operator_mode_label));
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
            let mode = stream.mode.label();
            let chars = stream.assistant_text.chars().count();
            let tool_calls = stream.tool_calls.len();
            let mut parts = vec![
                format!("backend: {backend}"),
                format!("target: {target}"),
                format!("model: {}", preview(model, 28)),
                format!("activity: {activity}"),
                format!("round: {}", stream.round_trip),
                format!("stream: {mode}"),
                format!("chars: {chars}"),
            ];
            if tool_calls > 0 {
                parts.push(format!("tool_deltas: {tool_calls}"));
            }
            if let Some(ms) = stream.first_chunk_ms {
                parts.push(format!("ttft_ms: {ms}"));
            }
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
                self.record_worker_event(format!("tool call requested: {tool_name}"));
                format!("tool call requested: {tool_name}")
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
                self.record_worker_event(format!("tool execution started: {tool_name}"));
                format!("tool execution started: {tool_name}")
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
            AppMessage::ProbeRuntimeSessionReady {
                session_id,
                profile_name,
                model_id,
                cwd,
                runtime_activity,
                latest_task_workspace_summary,
                latest_task_receipt,
                recovery_note,
            } => {
                self.runtime = ProbeRuntimeState {
                    session_id: Some(session_id.clone()),
                    profile_name: Some(profile_name.clone()),
                    model_id: Some(model_id.clone()),
                    cwd: Some(cwd),
                    backend_kind: self.runtime.backend_kind.clone(),
                    activity: runtime_activity.or_else(|| self.runtime.activity.clone()),
                    round_trip: self.runtime.round_trip,
                    active_tool: self.runtime.active_tool.clone(),
                    active_tool_targets: self.runtime.active_tool_targets.clone(),
                    pending_approvals: self.runtime.pending_approvals.clone(),
                    latest_task_workspace_summary,
                    latest_task_receipt,
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
            UiEvent::OpenSetupOverlay => ScreenOutcome::with_status(
                ScreenAction::OpenSetupOverlay,
                String::from("opened backend overlay"),
            ),
            UiEvent::OpenApprovalOverlay => ScreenOutcome::with_status(
                ScreenAction::OpenApprovalOverlay,
                String::from("opened approval overlay"),
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
        if area.width < 74 {
            let scroll_y = self.transcript_scroll_y(body.lines.len(), area.height);
            let title = self.transcript_panel_title();
            InfoPanel::new(title.as_str(), body)
                .with_scroll(scroll_y)
                .render(frame, area);
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
            lines.push(Line::from(format!("active_tool: {tool}")));
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

    fn operator_endpoint_label(&self) -> String {
        self.operator_backend
            .as_ref()
            .map(ServerOperatorSummary::endpoint_label)
            .unwrap_or_else(|| String::from("pending"))
    }

    fn operator_transport_label(&self) -> String {
        self.operator_backend
            .as_ref()
            .map(|summary| compact_target_kind_label(summary.target_kind_label()))
            .unwrap_or("pending")
            .to_string()
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
            if let Some(approval) = self.runtime.pending_approvals.first() {
                return format!("action needed: approve {}", approval.tool_name);
            }
            return String::from("action needed: review approval");
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
        if !self.runtime.pending_approvals.is_empty()
            || matches!(
                self.runtime_activity_kind(),
                Some(RuntimeActivityKind::WaitingForApproval)
            )
        {
            return String::from("Ctrl+A opens approval; Enter decides");
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
        let Some(receipt) = self.runtime.latest_task_receipt.as_ref() else {
            return vec![String::from("verify: none yet")];
        };
        let mut lines = vec![format!("verify: {}", compact_verification_line(receipt))];
        if let Some(reason) = receipt.uncertainty_reasons.first() {
            lines.push(format!("risk: {}", preview(reason, 72)));
        }
        lines
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
        lines.extend([
            Line::from(format!("lane: {}", self.active_lane_label())),
            Line::from(format!("target: {}", self.operator_endpoint_label())),
            Line::from(format!("transport: {}", self.operator_transport_label())),
            Line::from(format!("cwd: {}", self.workspace_label())),
        ]);
        lines.push(Line::from(format!("mode: {}", self.operator_mode_label)));
        if self.carries_compacted_context() {
            lines.push(Line::from(String::from("context: compact summary")));
        } else if self.runtime.session_id.is_none() && self.committed_transcript_entry_count() == 0
        {
            lines.push(Line::from(String::from("context: fresh session")));
        }
        lines.extend(self.approval_posture_lines.iter().cloned().map(Line::from));
        if let Some(line) = self.render_recovery_line() {
            lines.push(Line::from(line));
        }
        lines.extend(
            self.render_task_workspace_lines()
                .into_iter()
                .map(Line::from),
        );
        lines.extend(self.render_task_receipt_lines().into_iter().map(Line::from));
        lines.push(Line::from("keys: Tab lanes · Ctrl+S backend"));
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
            Line::from("Keys: Tab lanes · Ctrl+S backend"),
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
        self.transcript.as_conversation_text()
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
            Line::from("Ctrl+R            run Apple FM check"),
            Line::from("Ctrl+O / Ctrl+T   attachment / notes"),
            Line::from(""),
            Line::from("Inspect"),
            Line::from(""),
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
            Self::Approve => "approve",
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
    pub turns: u64,
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
                        String::from("there are no saved sessions to resume"),
                    );
                };
                ScreenOutcome::with_action_and_command(
                    ScreenAction::CloseModal,
                    format!("resuming {}", session.title),
                    ScreenCommand::ResumeDetachedSession {
                        session_id: session.id.clone(),
                    },
                )
            }
            UiEvent::Dismiss => ScreenOutcome::with_status(
                ScreenAction::CloseModal,
                String::from("dismissed resume picker"),
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
            Line::from("Resume a previous Probe session from this Probe home."),
            Line::from("Use Up/Down to choose. Enter attaches. Esc closes."),
            Line::from(""),
            Line::from(format!("saved sessions: {}", self.sessions.len())),
            Line::from(""),
        ];
        if self.sessions.is_empty() {
            lines.push(Line::from("No saved sessions yet."));
        } else {
            for (index, session) in self.sessions.iter().enumerate() {
                let marker = if index == self.selected { ">" } else { " " };
                lines.push(Line::from(format!(
                    "{marker} {}  {}",
                    session.title, session.backend
                )));
            }
            if let Some(session) = self.sessions.get(self.selected) {
                lines.push(Line::from(""));
                lines.push(Line::from(format!("selected: {}", session.title)));
                lines.push(Line::from(format!("  session: {}", session.id)));
                lines.push(Line::from(format!("  backend: {}", session.backend)));
                lines.push(Line::from(format!("  cwd: {}", session.cwd)));
                lines.push(Line::from(format!("  turns: {}", session.turns)));
                lines.push(Line::from(""));
                lines.push(Line::from(
                    "next: Enter attaches this session to the matching lane.",
                ));
            }
        }
        let content = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        ModalCard::new("Resume", content).render(frame, area);
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
            Line::from(""),
        ];
        lines.push(Line::from(format!(
            "saved MCP servers: {} configured, {} enabled",
            self.configured_count, self.enabled_count
        )));
        lines.push(Line::from(""));
        for (index, card) in self.cards.iter().enumerate() {
            let marker = if index == self.selected { ">" } else { " " };
            lines.push(Line::from(format!(
                "{marker} {}  {}",
                card.label, card.status
            )));
        }
        if let Some(card) = self.cards.get(self.selected) {
            lines.push(Line::from(""));
            lines.push(Line::from(format!("selected: {}", card.label)));
            for line in &card.detail_lines {
                lines.push(Line::from(format!("  {line}")));
            }
            lines.push(Line::from(""));
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
            UiEvent::ComposerSubmit => ScreenOutcome::with_status(
                ScreenAction::None,
                String::from("use E to enable, D to disable, or R to remove the selected MCP"),
            ),
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
                "Up/Down choose a server. E enables. D disables. R removes. A adds. Esc returns.",
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
                for line in &server.detail_lines {
                    lines.push(Line::from(format!("  {line}")));
                }
                lines.push(Line::from(""));
                let next = if server.enabled {
                    "next: D disables this server. R removes it."
                } else {
                    "next: E enables this server. R removes it."
                };
                lines.push(Line::from(next));
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
                    ScreenCommand::OpenMcpManualEditorOverlay,
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
    selected: McpEditorField,
    name: String,
    transport: McpServerTransportDraft,
    target: String,
    provider_command: Option<String>,
}

impl McpEditorOverlay {
    pub fn new() -> Self {
        Self {
            selected: McpEditorField::Name,
            name: String::new(),
            transport: McpServerTransportDraft::Stdio,
            target: String::new(),
            provider_command: None,
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
                    McpEditorField::Target => self.target.push(ch),
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
                    McpEditorField::Target => self.target.push_str(payload.as_str()),
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
            Line::from("Add a manual MCP integration entry."),
            Line::from("This path is for when you know the final launch command or MCP URL."),
            Line::from("Tab or Up/Down changes fields. Enter saves. Esc cancels."),
            Line::from(""),
        ];
        if let Some(command) = &self.provider_command {
            lines.push(Line::from("provider command reference:"));
            lines.push(Line::from(format!("  {command}")));
            lines.push(Line::from(
                "Probe cannot import provider setup commands automatically yet, so enter the final launch command or URL below.",
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
            "saved entries are registry state only until generic MCP runtime support lands.",
        ));
        let content = Paragraph::new(Text::from(lines));
        ModalCard::new("Manual MCP Setup", content).render(frame, area);
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
            Line::from("Review this tool request."),
            Line::from(""),
            Line::from(format!("tool: {}", self.approval.tool_name)),
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
            Line::from("preview"),
        ];
        for line in compact_json_lines(&self.approval.arguments, 5) {
            lines.push(Line::from(format!("  {line}")));
        }
        lines.extend([
            Line::from(""),
            Line::from(format!("{approve_marker} Approve")),
            Line::from(format!("{reject_marker} Reject")),
            Line::from(""),
            Line::from("Tab changes selection. Enter decides. Esc closes."),
            Line::from(format!("stack depth: {stack_depth}")),
        ]);
        let content = Paragraph::new(Text::from(lines));
        ModalCard::new("Approval", content).render(frame, area);
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
        body.push(format!("detail: {error}"));
        body.push(format!("next: {action_hint}"));
    }

    if !stream.tool_calls.is_empty() {
        for tool in &stream.tool_calls {
            body.push(format!(
                "{} {}",
                tool.tool_index + 1,
                tool.tool_name.as_deref().unwrap_or("unknown")
            ));
            if let Some(call_id) = tool.call_id.as_deref() {
                body.push(format!("call: {}", preview(call_id, 48)));
            }
            if !tool.arguments.is_empty() {
                body.push(format!("args: {}", tool.arguments));
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
        "Backend Request Failed"
    } else if display_text.is_empty() && !stream.tool_calls.is_empty() {
        activity
            .map(|activity| runtime_activity_title(Some(activity), "Streaming Tool Call"))
            .unwrap_or("Streaming Tool Call")
    } else {
        "Probe"
    };
    ActiveTurn::new(role, title, body)
}

fn action_needed_label(label: &str) -> String {
    if label.starts_with("action needed: ") {
        label.to_string()
    } else if let Some(tool_name) = label.strip_prefix("waiting for approval: ") {
        format!("action needed: approve {tool_name}")
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
        tool_name.to_string(),
        vec![runtime_tool_argument_summary(arguments)],
    )
}

fn runtime_tool_result_entry(
    round_trip: usize,
    tool: &probe_core::tools::ExecutedToolCall,
) -> TranscriptEntry {
    match tool.tool_execution.policy_decision {
        probe_protocol::session::ToolPolicyDecision::Paused => TranscriptEntry::approval_pending(
            tool.name.clone(),
            runtime_tool_body_lines(round_trip, tool),
        ),
        probe_protocol::session::ToolPolicyDecision::Refused => TranscriptEntry::tool_refused(
            tool.name.clone(),
            runtime_tool_body_lines(round_trip, tool),
        ),
        probe_protocol::session::ToolPolicyDecision::AutoAllow
        | probe_protocol::session::ToolPolicyDecision::Approved => TranscriptEntry::tool_result(
            tool.name.clone(),
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

fn compact_target_kind_label(value: &str) -> &'static str {
    match value {
        "loopback_or_ssh_forward" => "loopback/ssh",
        "remote_https" => "remote_https",
        "localhost" => "localhost",
        _ => "custom",
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
