//! Tool catalog and execution. Real tools land with Phase 4 (#210); until
//! then probe advertises no tools and any invocation is an honest typed
//! error, never a silent no-op.

use probe_core::contract::message::ToolResultValue;
use probe_core::contract::request::ToolDefinition;
use probe_core::permission::ToolKind;
use probe_core::redact::SecretSet;

pub fn tool_definitions() -> Vec<ToolDefinition> {
    Vec::new()
}

pub fn tool_kinds() -> std::collections::BTreeMap<String, ToolKind> {
    std::collections::BTreeMap::new()
}

pub fn execute(
    name: &str,
    _input: &serde_json::Value,
    _workspace: &std::path::Path,
    _secrets: &SecretSet,
) -> ToolResultValue {
    ToolResultValue::error(format!("tool not available: {name} (tools land with #210)"))
}
