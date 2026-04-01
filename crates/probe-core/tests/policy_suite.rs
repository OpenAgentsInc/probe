use std::fs;

use probe_core::tools::{ToolApprovalConfig, ToolDeniedAction, ToolExecutionContext, ToolRegistry};
use probe_protocol::session::{ToolPolicyDecision, ToolRiskClass};
use probe_provider_openai::{ChatToolCall, ChatToolCallFunction};
use tempfile::tempdir;

#[test]
fn policy_suite_refuses_write_tools_without_explicit_approval() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("hello.txt");
    fs::write(&path, "hello world\n").expect("write fixture");
    let registry = ToolRegistry::coding_bootstrap(false, false);
    let context = ToolExecutionContext::new(temp.path());

    let result = registry.execute_batch(
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

    assert_eq!(result[0].tool_execution.risk_class, ToolRiskClass::Write);
    assert_eq!(
        result[0].tool_execution.policy_decision,
        ToolPolicyDecision::Refused
    );
    assert_eq!(
        fs::read_to_string(path).expect("read file"),
        "hello world\n"
    );
}

#[test]
fn policy_suite_pauses_destructive_shell_when_operator_requests_pauses() {
    let temp = tempdir().expect("tempdir");
    let registry = ToolRegistry::coding_bootstrap(false, false);
    let context = ToolExecutionContext::new(temp.path());

    let result = registry.execute_batch(
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
        result[0].tool_execution.risk_class,
        ToolRiskClass::Destructive
    );
    assert_eq!(
        result[0].tool_execution.policy_decision,
        ToolPolicyDecision::Paused
    );
}

#[test]
fn policy_suite_auto_allows_read_only_shell() {
    let temp = tempdir().expect("tempdir");
    let registry = ToolRegistry::coding_bootstrap(false, false);
    let context = ToolExecutionContext::new(temp.path());

    let result = registry.execute_batch(
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

    assert_eq!(
        result[0].tool_execution.risk_class,
        ToolRiskClass::ShellReadOnly
    );
    assert_eq!(
        result[0].tool_execution.policy_decision,
        ToolPolicyDecision::AutoAllow
    );
    assert_eq!(result[0].output["stdout"], "hello");
}
