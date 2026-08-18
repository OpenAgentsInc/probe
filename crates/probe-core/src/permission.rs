//! Permission as data. The core never prompts anyone: a tool invocation that
//! requires consent yields a typed `PermissionRequest` and the turn suspends
//! until a `PermissionDecision` arrives. The kind vocabulary is ACP's, which
//! is also what the sarah-computer-controller's tier policy evaluates; the
//! policy triple is salvaged from the archived Blueprint tool-menu.

use serde::{Deserialize, Serialize};

/// ACP tool-call kinds. `read`/`search`/`fetch`/`think` are the read-shaped
/// set the controller's curated tier grants outright; `execute` is evaluated
/// against the machine's command allowlist and must carry the real command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    Other,
}

impl ToolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolKind::Read => "read",
            ToolKind::Edit => "edit",
            ToolKind::Delete => "delete",
            ToolKind::Move => "move",
            ToolKind::Search => "search",
            ToolKind::Execute => "execute",
            ToolKind::Think => "think",
            ToolKind::Fetch => "fetch",
            ToolKind::Other => "other",
        }
    }
}

/// The policy triple, salvaged from the archived tool-menu vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPolicy {
    Allow,
    ApprovalRequired,
    Deny,
}

/// Effect classification, salvaged from the archived action-submission
/// boundary: the distinction that matters is local-and-reversible versus
/// external-and-irreversible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    Read,
    ReversibleWrite,
    ExternalEffect,
}

/// Per-kind policy table. The default is the shape the controller addendum
/// records: read-shaped kinds and in-workspace edits are agent-side allows
/// (edits are disclosed via tool_call frames, matching the pinned claude
/// adapter); execute, fetch, and everything irreversible escalates.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyTable {
    pub read: PermissionPolicy,
    pub edit: PermissionPolicy,
    pub delete: PermissionPolicy,
    pub r#move: PermissionPolicy,
    pub search: PermissionPolicy,
    pub execute: PermissionPolicy,
    pub think: PermissionPolicy,
    pub fetch: PermissionPolicy,
    pub other: PermissionPolicy,
}

impl Default for PolicyTable {
    fn default() -> Self {
        PolicyTable {
            read: PermissionPolicy::Allow,
            edit: PermissionPolicy::Allow,
            delete: PermissionPolicy::ApprovalRequired,
            r#move: PermissionPolicy::ApprovalRequired,
            search: PermissionPolicy::Allow,
            execute: PermissionPolicy::ApprovalRequired,
            think: PermissionPolicy::Allow,
            fetch: PermissionPolicy::ApprovalRequired,
            other: PermissionPolicy::ApprovalRequired,
        }
    }
}

impl PolicyTable {
    pub fn policy_for(&self, kind: ToolKind) -> PermissionPolicy {
        match kind {
            ToolKind::Read => self.read,
            ToolKind::Edit => self.edit,
            ToolKind::Delete => self.delete,
            ToolKind::Move => self.r#move,
            ToolKind::Search => self.search,
            ToolKind::Execute => self.execute,
            ToolKind::Think => self.think,
            ToolKind::Fetch => self.fetch,
            ToolKind::Other => self.other,
        }
    }
}

/// What the host must ask its client (over ACP, `session/request_permission`).
/// `command` carries the real command string for execute kinds so tier
/// policies can evaluate it; `input_digest` is a stable label for journaling,
/// never a secret channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    pub tool_call_id: String,
    pub tool_name: String,
    pub kind: ToolKind,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub input_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allowed,
    Denied,
}

/// Stable FNV-1a digest of a tool input, for journals and receipts.
pub fn input_digest(input: &serde_json::Value) -> String {
    let encoded = serde_json::to_string(input).unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in encoded.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("fnv1a:{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_escalates_execute_and_allows_read_shaped_kinds() {
        let table = PolicyTable::default();
        assert_eq!(table.policy_for(ToolKind::Read), PermissionPolicy::Allow);
        assert_eq!(table.policy_for(ToolKind::Search), PermissionPolicy::Allow);
        assert_eq!(table.policy_for(ToolKind::Execute), PermissionPolicy::ApprovalRequired);
        assert_eq!(table.policy_for(ToolKind::Fetch), PermissionPolicy::ApprovalRequired);
    }

    #[test]
    fn digest_is_stable_and_content_free() {
        let a = input_digest(&serde_json::json!({ "command": "ls" }));
        let b = input_digest(&serde_json::json!({ "command": "ls" }));
        assert_eq!(a, b);
        assert!(a.starts_with("fnv1a:"));
        assert!(!a.contains("ls"));
    }
}
