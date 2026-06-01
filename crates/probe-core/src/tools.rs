use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use probe_protocol::backend::BackendProfile;
use probe_protocol::session::{
    ToolApprovalResolution, ToolApprovalState, ToolExecutionRecord, ToolPermissionDecision,
    ToolPermissionOverride, ToolPolicyDecision, ToolRiskClass,
};
use probe_provider_openai::{
    ChatNamedToolChoice, ChatNamedToolChoiceFunction, ChatToolCall, ChatToolChoice,
    ChatToolDefinition, ChatToolDefinitionEnvelope,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
const TOOL_SUMMARY_MAX_CHARS: usize = 512;
const TOOL_SUMMARY_MAX_ARRAY_ITEMS: usize = 12;
const LONG_CONTEXT_DEFAULT_MAX_LINES_PER_FILE: u64 = 160;
const LONG_CONTEXT_DEFAULT_MAX_EVIDENCE_FILES: usize = 6;
const LEGAL_INSPECT_DEFAULT_MAX_BYTES: u64 = 1_000_000;
const LEGAL_INSPECT_HARD_MAX_BYTES: u64 = 2_000_000;

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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolExecutionContext {
    cwd: PathBuf,
    oracle: Option<ToolOracleContext>,
    long_context: Option<ToolLongContextContext>,
    write_gate: Option<ToolWriteGate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolWriteGate {
    reason: String,
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
    pub max_model_round_trips: Option<usize>,
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
    pub overrides: Vec<ToolPermissionOverride>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolRegistryManifest {
    pub registry_name: String,
    pub tools: Vec<ToolManifestEntry>,
    pub policy: ToolPermissionPolicyManifest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolManifestEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
    pub result_schema: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_risk_class: Option<ToolRiskClass>,
    pub possible_risk_classes: Vec<ToolRiskClass>,
    pub timeout_policy: ToolTimeoutPolicy,
    pub execution_owner: String,
    pub redaction: ToolRedactionBehavior,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolTimeoutPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_chars: Option<usize>,
    pub operator_configurable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRedactionBehavior {
    pub input_summary: String,
    pub output_summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPermissionPolicyManifest {
    pub default_decisions: Vec<ToolRiskPermission>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<ToolPermissionOverride>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRiskPermission {
    pub risk_class: ToolRiskClass,
    pub decision: ToolPermissionDecision,
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
            write_gate: None,
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

    #[must_use]
    pub fn with_write_gate(mut self, write_gate: ToolWriteGate) -> Self {
        self.write_gate = Some(write_gate);
        self
    }

    #[must_use]
    pub fn write_gate(&self) -> Option<&ToolWriteGate> {
        self.write_gate.as_ref()
    }
}

impl ToolWriteGate {
    #[must_use]
    pub fn blocked(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    #[must_use]
    pub fn reason(&self) -> &str {
        self.reason.as_str()
    }
}

impl ToolApprovalConfig {
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            allow_write_tools: false,
            allow_network_shell: false,
            allow_destructive_shell: false,
            denied_action: ToolDeniedAction::Refuse,
            overrides: Vec::new(),
        }
    }

    #[must_use]
    pub fn allow_all() -> Self {
        Self {
            allow_write_tools: true,
            allow_network_shell: true,
            allow_destructive_shell: true,
            denied_action: ToolDeniedAction::Refuse,
            overrides: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_override(mut self, override_rule: ToolPermissionOverride) -> Self {
        self.overrides.push(override_rule);
        self
    }

    #[must_use]
    pub fn default_decision_for_risk(&self, risk_class: ToolRiskClass) -> ToolPermissionDecision {
        match risk_class {
            ToolRiskClass::ReadOnly | ToolRiskClass::ShellReadOnly => ToolPermissionDecision::Allow,
            ToolRiskClass::Write if self.allow_write_tools => ToolPermissionDecision::Allow,
            ToolRiskClass::Network if self.allow_network_shell => ToolPermissionDecision::Allow,
            ToolRiskClass::Destructive if self.allow_destructive_shell => {
                ToolPermissionDecision::Allow
            }
            ToolRiskClass::Write | ToolRiskClass::Network | ToolRiskClass::Destructive => {
                match self.denied_action {
                    ToolDeniedAction::Refuse => ToolPermissionDecision::Deny,
                    ToolDeniedAction::Pause => ToolPermissionDecision::Ask,
                }
            }
        }
    }
}

impl ToolOracleContext {
    #[must_use]
    pub fn new(profile: BackendProfile, max_calls: usize, calls_used: usize) -> Self {
        Self {
            profile,
            max_calls,
            calls_used,
        }
    }

    #[must_use]
    pub fn profile(&self) -> &BackendProfile {
        &self.profile
    }

    #[must_use]
    pub fn max_calls(&self) -> usize {
        self.max_calls
    }

    #[must_use]
    pub fn calls_used(&self) -> usize {
        self.calls_used
    }
}

impl ToolLongContextConfig {
    #[must_use]
    pub fn bounded(profile: BackendProfile, max_calls: usize) -> Self {
        Self {
            profile,
            max_calls,
            max_evidence_files: LONG_CONTEXT_DEFAULT_MAX_EVIDENCE_FILES,
            max_lines_per_file: LONG_CONTEXT_DEFAULT_MAX_LINES_PER_FILE,
        }
    }
}

impl ToolLongContextContext {
    #[must_use]
    pub fn new(
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
    ) -> Self {
        Self {
            profile,
            max_calls,
            calls_used,
            max_evidence_files,
            max_lines_per_file,
            prompt_char_count,
            files_listed,
            files_searched,
            files_read,
            too_many_turns,
            oracle_calls,
        }
    }

    #[must_use]
    pub fn profile(&self) -> &BackendProfile {
        &self.profile
    }

    #[must_use]
    pub fn max_calls(&self) -> usize {
        self.max_calls
    }

    #[must_use]
    pub fn calls_used(&self) -> usize {
        self.calls_used
    }

    #[must_use]
    pub fn max_evidence_files(&self) -> usize {
        self.max_evidence_files
    }

    #[must_use]
    pub fn max_lines_per_file(&self) -> u64 {
        self.max_lines_per_file
    }

    #[must_use]
    pub fn escalation_context(
        &self,
        requested_task_kind: impl Into<String>,
        requested_evidence_files: usize,
    ) -> LongContextEscalationContext {
        LongContextEscalationContext {
            prompt_char_count: self.prompt_char_count,
            files_listed: self.files_listed,
            files_searched: self.files_searched,
            files_read: self.files_read,
            too_many_turns: self.too_many_turns,
            oracle_calls: self.oracle_calls,
            long_context_calls: self.calls_used,
            requested_task_kind: requested_task_kind.into(),
            requested_evidence_files,
        }
    }
}

impl ToolInvocationOutcome {
    #[must_use]
    pub fn new(output: serde_json::Value) -> Self {
        Self {
            output,
            command: None,
            exit_code: None,
            timed_out: None,
            truncated: None,
            bytes_returned: None,
            files_touched: Vec::new(),
        }
    }
}

impl ExecutedToolCall {
    #[must_use]
    pub fn was_executed(&self) -> bool {
        matches!(
            self.tool_execution.policy_decision,
            ToolPolicyDecision::AutoAllow | ToolPolicyDecision::Approved
        )
    }

    #[must_use]
    pub fn was_refused(&self) -> bool {
        self.tool_execution.policy_decision == ToolPolicyDecision::Refused
    }

    #[must_use]
    pub fn was_paused(&self) -> bool {
        self.tool_execution.policy_decision == ToolPolicyDecision::Paused
    }
}

impl ProbeToolChoice {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "none" => Ok(Self::None),
            "auto" => Ok(Self::Auto),
            "required" => Ok(Self::Required),
            _ => value
                .strip_prefix("named:")
                .map(|name| Self::Named(String::from(name)))
                .ok_or_else(|| {
                    String::from("tool choice must be one of: none, auto, required, named:<tool>")
                }),
        }
    }

    #[must_use]
    pub fn to_provider_choice(&self) -> Option<ChatToolChoice> {
        match self {
            Self::None => Some(ChatToolChoice::Mode(String::from("none"))),
            Self::Auto => Some(ChatToolChoice::Mode(String::from("auto"))),
            Self::Required => Some(ChatToolChoice::Mode(String::from("required"))),
            Self::Named(name) => Some(ChatToolChoice::Named(ChatNamedToolChoice {
                kind: String::from("function"),
                function: ChatNamedToolChoiceFunction { name: name.clone() },
            })),
        }
    }
}

impl ToolLoopConfig {
    #[must_use]
    pub fn coding_bootstrap(tool_choice: ProbeToolChoice, parallel_tool_calls: bool) -> Self {
        Self {
            registry: ToolRegistry::coding_bootstrap(false, false),
            tool_choice,
            parallel_tool_calls,
            max_model_round_trips: None,
            approval: ToolApprovalConfig::conservative(),
            oracle: None,
            long_context: None,
        }
    }

    #[must_use]
    pub fn with_oracle(mut self, oracle: ToolOracleConfig) -> Self {
        self.oracle = Some(oracle);
        self.refresh_registry();
        self
    }

    #[must_use]
    pub fn with_long_context(mut self, long_context: ToolLongContextConfig) -> Self {
        self.long_context = Some(long_context);
        self.refresh_registry();
        self
    }

    fn refresh_registry(&mut self) {
        if self.registry.name() == "coding_bootstrap" {
            self.registry =
                ToolRegistry::coding_bootstrap(self.oracle.is_some(), self.long_context.is_some());
        }
    }
}

impl ToolRegistry {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tools: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn coding_bootstrap(include_oracle: bool, include_long_context: bool) -> Self {
        let registry = Self::new("coding_bootstrap")
            .register(
                String::from("read_file"),
                Some(String::from(
                    "Read a bounded set of lines from a relative text file inside the session cwd.",
                )),
                Some(read_file_parameters()),
                RegisteredToolRisk::Fixed(ToolRiskClass::ReadOnly),
                read_file,
            )
            .register(
                String::from("list_files"),
                Some(String::from(
                    "List files and directories relative to the session cwd with bounded depth and entry count.",
                )),
                Some(list_files_parameters()),
                RegisteredToolRisk::Fixed(ToolRiskClass::ReadOnly),
                list_files,
            )
            .register(
                String::from("code_search"),
                Some(String::from(
                    "Search the codebase with ripgrep using a bounded pattern search relative to the session cwd.",
                )),
                Some(code_search_parameters()),
                RegisteredToolRisk::Fixed(ToolRiskClass::ReadOnly),
                code_search,
            )
            .register(
                String::from("shell"),
                Some(String::from(
                    "Run a bounded literal shell command inside the session cwd and capture stdout, stderr, exit code, and timeout state. Do not pass user questions or natural-language requests as command text.",
                )),
                Some(shell_parameters()),
                RegisteredToolRisk::Shell,
                run_shell,
            )
            .register(
                String::from("apply_patch"),
                Some(String::from(
                    "Apply a deterministic text replacement to a relative file in the session cwd.",
                )),
                Some(apply_patch_parameters()),
                RegisteredToolRisk::Fixed(ToolRiskClass::Write),
                apply_patch,
            );

        let registry = if include_oracle {
            registry.register(
                String::from("consult_oracle"),
                Some(String::from(
                    "Consult a bounded auxiliary oracle model for planning, checking, or research support.",
                )),
                Some(consult_oracle_parameters()),
                RegisteredToolRisk::Fixed(ToolRiskClass::ReadOnly),
                consult_oracle,
            )
        } else {
            registry
        };
        if include_long_context {
            registry.register(
                String::from("analyze_repository"),
                Some(String::from(
                    "Run a bounded long-context repo-analysis pass over explicit evidence files for architecture, synthesis, or change-impact questions.",
                )),
                Some(analyze_repository_parameters()),
                RegisteredToolRisk::Fixed(ToolRiskClass::ReadOnly),
                analyze_repository,
            )
        } else {
            registry
        }
    }

    #[must_use]
    pub fn coding_bootstrap_with_domain_helpers(
        include_oracle: bool,
        include_long_context: bool,
        include_domain_helpers: bool,
    ) -> Self {
        let registry = Self::coding_bootstrap(include_oracle, include_long_context);
        if include_domain_helpers {
            registry.register(
                String::from("legal.inspect_answer_file"),
                Some(String::from(
                    "Inspect a legal benchmark answer file without returning raw answer content; reports digests, size, line count, and required-substring presence.",
                )),
                Some(legal_inspect_answer_file_parameters()),
                RegisteredToolRisk::Fixed(ToolRiskClass::ReadOnly),
                legal_inspect_answer_file,
            )
        } else {
            registry
        }
    }

    #[must_use]
    fn register(
        mut self,
        name: String,
        description: Option<String>,
        parameters: Option<serde_json::Value>,
        risk: RegisteredToolRisk,
        handler: ToolHandler,
    ) -> Self {
        self.tools.insert(
            name.clone(),
            RegisteredTool {
                definition: ChatToolDefinition {
                    name,
                    description,
                    parameters,
                },
                handler,
                risk,
            },
        );
        self
    }

    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    #[must_use]
    pub fn declared_tools(&self) -> Vec<ChatToolDefinitionEnvelope> {
        self.tools
            .values()
            .map(|tool| ChatToolDefinitionEnvelope {
                kind: String::from("function"),
                function: tool.definition.clone(),
            })
            .collect()
    }

    #[must_use]
    pub fn managed_tool_manifest(&self, approval: &ToolApprovalConfig) -> ToolRegistryManifest {
        ToolRegistryManifest {
            registry_name: self.name.clone(),
            tools: self
                .tools
                .values()
                .map(|tool| tool_manifest_entry(tool))
                .collect(),
            policy: ToolPermissionPolicyManifest {
                default_decisions: all_tool_risk_classes()
                    .into_iter()
                    .map(|risk_class| ToolRiskPermission {
                        risk_class,
                        decision: approval.default_decision_for_risk(risk_class),
                    })
                    .collect(),
                overrides: approval.overrides.clone(),
            },
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    #[must_use]
    pub fn execution_session(
        &self,
        context: &ToolExecutionContext,
        approval: &ToolApprovalConfig,
    ) -> ToolExecutionSession {
        ToolExecutionSession::new(self.clone(), context.clone(), approval.clone())
    }

    pub fn execute_batch(
        &self,
        context: &ToolExecutionContext,
        tool_calls: &[ChatToolCall],
        approval: &ToolApprovalConfig,
    ) -> Vec<ExecutedToolCall> {
        let mut session = self.execution_session(context, approval);
        tool_calls
            .iter()
            .map(|tool_call| session.execute_openai_tool_call(tool_call))
            .collect()
    }

    pub(crate) fn execute_batch_with_observer(
        &self,
        context: &ToolExecutionContext,
        tool_calls: &[ChatToolCall],
        approval: &ToolApprovalConfig,
        observer: &mut impl FnMut(&str, &str, &serde_json::Value, ToolRiskClass),
    ) -> Vec<ExecutedToolCall> {
        let mut session = self.execution_session(context, approval);
        tool_calls
            .iter()
            .map(|tool_call| session.execute_openai_tool_call_with_observer(tool_call, observer))
            .collect()
    }
}

fn tool_manifest_entry(tool: &RegisteredTool) -> ToolManifestEntry {
    let (declared_risk_class, possible_risk_classes) = match tool.risk {
        RegisteredToolRisk::Fixed(risk_class) => (Some(risk_class), vec![risk_class]),
        RegisteredToolRisk::Shell => (None, shell_possible_risk_classes()),
    };
    ToolManifestEntry {
        name: tool.definition.name.clone(),
        description: tool.definition.description.clone(),
        input_schema: tool
            .definition
            .parameters
            .clone()
            .unwrap_or_else(|| serde_json::json!({ "type": "object" })),
        result_schema: tool_result_schema(tool.definition.name.as_str()),
        declared_risk_class,
        possible_risk_classes,
        timeout_policy: timeout_policy_for_tool(tool.definition.name.as_str()),
        execution_owner: String::from("probe_runtime"),
        redaction: redaction_behavior_for_tool(tool.definition.name.as_str()),
    }
}

fn all_tool_risk_classes() -> Vec<ToolRiskClass> {
    vec![
        ToolRiskClass::ReadOnly,
        ToolRiskClass::ShellReadOnly,
        ToolRiskClass::Write,
        ToolRiskClass::Network,
        ToolRiskClass::Destructive,
    ]
}

fn shell_possible_risk_classes() -> Vec<ToolRiskClass> {
    vec![
        ToolRiskClass::ShellReadOnly,
        ToolRiskClass::Write,
        ToolRiskClass::Network,
        ToolRiskClass::Destructive,
    ]
}

fn timeout_policy_for_tool(tool_name: &str) -> ToolTimeoutPolicy {
    if tool_name == "shell" {
        return ToolTimeoutPolicy {
            default_secs: Some(SHELL_DEFAULT_TIMEOUT_SECS),
            max_output_chars: Some(SHELL_MAX_OUTPUT_CHARS),
            operator_configurable: true,
        };
    }
    ToolTimeoutPolicy {
        default_secs: None,
        max_output_chars: Some(TOOL_MODEL_TEXT_MAX_CHARS),
        operator_configurable: false,
    }
}

fn redaction_behavior_for_tool(tool_name: &str) -> ToolRedactionBehavior {
    match tool_name {
        "apply_patch" => ToolRedactionBehavior {
            input_summary: String::from(
                "path and replacement sizes only; patch text is not exposed in managed summaries",
            ),
            output_summary: String::from("path, created flag, replacement mode, and bytes written"),
        },
        "shell" => ToolRedactionBehavior {
            input_summary: String::from(
                "command preview with secret assignments and tokens redacted",
            ),
            output_summary: String::from(
                "exit, timeout, truncation flags, and bounded redacted stdout/stderr previews",
            ),
        },
        "read_file" => ToolRedactionBehavior {
            input_summary: String::from("path and line bounds with secret-looking values redacted"),
            output_summary: String::from(
                "path, line bounds, truncation, and bounded redacted content preview",
            ),
        },
        "code_search" => ToolRedactionBehavior {
            input_summary: String::from("search options with secret-looking values redacted"),
            output_summary: String::from(
                "match counts, matched paths, status, and truncation state",
            ),
        },
        "consult_oracle" | "analyze_repository" => ToolRedactionBehavior {
            input_summary: String::from("task metadata and bounded redacted question preview"),
            output_summary: String::from("bounded redacted answer or analysis preview plus counts"),
        },
        "legal.inspect_answer_file" => ToolRedactionBehavior {
            input_summary: String::from(
                "path, marker count, and byte cap only; raw legal answer markers are never exposed",
            ),
            output_summary: String::from(
                "path, size, line count, file digest, marker digests, and presence flags without raw answer content",
            ),
        },
        _ => ToolRedactionBehavior {
            input_summary: String::from("bounded JSON summary with secret-looking values redacted"),
            output_summary: String::from(
                "bounded JSON summary with secret-looking values redacted",
            ),
        },
    }
}

impl ToolExecutionSession {
    #[must_use]
    pub fn new(
        registry: ToolRegistry,
        context: ToolExecutionContext,
        approval: ToolApprovalConfig,
    ) -> Self {
        let oracle_calls_remaining = context
            .oracle()
            .map(|oracle| oracle.max_calls().saturating_sub(oracle.calls_used()))
            .unwrap_or(0);
        let long_context_calls_remaining = context
            .long_context()
            .map(|long_context| {
                long_context
                    .max_calls()
                    .saturating_sub(long_context.calls_used())
            })
            .unwrap_or(0);
        Self {
            registry,
            context,
            approval,
            oracle_calls_remaining,
            long_context_calls_remaining,
        }
    }

    #[must_use]
    pub fn execute_openai_tool_call(&mut self, tool_call: &ChatToolCall) -> ExecutedToolCall {
        let mut observer = |_: &str, _: &str, _: &serde_json::Value, _: ToolRiskClass| {};
        self.execute_openai_tool_call_with_observer(tool_call, &mut observer)
    }

    #[must_use]
    pub(crate) fn execute_openai_tool_call_with_observer(
        &mut self,
        tool_call: &ChatToolCall,
        observer: &mut impl FnMut(&str, &str, &serde_json::Value, ToolRiskClass),
    ) -> ExecutedToolCall {
        let parsed_arguments =
            serde_json::from_str::<serde_json::Value>(tool_call.function.arguments.as_str())
                .unwrap_or_else(|error| {
                    serde_json::json!({
                        "error": format!("invalid tool arguments json: {error}")
                    })
                });
        self.execute_named_call_with_observer(
            tool_call.id.clone(),
            tool_call.function.name.clone(),
            parsed_arguments,
            observer,
        )
    }

    #[must_use]
    pub fn execute_named_call(
        &mut self,
        call_id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> ExecutedToolCall {
        let mut observer = |_: &str, _: &str, _: &serde_json::Value, _: ToolRiskClass| {};
        self.execute_named_call_with_observer(call_id, name, arguments, &mut observer)
    }

    #[must_use]
    pub(crate) fn execute_named_call_with_resolution(
        &mut self,
        call_id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
        risk_class: ToolRiskClass,
        resolution: ToolApprovalResolution,
        observer: &mut impl FnMut(&str, &str, &serde_json::Value, ToolRiskClass),
    ) -> ExecutedToolCall {
        let name = name.into();
        let mut approval = self.approval.clone();
        approval.denied_action = ToolDeniedAction::Refuse;
        match resolution {
            ToolApprovalResolution::Approved => match risk_class {
                ToolRiskClass::ReadOnly | ToolRiskClass::ShellReadOnly => {}
                ToolRiskClass::Write => approval.allow_write_tools = true,
                ToolRiskClass::Network => approval.allow_network_shell = true,
                ToolRiskClass::Destructive => approval.allow_destructive_shell = true,
            },
            ToolApprovalResolution::Rejected => {}
        }
        let denied_reason_override =
            matches!(resolution, ToolApprovalResolution::Rejected).then(|| {
                format!(
                    "operator rejected the pending approval request for tool `{}`",
                    name
                )
            });
        self.execute_named_call_with_policy(
            call_id.into(),
            name,
            arguments,
            &approval,
            denied_reason_override,
            observer,
        )
    }

    #[must_use]
    pub(crate) fn execute_named_call_with_observer(
        &mut self,
        call_id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
        observer: &mut impl FnMut(&str, &str, &serde_json::Value, ToolRiskClass),
    ) -> ExecutedToolCall {
        self.execute_named_call_with_policy(
            call_id.into(),
            name.into(),
            arguments,
            &self.approval.clone(),
            None,
            observer,
        )
    }

    fn execute_named_call_with_policy(
        &mut self,
        call_id: String,
        name: String,
        arguments: serde_json::Value,
        approval: &ToolApprovalConfig,
        denied_reason_override: Option<String>,
        observer: &mut impl FnMut(&str, &str, &serde_json::Value, ToolRiskClass),
    ) -> ExecutedToolCall {
        let Some(tool) = self.registry.tools.get(name.as_str()) else {
            return undeclared_tool_call(call_id, name, arguments);
        };

        if name == "consult_oracle" && self.oracle_calls_remaining == 0 {
            return refused_named_tool_call(
                &self.context,
                call_id,
                name,
                arguments,
                ToolRiskClass::ReadOnly,
                Some(String::from(
                    "oracle call budget exhausted for this session",
                )),
            );
        }
        if name == "analyze_repository" && self.long_context_calls_remaining == 0 {
            return refused_named_tool_call(
                &self.context,
                call_id,
                name,
                arguments,
                ToolRiskClass::ReadOnly,
                Some(String::from(
                    "long-context repo-analysis budget exhausted for this session",
                )),
            );
        }
        if name == "analyze_repository"
            && let Some(decision) = long_context_refusal_decision(&self.context, &arguments)
        {
            return refused_named_tool_call(
                &self.context,
                call_id,
                name,
                arguments,
                ToolRiskClass::ReadOnly,
                Some(decision.reason),
            );
        }

        let risk_class = match tool.risk {
            RegisteredToolRisk::Fixed(risk) => risk,
            RegisteredToolRisk::Shell => classify_shell_command(&arguments),
        };
        if let Some(reason) = write_gate_refusal_reason(&self.context, risk_class) {
            return refused_named_tool_call(
                &self.context,
                call_id,
                name,
                arguments,
                risk_class,
                Some(reason),
            );
        }
        let (policy_decision, approval_state, policy_reason) =
            evaluate_tool_policy(name.as_str(), risk_class, approval);
        let reason = denied_reason_override.or(policy_reason);
        let invocation = if matches!(
            policy_decision,
            ToolPolicyDecision::AutoAllow | ToolPolicyDecision::Approved
        ) {
            observer(call_id.as_str(), name.as_str(), &arguments, risk_class);
            (tool.handler)(&self.context, &arguments).unwrap_or_else(|error| {
                ToolInvocationOutcome::new(serde_json::json!({ "error": error.to_string() }))
            })
        } else {
            denied_tool_invocation(
                &self.context,
                name.as_str(),
                &arguments,
                risk_class,
                &reason,
            )
        };
        if name == "consult_oracle"
            && matches!(
                policy_decision,
                ToolPolicyDecision::AutoAllow | ToolPolicyDecision::Approved
            )
            && self.oracle_calls_remaining > 0
        {
            self.oracle_calls_remaining -= 1;
        }
        if name == "analyze_repository"
            && matches!(
                policy_decision,
                ToolPolicyDecision::AutoAllow | ToolPolicyDecision::Approved
            )
            && self.long_context_calls_remaining > 0
        {
            self.long_context_calls_remaining -= 1;
        }

        ExecutedToolCall {
            call_id,
            name,
            arguments,
            output: invocation.output.clone(),
            tool_execution: ToolExecutionRecord {
                risk_class,
                policy_decision,
                approval_state,
                command: invocation.command,
                exit_code: invocation.exit_code,
                timed_out: invocation.timed_out,
                truncated: invocation.truncated,
                bytes_returned: invocation.bytes_returned,
                files_touched: invocation.files_touched,
                reason,
            },
        }
    }
}

fn write_gate_refusal_reason(
    context: &ToolExecutionContext,
    risk_class: ToolRiskClass,
) -> Option<String> {
    if matches!(
        risk_class,
        ToolRiskClass::ReadOnly | ToolRiskClass::ShellReadOnly
    ) {
        return None;
    }
    context.write_gate().map(|gate| {
        format!(
            "workspace sync gate blocked non-read-only tool execution: {}",
            gate.reason()
        )
    })
}

fn read_file(
    context: &ToolExecutionContext,
    arguments: &serde_json::Value,
) -> Result<ToolInvocationOutcome, ToolInvocationError> {
    let path = expect_string(arguments, "path", "read_file")?;
    let start_line = expect_u64(arguments, "start_line").unwrap_or(1);
    let max_lines = expect_u64(arguments, "max_lines").unwrap_or(READ_FILE_DEFAULT_MAX_LINES);
    if start_line == 0 || max_lines == 0 {
        return Err(ToolInvocationError::InvalidArguments(String::from(
            "read_file requires start_line >= 1 and max_lines >= 1",
        )));
    }

    let resolved_path = resolve_workspace_path(context.cwd(), path)?;
    let content = fs::read_to_string(&resolved_path).map_err(|error| {
        ToolInvocationError::ExecutionFailed(format!("failed to read file `{path}`: {error}"))
    })?;
    let lines = content.lines().collect::<Vec<_>>();
    if !lines.is_empty() && start_line as usize > lines.len() {
        return Err(ToolInvocationError::InvalidArguments(format!(
            "read_file start_line {} exceeds file line count {}",
            start_line,
            lines.len()
        )));
    }

    let start_index = if lines.is_empty() {
        0
    } else {
        (start_line - 1) as usize
    };
    let end_index = start_index
        .saturating_add(max_lines as usize)
        .min(lines.len());
    let selected_lines = lines[start_index..end_index].join("\n");
    let end_line = if end_index == 0 { 0 } else { end_index as u64 };
    let relative_path = display_relative_path(context.cwd(), &resolved_path);
    let truncated = end_index < lines.len();
    let content_bytes = selected_lines.len() as u64;
    Ok(ToolInvocationOutcome {
        output: serde_json::json!({
            "path": relative_path.clone(),
            "start_line": start_line,
            "end_line": end_line,
            "total_lines": lines.len(),
            "truncated": truncated,
            "content": selected_lines,
        }),
        command: None,
        exit_code: None,
        timed_out: None,
        truncated: Some(truncated),
        bytes_returned: Some(content_bytes),
        files_touched: vec![relative_path],
    })
}

fn list_files(
    context: &ToolExecutionContext,
    arguments: &serde_json::Value,
) -> Result<ToolInvocationOutcome, ToolInvocationError> {
    let requested_path = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(normalize_tool_directory_path)
        .unwrap_or(".");
    let max_depth = expect_u64(arguments, "max_depth").unwrap_or(LIST_FILES_DEFAULT_MAX_DEPTH);
    let max_entries = expect_u64(arguments, "max_entries")
        .unwrap_or(LIST_FILES_DEFAULT_MAX_ENTRIES as u64) as usize;
    let include_hidden = arguments
        .get("include_hidden")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if max_entries == 0 {
        return Err(ToolInvocationError::InvalidArguments(String::from(
            "list_files requires max_entries >= 1",
        )));
    }

    let root = resolve_workspace_path(context.cwd(), requested_path)?;
    let metadata = fs::metadata(&root).map_err(|error| {
        ToolInvocationError::ExecutionFailed(format!(
            "failed to stat directory `{requested_path}`: {error}"
        ))
    })?;
    if !metadata.is_dir() {
        return Err(ToolInvocationError::InvalidArguments(format!(
            "list_files requires a directory path, got `{requested_path}`"
        )));
    }

    let mut entries = Vec::new();
    let mut truncated = false;
    visit_directory(
        &root,
        &root,
        max_depth as usize,
        max_entries,
        include_hidden,
        &mut entries,
        &mut truncated,
    )?;

    let relative_path = display_relative_path(context.cwd(), &root);
    let entries_bytes = serde_json::to_vec(&entries).unwrap_or_default().len() as u64;
    Ok(ToolInvocationOutcome {
        output: serde_json::json!({
            "path": relative_path.clone(),
            "max_depth": max_depth,
            "truncated": truncated,
            "entries": entries,
        }),
        command: None,
        exit_code: None,
        timed_out: None,
        truncated: Some(truncated),
        bytes_returned: Some(entries_bytes),
        files_touched: vec![relative_path],
    })
}

fn code_search(
    context: &ToolExecutionContext,
    arguments: &serde_json::Value,
) -> Result<ToolInvocationOutcome, ToolInvocationError> {
    let pattern = expect_string(arguments, "pattern", "code_search")?;
    let requested_path = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(normalize_tool_directory_path)
        .unwrap_or(".");
    let glob = arguments
        .get("glob")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    let max_matches = expect_u64(arguments, "max_matches")
        .unwrap_or(CODE_SEARCH_DEFAULT_MAX_MATCHES as u64) as usize;
    let case_sensitive = arguments
        .get("case_sensitive")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if max_matches == 0 {
        return Err(ToolInvocationError::InvalidArguments(String::from(
            "code_search requires max_matches >= 1",
        )));
    }

    let search_root = resolve_workspace_path(context.cwd(), requested_path)?;
    let mut command = Command::new("rg");
    command.current_dir(context.base_dir());
    command
        .arg("--line-number")
        .arg("--column")
        .arg("--with-filename");
    command.arg("--color").arg("never");
    command.arg("--max-count").arg(max_matches.to_string());
    if !case_sensitive {
        command.arg("--ignore-case");
    }
    if let Some(glob) = &glob {
        command.arg("--glob").arg(glob);
    }
    command.arg(pattern).arg(&search_root);

    let output = match command.output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return code_search_without_ripgrep(
                context,
                search_root.as_path(),
                pattern,
                requested_path,
                glob.as_deref(),
                max_matches,
                case_sensitive,
            );
        }
        Err(error) => {
            return Err(ToolInvocationError::ExecutionFailed(format!(
                "failed to run ripgrep: {error}"
            )));
        }
    };
    let status_code = output.status.code().unwrap_or(-1);
    if !output.status.success() && status_code != 1 {
        let (stderr, _) = truncate_text(String::from_utf8_lossy(&output.stderr).as_ref());
        return Err(ToolInvocationError::ExecutionFailed(format!(
            "ripgrep failed with exit code {status_code}: {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut matches = Vec::new();
    for line in stdout.lines() {
        if matches.len() >= max_matches {
            break;
        }
        let mut parts = line.splitn(4, ':');
        let path = parts.next().unwrap_or_default();
        let line_number = parts.next().unwrap_or_default().parse::<u64>().ok();
        let column = parts.next().unwrap_or_default().parse::<u64>().ok();
        let snippet = parts.next().unwrap_or_default();
        matches.push(serde_json::json!({
            "path": relativize_search_match(context.cwd(), path),
            "line": line_number,
            "column": column,
            "snippet": snippet,
        }));
    }

    let matched_paths = matches
        .iter()
        .filter_map(|entry| entry.get("path").and_then(serde_json::Value::as_str))
        .map(String::from)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let relative_path = display_relative_path(context.cwd(), &search_root);
    let truncated = matches.len() >= max_matches;
    Ok(ToolInvocationOutcome {
        output: serde_json::json!({
            "path": relative_path.clone(),
            "pattern": pattern,
            "glob": glob,
            "case_sensitive": case_sensitive,
            "matches": matches,
            "truncated": truncated,
            "status_code": status_code,
        }),
        command: None,
        exit_code: Some(status_code),
        timed_out: Some(false),
        truncated: Some(truncated),
        bytes_returned: Some(output.stdout.len() as u64),
        files_touched: matched_paths,
    })
}

fn code_search_without_ripgrep(
    context: &ToolExecutionContext,
    search_root: &Path,
    pattern: &str,
    requested_path: &str,
    glob: Option<&str>,
    max_matches: usize,
    case_sensitive: bool,
) -> Result<ToolInvocationOutcome, ToolInvocationError> {
    let mut matches = Vec::new();
    let mut hit_limit = false;
    collect_code_search_matches_without_ripgrep(
        context.cwd(),
        search_root,
        pattern,
        glob,
        max_matches,
        case_sensitive,
        &mut matches,
        &mut hit_limit,
    )?;

    let matched_paths = matches
        .iter()
        .filter_map(|entry| entry.get("path").and_then(serde_json::Value::as_str))
        .map(String::from)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let status_code = if matched_paths.is_empty() { 1 } else { 0 };

    Ok(ToolInvocationOutcome {
        output: serde_json::json!({
            "path": requested_path,
            "pattern": pattern,
            "glob": glob,
            "case_sensitive": case_sensitive,
            "matches": matches,
            "truncated": hit_limit,
            "status_code": status_code,
            "search_backend": "rust_fallback",
        }),
        command: None,
        exit_code: Some(status_code),
        timed_out: Some(false),
        truncated: Some(hit_limit),
        bytes_returned: None,
        files_touched: matched_paths,
    })
}

fn collect_code_search_matches_without_ripgrep(
    cwd: &Path,
    path: &Path,
    pattern: &str,
    glob: Option<&str>,
    max_matches: usize,
    case_sensitive: bool,
    matches: &mut Vec<serde_json::Value>,
    hit_limit: &mut bool,
) -> Result<(), ToolInvocationError> {
    if matches.len() >= max_matches {
        *hit_limit = true;
        return Ok(());
    }

    if path.is_dir() {
        let mut entries = fs::read_dir(path)
            .map_err(|error| {
                ToolInvocationError::ExecutionFailed(format!(
                    "failed to read search directory: {error}"
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                ToolInvocationError::ExecutionFailed(format!(
                    "failed to read search directory: {error}"
                ))
            })?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            collect_code_search_matches_without_ripgrep(
                cwd,
                entry.path().as_path(),
                pattern,
                glob,
                max_matches,
                case_sensitive,
                matches,
                hit_limit,
            )?;
            if *hit_limit {
                break;
            }
        }
        return Ok(());
    }

    if !path.is_file() {
        return Ok(());
    }

    let relative_path = display_relative_path(cwd, path);
    if let Some(glob) = glob
        && !simple_glob_matches(glob, relative_path.as_str())
    {
        return Ok(());
    }

    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(error) => {
            return Err(ToolInvocationError::ExecutionFailed(format!(
                "failed to read search file: {error}"
            )));
        }
    };
    if contents.contains(&0) {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&contents);
    let needle = if case_sensitive {
        pattern.to_string()
    } else {
        pattern.to_ascii_lowercase()
    };

    for (index, line) in text.lines().enumerate() {
        let haystack = if case_sensitive {
            line.to_string()
        } else {
            line.to_ascii_lowercase()
        };
        if let Some(column_index) = haystack.find(needle.as_str()) {
            matches.push(serde_json::json!({
                "path": relative_path.clone(),
                "line": index as u64 + 1,
                "column": column_index as u64 + 1,
                "snippet": line,
            }));
            if matches.len() >= max_matches {
                *hit_limit = true;
                break;
            }
        }
    }

    Ok(())
}

fn run_shell(
    context: &ToolExecutionContext,
    arguments: &serde_json::Value,
) -> Result<ToolInvocationOutcome, ToolInvocationError> {
    let command_text = expect_string(arguments, "command", "shell")?;
    let timeout_secs = expect_u64(arguments, "timeout_secs").unwrap_or(SHELL_DEFAULT_TIMEOUT_SECS);
    if looks_like_natural_language_shell_misuse(command_text) {
        return Err(ToolInvocationError::InvalidArguments(String::from(
            "shell requires a literal shell command, not a natural-language request",
        )));
    }
    if timeout_secs == 0 {
        return Err(ToolInvocationError::InvalidArguments(String::from(
            "shell requires timeout_secs >= 1",
        )));
    }

    #[cfg(target_family = "unix")]
    let mut child = Command::new("sh")
        .arg("-lc")
        .arg(command_text)
        .current_dir(context.base_dir())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            ToolInvocationError::ExecutionFailed(format!("failed to spawn shell: {error}"))
        })?;

    #[cfg(target_family = "windows")]
    let mut child = Command::new("cmd")
        .arg("/C")
        .arg(command_text)
        .current_dir(context.base_dir())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            ToolInvocationError::ExecutionFailed(format!("failed to spawn shell: {error}"))
        })?;

    let timeout = Duration::from_secs(timeout_secs);
    let timed_out = child
        .wait_timeout(timeout)
        .map_err(|error| {
            ToolInvocationError::ExecutionFailed(format!("failed while waiting on shell: {error}"))
        })?
        .is_none();
    if timed_out {
        let _ = child.kill();
        thread::sleep(Duration::from_millis(20));
    }

    let output = child.wait_with_output().map_err(|error| {
        ToolInvocationError::ExecutionFailed(format!("failed to collect shell output: {error}"))
    })?;
    let exit_code = output.status.code();
    let (stdout, stdout_truncated) =
        truncate_text(String::from_utf8_lossy(&output.stdout).as_ref());
    let (stderr, stderr_truncated) =
        truncate_text(String::from_utf8_lossy(&output.stderr).as_ref());
    Ok(ToolInvocationOutcome {
        output: serde_json::json!({
            "command": command_text,
            "timeout_secs": timeout_secs,
            "timed_out": timed_out,
            "exit_code": exit_code,
            "stdout": stdout,
            "stderr": stderr,
            "stdout_truncated": stdout_truncated,
            "stderr_truncated": stderr_truncated,
        }),
        command: Some(String::from(command_text)),
        exit_code,
        timed_out: Some(timed_out),
        truncated: Some(stdout_truncated || stderr_truncated),
        bytes_returned: Some((output.stdout.len() + output.stderr.len()) as u64),
        files_touched: Vec::new(),
    })
}

fn apply_patch(
    context: &ToolExecutionContext,
    arguments: &serde_json::Value,
) -> Result<ToolInvocationOutcome, ToolInvocationError> {
    let path = expect_string(arguments, "path", "apply_patch")?;
    let old_text = expect_string(arguments, "old_text", "apply_patch")?;
    let new_text = expect_string(arguments, "new_text", "apply_patch")?;
    let replace_all = arguments
        .get("replace_all")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let create_if_missing = arguments
        .get("create_if_missing")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let resolved_path = resolve_workspace_path(context.cwd(), path)?;
    let existed = resolved_path.exists();
    let new_contents = if existed {
        let existing = fs::read_to_string(&resolved_path).map_err(|error| {
            ToolInvocationError::ExecutionFailed(format!(
                "failed to read file `{path}` for patching: {error}"
            ))
        })?;
        if old_text.is_empty() {
            if !existing.is_empty() {
                return Err(ToolInvocationError::InvalidArguments(String::from(
                    "apply_patch with empty old_text only supports creating or replacing an empty file",
                )));
            }
            new_text.to_string()
        } else {
            let occurrences = existing.matches(old_text).count();
            if occurrences == 0 {
                return Err(ToolInvocationError::ExecutionFailed(format!(
                    "apply_patch could not find the requested old_text in `{path}`"
                )));
            }
            if !replace_all && occurrences != 1 {
                return Err(ToolInvocationError::ExecutionFailed(format!(
                    "apply_patch expected exactly one match in `{path}`, found {occurrences}",
                )));
            }
            if replace_all {
                existing.replace(old_text, new_text)
            } else {
                existing.replacen(old_text, new_text, 1)
            }
        }
    } else {
        if !create_if_missing || !old_text.is_empty() {
            return Err(ToolInvocationError::ExecutionFailed(format!(
                "apply_patch cannot create missing file `{path}` unless create_if_missing is true and old_text is empty",
            )));
        }
        new_text.to_string()
    };

    if let Some(parent) = resolved_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ToolInvocationError::ExecutionFailed(format!(
                "failed to create parent directories for `{path}`: {error}"
            ))
        })?;
    }
    fs::write(&resolved_path, new_contents.as_bytes()).map_err(|error| {
        ToolInvocationError::ExecutionFailed(format!(
            "failed to write patched file `{path}`: {error}"
        ))
    })?;

    let relative_path = display_relative_path(context.cwd(), &resolved_path);
    Ok(ToolInvocationOutcome {
        output: serde_json::json!({
            "path": relative_path.clone(),
            "created": !existed,
            "replace_all": replace_all,
            "bytes_written": new_contents.len(),
        }),
        command: None,
        exit_code: None,
        timed_out: None,
        truncated: Some(false),
        bytes_returned: None,
        files_touched: vec![relative_path],
    })
}

fn consult_oracle(
    context: &ToolExecutionContext,
    arguments: &serde_json::Value,
) -> Result<ToolInvocationOutcome, ToolInvocationError> {
    let oracle = context.oracle().ok_or_else(|| {
        ToolInvocationError::ExecutionFailed(String::from(
            "consult_oracle requires an oracle profile in the active tool loop",
        ))
    })?;
    let task_kind = expect_string(arguments, "task_kind", "consult_oracle")?;
    if !matches!(task_kind, "planning" | "checking" | "research") {
        return Err(ToolInvocationError::InvalidArguments(String::from(
            "consult_oracle requires task_kind to be one of: planning, checking, research",
        )));
    }
    let question = expect_string(arguments, "question", "consult_oracle")?;

    let response = complete_plain_text(
        oracle.profile(),
        vec![
            PlainTextMessage::system(format!(
                "You are Probe's bounded oracle. Only help with {} support. Do not execute tools or claim filesystem changes. Return concise guidance grounded in the question.",
                task_kind
            )),
            PlainTextMessage::user(question),
        ],
    )
    .map_err(|error| {
            ToolInvocationError::ExecutionFailed(format!("oracle request failed: {error}"))
    })?;
    let answer = response.assistant_text.ok_or_else(|| {
        ToolInvocationError::ExecutionFailed(String::from(
            "oracle response did not include assistant text",
        ))
    })?;

    Ok(ToolInvocationOutcome {
        output: serde_json::json!({
            "task_kind": task_kind,
            "question": question,
            "oracle_profile": oracle.profile().name,
            "oracle_model": oracle.profile().model,
            "oracle_answer": answer,
            "calls_used_after": oracle.calls_used() + 1,
            "max_calls": oracle.max_calls(),
        }),
        command: None,
        exit_code: None,
        timed_out: Some(false),
        truncated: Some(false),
        bytes_returned: Some(answer.len() as u64),
        files_touched: Vec::new(),
    })
}

fn analyze_repository(
    context: &ToolExecutionContext,
    arguments: &serde_json::Value,
) -> Result<ToolInvocationOutcome, ToolInvocationError> {
    let long_context = context.long_context().ok_or_else(|| {
        ToolInvocationError::ExecutionFailed(String::from(
            "analyze_repository requires a long-context profile in the active tool loop",
        ))
    })?;
    let task_kind = expect_string(arguments, "task_kind", "analyze_repository")?;
    if !is_long_context_task_kind(task_kind) {
        return Err(ToolInvocationError::InvalidArguments(String::from(
            "analyze_repository requires task_kind to be one of: repo_analysis, architecture_summary, change_impact, synthesis",
        )));
    }
    let question = expect_string(arguments, "question", "analyze_repository")?;
    let evidence_paths = expect_string_array(arguments, "evidence_paths", "analyze_repository")?;
    if evidence_paths.is_empty() {
        return Err(ToolInvocationError::InvalidArguments(String::from(
            "analyze_repository requires at least one evidence_paths entry",
        )));
    }
    if evidence_paths.len() > long_context.max_evidence_files() {
        return Err(ToolInvocationError::InvalidArguments(format!(
            "analyze_repository allows at most {} evidence paths per call",
            long_context.max_evidence_files()
        )));
    }

    let decision = heuristic_long_context_escalation(
        &long_context.escalation_context(task_kind, evidence_paths.len()),
    );
    if !decision.should_escalate {
        return Err(ToolInvocationError::ExecutionFailed(format!(
            "long-context escalation denied: {}",
            decision.reason
        )));
    }

    let mut evidence = Vec::new();
    let mut evidence_blocks = Vec::new();
    let mut files_touched = Vec::new();
    let mut evidence_bytes = 0_u64;
    for path in &evidence_paths {
        let (record, block) =
            build_long_context_evidence(context.cwd(), path, long_context.max_lines_per_file())?;
        evidence_bytes += record
            .get("bytes_loaded")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        evidence.push(record);
        evidence_blocks.push(block);
        files_touched.push(path.clone());
    }

    let response = complete_plain_text(
        long_context.profile(),
        vec![
            PlainTextMessage::system(format!(
                "You are Probe's bounded long-context repo-analysis lane. Only answer {} tasks. Use only the provided evidence. Cite the relevant file paths in your answer. Do not claim edits, commands, or repo facts that are not present in the evidence.",
                task_kind
            )),
            PlainTextMessage::user(format!(
                "Question:\n{}\n\nEvidence:\n\n{}",
                question,
                evidence_blocks.join("\n\n")
            )),
        ],
    )
    .map_err(|error| {
            ToolInvocationError::ExecutionFailed(format!(
                "long-context analysis request failed: {error}"
            ))
    })?;
    let analysis = response.assistant_text.ok_or_else(|| {
        ToolInvocationError::ExecutionFailed(String::from(
            "long-context analysis response did not include assistant text",
        ))
    })?;

    Ok(ToolInvocationOutcome {
        output: serde_json::json!({
            "task_kind": task_kind,
            "question": question,
            "analysis_profile": long_context.profile().name,
            "analysis_model": long_context.profile().model,
            "analysis": analysis,
            "calls_used_after": long_context.calls_used() + 1,
            "max_calls": long_context.max_calls(),
            "decision_reason": decision.reason,
            "evidence": evidence,
        }),
        command: None,
        exit_code: None,
        timed_out: Some(false),
        truncated: Some(false),
        bytes_returned: Some(analysis.len() as u64 + evidence_bytes),
        files_touched,
    })
}

fn legal_inspect_answer_file(
    context: &ToolExecutionContext,
    arguments: &serde_json::Value,
) -> Result<ToolInvocationOutcome, ToolInvocationError> {
    let path = expect_string(arguments, "path", "legal.inspect_answer_file")?;
    let required_substrings = match arguments.get("required_substrings") {
        Some(_) => expect_string_array(
            arguments,
            "required_substrings",
            "legal.inspect_answer_file",
        )?,
        None => Vec::new(),
    };
    let max_bytes = expect_u64(arguments, "max_bytes")
        .unwrap_or(LEGAL_INSPECT_DEFAULT_MAX_BYTES)
        .min(LEGAL_INSPECT_HARD_MAX_BYTES);
    if max_bytes == 0 {
        return Err(ToolInvocationError::InvalidArguments(String::from(
            "legal.inspect_answer_file requires max_bytes >= 1",
        )));
    }

    let resolved_path = resolve_workspace_path(context.cwd(), path)?;
    let metadata = fs::metadata(&resolved_path).map_err(|error| {
        ToolInvocationError::ExecutionFailed(format!(
            "failed to stat legal answer file `{path}`: {error}"
        ))
    })?;
    if !metadata.is_file() {
        return Err(ToolInvocationError::InvalidArguments(format!(
            "legal.inspect_answer_file requires a file path, got `{path}`"
        )));
    }

    let bytes = fs::read(&resolved_path).map_err(|error| {
        ToolInvocationError::ExecutionFailed(format!(
            "failed to read legal answer file `{path}`: {error}"
        ))
    })?;
    let inspected_len = bytes.len().min(max_bytes as usize);
    let truncated = inspected_len < bytes.len();
    let inspected_text = String::from_utf8_lossy(&bytes[..inspected_len]);
    let full_text = String::from_utf8_lossy(bytes.as_slice());
    let required_substring_results = required_substrings
        .iter()
        .map(|substring| {
            serde_json::json!({
                "substring_sha256": sha256_hex_prefixed(substring.as_bytes()),
                "present": inspected_text.contains(substring),
                "search_complete": !truncated,
            })
        })
        .collect::<Vec<_>>();
    let all_required_substrings_present = required_substring_results.iter().all(|entry| {
        entry
            .get("present")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    });
    let relative_path = display_relative_path(context.cwd(), &resolved_path);

    Ok(ToolInvocationOutcome {
        output: serde_json::json!({
            "path": relative_path.clone(),
            "exists": true,
            "bytes": bytes.len() as u64,
            "bytes_inspected": inspected_len as u64,
            "line_count": full_text.lines().count() as u64,
            "sha256": sha256_hex_prefixed(bytes.as_slice()),
            "truncated": truncated,
            "required_substring_count": required_substrings.len(),
            "required_substrings": required_substring_results,
            "all_required_substrings_present": all_required_substrings_present,
        }),
        command: None,
        exit_code: None,
        timed_out: Some(false),
        truncated: Some(truncated),
        bytes_returned: Some(bytes.len() as u64),
        files_touched: vec![relative_path],
    })
}

fn build_long_context_evidence(
    base: &Path,
    requested_path: &str,
    max_lines: u64,
) -> Result<(serde_json::Value, String), ToolInvocationError> {
    let resolved_path = resolve_workspace_path(base, requested_path)?;
    let metadata = fs::metadata(&resolved_path).map_err(|error| {
        ToolInvocationError::ExecutionFailed(format!(
            "failed to stat evidence file `{requested_path}`: {error}"
        ))
    })?;
    if !metadata.is_file() {
        return Err(ToolInvocationError::InvalidArguments(format!(
            "analyze_repository evidence path `{requested_path}` must be a file",
        )));
    }

    let content = fs::read_to_string(&resolved_path).map_err(|error| {
        ToolInvocationError::ExecutionFailed(format!(
            "failed to read evidence file `{requested_path}`: {error}"
        ))
    })?;
    let lines = content.lines().collect::<Vec<_>>();
    let end_index = (max_lines as usize).min(lines.len());
    let excerpt = lines[..end_index].join("\n");
    let relative_path = display_relative_path(base, &resolved_path);
    let record = serde_json::json!({
        "path": relative_path,
        "start_line": 1,
        "end_line": end_index,
        "total_lines": lines.len(),
        "truncated": end_index < lines.len(),
        "bytes_loaded": excerpt.len(),
    });
    let block = format!(
        "### FILE {} lines 1-{} truncated={}\n{}",
        record
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(requested_path),
        end_index,
        record
            .get("truncated")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        excerpt
    );
    Ok((record, block))
}

fn long_context_refusal_decision(
    context: &ToolExecutionContext,
    arguments: &serde_json::Value,
) -> Option<crate::long_context::LongContextEscalationDecision> {
    let long_context = context.long_context()?;
    let task_kind = arguments
        .get("task_kind")
        .and_then(serde_json::Value::as_str)?;
    if !is_long_context_task_kind(task_kind) {
        return None;
    }
    let evidence_paths = arguments
        .get("evidence_paths")
        .and_then(serde_json::Value::as_array)?;
    let decision = heuristic_long_context_escalation(
        &long_context.escalation_context(task_kind, evidence_paths.len()),
    );
    (!decision.should_escalate).then_some(decision)
}

fn refused_named_tool_call(
    context: &ToolExecutionContext,
    call_id: String,
    tool_name: String,
    arguments: serde_json::Value,
    risk_class: ToolRiskClass,
    reason: Option<String>,
) -> ExecutedToolCall {
    let invocation =
        denied_tool_invocation(context, tool_name.as_str(), &arguments, risk_class, &reason);
    ExecutedToolCall {
        call_id,
        name: tool_name,
        arguments,
        output: invocation.output.clone(),
        tool_execution: ToolExecutionRecord {
            risk_class,
            policy_decision: ToolPolicyDecision::Refused,
            approval_state: ToolApprovalState::Refused,
            command: invocation.command,
            exit_code: invocation.exit_code,
            timed_out: invocation.timed_out,
            truncated: invocation.truncated,
            bytes_returned: invocation.bytes_returned,
            files_touched: invocation.files_touched,
            reason,
        },
    }
}

fn undeclared_tool_call(
    call_id: String,
    tool_name: String,
    arguments: serde_json::Value,
) -> ExecutedToolCall {
    ExecutedToolCall {
        call_id,
        name: tool_name.clone(),
        arguments,
        output: serde_json::json!({
            "error": format!("undeclared tool `{tool_name}`"),
        }),
        tool_execution: ToolExecutionRecord {
            risk_class: ToolRiskClass::ReadOnly,
            policy_decision: ToolPolicyDecision::Refused,
            approval_state: ToolApprovalState::Refused,
            command: None,
            exit_code: None,
            timed_out: None,
            truncated: None,
            bytes_returned: None,
            files_touched: Vec::new(),
            reason: Some(format!(
                "tool `{tool_name}` is not registered in this tool loop"
            )),
        },
    }
}

fn classify_shell_command(arguments: &serde_json::Value) -> ToolRiskClass {
    let Some(command) = arguments.get("command").and_then(serde_json::Value::as_str) else {
        return ToolRiskClass::Write;
    };
    let lowered = command.to_ascii_lowercase();

    if contains_any(
        lowered.as_str(),
        &[
            " rm ",
            "rm -",
            " rm\n",
            "git reset --hard",
            "git clean ",
            "kill ",
            "pkill ",
            "killall ",
            "shutdown",
            "reboot",
            " mkfs",
            "dd if=",
        ],
    ) {
        return ToolRiskClass::Destructive;
    }

    if contains_any(
        lowered.as_str(),
        &[
            "curl ",
            "wget ",
            "ssh ",
            "scp ",
            "rsync ",
            "git clone ",
            "git fetch ",
            "git pull ",
            "git push ",
            "gh ",
            "cargo install ",
            "pip install ",
            "uv pip install ",
            "npm install ",
            "pnpm add ",
            "yarn add ",
            "brew install ",
            "apt-get ",
            "apt ",
            "go get ",
        ],
    ) {
        return ToolRiskClass::Network;
    }

    if contains_any(
        lowered.as_str(),
        &[
            " >",
            ">>",
            "touch ",
            "mkdir ",
            "cp ",
            "mv ",
            "sed -i",
            "perl -i",
            "python -c",
            "python3 -c",
            "tee ",
            "truncate ",
            "chmod ",
            "chown ",
            "git add ",
            "git apply ",
            "git checkout ",
            "git commit ",
            "git merge ",
            "git rebase ",
            "git restore ",
            "git stash ",
            "git switch ",
        ],
    ) {
        return ToolRiskClass::Write;
    }

    if is_read_only_shell_command(lowered.as_str()) {
        ToolRiskClass::ShellReadOnly
    } else {
        ToolRiskClass::Write
    }
}

fn evaluate_tool_policy(
    tool_name: &str,
    risk_class: ToolRiskClass,
    approval: &ToolApprovalConfig,
) -> (ToolPolicyDecision, ToolApprovalState, Option<String>) {
    if let Some(override_rule) = matching_permission_override(tool_name, risk_class, approval) {
        return evaluate_permission_decision(
            tool_name,
            risk_class,
            override_rule.decision,
            override_rule.reason.as_deref(),
            true,
        );
    }

    match risk_class {
        ToolRiskClass::ReadOnly | ToolRiskClass::ShellReadOnly => (
            ToolPolicyDecision::AutoAllow,
            ToolApprovalState::NotRequired,
            None,
        ),
        ToolRiskClass::Write if approval.allow_write_tools => (
            ToolPolicyDecision::Approved,
            ToolApprovalState::Approved,
            Some(format!("tool `{tool_name}` was approved for write access")),
        ),
        ToolRiskClass::Network if approval.allow_network_shell => (
            ToolPolicyDecision::Approved,
            ToolApprovalState::Approved,
            Some(format!(
                "tool `{tool_name}` was approved for network access"
            )),
        ),
        ToolRiskClass::Destructive if approval.allow_destructive_shell => (
            ToolPolicyDecision::Approved,
            ToolApprovalState::Approved,
            Some(format!(
                "tool `{tool_name}` was approved for destructive access"
            )),
        ),
        ToolRiskClass::Write => denied_by_policy(approval, tool_name, "write"),
        ToolRiskClass::Network => denied_by_policy(approval, tool_name, "network"),
        ToolRiskClass::Destructive => denied_by_policy(approval, tool_name, "destructive"),
    }
}

fn matching_permission_override<'a>(
    tool_name: &str,
    risk_class: ToolRiskClass,
    approval: &'a ToolApprovalConfig,
) -> Option<&'a ToolPermissionOverride> {
    approval
        .overrides
        .iter()
        .enumerate()
        .filter(|(_, override_rule)| {
            override_rule
                .tool_name
                .as_deref()
                .map_or(true, |configured_tool| configured_tool == tool_name)
                && override_rule
                    .risk_class
                    .map_or(true, |configured_risk| configured_risk == risk_class)
        })
        .max_by_key(|(index, override_rule)| {
            let specificity = usize::from(override_rule.tool_name.is_some())
                + usize::from(override_rule.risk_class.is_some());
            (specificity, *index)
        })
        .map(|(_, override_rule)| override_rule)
}

fn evaluate_permission_decision(
    tool_name: &str,
    risk_class: ToolRiskClass,
    decision: ToolPermissionDecision,
    reason: Option<&str>,
    scoped_override: bool,
) -> (ToolPolicyDecision, ToolApprovalState, Option<String>) {
    let reason = reason
        .map(String::from)
        .or_else(|| default_permission_reason(tool_name, risk_class, decision, scoped_override));
    match decision {
        ToolPermissionDecision::Allow => {
            if matches!(
                risk_class,
                ToolRiskClass::ReadOnly | ToolRiskClass::ShellReadOnly
            ) {
                (
                    ToolPolicyDecision::AutoAllow,
                    ToolApprovalState::NotRequired,
                    reason,
                )
            } else {
                (
                    ToolPolicyDecision::Approved,
                    ToolApprovalState::Approved,
                    reason,
                )
            }
        }
        ToolPermissionDecision::Ask => (
            ToolPolicyDecision::Paused,
            ToolApprovalState::Pending,
            reason,
        ),
        ToolPermissionDecision::Deny => (
            ToolPolicyDecision::Refused,
            ToolApprovalState::Refused,
            reason,
        ),
    }
}

fn default_permission_reason(
    tool_name: &str,
    risk_class: ToolRiskClass,
    decision: ToolPermissionDecision,
    scoped_override: bool,
) -> Option<String> {
    let scope = if scoped_override {
        "scoped policy override"
    } else {
        "default policy"
    };
    match decision {
        ToolPermissionDecision::Allow => Some(format!(
            "tool `{tool_name}` allowed by {scope} for {} access",
            render_risk_class(risk_class)
        )),
        ToolPermissionDecision::Ask => Some(format!(
            "tool `{tool_name}` requires {} approval by {scope}",
            render_risk_class(risk_class)
        )),
        ToolPermissionDecision::Deny => Some(format!(
            "tool `{tool_name}` denied by {scope} for {} access",
            render_risk_class(risk_class)
        )),
    }
}

fn denied_by_policy(
    approval: &ToolApprovalConfig,
    tool_name: &str,
    class_name: &str,
) -> (ToolPolicyDecision, ToolApprovalState, Option<String>) {
    let reason = Some(format!("tool `{tool_name}` requires {class_name} approval"));
    match approval.denied_action {
        ToolDeniedAction::Refuse => (
            ToolPolicyDecision::Refused,
            ToolApprovalState::Refused,
            reason,
        ),
        ToolDeniedAction::Pause => (
            ToolPolicyDecision::Paused,
            ToolApprovalState::Pending,
            reason,
        ),
    }
}

fn denied_tool_invocation(
    context: &ToolExecutionContext,
    tool_name: &str,
    arguments: &serde_json::Value,
    risk_class: ToolRiskClass,
    reason: &Option<String>,
) -> ToolInvocationOutcome {
    let command = arguments
        .get("command")
        .and_then(serde_json::Value::as_str)
        .map(String::from);
    let files_touched = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .and_then(|path| resolve_workspace_path(context.cwd(), path).ok())
        .map(|path| vec![display_relative_path(context.cwd(), &path)])
        .unwrap_or_default();
    ToolInvocationOutcome {
        output: serde_json::json!({
            "error": "tool execution blocked by local approval policy",
            "tool": tool_name,
            "risk_class": render_risk_class(risk_class),
            "approval_required": true,
            "reason": reason,
        }),
        command,
        exit_code: None,
        timed_out: None,
        truncated: Some(false),
        bytes_returned: None,
        files_touched,
    }
}

#[must_use]
pub fn tool_input_summary(tool_name: &str, arguments: &serde_json::Value) -> serde_json::Value {
    match tool_name {
        "apply_patch" => serde_json::json!({
            "path": redacted_json_string_field(arguments, "path"),
            "old_text_chars": arguments.get("old_text").and_then(serde_json::Value::as_str).map(str::len),
            "new_text_chars": arguments.get("new_text").and_then(serde_json::Value::as_str).map(str::len),
            "replace_all": arguments.get("replace_all").and_then(serde_json::Value::as_bool).unwrap_or(false),
            "create_if_missing": arguments.get("create_if_missing").and_then(serde_json::Value::as_bool).unwrap_or(false)
        }),
        "shell" => serde_json::json!({
            "command": arguments
                .get("command")
                .and_then(serde_json::Value::as_str)
                .map(redact_and_truncate_text),
            "timeout_secs": arguments.get("timeout_secs").and_then(serde_json::Value::as_u64)
        }),
        "consult_oracle" => serde_json::json!({
            "task_kind": redacted_json_string_field(arguments, "task_kind"),
            "question_preview": arguments
                .get("question")
                .and_then(serde_json::Value::as_str)
                .map(redact_and_truncate_text),
            "question_chars": arguments.get("question").and_then(serde_json::Value::as_str).map(str::len)
        }),
        "analyze_repository" => serde_json::json!({
            "task_kind": redacted_json_string_field(arguments, "task_kind"),
            "question_preview": arguments
                .get("question")
                .and_then(serde_json::Value::as_str)
                .map(redact_and_truncate_text),
            "question_chars": arguments.get("question").and_then(serde_json::Value::as_str).map(str::len),
            "evidence_paths": redact_json_value(arguments.get("evidence_paths").unwrap_or(&serde_json::Value::Null))
        }),
        "legal.inspect_answer_file" => {
            let required_substring_count = arguments
                .get("required_substrings")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            serde_json::json!({
                "path": redacted_json_string_field(arguments, "path"),
                "required_substring_count": required_substring_count,
                "max_bytes": arguments.get("max_bytes").and_then(serde_json::Value::as_u64)
            })
        }
        _ => redact_json_value(arguments),
    }
}

#[must_use]
pub fn tool_output_summary(tool_name: &str, output: &serde_json::Value) -> serde_json::Value {
    if output.get("error").is_some() {
        return serde_json::json!({
            "error": output
                .get("error")
                .and_then(serde_json::Value::as_str)
                .map(redact_and_truncate_text),
            "reason": output
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .map(redact_and_truncate_text),
            "risk_class": output.get("risk_class").cloned().unwrap_or(serde_json::Value::Null),
            "approval_required": output
                .get("approval_required")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        });
    }

    match tool_name {
        "read_file" => serde_json::json!({
            "path": redacted_json_string_field(output, "path"),
            "start_line": output.get("start_line").and_then(serde_json::Value::as_u64),
            "end_line": output.get("end_line").and_then(serde_json::Value::as_u64),
            "total_lines": output.get("total_lines").and_then(serde_json::Value::as_u64),
            "truncated": output.get("truncated").and_then(serde_json::Value::as_bool).unwrap_or(false),
            "content_preview": output
                .get("content")
                .and_then(serde_json::Value::as_str)
                .map(redact_and_truncate_text)
        }),
        "list_files" => {
            let entries = output
                .get("entries")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            serde_json::json!({
                "path": redacted_json_string_field(output, "path"),
                "entry_count": entries.len(),
                "entries_preview": redact_json_value(&serde_json::Value::Array(
                    entries.into_iter().take(TOOL_SUMMARY_MAX_ARRAY_ITEMS).collect()
                )),
                "truncated": output.get("truncated").and_then(serde_json::Value::as_bool).unwrap_or(false)
            })
        }
        "code_search" => {
            let matches = output
                .get("matches")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            let paths = matches
                .iter()
                .filter_map(|entry| entry.get("path").and_then(serde_json::Value::as_str))
                .take(TOOL_SUMMARY_MAX_ARRAY_ITEMS)
                .map(|path| serde_json::Value::String(redact_and_truncate_text(path)))
                .collect::<Vec<_>>();
            serde_json::json!({
                "path": redacted_json_string_field(output, "path"),
                "match_count": matches.len(),
                "matched_paths_preview": paths,
                "status_code": output.get("status_code").and_then(serde_json::Value::as_i64),
                "truncated": output.get("truncated").and_then(serde_json::Value::as_bool).unwrap_or(false)
            })
        }
        "shell" => serde_json::json!({
            "command": output
                .get("command")
                .and_then(serde_json::Value::as_str)
                .map(redact_and_truncate_text),
            "timed_out": output.get("timed_out").and_then(serde_json::Value::as_bool).unwrap_or(false),
            "exit_code": output.get("exit_code").and_then(serde_json::Value::as_i64),
            "stdout_preview": output
                .get("stdout")
                .and_then(serde_json::Value::as_str)
                .map(redact_and_truncate_text),
            "stderr_preview": output
                .get("stderr")
                .and_then(serde_json::Value::as_str)
                .map(redact_and_truncate_text),
            "stdout_truncated": output.get("stdout_truncated").and_then(serde_json::Value::as_bool).unwrap_or(false),
            "stderr_truncated": output.get("stderr_truncated").and_then(serde_json::Value::as_bool).unwrap_or(false)
        }),
        "apply_patch" => serde_json::json!({
            "path": redacted_json_string_field(output, "path"),
            "created": output.get("created").and_then(serde_json::Value::as_bool).unwrap_or(false),
            "replace_all": output.get("replace_all").and_then(serde_json::Value::as_bool).unwrap_or(false),
            "bytes_written": output.get("bytes_written").and_then(serde_json::Value::as_u64)
        }),
        "consult_oracle" => serde_json::json!({
            "task_kind": redacted_json_string_field(output, "task_kind"),
            "oracle_profile": redacted_json_string_field(output, "oracle_profile"),
            "oracle_model": redacted_json_string_field(output, "oracle_model"),
            "answer_preview": output
                .get("oracle_answer")
                .and_then(serde_json::Value::as_str)
                .map(redact_and_truncate_text),
            "calls_used_after": output.get("calls_used_after").and_then(serde_json::Value::as_u64),
            "max_calls": output.get("max_calls").and_then(serde_json::Value::as_u64)
        }),
        "analyze_repository" => serde_json::json!({
            "task_kind": redacted_json_string_field(output, "task_kind"),
            "analysis_profile": redacted_json_string_field(output, "analysis_profile"),
            "analysis_model": redacted_json_string_field(output, "analysis_model"),
            "analysis_preview": output
                .get("analysis")
                .and_then(serde_json::Value::as_str)
                .map(redact_and_truncate_text),
            "evidence_count": output
                .get("evidence")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
                .unwrap_or(0),
            "calls_used_after": output.get("calls_used_after").and_then(serde_json::Value::as_u64),
            "max_calls": output.get("max_calls").and_then(serde_json::Value::as_u64)
        }),
        "legal.inspect_answer_file" => serde_json::json!({
            "path": redacted_json_string_field(output, "path"),
            "exists": output.get("exists").and_then(serde_json::Value::as_bool).unwrap_or(false),
            "bytes": output.get("bytes").and_then(serde_json::Value::as_u64),
            "bytes_inspected": output.get("bytes_inspected").and_then(serde_json::Value::as_u64),
            "line_count": output.get("line_count").and_then(serde_json::Value::as_u64),
            "sha256": redacted_json_string_field(output, "sha256"),
            "truncated": output.get("truncated").and_then(serde_json::Value::as_bool).unwrap_or(false),
            "required_substring_count": output
                .get("required_substring_count")
                .and_then(serde_json::Value::as_u64),
            "all_required_substrings_present": output
                .get("all_required_substrings_present")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        }),
        _ => redact_json_value(output),
    }
}

fn redacted_json_string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(redact_and_truncate_text)
}

