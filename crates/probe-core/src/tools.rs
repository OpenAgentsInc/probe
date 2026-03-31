use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use probe_protocol::session::{
    ToolApprovalState, ToolExecutionRecord, ToolPolicyDecision, ToolRiskClass,
};
use probe_provider_openai::{
    ChatNamedToolChoice, ChatNamedToolChoiceFunction, ChatToolCall, ChatToolChoice,
    ChatToolDefinition, ChatToolDefinitionEnvelope,
};
use wait_timeout::ChildExt;

const READ_FILE_DEFAULT_MAX_LINES: u64 = 200;
const LIST_FILES_DEFAULT_MAX_DEPTH: u64 = 4;
const LIST_FILES_DEFAULT_MAX_ENTRIES: usize = 200;
const CODE_SEARCH_DEFAULT_MAX_MATCHES: usize = 50;
const SHELL_DEFAULT_TIMEOUT_SECS: u64 = 5;
const SHELL_MAX_OUTPUT_CHARS: usize = 4_000;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolExecutionContext {
    cwd: PathBuf,
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
        Self { cwd: cwd.into() }
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
}

impl ToolApprovalConfig {
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            allow_write_tools: false,
            allow_network_shell: false,
            allow_destructive_shell: false,
            denied_action: ToolDeniedAction::Refuse,
        }
    }

    #[must_use]
    pub fn allow_all() -> Self {
        Self {
            allow_write_tools: true,
            allow_network_shell: true,
            allow_destructive_shell: true,
            denied_action: ToolDeniedAction::Refuse,
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
    pub fn weather_demo(tool_choice: ProbeToolChoice, parallel_tool_calls: bool) -> Self {
        Self {
            registry: ToolRegistry::weather_demo(),
            tool_choice,
            parallel_tool_calls,
            max_model_round_trips: 4,
            approval: ToolApprovalConfig::allow_all(),
        }
    }

    #[must_use]
    pub fn coding_bootstrap(tool_choice: ProbeToolChoice, parallel_tool_calls: bool) -> Self {
        Self {
            registry: ToolRegistry::coding_bootstrap(),
            tool_choice,
            parallel_tool_calls,
            max_model_round_trips: 8,
            approval: ToolApprovalConfig::conservative(),
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
    pub fn weather_demo() -> Self {
        let parameters = serde_json::json!({
            "type": "object",
            "properties": {
                "city": {
                    "type": "string",
                    "description": "The city to look up"
                }
            },
            "required": ["city"],
            "additionalProperties": false
        });

        Self::new("weather").register(
            String::from("lookup_weather"),
            Some(String::from(
                "Look up the retained demo weather for a city.",
            )),
            Some(parameters),
            RegisteredToolRisk::Fixed(ToolRiskClass::ReadOnly),
            lookup_weather,
        )
    }

    #[must_use]
    pub fn coding_bootstrap() -> Self {
        Self::new("coding_bootstrap")
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
                    "Run a bounded shell command inside the session cwd and capture stdout, stderr, exit code, and timeout state.",
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
            )
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
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn execute_batch(
        &self,
        context: &ToolExecutionContext,
        tool_calls: &[ChatToolCall],
        approval: &ToolApprovalConfig,
    ) -> Vec<ExecutedToolCall> {
        tool_calls
            .iter()
            .map(|tool_call| {
                let parsed_arguments = serde_json::from_str::<serde_json::Value>(
                    tool_call.function.arguments.as_str(),
                )
                .unwrap_or_else(|error| {
                    serde_json::json!({
                        "error": format!("invalid tool arguments json: {error}")
                    })
                });

                if let Some(tool) = self.tools.get(tool_call.function.name.as_str()) {
                    let risk_class = match tool.risk {
                        RegisteredToolRisk::Fixed(risk) => risk,
                        RegisteredToolRisk::Shell => classify_shell_command(&parsed_arguments),
                    };
                    let (policy_decision, approval_state, reason) = evaluate_tool_policy(
                        tool_call.function.name.as_str(),
                        risk_class,
                        approval,
                    );
                    let invocation = if matches!(
                        policy_decision,
                        ToolPolicyDecision::AutoAllow | ToolPolicyDecision::Approved
                    ) {
                        (tool.handler)(context, &parsed_arguments).unwrap_or_else(|error| {
                            ToolInvocationOutcome::new(
                                serde_json::json!({ "error": error.to_string() }),
                            )
                        })
                    } else {
                        denied_tool_invocation(
                            context,
                            tool_call.function.name.as_str(),
                            &parsed_arguments,
                            risk_class,
                            &reason,
                        )
                    };

                    ExecutedToolCall {
                        call_id: tool_call.id.clone(),
                        name: tool_call.function.name.clone(),
                        arguments: parsed_arguments,
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
                } else {
                    ExecutedToolCall {
                        call_id: tool_call.id.clone(),
                        name: tool_call.function.name.clone(),
                        arguments: parsed_arguments,
                        output: serde_json::json!({
                            "error": format!("undeclared tool `{}`", tool_call.function.name),
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
                                "tool `{}` is not registered in this tool loop",
                                tool_call.function.name
                            )),
                        },
                    }
                }
            })
            .collect()
    }
}

fn lookup_weather(
    _context: &ToolExecutionContext,
    arguments: &serde_json::Value,
) -> Result<ToolInvocationOutcome, ToolInvocationError> {
    let city = expect_string(arguments, "city", "lookup_weather")?;
    let payload = match city {
        "Paris" => serde_json::json!({
            "city": "Paris",
            "conditions": "sunny",
            "temperature_c": 18
        }),
        "Tokyo" => serde_json::json!({
            "city": "Tokyo",
            "conditions": "rainy",
            "temperature_c": 12
        }),
        other => serde_json::json!({
            "error": format!("unsupported city: {other}")
        }),
    };
    Ok(ToolInvocationOutcome::new(payload))
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

    let output = command.output().map_err(|error| {
        ToolInvocationError::ExecutionFailed(format!("failed to run ripgrep: {error}"))
    })?;
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

fn run_shell(
    context: &ToolExecutionContext,
    arguments: &serde_json::Value,
) -> Result<ToolInvocationOutcome, ToolInvocationError> {
    let command_text = expect_string(arguments, "command", "shell")?;
    let timeout_secs = expect_u64(arguments, "timeout_secs").unwrap_or(SHELL_DEFAULT_TIMEOUT_SECS);
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
            "git apply ",
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

fn denied_by_policy(
    approval: &ToolApprovalConfig,
    tool_name: &str,
    class_name: &str,
) -> (ToolPolicyDecision, ToolApprovalState, Option<String>) {
    let reason = Some(format!(
        "tool `{tool_name}` requires {class_name} approval under the active local policy"
    ));
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

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
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
    matches!(
        parts.next().unwrap_or_default(),
        "status" | "diff" | "show" | "log" | "branch" | "rev-parse" | "grep"
    )
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

#[cfg(test)]
mod tests {
    use std::fs;

    use probe_protocol::session::{ToolPolicyDecision, ToolRiskClass};
    use probe_provider_openai::{ChatToolCall, ChatToolCallFunction};
    use tempfile::tempdir;

    use super::{
        ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction, ToolExecutionContext,
        ToolLoopConfig, ToolRegistry,
    };

    #[test]
    fn weather_demo_registry_declares_lookup_weather() {
        let registry = ToolRegistry::weather_demo();
        let tools = registry.declared_tools();
        assert_eq!(registry.name(), "weather");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].function.name, "lookup_weather");
    }

    #[test]
    fn weather_demo_executes_lookup_weather() {
        let registry = ToolRegistry::weather_demo();
        let context = ToolExecutionContext::new(".");
        let results = registry.execute_batch(
            &context,
            &[ChatToolCall {
                id: String::from("call_1"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("lookup_weather"),
                    arguments: String::from("{\"city\":\"Paris\"}"),
                },
            }],
            &ToolApprovalConfig::allow_all(),
        );

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "lookup_weather");
        assert_eq!(results[0].output["conditions"], "sunny");
    }

    #[test]
    fn coding_bootstrap_registry_declares_all_tools() {
        let registry = ToolRegistry::coding_bootstrap();
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
    fn coding_bootstrap_reads_files_relative_to_context() {
        let tempdir = tempdir().expect("tempdir");
        fs::write(
            tempdir.path().join("notes.txt"),
            "one\ntwo\nthree\nfour\nfive\n",
        )
        .expect("write notes");
        let registry = ToolRegistry::coding_bootstrap();
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
        let registry = ToolRegistry::coding_bootstrap();
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
        let registry = ToolRegistry::coding_bootstrap();
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
        let registry = ToolRegistry::coding_bootstrap();
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
    fn coding_bootstrap_runs_bounded_shell_command() {
        let tempdir = tempdir().expect("tempdir");
        let registry = ToolRegistry::coding_bootstrap();
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
    fn coding_bootstrap_refuses_write_tools_without_approval() {
        let tempdir = tempdir().expect("tempdir");
        let path = tempdir.path().join("hello.txt");
        fs::write(&path, "hello world\n").expect("write file");
        let registry = ToolRegistry::coding_bootstrap();
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
    fn coding_bootstrap_can_pause_on_destructive_shell_requests() {
        let tempdir = tempdir().expect("tempdir");
        let registry = ToolRegistry::coding_bootstrap();
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
    fn probe_tool_choice_parses_named_mode() {
        let choice = ProbeToolChoice::parse("named:lookup_weather").expect("named choice");
        let config = ToolLoopConfig::weather_demo(choice.clone(), true);
        assert_eq!(config.registry.name(), "weather");
        assert!(matches!(choice, ProbeToolChoice::Named(name) if name == "lookup_weather"));
    }
}
