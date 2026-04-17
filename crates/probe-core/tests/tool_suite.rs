use std::fs;

use probe_core::tools::{
    ToolApprovalConfig, ToolExecutionContext, ToolRegistry, stored_tool_result_model_text,
    tool_result_model_text,
};
use probe_provider_openai::{ChatToolCall, ChatToolCallFunction};
use tempfile::tempdir;

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
fn tool_suite_treats_empty_directory_paths_as_workspace_root() {
    let temp = tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("create src tree");
    fs::write(
        temp.path().join("src/lib.rs"),
        "pub fn issue_4368_marker() {}\n",
    )
    .expect("write lib.rs");
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
                    arguments: String::from("{\"path\":\"\",\"max_depth\":2,\"max_entries\":10}"),
                },
            },
            ChatToolCall {
                id: String::from("call_search"),
                kind: String::from("function"),
                function: ChatToolCallFunction {
                    name: String::from("code_search"),
                    arguments: String::from(
                        "{\"pattern\":\"issue_4368_marker\",\"path\":\"\",\"max_matches\":5}",
                    ),
                },
            },
        ],
        &ToolApprovalConfig::conservative(),
    );

    let entries = results[0].output["entries"]
        .as_array()
        .expect("list_files should return entries");
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
