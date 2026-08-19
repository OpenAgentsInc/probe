//! Real tools, for the first time in Probe's history (the archived tree's
//! `shell` was a noop mock). Requirements input: the kept OpenCode parity
//! docs. Discipline: every mutation runs on the probe-core pure edit policy
//! with the stale-content guard surfaced as a typed error; execution is
//! confined to the session workspace; outputs are bounded and scrubbed; and
//! per spec Lesson 3, none of this merged without fixture tests.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use probe_core::contract::message::ToolResultValue;
use probe_core::contract::request::ToolDefinition;
use probe_core::editing;
use probe_core::permission::ToolKind;
use probe_core::redact::SecretSet;

const MAX_OUTPUT_BYTES: usize = 48 * 1024;
const MAX_GREP_LINES: usize = 200;
const MAX_LIST_ENTRIES: usize = 500;
const DEFAULT_SHELL_TIMEOUT_MS: u64 = 60_000;
const MAX_SHELL_TIMEOUT_MS: u64 = 240_000;
const SKIPPED_DIRS: [&str; 4] = [".git", "node_modules", "target", "dist"];

fn schema(json: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    json.as_object().cloned().unwrap_or_default()
}

pub fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "shell".into(),
            description: "Run a shell command in the session workspace. Output is captured and bounded; long or interactive commands will be cut off.".into(),
            input_schema: schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command to run." },
                    "timeoutMs": { "type": "number", "description": "Optional timeout in milliseconds (max 240000)." }
                },
                "required": ["command"]
            })),
            output_schema: None,
        },
        ToolDefinition {
            name: "read_file".into(),
            description: "Read a text file in the workspace. Optionally pass offset/limit line bounds.".into(),
            input_schema: schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "offset": { "type": "number", "description": "1-based first line to read." },
                    "limit": { "type": "number", "description": "Maximum number of lines." }
                },
                "required": ["path"]
            })),
            output_schema: None,
        },
        ToolDefinition {
            name: "list_files".into(),
            description: "List a workspace directory (non-recursive). Directories end with '/'.".into(),
            input_schema: schema(serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "Directory, default workspace root." } }
            })),
            output_schema: None,
        },
        ToolDefinition {
            name: "grep_files".into(),
            description: "Search workspace files for a substring, recursively. Reports path:line: text.".into(),
            input_schema: schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "path": { "type": "string", "description": "Subdirectory to search, default workspace root." }
                },
                "required": ["pattern"]
            })),
            output_schema: None,
        },
        ToolDefinition {
            name: "write_file".into(),
            description: "Create or replace a file. Preserves an existing UTF-8 BOM.".into(),
            input_schema: schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            })),
            output_schema: None,
        },
        ToolDefinition {
            name: "edit_file".into(),
            description: "Replace oldString with newString in a file. oldString must match exactly (including whitespace); with multiple matches, pass more context or set replaceAll.".into(),
            input_schema: schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "oldString": { "type": "string" },
                    "newString": { "type": "string" },
                    "replaceAll": { "type": "boolean" }
                },
                "required": ["path", "oldString", "newString"]
            })),
            output_schema: None,
        },
    ]
}

/// ACP kinds per tool — the vocabulary the controller's tier policy
/// evaluates. shell is `execute` and always escalates; edits are
/// agent-side policy disclosed via tool_call frames (spec Addendum A2).
pub fn tool_kinds() -> std::collections::BTreeMap<String, ToolKind> {
    [
        ("shell".to_string(), ToolKind::Execute),
        ("read_file".to_string(), ToolKind::Read),
        ("list_files".to_string(), ToolKind::Read),
        ("grep_files".to_string(), ToolKind::Search),
        ("write_file".to_string(), ToolKind::Edit),
        ("edit_file".to_string(), ToolKind::Edit),
    ]
    .into_iter()
    .collect()
}

