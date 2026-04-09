use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use probe_core::tools::{
    ToolApprovalConfig, ToolExecutionContext, ToolRegistry, stored_tool_result_model_text,
    tool_result_model_text,
};
use probe_protocol::session::{
    SessionMcpConnectionStatus, SessionMcpServer, SessionMcpServerSource,
    SessionMcpServerTransport, SessionMcpState, SessionMcpTool,
};
use probe_provider_openai::{ChatToolCall, ChatToolCallFunction};
use tempfile::tempdir;

#[cfg(unix)]
fn write_fake_mcp_call_server(path: &std::path::Path) {
    let script = r#"#!/usr/bin/env python3
import json
import sys

def read_message():
    content_length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        if line.lower().startswith(b"content-length:"):
            content_length = int(line.split(b":", 1)[1].strip())
    if content_length is None:
        return None
    body = sys.stdin.buffer.read(content_length)
    return json.loads(body.decode("utf-8"))

def send_message(message):
    body = json.dumps(message).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method")
    if method == "initialize":
        send_message({
            "jsonrpc": "2.0",
            "id": message.get("id"),
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake-mcp", "version": "1.0.0"}
            }
        })
    elif method == "tools/call":
        params = message.get("params", {})
        send_message({
            "jsonrpc": "2.0",
            "id": message.get("id"),
            "result": {
                "content": [
                    {
                        "type": "text",
                        "text": f"read {params.get('arguments', {}).get('path', '')}"
                    }
                ]
            }
        })
"#;
    fs::write(path, script).expect("write fake mcp script");
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("set execute permissions");
}

#[test]
fn tool_suite_reads_lists_and_searches_relative_to_the_workspace() {
    let temp = tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src/bin")).expect("create src tree");
    fs::write(
        temp.path().join("src/lib.rs"),
        "pub fn alpha() {}\npub fn beta_function() {}\n",
    )
    .expect("write lib.rs");
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write main.rs");
    let registry = ToolRegistry::coding_bootstrap(false, false);
    let context = ToolExecutionContext::new(temp.path());

    let results = registry.execute_batch(
        &context,
        &[
            ChatToolCall {
                id: String::from("call_list"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("list_files"),
                    arguments: String::from(
                        "{\"path\":\"src\",\"max_depth\":2,\"max_entries\":10}",
                    ),
                },
            },
            ChatToolCall {
                id: String::from("call_read"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("read_file"),
                    arguments: String::from(
                        "{\"path\":\"src/lib.rs\",\"start_line\":2,\"max_lines\":1}",
                    ),
                },
            },
            ChatToolCall {
                id: String::from("call_search"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("code_search"),
                    arguments: String::from(
                        "{\"pattern\":\"beta_function\",\"path\":\"src\",\"max_matches\":5}",
                    ),
                },
            },
        ],
        &ToolApprovalConfig::conservative(),
    );

    let entries = results[0].output["entries"]
        .as_array()
        .expect("list_files should return entries");
    assert!(entries.iter().any(|entry| entry["path"] == "bin"));
    assert!(entries.iter().any(|entry| entry["path"] == "lib.rs"));
    assert_eq!(results[1].output["content"], "pub fn beta_function() {}");
    assert_eq!(results[2].output["matches"][0]["path"], "src/lib.rs");
}

#[test]
fn tool_suite_treats_blank_navigation_paths_as_workspace_root() {
    let temp = tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("create src tree");
    fs::write(
        temp.path().join("src/lib.rs"),
        "pub fn beta_function() {}\n",
    )
    .expect("write lib.rs");
    let registry = ToolRegistry::coding_bootstrap(false, false);
    let context = ToolExecutionContext::new(temp.path());

    let results = registry.execute_batch(
        &context,
        &[
            ChatToolCall {
                id: String::from("call_list_root"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("list_files"),
                    arguments: String::from("{\"path\":\"\",\"max_depth\":2,\"max_entries\":10}"),
                },
            },
            ChatToolCall {
                id: String::from("call_search_root"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("code_search"),
                    arguments: String::from(
                        "{\"pattern\":\"beta_function\",\"path\":\"\",\"max_matches\":5}",
                    ),
                },
            },
        ],
        &ToolApprovalConfig::conservative(),
    );

    let entries = results[0].output["entries"]
        .as_array()
        .expect("list_files should return root entries");
    assert!(entries.iter().any(|entry| entry["path"] == "src"));
    assert_eq!(results[1].output["matches"][0]["path"], "src/lib.rs");
}

#[test]
fn tool_suite_renders_compact_model_text_for_read_and_shell_results() {
    let read_rendered = tool_result_model_text(
        "read_file",
        &serde_json::json!({
            "path": "README.md",
            "start_line": 1,
            "end_line": 3,
            "total_lines": 3,
            "truncated": false,
            "content": "# Probe\nruntime\nnotes"
        }),
    );
    let stored_rendered = stored_tool_result_model_text(
        "shell",
        r#"{"command":"pwd","stdout":"/tmp/probe","stderr":"","timed_out":false}"#,
    );

    assert!(read_rendered.contains("path: README.md"));
    assert!(read_rendered.contains("# Probe"));
    assert!(!read_rendered.contains("\"content\""));
    assert!(stored_rendered.contains("/tmp/probe"));
    assert!(!stored_rendered.contains("\"stdout\""));
}

#[cfg(unix)]
#[test]
fn tool_suite_executes_connected_stdio_mcp_tools_through_the_registry() {
    let temp = tempdir().expect("tempdir");
    let script_path = temp.path().join("fake_mcp.py");
    write_fake_mcp_call_server(&script_path);

    let registry = ToolRegistry::coding_bootstrap(false, false).with_session_mcp_tools(Some(
        &SessionMcpState {
            load_error: None,
            servers: vec![SessionMcpServer {
                id: String::from("filesystem"),
                name: String::from("Filesystem"),
                enabled: true,
                source: SessionMcpServerSource::ManualLaunch,
                transport: Some(SessionMcpServerTransport::Stdio),
                target: Some(format!("python3 {}", script_path.display())),
                provider_setup_command: None,
                provider_hint: None,
                client_hint: None,
                connection_status: Some(SessionMcpConnectionStatus::Connected),
                connection_note: Some(String::from("Attached at session start.")),
                discovered_tools: vec![SessionMcpTool {
                    name: String::from("filesystem/read"),
                    description: Some(String::from("Read a file from disk.")),
                    input_schema: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" }
                        },
                        "required": ["path"]
                    })),
                }],
            }],
        },
    ));
    let context = ToolExecutionContext::new(temp.path());
    let tool_name = String::from("mcp__filesystem__filesystem_read");

    let results = registry.execute_batch(
        &context,
        &[ChatToolCall {
            id: String::from("call_mcp_read"),
            kind: String::from("function"),
            function: ChatToolCallFunction {
                name: tool_name.clone(),
                arguments: String::from("{\"path\":\"README.md\"}"),
            },
        }],
        &ToolApprovalConfig {
            allow_write_tools: false,
            allow_network_shell: true,
            allow_destructive_shell: false,
            denied_action: probe_core::tools::ToolDeniedAction::Refuse,
        },
    );

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, tool_name);
    assert_eq!(results[0].output["server_name"], "Filesystem");
    assert_eq!(results[0].output["tool"], "filesystem/read");
    assert_eq!(
        results[0].output["result"]["content"][0]["text"],
        "read README.md"
    );
    assert_eq!(
        results[0].tool_execution.policy_decision,
        probe_protocol::session::ToolPolicyDecision::Approved
    );
}