fn redact_json_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, value)| {
                    if is_sensitive_key(key) {
                        (
                            key.clone(),
                            serde_json::Value::String(String::from("[redacted]")),
                        )
                    } else {
                        (key.clone(), redact_json_value(value))
                    }
                })
                .collect(),
        ),
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .take(TOOL_SUMMARY_MAX_ARRAY_ITEMS)
                .map(redact_json_value)
                .collect(),
        ),
        serde_json::Value::String(text) => {
            serde_json::Value::String(redact_and_truncate_text(text))
        }
        other => other.clone(),
    }
}

fn redact_and_truncate_text(text: &str) -> String {
    let redacted = redact_sensitive_text(text);
    let total = redacted.chars().count();
    if total <= TOOL_SUMMARY_MAX_CHARS {
        return redacted;
    }
    let mut preview = redacted
        .chars()
        .take(TOOL_SUMMARY_MAX_CHARS)
        .collect::<String>();
    preview.push_str("...");
    preview
}

fn redact_sensitive_text(text: &str) -> String {
    text.split_whitespace()
        .map(|token| {
            if should_redact_token(token) {
                String::from("[redacted]")
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .replace("/Users/", "[redacted-path]/")
        .replace("/private/var/", "[redacted-path]/")
}

fn should_redact_token(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    token.starts_with("sk-")
        || lower.starts_with("bearer ")
        || lower.starts_with("authorization:")
        || lower.contains("refresh_token")
        || lower.contains("access_token")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || token
            .split_once('=')
            .map_or(false, |(key, _)| is_sensitive_key(key))
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    if normalized.contains("apikey")
        || normalized.contains("secret")
        || normalized.contains("password")
        || normalized.contains("authorization")
        || normalized.contains("credential")
        || normalized.contains("token")
    {
        return true;
    }
    matches!(
        normalized.as_str(),
        "password"
            | "passwd"
            | "secret"
            | "token"
            | "apikey"
            | "authorization"
            | "credential"
            | "credentials"
            | "privatekey"
            | "refreshtoken"
            | "accesstoken"
            | "clientsecret"
            | "signingsecret"
            | "nonce"
    )
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn looks_like_natural_language_shell_misuse(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.ends_with('?') {
        return true;
    }
    if contains_any(trimmed, &["\n", "\r"]) {
        return false;
    }

    let tokens = trimmed.split_whitespace().collect::<Vec<_>>();
    let first = tokens[0].to_ascii_lowercase();
    if is_common_shell_command(first.as_str()) {
        return false;
    }

    let all_word_tokens = tokens.iter().all(|token| {
        token
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || ch == '\'' || ch == '"')
    });
    let has_shell_punctuation = trimmed.chars().any(|ch| {
        matches!(
            ch,
            '/' | '\\'
                | '.'
                | '-'
                | '_'
                | '='
                | ':'
                | '$'
                | '~'
                | '*'
                | '|'
                | '&'
                | ';'
                | '<'
                | '>'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '@'
        )
    });

    if tokens.len() == 1 && all_word_tokens && !has_shell_punctuation {
        return true;
    }

    all_word_tokens && !has_shell_punctuation
}

fn is_common_shell_command(token: &str) -> bool {
    matches!(
        token,
        "ls" | "cat"
            | "pwd"
            | "cd"
            | "git"
            | "cargo"
            | "rg"
            | "grep"
            | "find"
            | "sed"
            | "awk"
            | "bash"
            | "sh"
            | "zsh"
            | "python"
            | "python3"
            | "node"
            | "npm"
            | "pnpm"
            | "yarn"
            | "uv"
            | "pip"
            | "make"
            | "cmake"
            | "cp"
            | "mv"
            | "rm"
            | "mkdir"
            | "touch"
            | "echo"
            | "printf"
            | "whoami"
            | "uname"
            | "ps"
            | "kill"
            | "chmod"
            | "chown"
            | "tar"
            | "zip"
            | "unzip"
            | "brew"
            | "swift"
            | "swiftc"
            | "xcodebuild"
    )
}

fn is_read_only_shell_command(command: &str) -> bool {
    let first = command.split_whitespace().next().unwrap_or_default();
    matches!(
        first,
        "pwd"
            | "ls"
            | "cat"
            | "sed"
            | "head"
            | "tail"
            | "rg"
            | "find"
            | "fd"
            | "wc"
            | "stat"
            | "file"
            | "du"
            | "tree"
            | "ps"
            | "env"
            | "which"
            | "readlink"
            | "printf"
            | "echo"
            | "git"
    ) && is_safe_git_read_only(command)
}

fn is_safe_git_read_only(command: &str) -> bool {
    if !command.starts_with("git") {
        return true;
    }
    let mut parts = command.split_whitespace();
    let _ = parts.next();
    let subcommand = parts.next().unwrap_or_default();
    match subcommand {
        "status" | "diff" | "show" | "log" | "rev-parse" | "grep" => true,
        "branch" => parts.all(|part| part.starts_with('-')),
        _ => false,
    }
}

fn render_risk_class(risk_class: ToolRiskClass) -> &'static str {
    match risk_class {
        ToolRiskClass::ReadOnly => "read_only",
        ToolRiskClass::ShellReadOnly => "shell_read_only",
        ToolRiskClass::Write => "write",
        ToolRiskClass::Network => "network",
        ToolRiskClass::Destructive => "destructive",
    }
}

fn tool_result_schema(tool_name: &str) -> serde_json::Value {
    match tool_name {
        "read_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "start_line": { "type": "integer" },
                "end_line": { "type": "integer" },
                "total_lines": { "type": "integer" },
                "truncated": { "type": "boolean" },
                "content": { "type": "string" }
            },
            "required": ["path", "start_line", "end_line", "total_lines", "truncated", "content"]
        }),
        "list_files" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "max_depth": { "type": "integer" },
                "truncated": { "type": "boolean" },
                "entries": { "type": "array" }
            },
            "required": ["path", "max_depth", "truncated", "entries"]
        }),
        "code_search" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "pattern": { "type": "string" },
                "matches": { "type": "array" },
                "truncated": { "type": "boolean" },
                "status_code": { "type": "integer" }
            },
            "required": ["path", "pattern", "matches", "truncated", "status_code"]
        }),
        "shell" => serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "timeout_secs": { "type": "integer" },
                "timed_out": { "type": "boolean" },
                "exit_code": { "type": ["integer", "null"] },
                "stdout": { "type": "string" },
                "stderr": { "type": "string" },
                "stdout_truncated": { "type": "boolean" },
                "stderr_truncated": { "type": "boolean" }
            },
            "required": ["command", "timeout_secs", "timed_out", "stdout", "stderr"]
        }),
        "apply_patch" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "created": { "type": "boolean" },
                "replace_all": { "type": "boolean" },
                "bytes_written": { "type": "integer" }
            },
            "required": ["path", "created", "replace_all", "bytes_written"]
        }),
        "consult_oracle" => serde_json::json!({
            "type": "object",
            "properties": {
                "task_kind": { "type": "string" },
                "question": { "type": "string" },
                "oracle_profile": { "type": "string" },
                "oracle_model": { "type": "string" },
                "oracle_answer": { "type": "string" },
                "calls_used_after": { "type": "integer" },
                "max_calls": { "type": "integer" }
            }
        }),
        "analyze_repository" => serde_json::json!({
            "type": "object",
            "properties": {
                "task_kind": { "type": "string" },
                "question": { "type": "string" },
                "analysis_profile": { "type": "string" },
                "analysis_model": { "type": "string" },
                "analysis": { "type": "string" },
                "calls_used_after": { "type": "integer" },
                "max_calls": { "type": "integer" },
                "evidence": { "type": "array" }
            }
        }),
        "legal.inspect_answer_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "exists": { "type": "boolean" },
                "bytes": { "type": "integer" },
                "bytes_inspected": { "type": "integer" },
                "line_count": { "type": "integer" },
                "sha256": { "type": "string" },
                "truncated": { "type": "boolean" },
                "required_substring_count": { "type": "integer" },
                "required_substrings": { "type": "array" },
                "all_required_substrings_present": { "type": "boolean" }
            },
            "required": [
                "path",
                "exists",
                "bytes",
                "bytes_inspected",
                "line_count",
                "sha256",
                "truncated",
                "required_substring_count",
                "required_substrings",
                "all_required_substrings_present"
            ]
        }),
        _ => serde_json::json!({
            "type": "object",
            "additionalProperties": true
        }),
    }
}