/// Resolve a workspace-relative path. Absolute paths, parent traversal, and
/// NUL are refused outright; mutations additionally refuse `.git`.
fn resolve_path(root: &Path, raw: &str, mutation: bool) -> Result<PathBuf, String> {
    if raw.contains('\0') {
        return Err("path contains NUL".into());
    }
    let candidate = Path::new(raw);
    if candidate.is_absolute() {
        return Err(format!("path is outside the workspace: {raw}"));
    }
    for component in candidate.components() {
        match component {
            Component::ParentDir => return Err(format!("path escapes the workspace: {raw}")),
            Component::Normal(part) if mutation && part == ".git" => {
                return Err("refusing to modify .git".into())
            }
            _ => {}
        }
    }
    Ok(root.join(candidate))
}

fn bounded(text: String) -> (String, bool) {
    if text.len() <= MAX_OUTPUT_BYTES {
        return (text, false);
    }
    let mut cut = MAX_OUTPUT_BYTES;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (text[..cut].to_string(), true)
}

pub fn execute(
    name: &str,
    input: &serde_json::Value,
    workspace: &Path,
    secrets: &SecretSet,
) -> ToolResultValue {
    let result = match name {
        "shell" => run_shell(input, workspace),
        "read_file" => run_read(input, workspace),
        "list_files" => run_list(input, workspace),
        "grep_files" => run_grep(input, workspace),
        "write_file" => run_write(input, workspace),
        "edit_file" => run_edit(input, workspace),
        other => Err(format!("tool not available: {other}")),
    };
    // Tool output flows to the client AND into the provider continuation;
    // scrub on the way out so neither wall sees a registered secret.
    match result {
        Ok(text) => ToolResultValue::text(secrets.scrub(&text)),
        Err(message) => ToolResultValue::error(secrets.scrub(&message)),
    }
}

fn required_str<'a>(input: &'a serde_json::Value, field: &str) -> Result<&'a str, String> {
    input[field].as_str().filter(|value| !value.is_empty()).ok_or_else(|| format!("{field} is required"))
}

fn run_shell(input: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let command = required_str(input, "command")?;
    let timeout_ms = input["timeoutMs"]
        .as_u64()
        .unwrap_or(DEFAULT_SHELL_TIMEOUT_MS)
        .min(MAX_SHELL_TIMEOUT_MS);
    let mut child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(workspace)
        // Delegation-scoped credentials never reach shell children.
        .env_remove("PROBE_INFERENCE_GRANT")
        .env_remove("GOOGLE_GENERATIVE_AI_API_KEY")
        .env_remove("GEMINI_API_KEY")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start shell: {error}"))?;

    let mut stdout_pipe = child.stdout.take().unwrap();
    let mut stderr_pipe = child.stderr.take().unwrap();
    let stdout_thread = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buffer);
        buffer
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buffer);
        buffer
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(error) => return Err(format!("wait failed: {error}")),
        }
    };
    let stdout = String::from_utf8_lossy(&stdout_thread.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_thread.join().unwrap_or_default()).into_owned();
    let mut combined = stdout;
    if !stderr.is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str("[stderr]\n");
        combined.push_str(&stderr);
    }
    let (mut output, truncated) = bounded(combined);
    if truncated {
        output.push_str("\n[output truncated]");
    }
    match status {
        None => Err(format!("command timed out after {timeout_ms} ms\n{output}")),
        Some(status) if status.success() => Ok(output),
        Some(status) => Err(format!("command exited with {status}\n{output}")),
    }
}

fn run_read(input: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let path = resolve_path(workspace, required_str(input, "path")?, false)?;
    let bytes = std::fs::read(&path).map_err(|error| format!("read failed: {error}"))?;
    let decoded = String::from_utf8_lossy(&bytes);
    let (_, text) = editing::split_bom(&decoded);
    let text = text.to_string();
    let offset = input["offset"].as_u64().map(|value| value.saturating_sub(1) as usize).unwrap_or(0);
    let limit = input["limit"].as_u64().map(|value| value as usize);
    let selected: String = match limit {
        Some(limit) => text.lines().skip(offset).take(limit).collect::<Vec<_>>().join("\n"),
        None if offset > 0 => text.lines().skip(offset).collect::<Vec<_>>().join("\n"),
        None => text,
    };
    let (mut output, truncated) = bounded(selected);
    if truncated {
        output.push_str("\n[output truncated — pass offset/limit to read more]");
    }
    Ok(output)
}

