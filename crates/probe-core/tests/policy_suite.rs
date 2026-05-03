use std::fs;

use probe_core::tools::{ToolApprovalConfig, ToolDeniedAction, ToolExecutionContext, ToolRegistry};
use probe_protocol::session::{
    ToolPermissionDecision, ToolPermissionOverride, ToolPolicyDecision, ToolRiskClass,
};
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
            overrides: Vec::new(),
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

#[test]
fn policy_suite_scoped_override_can_pause_read_only_tools() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("hello.txt");
    fs::write(&path, "hello world\n").expect("write fixture");
    let registry = ToolRegistry::coding_bootstrap(false, false);
    let context = ToolExecutionContext::new(temp.path());
    let approval = ToolApprovalConfig::allow_all().with_override(ToolPermissionOverride {
        tool_name: Some(String::from("read_file")),
        risk_class: Some(ToolRiskClass::ReadOnly),
        decision: ToolPermissionDecision::Ask,
        reason: Some(String::from("admin wants read visibility")),
    });

    let result = registry.execute_batch(
        &context,
        &[ChatToolCall {
            id: String::from("call_read"),
            kind: String::from("function"),
            function: ChatToolCallFunction {
                name: String::from("read_file"),
                arguments: String::from("{\"path\":\"hello.txt\"}"),
            },
        }],
        &approval,
    );

    assert_eq!(result[0].tool_execution.risk_class, ToolRiskClass::ReadOnly);
    assert_eq!(
        result[0].tool_execution.policy_decision,
        ToolPolicyDecision::Paused
    );
    assert_eq!(
        result[0].tool_execution.reason.as_deref(),
        Some("admin wants read visibility")
    );
}

#[test]
fn policy_suite_scoped_override_can_deny_network_shell() {
    let temp = tempdir().expect("tempdir");
    let registry = ToolRegistry::coding_bootstrap(false, false);
    let context = ToolExecutionContext::new(temp.path());
    let approval = ToolApprovalConfig::allow_all().with_override(ToolPermissionOverride {
        tool_name: Some(String::from("shell")),
        risk_class: Some(ToolRiskClass::Network),
        decision: ToolPermissionDecision::Deny,
        reason: Some(String::from("network blocked for this managed session")),
    });

    let result = registry.execute_batch(
        &context,
        &[ChatToolCall {
            id: String::from("call_network"),
            kind: String::from("function"),
            function: ChatToolCallFunction {
                name: String::from("shell"),
                arguments: String::from(
                    "{\"command\":\"git push origin main\",\"timeout_secs\":2}",
                ),
            },
        }],
        &approval,
    );

    assert_eq!(result[0].tool_execution.risk_class, ToolRiskClass::Network);
    assert_eq!(
        result[0].tool_execution.policy_decision,
        ToolPolicyDecision::Refused
    );
    assert_eq!(result[0].output["approval_required"], true);
}

#[test]
fn policy_suite_manifest_declares_schemas_policy_and_timeout() {
    let registry = ToolRegistry::coding_bootstrap(false, false);
    let approval = ToolApprovalConfig::conservative().with_override(ToolPermissionOverride {
        tool_name: Some(String::from("shell")),
        risk_class: Some(ToolRiskClass::Network),
        decision: ToolPermissionDecision::Ask,
        reason: None,
    });

    let manifest = registry.managed_tool_manifest(&approval);
    let shell = manifest
        .tools
        .iter()
        .find(|tool| tool.name == "shell")
        .expect("shell manifest entry");
    assert_eq!(shell.execution_owner, "probe_runtime");
    assert_eq!(shell.timeout_policy.default_secs, Some(5));
    assert!(
        shell
            .possible_risk_classes
            .contains(&ToolRiskClass::Network)
    );
    assert_eq!(shell.input_schema["type"], "object");
    assert_eq!(shell.result_schema["type"], "object");
    assert_eq!(manifest.policy.overrides.len(), 1);
    assert!(manifest.policy.default_decisions.iter().any(|entry| {
        entry.risk_class == ToolRiskClass::Write && entry.decision == ToolPermissionDecision::Deny
    }));
}

#[test]
fn policy_suite_tool_summaries_redact_secret_values() {
    let input = probe_core::tools::tool_input_summary(
        "shell",
        &serde_json::json!({
            "command": "OPENAI_API_KEY=sk-secret curl https://example.com",
            "timeout_secs": 2
        }),
    );
    assert_eq!(input["command"], "[redacted] curl https://example.com");

    let output = probe_core::tools::tool_output_summary(
        "shell",
        &serde_json::json!({
            "command": "OPENAI_API_KEY=sk-secret curl https://example.com",
            "timed_out": false,
            "exit_code": 0,
            "stdout": "token=super-secret ok",
            "stderr": "",
            "stdout_truncated": false,
            "stderr_truncated": false
        }),
    );
    assert_eq!(output["stdout_preview"], "[redacted] ok");
}

#[test]
fn policy_suite_shell_timeout_is_recorded() {
    let temp = tempdir().expect("tempdir");
    let registry = ToolRegistry::coding_bootstrap(false, false);
    let context = ToolExecutionContext::new(temp.path());

    let result = registry.execute_batch(
        &context,
        &[ChatToolCall {
            id: String::from("call_timeout"),
            kind: String::from("function"),
            function: ChatToolCallFunction {
                name: String::from("shell"),
                arguments: String::from("{\"command\":\"sleep 2\",\"timeout_secs\":1}"),
            },
        }],
        &ToolApprovalConfig::allow_all(),
    );

    assert_eq!(
        result[0].tool_execution.policy_decision,
        ToolPolicyDecision::Approved
    );
    assert_eq!(result[0].tool_execution.timed_out, Some(true));
    assert_eq!(result[0].output["timed_out"], true);
}