fn read_file_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Relative path to a text file inside the session cwd." },
            "start_line": { "type": "integer", "description": "1-based line to start reading from.", "minimum": 1 },
            "max_lines": { "type": "integer", "description": "Maximum number of lines to return.", "minimum": 1 }
        },
        "required": ["path"],
        "additionalProperties": false
    })
}

fn list_files_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Relative directory path inside the session cwd." },
            "max_depth": { "type": "integer", "description": "Maximum recursion depth.", "minimum": 0 },
            "max_entries": { "type": "integer", "description": "Maximum number of entries to return.", "minimum": 1 },
            "include_hidden": { "type": "boolean", "description": "Whether dotfiles and dot-directories should be included." }
        },
        "additionalProperties": false
    })
}

fn code_search_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "pattern": { "type": "string", "description": "ripgrep search pattern." },
            "path": { "type": "string", "description": "Optional relative path to limit the search scope." },
            "glob": { "type": "string", "description": "Optional glob passed to ripgrep." },
            "max_matches": { "type": "integer", "description": "Maximum number of matches to return.", "minimum": 1 },
            "case_sensitive": { "type": "boolean", "description": "Whether the search should be case sensitive." }
        },
        "required": ["pattern"],
        "additionalProperties": false
    })
}

fn shell_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "command": { "type": "string", "description": "Shell command to execute inside the session cwd." },
            "timeout_secs": { "type": "integer", "description": "Maximum runtime before the command is killed.", "minimum": 1 }
        },
        "required": ["command"],
        "additionalProperties": false
    })
}

