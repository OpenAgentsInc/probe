use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use probe_protocol::backend::BackendProfile;
use probe_protocol::session::{
    ToolApprovalResolution, ToolApprovalState, ToolExecutionRecord, ToolPolicyDecision,
    ToolRiskClass,
};
use probe_provider_openai::{
    ChatNamedToolChoice, ChatNamedToolChoiceFunction, ChatToolCall, ChatToolChoice,
    ChatToolDefinition, ChatToolDefinitionEnvelope,
};
use wait_timeout::ChildExt;

use crate::long_context::{
    LongContextEscalationContext, heuristic_long_context_escalation, is_long_context_task_kind,
};
use crate::provider::{PlainTextMessage, complete_plain_text};

const READ_FILE_DEFAULT_MAX_LINES: u64 = 200;
const LIST_FILES_DEFAULT_MAX_DEPTH: u64 = 4;
const LIST_FILES_DEFAULT_MAX_ENTRIES: usize = 200;
const CODE_SEARCH_DEFAULT_MAX_MATCHES: usize = 50;
const SHELL_DEFAULT_TIMEOUT_SECS: u64 = 5;
const SHELL_MAX_OUTPUT_CHARS: usize = 4_000;
const TOOL_MODEL_TEXT_MAX_CHARS: usize = 3_000;
const LONG_CONTEXT_DEFAULT_MAX_LINES_PER_FILE: u64 = 160;
const LONG_CONTEXT_DEFAULT_MAX_EVIDENCE_FILES: usize = 6;

pub type ToolHandler = fn(
    &ToolExecutionContext,
    &serde_json::Value,
) -> Result<ToolInvocationOutcome, ToolInvocationError>;

#[derive(Clone, Copy, Debug)]
enum RegisteredToolRisk {
    Fixed(ToolRiskClass),
    Shell,
}

#[derive(Clone, Debug)]
struct RegisteredTool {
    definition: ChatToolDefinition,
    handler: ToolHandler,
    risk: RegisteredToolRisk,
}

#[derive(Clone, Debug)]
pub struct ToolRegistry {
    name: String,
    tools: BTreeMap<String, RegisteredTool>,
}

#[derive(Clone, Debug)]
pub struct ToolExecutionSession {
    registry: ToolRegistry,
    context: ToolExecutionContext,
    approval: ToolApprovalConfig,
    oracle_calls_remaining: usize,
    long_context_calls_remaining: usize,
    session_summary: SessionSummary,
    accepted_patch_summary: AcceptedPatchSummary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolExecutionContext {
    cwd: PathBuf,
    oracle: Option<ToolOracleContext>,
    long_context: Option<ToolLongContextContext>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeToolChoice {
    None,
    Auto,
    Required,
    Named(String),
}

#[derive(Clone, Debug)]
pub struct ToolLoopConfig {
    pub registry: ToolRegistry,
    pub tool_choice: ProbeToolChoice,
    pub parallel_tool_calls: bool,
    pub max_model_round_trips: usize,
    pub approval: ToolApprovalConfig,
    pub oracle: Option<ToolOracleConfig>,
    pub long_context: Option<ToolLongContextConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutedToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub output: serde_json::Value,
    pub tool_execution: ToolExecutionRecord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolDeniedAction {
    Refuse,
    Pause,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolApprovalConfig {
    pub allow_write_tools: bool,
    pub allow_network_shell: bool,
    pub allow_destructive_shell: bool,
    pub denied_action: ToolDeniedAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOracleConfig {
    pub profile: BackendProfile,
    pub max_calls: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOracleContext {
    profile: BackendProfile,
    max_calls: usize,
    calls_used: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolLongContextConfig {
    pub profile: BackendProfile,
    pub max_calls: usize,
    pub max_evidence_files: usize,
    pub max_lines_per_file: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolLongContextContext {
    profile: BackendProfile,
    max_calls: usize,
    calls_used: usize,
    max_evidence_files: usize,
    max_lines_per_file: u64,
    prompt_char_count: usize,
    files_listed: usize,
    files_searched: usize,
    files_read: usize,
    too_many_turns: bool,
    oracle_calls: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolInvocationOutcome {
    pub output: serde_json::Value,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub timed_out: Option<bool>,
    pub truncated: Option<bool>,
    pub bytes_returned: Option<u64>,
    pub files_touched: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolInvocationError {
    InvalidArguments(String),
    ExecutionFailed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: String,
    pub tool_calls: Vec<ExecutedToolCall>,
    pub output: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedPatchSummary {
    pub patch_id: String,
    pub tool_calls: Vec<ExecutedToolCall>,
    pub output: serde_json::Value,
}

impl Display for ToolInvocationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidArguments(message) => write!(f, "{message}"),
            Self::ExecutionFailed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ToolInvocationError {}

impl ToolExecutionContext {
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            oracle: None,
            long_context: None,
        }
    }

    #[must_use]
    pub fn cwd(&self) -> &Path {
        self.cwd.as_path()
    }

    #[must_use]
    pub fn base_dir(&self) -> PathBuf {
        if self.cwd.is_absolute() {
            self.cwd.clone()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(&self.cwd)
        }
    }

    #[must_use]
    pub fn with_oracle(mut self, oracle: ToolOracleContext) -> Self {
        self.oracle = Some(oracle);
        self
    }

    #[must_use]
    pub fn oracle(&self) -> Option<&ToolOracleContext> {
        self.oracle.as_ref()
    }

    #[must_use]
    pub fn with_long_context(mut self, long_context: ToolLongContextContext) -> Self {
        self.long_context = Some(long_context);
        self
    }

    #[must_use]
    pub fn long_context(&self) -> Option<&ToolLongContextContext> {
        self.long_context.as_ref()
    }
}

impl ToolExecutionSession {
    #[must_use]
    pub fn new(registry: ToolRegistry, context: ToolExecutionContext, approval: ToolApprovalConfig) -> Self {
        Self {
            registry,
            context,
            approval,
            oracle_calls_remaining: 0,
            long_context_calls_remaining: 0,
            session_summary: SessionSummary {
                session_id: String::new(),
                tool_calls: Vec::new(),
                output: serde_json::Value::Null,
            },
            accepted_patch_summary: AcceptedPatchSummary {
                patch_id: String::new(),
                tool_calls: Vec::new(),
                output: serde_json::Value::Null,
            },
        }
    }

    pub fn persist_session_summary(&self) {
        let summary = &self.session_summary;
        let file_path = Path::new("session_summary.json");
        let json = serde_json::to_string_pretty(summary).unwrap();
        fs::write(file_path, json).unwrap();
    }

    pub fn persist_accepted_patch_summary(&self) {
        let summary = &self.accepted_patch_summary;
        let file_path = Path::new("accepted_patch_summary.json");
        let json = serde_json::to_string_pretty(summary).unwrap();
        fs::write(file_path, json).unwrap();
    }
}