fn run_list(input: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let relative = input["path"].as_str().unwrap_or("");
    let path = if relative.is_empty() {
        workspace.to_path_buf()
    } else {
        resolve_path(workspace, relative, false)?
    };
    let mut entries: Vec<String> = std::fs::read_dir(&path)
        .map_err(|error| format!("list failed: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    entries.sort();
    let total = entries.len();
    entries.truncate(MAX_LIST_ENTRIES);
    let mut output = entries.join("\n");
    if total > MAX_LIST_ENTRIES {
        output.push_str(&format!("\n[{} more entries not listed]", total - MAX_LIST_ENTRIES));
    }
    Ok(output)
}

fn run_grep(input: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let pattern = required_str(input, "pattern")?;
    let relative = input["path"].as_str().unwrap_or("");
    let root = if relative.is_empty() {
        workspace.to_path_buf()
    } else {
        resolve_path(workspace, relative, false)?
    };
    let mut matches: Vec<String> = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        if matches.len() >= MAX_GREP_LINES {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut children: Vec<_> = entries.filter_map(Result::ok).collect();
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
            if matches.len() >= MAX_GREP_LINES {
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                if !SKIPPED_DIRS.contains(&name.as_str()) {
                    stack.push(path);
                }
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else { continue };
            for (line_number, line) in content.lines().enumerate() {
                if line.contains(pattern) {
                    let display = path.strip_prefix(workspace).unwrap_or(&path).display();
                    matches.push(format!("{display}:{}: {}", line_number + 1, line.trim_end()));
                    if matches.len() >= MAX_GREP_LINES {
                        break;
                    }
                }
            }
        }
    }
    if matches.is_empty() {
        return Ok(format!("no matches for {pattern:?}"));
    }
    let capped = matches.len() >= MAX_GREP_LINES;
    let mut output = matches.join("\n");
    if capped {
        output.push_str("\n[match limit reached; narrow the pattern or path]");
    }
    let (mut output, truncated) = bounded(output);
    if truncated {
        output.push_str("\n[output truncated]");
    }
    Ok(output)
}

fn run_write(input: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let raw_path = required_str(input, "path")?;
    let content = input["content"].as_str().ok_or("content is required")?;
    let path = resolve_path(workspace, raw_path, true)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("mkdir failed: {error}"))?;
    }
    // Preserve an existing file's BOM, exactly as the archived tool did.
    let existing_bom = std::fs::read(&path).map(|bytes| editing::has_utf8_bom(&bytes)).unwrap_or(false);
    let final_content = editing::join_bom(content, existing_bom || content.starts_with('\u{feff}'));
    std::fs::write(&path, final_content.as_bytes()).map_err(|error| format!("write failed: {error}"))?;
    Ok(format!("wrote {raw_path} ({} bytes)", final_content.len()))
}

fn run_edit(input: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let raw_path = required_str(input, "path")?;
    let old_string = input["oldString"].as_str().ok_or("oldString is required")?;
    let new_string = input["newString"].as_str().ok_or("newString is required")?;
    let replace_all = input["replaceAll"].as_bool().unwrap_or(false);
    let path = resolve_path(workspace, raw_path, true)?;

    let original_bytes = std::fs::read(&path).map_err(|error| format!("read failed: {error}"))?;
    let original_text = String::from_utf8_lossy(&original_bytes).into_owned();
    let plan = editing::plan_exact_edit(&original_text, old_string, new_string, replace_all)
        .map_err(|error| error.message())?;

    // The stale guard the archived tool swallowed: re-read and verify before
    // writing; a concurrent change is a typed failure the model sees.
    let current = std::fs::read(&path).map_err(|error| format!("re-read failed: {error}"))?;
    editing::verify_unchanged(&original_bytes, &current)
        .map_err(|_| "file changed on disk during the edit; re-read it and retry".to_string())?;
    std::fs::write(&path, plan.output.as_bytes()).map_err(|error| format!("write failed: {error}"))?;
    Ok(format!(
        "edited {raw_path} ({} replacement{})",
        plan.replacements,
        if plan.replacements == 1 { "" } else { "s" }
    ))
}