fn apply_patch_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Relative file path to patch inside the session cwd." },
            "old_text": { "type": "string", "description": "Existing text that must match before replacement." },
            "new_text": { "type": "string", "description": "Replacement text." },
            "replace_all": { "type": "boolean", "description": "Whether every match should be replaced instead of exactly one." },
            "create_if_missing": { "type": "boolean", "description": "Whether the file can be created when old_text is empty." }
        },
        "required": ["path", "old_text", "new_text"],
        "additionalProperties": false
    })
}

fn consult_oracle_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "task_kind": { "type": "string", "description": "Oracle task type: planning, checking, or research." },
            "question": { "type": "string", "description": "Bounded question for the oracle model." }
        },
        "required": ["task_kind", "question"],
        "additionalProperties": false
    })
}

fn analyze_repository_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "task_kind": { "type": "string", "description": "Repo-analysis task type: repo_analysis, architecture_summary, change_impact, or synthesis." },
            "question": { "type": "string", "description": "Bounded repo-analysis question to answer from the evidence files." },
            "evidence_paths": {
                "type": "array",
                "description": "Relative file paths that provide the evidence for the analysis.",
                "items": { "type": "string" },
                "minItems": 1
            }
        },
        "required": ["task_kind", "question", "evidence_paths"],
        "additionalProperties": false
    })
}

fn legal_inspect_answer_file_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Relative path to the legal benchmark answer file inside the session cwd." },
            "required_substrings": {
                "type": "array",
                "description": "Optional exact answer markers to check for. Raw markers are not returned in tool output.",
                "items": { "type": "string" }
            },
            "max_bytes": {
                "type": "integer",
                "description": "Maximum leading bytes to inspect for substring presence. Hard-capped by Probe.",
                "minimum": 1
            }
        },
        "required": ["path"],
        "additionalProperties": false
    })
}

fn expect_string<'a>(
    arguments: &'a serde_json::Value,
    key: &str,
    tool_name: &str,
) -> Result<&'a str, ToolInvocationError> {
    arguments
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ToolInvocationError::InvalidArguments(format!(
                "{tool_name} requires a string `{key}` argument"
            ))
        })
}

fn expect_u64(arguments: &serde_json::Value, key: &str) -> Option<u64> {
    arguments.get(key).and_then(serde_json::Value::as_u64)
}

fn expect_string_array(
    arguments: &serde_json::Value,
    key: &str,
    tool_name: &str,
) -> Result<Vec<String>, ToolInvocationError> {
    arguments
        .get(key)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            ToolInvocationError::InvalidArguments(format!(
                "{tool_name} requires an array `{key}` argument"
            ))
        })
        .and_then(|values| {
            values
                .iter()
                .map(|value| {
                    value.as_str().map(String::from).ok_or_else(|| {
                        ToolInvocationError::InvalidArguments(format!(
                            "{tool_name} requires every `{key}` entry to be a string"
                        ))
                    })
                })
                .collect()
        })
}

fn sha256_hex_prefixed(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn resolve_workspace_path(
    base: &Path,
    requested_path: &str,
) -> Result<PathBuf, ToolInvocationError> {
    if requested_path.trim().is_empty() {
        return Err(ToolInvocationError::InvalidArguments(String::from(
            "tool paths must not be empty",
        )));
    }
    let base_dir = if base.is_absolute() {
        base.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(base)
    };
    let mut resolved = base_dir.clone();
    for component in Path::new(requested_path).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => resolved.push(part),
            Component::ParentDir => {
                if resolved == base_dir {
                    return Err(ToolInvocationError::InvalidArguments(format!(
                        "path `{requested_path}` escapes the session cwd"
                    )));
                }
                resolved.pop();
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ToolInvocationError::InvalidArguments(format!(
                    "path `{requested_path}` must be relative to the session cwd"
                )));
            }
        }
    }
    Ok(resolved)
}

fn display_relative_path(base: &Path, path: &Path) -> String {
    let base_dir = if base.is_absolute() {
        base.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(base)
    };
    path.strip_prefix(base_dir)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn relativize_search_match(base: &Path, raw_path: &str) -> String {
    let candidate = Path::new(raw_path);
    if candidate.is_absolute() {
        display_relative_path(base, candidate)
    } else {
        candidate.display().to_string()
    }
}

fn visit_directory(
    root: &Path,
    current: &Path,
    remaining_depth: usize,
    max_entries: usize,
    include_hidden: bool,
    entries: &mut Vec<serde_json::Value>,
    truncated: &mut bool,
) -> Result<(), ToolInvocationError> {
    if entries.len() >= max_entries {
        *truncated = true;
        return Ok(());
    }

    let mut directory_entries = fs::read_dir(current)
        .map_err(|error| {
            ToolInvocationError::ExecutionFailed(format!(
                "failed to list directory `{}`: {error}",
                current.display()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            ToolInvocationError::ExecutionFailed(format!(
                "failed to read directory entries for `{}`: {error}",
                current.display()
            ))
        })?;
    directory_entries.sort_by_key(|entry| entry.file_name());

    for entry in directory_entries {
        if entries.len() >= max_entries {
            *truncated = true;
            break;
        }
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !include_hidden && file_name.starts_with('.') {
            continue;
        }

        let entry_path = entry.path();
        let metadata = entry.metadata().map_err(|error| {
            ToolInvocationError::ExecutionFailed(format!(
                "failed to read metadata for `{}`: {error}",
                entry_path.display()
            ))
        })?;
        let kind = if metadata.is_dir() {
            "directory"
        } else if metadata.is_file() {
            "file"
        } else {
            "other"
        };
        let relative = entry_path
            .strip_prefix(root)
            .unwrap_or(&entry_path)
            .display()
            .to_string();
        entries.push(serde_json::json!({
            "path": relative,
            "kind": kind,
        }));

        if metadata.is_dir() && remaining_depth > 0 {
            visit_directory(
                root,
                &entry_path,
                remaining_depth - 1,
                max_entries,
                include_hidden,
                entries,
                truncated,
            )?;
        }
    }

    Ok(())
}

fn truncate_text(text: &str) -> (String, bool) {
    let total = text.chars().count();
    if total <= SHELL_MAX_OUTPUT_CHARS {
        return (String::from(text), false);
    }
    let truncated = text
        .chars()
        .take(SHELL_MAX_OUTPUT_CHARS)
        .collect::<String>();
    (
        format!(
            "{truncated}\n...[truncated {} chars]",
            total - SHELL_MAX_OUTPUT_CHARS
        ),
        true,
    )
}

#[must_use]
pub fn tool_result_model_text(tool_name: &str, output: &serde_json::Value) -> String {
    truncate_model_text(&match tool_name {
        "read_file" => render_read_file_model_text(output),
        "list_files" => render_list_files_model_text(output),
        "code_search" => render_code_search_model_text(output),
        "shell" => render_shell_model_text(output),
        "apply_patch" => render_apply_patch_model_text(output),
        "consult_oracle" => render_consult_oracle_model_text(output),
        "analyze_repository" => render_analyze_repository_model_text(output),
        "legal.inspect_answer_file" => render_legal_inspect_answer_file_model_text(output),
        _ => serde_json::to_string_pretty(output).unwrap_or_else(|_| output.to_string()),
    })
}

#[must_use]
pub fn stored_tool_result_model_text(tool_name: &str, stored_text: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(stored_text) {
        Ok(output) => tool_result_model_text(tool_name, &output),
        Err(_) => truncate_model_text(stored_text),
    }
}

fn render_read_file_model_text(output: &serde_json::Value) -> String {
    let path = output
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let start_line = output.get("start_line").and_then(serde_json::Value::as_u64);
    let end_line = output.get("end_line").and_then(serde_json::Value::as_u64);
    let total_lines = output
        .get("total_lines")
        .and_then(serde_json::Value::as_u64);
    let mut lines = vec![format!("path: {path}")];
    match (start_line, end_line, total_lines) {
        (Some(start), Some(end), Some(total)) => {
            lines.push(format!("lines: {start}-{end} of {total}"));
        }
        (Some(start), Some(end), None) => {
            lines.push(format!("lines: {start}-{end}"));
        }
        _ => {}
    }
    if let Some(truncated) = output.get("truncated").and_then(serde_json::Value::as_bool) {
        lines.push(format!("tool_truncated: {truncated}"));
    }
    if let Some(content) = output.get("content").and_then(serde_json::Value::as_str) {
        lines.push(String::from("content:"));
        if content.is_empty() {
            lines.push(String::from("[empty file segment]"));
        } else {
            lines.extend(content.lines().map(ToOwned::to_owned));
        }
    }
    lines.join("\n")
}

fn render_list_files_model_text(output: &serde_json::Value) -> String {
    let path = output
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(".");
    let max_depth = output.get("max_depth").and_then(serde_json::Value::as_u64);
    let entries = output
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut lines = vec![format!("path: {path}")];
    if let Some(max_depth) = max_depth {
        lines.push(format!("max_depth: {max_depth}"));
    }
    lines.push(format!("entries: {}", entries.len()));
    if let Some(truncated) = output.get("truncated").and_then(serde_json::Value::as_bool) {
        lines.push(format!("tool_truncated: {truncated}"));
    }
    if !entries.is_empty() {
        lines.push(String::from("listing:"));
        for entry in entries {
            let entry_path = entry
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let kind = entry
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("other");
            lines.push(format!("- {kind} {entry_path}"));
        }
    }
    lines.join("\n")
}

fn normalize_tool_directory_path(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.is_empty() { "." } else { trimmed }
}

fn render_code_search_model_text(output: &serde_json::Value) -> String {
    let matches = output
        .get("matches")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut lines = vec![format!("matches: {}", matches.len())];
    if let Some(search_backend) = output
        .get("search_backend")
        .and_then(serde_json::Value::as_str)
    {
        lines.push(format!("engine: {search_backend}"));
    }
    if let Some(truncated) = output.get("truncated").and_then(serde_json::Value::as_bool) {
        lines.push(format!("tool_truncated: {truncated}"));
    }
    if !matches.is_empty() {
        lines.push(String::from("results:"));
        for entry in matches {
            let match_path = entry
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let line = entry
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let column = entry
                .get("column")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let snippet = entry
                .get("snippet")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            lines.push(format!("- {match_path}:{line}:{column} {snippet}"));
        }
    }
    lines.join("\n")
}

fn render_shell_model_text(output: &serde_json::Value) -> String {
    let timeout_secs = output
        .get("timeout_secs")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let timed_out = output
        .get("timed_out")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let exit_code = output
        .get("exit_code")
        .and_then(serde_json::Value::as_i64)
        .map_or_else(|| String::from("none"), |value| value.to_string());
    let stdout = output
        .get("stdout")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let stderr = output
        .get("stderr")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let stdout_truncated = output
        .get("stdout_truncated")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let stderr_truncated = output
        .get("stderr_truncated")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let mut lines = vec![
        format!("timed_out: {timed_out}"),
        format!("exit_code: {exit_code}"),
    ];
    if !stdout.is_empty() {
        lines.push(format!("stdout_truncated: {stdout_truncated}"));
        lines.push(String::from("output:"));
        lines.extend(stdout.lines().map(ToOwned::to_owned));
    }
    if !stderr.is_empty() {
        lines.push(format!("stderr_truncated: {stderr_truncated}"));
        lines.push(String::from("stderr:"));
        lines.extend(stderr.lines().map(ToOwned::to_owned));
    }
    if stdout.is_empty() && stderr.is_empty() {
        lines.push(format!("timeout_secs: {timeout_secs}"));
        lines.push(String::from("no output"));
    }
    lines.join("\n")
}

fn simple_glob_matches(glob: &str, candidate: &str) -> bool {
    fn inner(glob: &[u8], candidate: &[u8]) -> bool {
        if glob.is_empty() {
            return candidate.is_empty();
        }
        match glob[0] {
            b'*' => {
                inner(&glob[1..], candidate)
                    || (!candidate.is_empty() && inner(glob, &candidate[1..]))
            }
            b'?' => !candidate.is_empty() && inner(&glob[1..], &candidate[1..]),
            byte => {
                !candidate.is_empty() && byte == candidate[0] && inner(&glob[1..], &candidate[1..])
            }
        }
    }

    inner(glob.as_bytes(), candidate.as_bytes())
}

fn render_apply_patch_model_text(output: &serde_json::Value) -> String {
    let path = output
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let created = output
        .get("created")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let replace_all = output
        .get("replace_all")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let bytes_written = output
        .get("bytes_written")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    [
        format!("path: {path}"),
        format!("created: {created}"),
        format!("replace_all: {replace_all}"),
        format!("bytes_written: {bytes_written}"),
    ]
    .join("\n")
}

fn render_consult_oracle_model_text(output: &serde_json::Value) -> String {
    let task_kind = output
        .get("task_kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let answer = output
        .get("oracle_answer")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    [
        format!("task_kind: {task_kind}"),
        String::from("oracle_answer:"),
        answer.to_string(),
    ]
    .join("\n")
}

fn render_analyze_repository_model_text(output: &serde_json::Value) -> String {
    let task_kind = output
        .get("task_kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let analysis = output
        .get("analysis")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let evidence = output
        .get("evidence")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut lines = vec![format!("task_kind: {task_kind}")];
    if let Some(reason) = output
        .get("decision_reason")
        .and_then(serde_json::Value::as_str)
    {
        lines.push(format!("decision_reason: {reason}"));
    }
    if !evidence.is_empty() {
        lines.push(String::from("evidence_paths:"));
        for entry in evidence {
            let path = entry
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            lines.push(format!("- {path}"));
        }
    }
    lines.push(String::from("analysis:"));
    lines.push(analysis.to_string());
    lines.join("\n")
}

fn render_legal_inspect_answer_file_model_text(output: &serde_json::Value) -> String {
    let path = output
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let bytes = output
        .get("bytes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let bytes_inspected = output
        .get("bytes_inspected")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let line_count = output
        .get("line_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let sha256 = output
        .get("sha256")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("sha256:unknown");
    let required_substring_count = output
        .get("required_substring_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let all_required_substrings_present = output
        .get("all_required_substrings_present")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let truncated = output
        .get("truncated")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    [
        format!("path: {path}"),
        format!("bytes: {bytes}"),
        format!("bytes_inspected: {bytes_inspected}"),
        format!("line_count: {line_count}"),
        format!("sha256: {sha256}"),
        format!("tool_truncated: {truncated}"),
        format!("required_substring_count: {required_substring_count}"),
        format!("all_required_substrings_present: {all_required_substrings_present}"),
        String::from("raw_content_returned: false"),
    ]
    .join("\n")
}

fn truncate_model_text(text: &str) -> String {
    let total = text.chars().count();
    if total <= TOOL_MODEL_TEXT_MAX_CHARS {
        return text.to_string();
    }
    let prefix = text
        .chars()
        .take(TOOL_MODEL_TEXT_MAX_CHARS)
        .collect::<String>();
    format!(
        "{prefix}\n...[truncated {} chars for model replay]",
        total - TOOL_MODEL_TEXT_MAX_CHARS
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use probe_protocol::session::{ToolPermissionDecision, ToolPolicyDecision, ToolRiskClass};
    use probe_provider_openai::{ChatToolCall, ChatToolCallFunction};
    use probe_test_support::FakeAppleFmServer;
    use tempfile::tempdir;

    use crate::backend_profiles::{
        psionic_apple_fm_oracle, psionic_qwen35_2b_q8_long_context, psionic_qwen35_2b_q8_oracle,
    };

    use super::{
        ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction, ToolExecutionContext,
        ToolLongContextConfig, ToolLongContextContext, ToolLoopConfig, ToolOracleConfig,
        ToolOracleContext, ToolRegistry, ToolWriteGate, code_search_without_ripgrep,
        stored_tool_result_model_text, tool_input_summary, tool_output_summary,
        tool_result_model_text,
    };

    #[test]
    fn coding_bootstrap_registry_declares_all_tools() {
        let registry = ToolRegistry::coding_bootstrap(false, false);
        let tools = registry
            .declared_tools()
            .into_iter()
            .map(|tool| tool.function.name)
            .collect::<Vec<_>>();
        assert_eq!(registry.name(), "coding_bootstrap");
        assert_eq!(
            tools,
            vec![
                "apply_patch",
                "code_search",
                "list_files",
                "read_file",
                "shell"
            ]
        );
    }

    #[test]
    fn coding_bootstrap_domain_helper_manifest_projects_read_only_legal_helper() {
        let registry = ToolRegistry::coding_bootstrap_with_domain_helpers(false, false, true);
        let manifest = registry.managed_tool_manifest(&ToolApprovalConfig::conservative());
        let helper = manifest
            .tools
            .iter()
            .find(|tool| tool.name == "legal.inspect_answer_file")
            .expect("legal helper manifest entry");

        assert_eq!(helper.declared_risk_class, Some(ToolRiskClass::ReadOnly));
        assert_eq!(helper.possible_risk_classes, vec![ToolRiskClass::ReadOnly]);
        assert_eq!(helper.execution_owner, "probe_runtime");
        assert_eq!(
            helper.input_schema["required"]
                .as_array()
                .expect("required array")[0],
            "path"
        );
        assert_eq!(
            helper.result_schema["required"]
                .as_array()
                .expect("result required")
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>(),
            vec![
                "path",
                "exists",
                "bytes",
                "bytes_inspected",
                "line_count",
                "sha256",
                "truncated",
                "required_substring_count",
                "required_substrings",
                "all_required_substrings_present"
            ]
        );
        assert!(
            helper
                .redaction
                .input_summary
                .contains("raw legal answer markers")
        );
        let read_only_policy = manifest
            .policy
            .default_decisions
            .iter()
            .find(|entry| entry.risk_class == ToolRiskClass::ReadOnly)
            .expect("read-only policy");
        assert_eq!(read_only_policy.decision, ToolPermissionDecision::Allow);

        let default_registry = ToolRegistry::coding_bootstrap(false, false);
        assert!(
            default_registry
                .declared_tools()
                .iter()
                .all(|tool| tool.function.name != "legal.inspect_answer_file")
        );
    }

    #[test]
    fn legal_answer_helper_inspects_file_without_returning_raw_content() {
        let tempdir = tempdir().expect("tempdir");
        fs::write(
            tempdir.path().join("answer.md"),
            "The contract answer turns on contingency fee limits.\nPrivileged fact pattern.\n",
        )
        .expect("write answer");
        let registry = ToolRegistry::coding_bootstrap_with_domain_helpers(false, false, true);
        let context = ToolExecutionContext::new(tempdir.path());
        let arguments = "{\"path\":\"answer.md\",\"required_substrings\":[\"contingency fee\",\"missing marker\"]}";
        let results = registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_legal_inspect"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("legal.inspect_answer_file"),
                    arguments: String::from(arguments),
                },
            }],
            &ToolApprovalConfig::conservative(),
        );

        let result = &results[0];
        assert_eq!(result.tool_execution.risk_class, ToolRiskClass::ReadOnly);
        assert_eq!(
            result.tool_execution.policy_decision,
            ToolPolicyDecision::AutoAllow
        );
        assert_eq!(result.output["path"], "answer.md");
        assert_eq!(result.output["exists"], true);
        assert_eq!(result.output["required_substring_count"], 2);
        assert_eq!(
            result.output["required_substrings"][0]["present"],
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            result.output["required_substrings"][1]["present"],
            serde_json::Value::Bool(false)
        );
        assert_eq!(result.output["all_required_substrings_present"], false);
        assert_eq!(result.tool_execution.files_touched, vec!["answer.md"]);

        let serialized_output = serde_json::to_string(&result.output).expect("serialize output");
        assert!(!serialized_output.contains("contingency fee"));
        assert!(!serialized_output.contains("Privileged fact pattern"));
        assert!(serialized_output.contains("substring_sha256"));
        assert!(serialized_output.contains("sha256:"));

        let parsed_arguments =
            serde_json::from_str::<serde_json::Value>(arguments).expect("parse arguments");
        let input_summary = tool_input_summary("legal.inspect_answer_file", &parsed_arguments);
        let output_summary = tool_output_summary("legal.inspect_answer_file", &result.output);
        assert_eq!(input_summary["required_substring_count"], 2);
        assert!(!input_summary.to_string().contains("contingency fee"));
        assert!(
            !output_summary
                .to_string()
                .contains("Privileged fact pattern")
        );

        let rendered = tool_result_model_text("legal.inspect_answer_file", &result.output);
        assert!(rendered.contains("raw_content_returned: false"));
        assert!(!rendered.contains("contingency fee"));
        assert!(!rendered.contains("Privileged fact pattern"));
    }

    #[test]
    fn coding_bootstrap_reads_files_relative_to_context() {
        let tempdir = tempdir().expect("tempdir");
        fs::write(
            tempdir.path().join("notes.txt"),
            "one\ntwo\nthree\nfour\nfive\n",
        )
        .expect("write notes");
        let registry = ToolRegistry::coding_bootstrap(false, false);
        let context = ToolExecutionContext::new(tempdir.path());
        let results = registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_read"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("read_file"),
                    arguments: String::from(
                        "{\"path\":\"notes.txt\",\"start_line\":2,\"max_lines\":2}",
                    ),
                },
            }],
            &ToolApprovalConfig::conservative(),
        );

        assert_eq!(results[0].output["path"], "notes.txt");
        assert_eq!(results[0].output["content"], "two\nthree");
        assert_eq!(results[0].output["truncated"], true);
    }

    #[test]
    fn coding_bootstrap_lists_directories_relative_to_context() {
        let tempdir = tempdir().expect("tempdir");
        fs::create_dir_all(tempdir.path().join("src/bin")).expect("mkdirs");
        fs::write(tempdir.path().join("src/main.rs"), "fn main() {}").expect("write main");
        let registry = ToolRegistry::coding_bootstrap(false, false);
        let context = ToolExecutionContext::new(tempdir.path());
        let results = registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_list"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("list_files"),
                    arguments: String::from(
                        "{\"path\":\"src\",\"max_depth\":2,\"max_entries\":10}",
                    ),
                },
            }],
            &ToolApprovalConfig::conservative(),
        );

        let entries = results[0].output["entries"]
            .as_array()
            .expect("entries array");
        assert!(
            entries
                .iter()
                .any(|entry| entry["path"] == "bin" && entry["kind"] == "directory")
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry["path"] == "main.rs" && entry["kind"] == "file")
        );
    }

    #[test]
    fn coding_bootstrap_can_apply_deterministic_patch() {
        let tempdir = tempdir().expect("tempdir");
        let path = tempdir.path().join("hello.txt");
        fs::write(&path, "hello world\n").expect("write file");
        let registry = ToolRegistry::coding_bootstrap(false, false);
        let context = ToolExecutionContext::new(tempdir.path());
        let results = registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_patch"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("apply_patch"),
                    arguments: String::from(
                        "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}",
                    ),
                },
            }],
            &ToolApprovalConfig {
                allow_write_tools: true,
                allow_network_shell: false,
                allow_destructive_shell: false,
                denied_action: ToolDeniedAction::Refuse,
                overrides: Vec::new(),
            },
        );

        assert_eq!(results[0].output["path"], "hello.txt");
        assert_eq!(
            fs::read_to_string(path).expect("read patched file"),
            "hello probe\n"
        );
    }

    #[test]
    fn coding_bootstrap_can_search_with_ripgrep() {
        let tempdir = tempdir().expect("tempdir");
        fs::write(
            tempdir.path().join("lib.rs"),
            "fn alpha() {}\nfn beta() {}\n",
        )
        .expect("write file");
        let registry = ToolRegistry::coding_bootstrap(false, false);
        let context = ToolExecutionContext::new(tempdir.path());
        let results = registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_search"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("code_search"),
                    arguments: String::from(
                        "{\"pattern\":\"beta\",\"path\":\".\",\"max_matches\":5}",
                    ),
                },
            }],
            &ToolApprovalConfig::conservative(),
        );

        let matches = results[0].output["matches"]
            .as_array()
            .expect("matches array");
        assert!(!matches.is_empty());
        assert!(
            matches[0]["snippet"]
                .as_str()
                .unwrap_or_default()
                .contains("beta")
        );
    }

    #[test]
    fn code_search_falls_back_when_ripgrep_is_unavailable() {
        let tempdir = tempdir().expect("tempdir");
        fs::write(
            tempdir.path().join("lib.rs"),
            "fn alpha() {}\nfn beta() {}\n",
        )
        .expect("write file");
        let context = ToolExecutionContext::new(tempdir.path());

        let result =
            code_search_without_ripgrep(&context, tempdir.path(), "beta", ".", None, 5, false)
                .expect("fallback code search should succeed");

        let matches = result.output["matches"].as_array().expect("matches array");
        assert_eq!(result.output["search_backend"], "rust_fallback");
        assert!(!matches.is_empty());
        assert_eq!(matches[0]["path"], "lib.rs");
        assert!(
            matches[0]["snippet"]
                .as_str()
                .unwrap_or_default()
                .contains("beta")
        );
    }

    #[test]
    fn coding_bootstrap_runs_bounded_shell_command() {
        let tempdir = tempdir().expect("tempdir");
        let registry = ToolRegistry::coding_bootstrap(false, false);
        let context = ToolExecutionContext::new(tempdir.path());
        let results = registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_shell"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("shell"),
                    arguments: String::from("{\"command\":\"printf hello\",\"timeout_secs\":2}"),
                },
            }],
            &ToolApprovalConfig::conservative(),
        );

        assert_eq!(results[0].output["timed_out"], false);
        assert_eq!(results[0].output["stdout"], "hello");
        assert_eq!(
            results[0].tool_execution.risk_class,
            ToolRiskClass::ShellReadOnly
        );
        assert_eq!(
            results[0].tool_execution.policy_decision,
            ToolPolicyDecision::AutoAllow
        );
    }

    #[test]
    fn shell_rejects_natural_language_requests() {
        let tempdir = tempdir().expect("tempdir");
        let registry = ToolRegistry::coding_bootstrap(false, false);
        let context = ToolExecutionContext::new(tempdir.path());
        let results = registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_shell"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("shell"),
                    arguments: String::from(
                        "{\"command\":\"WHAT AI LAB TRAINED YOU\",\"timeout_secs\":2}",
                    ),
                },
            }],
            &ToolApprovalConfig::allow_all(),
        );

        assert_eq!(
            results[0].tool_execution.policy_decision,
            ToolPolicyDecision::Approved
        );
        assert_eq!(
            results[0].output["error"],
            "shell requires a literal shell command, not a natural-language request"
        );
    }

    #[test]
    fn shell_rejects_single_word_natural_language_requests() {
        let tempdir = tempdir().expect("tempdir");
        let registry = ToolRegistry::coding_bootstrap(false, false);
        let context = ToolExecutionContext::new(tempdir.path());
        let results = registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_shell"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("shell"),
                    arguments: String::from("{\"command\":\"hello\",\"timeout_secs\":2}"),
                },
            }],
            &ToolApprovalConfig::allow_all(),
        );

        assert_eq!(
            results[0].output["error"],
            "shell requires a literal shell command, not a natural-language request"
        );
    }

    #[test]
    fn read_file_model_text_is_plain_and_truncated_for_model_replay() {
        let rendered = tool_result_model_text(
            "read_file",
            &serde_json::json!({
                "path": "README.md",
                "start_line": 1,
                "end_line": 240,
                "total_lines": 240,
                "truncated": false,
                "content": "a".repeat(3_600),
            }),
        );

        assert!(rendered.contains("path: README.md"));
        assert!(rendered.contains("lines: 1-240 of 240"));
        assert!(rendered.contains("content:"));
        assert!(rendered.contains("truncated"));
        assert!(!rendered.contains("\"path\""));
    }

    #[test]
    fn stored_tool_result_model_text_converts_json_blob_into_compact_text() {
        let rendered = stored_tool_result_model_text(
            "read_file",
            r##"{"path":"README.md","start_line":1,"end_line":3,"total_lines":3,"truncated":false,"content":"# Probe\nruntime"}"##,
        );

        assert!(rendered.contains("path: README.md"));
        assert!(rendered.contains("lines: 1-3 of 3"));
        assert!(rendered.contains("# Probe"));
        assert!(!rendered.contains("\"content\""));
    }

    #[test]
    fn shell_model_text_omits_redundant_command_echo() {
        let rendered = tool_result_model_text(
            "shell",
            &serde_json::json!({
                "command": "whoami",
                "timeout_secs": 2,
                "timed_out": false,
                "exit_code": 0,
                "stdout": "christopherdavid",
                "stderr": "",
                "stdout_truncated": false,
                "stderr_truncated": false,
            }),
        );

        assert!(!rendered.contains("command: whoami"));
        assert!(rendered.contains("output:"));
        assert!(rendered.contains("christopherdavid"));
    }

    #[test]
    fn coding_bootstrap_refuses_write_tools_without_approval() {
        let tempdir = tempdir().expect("tempdir");
        let path = tempdir.path().join("hello.txt");
        fs::write(&path, "hello world\n").expect("write file");
        let registry = ToolRegistry::coding_bootstrap(false, false);
        let context = ToolExecutionContext::new(tempdir.path());
        let results = registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_patch"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("apply_patch"),
                    arguments: String::from(
                        "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}",
                    ),
                },
            }],
            &ToolApprovalConfig::conservative(),
        );

        assert_eq!(results[0].tool_execution.risk_class, ToolRiskClass::Write);
        assert_eq!(
            results[0].tool_execution.policy_decision,
            ToolPolicyDecision::Refused
        );
        assert_eq!(
            fs::read_to_string(path).expect("read file"),
            "hello world\n"
        );
    }

    #[test]
    fn workspace_sync_gate_allows_read_only_tools_before_sync_completes() {
        let tempdir = tempdir().expect("tempdir");
        fs::write(tempdir.path().join("hello.txt"), "hello world\n").expect("write file");
        let registry = ToolRegistry::coding_bootstrap(false, false);
        let context = ToolExecutionContext::new(tempdir.path())
            .with_write_gate(ToolWriteGate::blocked("main sync status is syncing"));
        let results = registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_read"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("read_file"),
                    arguments: String::from("{\"path\":\"hello.txt\"}"),
                },
            }],
            &ToolApprovalConfig::allow_all(),
        );

        assert_eq!(
            results[0].tool_execution.policy_decision,
            ToolPolicyDecision::AutoAllow
        );
        assert_eq!(results[0].output["content"], "hello world");
    }

    #[test]
    fn workspace_sync_gate_rejects_writes_even_when_write_approval_is_available() {
        let tempdir = tempdir().expect("tempdir");
        let path = tempdir.path().join("hello.txt");
        fs::write(&path, "hello world\n").expect("write file");
        let registry = ToolRegistry::coding_bootstrap(false, false);
        let context = ToolExecutionContext::new(tempdir.path())
            .with_write_gate(ToolWriteGate::blocked("main sync status is syncing"));
        let results = registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_patch"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("apply_patch"),
                    arguments: String::from(
                        "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}",
                    ),
                },
            }],
            &ToolApprovalConfig::allow_all(),
        );

        assert_eq!(results[0].tool_execution.risk_class, ToolRiskClass::Write);
        assert_eq!(
            results[0].tool_execution.policy_decision,
            ToolPolicyDecision::Refused
        );
        assert_eq!(
            results[0].tool_execution.reason.as_deref(),
            Some(
                "workspace sync gate blocked non-read-only tool execution: main sync status is syncing"
            )
        );
        assert_eq!(
            fs::read_to_string(path).expect("read file"),
            "hello world\n"
        );
    }

    #[test]
    fn coding_bootstrap_can_pause_on_destructive_shell_requests() {
        let tempdir = tempdir().expect("tempdir");
        let registry = ToolRegistry::coding_bootstrap(false, false);
        let context = ToolExecutionContext::new(tempdir.path());
        let results = registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_shell"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("shell"),
                    arguments: String::from("{\"command\":\"rm -rf build\",\"timeout_secs\":2}"),
                },
            }],
            &ToolApprovalConfig {
                allow_write_tools: false,
                allow_network_shell: false,
                allow_destructive_shell: false,
                denied_action: ToolDeniedAction::Pause,
                overrides: Vec::new(),
            },
        );

        assert_eq!(
            results[0].tool_execution.risk_class,
            ToolRiskClass::Destructive
        );
        assert_eq!(
            results[0].tool_execution.policy_decision,
            ToolPolicyDecision::Paused
        );
    }

    #[test]
    fn coding_bootstrap_can_consult_oracle_with_budget() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let body = serde_json::json!({
                "id": "oracle_tool_test",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "Inspect src/main.rs before editing."
                        },
                        "finish_reason": "stop"
                    }
                ]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let tempdir = tempdir().expect("tempdir");
        let mut oracle_profile = psionic_qwen35_2b_q8_oracle();
        oracle_profile.base_url = format!("http://{address}/v1");
        let tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Auto, false).with_oracle(
            ToolOracleConfig {
                profile: oracle_profile.clone(),
                max_calls: 1,
            },
        );
        let context = ToolExecutionContext::new(tempdir.path())
            .with_oracle(ToolOracleContext::new(oracle_profile, 1, 0));
        let results = tool_loop.registry.execute_batch(
            &context,
            &[
                ChatToolCall {
                    id: String::from("call_oracle_1"),
                    kind: String::from("function"),
                    function: ChatToolCallFunction {
                        name: String::from("consult_oracle"),
                        arguments: String::from(
                            "{\"task_kind\":\"planning\",\"question\":\"What should I inspect first?\"}",
                        ),
                    },
                },
                ChatToolCall {
                    id: String::from("call_oracle_2"),
                    kind: String::from("function"),
                    function: ChatToolCallFunction {
                        name: String::from("consult_oracle"),
                        arguments: String::from(
                            "{\"task_kind\":\"checking\",\"question\":\"What should I verify next?\"}",
                        ),
                    },
                },
            ],
            &ToolApprovalConfig::conservative(),
        );

        assert_eq!(
            results[0].output["oracle_answer"],
            "Inspect src/main.rs before editing."
        );
        assert_eq!(
            results[1].tool_execution.policy_decision,
            ToolPolicyDecision::Refused
        );
        assert!(
            results[1]
                .tool_execution
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("budget exhausted")
        );

        handle.join().expect("oracle server should exit cleanly");
    }

    #[test]
    fn coding_bootstrap_can_consult_apple_fm_oracle_with_budget() {
        let server = FakeAppleFmServer::from_json_responses(vec![serde_json::json!({
            "id": "apple_fm_oracle_tool_test",
            "model": "apple-foundation-model",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Inspect src/lib.rs before editing."
                    },
                    "finish_reason": "stop"
                }
            ]
        })]);

        let tempdir = tempdir().expect("tempdir");
        let mut oracle_profile = psionic_apple_fm_oracle();
        oracle_profile.base_url = server.base_url().to_string();
        let tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Auto, false).with_oracle(
            ToolOracleConfig {
                profile: oracle_profile.clone(),
                max_calls: 1,
            },
        );
        let context = ToolExecutionContext::new(tempdir.path())
            .with_oracle(ToolOracleContext::new(oracle_profile, 1, 0));
        let results = tool_loop.registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_oracle_1"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("consult_oracle"),
                    arguments: String::from(
                        "{\"task_kind\":\"planning\",\"question\":\"What should I inspect first?\"}",
                    ),
                },
            }],
            &ToolApprovalConfig::conservative(),
        );

        assert_eq!(
            results[0].output["oracle_answer"],
            "Inspect src/lib.rs before editing."
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(
            requests
                .iter()
                .any(|request| request.contains("POST /v1/chat/completions HTTP/1.1"))
        );
    }

    #[test]
    fn coding_bootstrap_can_analyze_repository_with_provenance() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buffer = [0_u8; 8192];
            let _ = stream.read(&mut buffer);
            let body = serde_json::json!({
                "id": "repo_analysis_tool_test",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "The repo uses a small Rust workspace. `src/main.rs` is the binary entrypoint and `README.md` describes the operator flow."
                        },
                        "finish_reason": "stop"
                    }
                ]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let tempdir = tempdir().expect("tempdir");
        fs::create_dir_all(tempdir.path().join("src")).expect("create src");
        fs::write(
            tempdir.path().join("src/main.rs"),
            "fn main() {\n    println!(\"probe\");\n}\n",
        )
        .expect("write main");
        fs::write(
            tempdir.path().join("README.md"),
            "# Probe\n\nA coding-agent runtime.\n",
        )
        .expect("write readme");

        let mut analysis_profile = psionic_qwen35_2b_q8_long_context();
        analysis_profile.base_url = format!("http://{address}/v1");
        let tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Auto, false)
            .with_long_context(ToolLongContextConfig {
                profile: analysis_profile.clone(),
                max_calls: 1,
                max_evidence_files: 6,
                max_lines_per_file: 120,
            });
        let context = ToolExecutionContext::new(tempdir.path()).with_long_context(
            ToolLongContextContext::new(analysis_profile, 1, 0, 6, 120, 280, 1, 1, 3, false, 1),
        );
        let results = tool_loop.registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_repo_analysis"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("analyze_repository"),
                    arguments: String::from(
                        "{\"task_kind\":\"repo_analysis\",\"question\":\"Summarize the repo structure.\",\"evidence_paths\":[\"src/main.rs\",\"README.md\"]}",
                    ),
                },
            }],
            &ToolApprovalConfig::conservative(),
        );

        assert_eq!(
            results[0].tool_execution.policy_decision,
            ToolPolicyDecision::AutoAllow
        );
        assert!(
            results[0].output["analysis"]
                .as_str()
                .unwrap_or_default()
                .contains("Rust workspace")
        );
        assert_eq!(results[0].output["calls_used_after"], 1);
        let evidence = results[0].output["evidence"]
            .as_array()
            .expect("evidence array");
        assert_eq!(evidence.len(), 2);
        assert_eq!(evidence[0]["path"], "src/main.rs");
        assert_eq!(evidence[1]["path"], "README.md");

        handle.join().expect("analysis server should exit cleanly");
    }

    #[test]
    fn coding_bootstrap_refuses_repo_analysis_without_enough_evidence() {
        let tempdir = tempdir().expect("tempdir");
        let tool_loop =
            ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Auto, false).with_long_context(
                ToolLongContextConfig::bounded(psionic_qwen35_2b_q8_long_context(), 1),
            );
        let context = ToolExecutionContext::new(tempdir.path()).with_long_context(
            ToolLongContextContext::new(
                psionic_qwen35_2b_q8_long_context(),
                1,
                0,
                6,
                120,
                64,
                0,
                0,
                0,
                false,
                0,
            ),
        );
        let results = tool_loop.registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_repo_analysis"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("analyze_repository"),
                    arguments: String::from(
                        "{\"task_kind\":\"repo_analysis\",\"question\":\"Summarize the repo structure.\",\"evidence_paths\":[\"README.md\"]}",
                    ),
                },
            }],
            &ToolApprovalConfig::conservative(),
        );

        assert_eq!(
            results[0].tool_execution.policy_decision,
            ToolPolicyDecision::Refused
        );
        assert!(
            results[0]
                .tool_execution
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("repository structure")
        );
    }

    #[test]
    fn probe_tool_choice_parses_named_mode() {
        let choice = ProbeToolChoice::parse("named:read_file").expect("named choice");
        let config = ToolLoopConfig::coding_bootstrap(choice.clone(), true);
        assert_eq!(config.registry.name(), "coding_bootstrap");
        assert!(matches!(choice, ProbeToolChoice::Named(name) if name == "read_file"));
    }
}
