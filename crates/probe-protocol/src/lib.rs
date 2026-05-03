pub mod admin_chat;
pub mod backend;
pub mod runtime;
pub mod session;
pub mod website_events;

use std::path::{Path, PathBuf};

pub const PROBE_PROTOCOL_VERSION: u32 = 17;
pub const PROBE_RUNTIME_NAME: &str = "probe";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolDescriptor {
    pub runtime_name: &'static str,
    pub version: u32,
}

impl ProtocolDescriptor {
    #[must_use]
    pub const fn current() -> Self {
        Self {
            runtime_name: PROBE_RUNTIME_NAME,
            version: PROBE_PROTOCOL_VERSION,
        }
    }
}

#[must_use]
pub fn default_local_daemon_socket_path(probe_home: &Path) -> PathBuf {
    probe_home.join("daemon").join("probe-daemon.sock")
}

#[cfg(test)]
mod tests {
    use super::ProtocolDescriptor;
    use super::admin_chat::{AdminChatBridgeEvent, AdminChatBridgeRequest};
    use super::backend::{BackendKind, PrefixCacheMode, ServerAttachMode};
    use super::session::{SessionId, SessionState, TurnId};
    use super::website_events::{
        PROBE_WEBSITE_EVENT_SCHEMA_VERSION, ProbeWebsiteEvent, ProbeWebsiteEventActor,
        ProbeWebsiteEventCorrelation, ProbeWebsiteEventSource, ProbeWebsiteEventType,
    };
    use serde_json::Map;

    #[test]
    fn current_descriptor_is_stable() {
        let descriptor = ProtocolDescriptor::current();
        assert_eq!(descriptor.runtime_name, "probe");
        assert_eq!(descriptor.version, 17);
    }

    #[test]
    fn session_types_are_constructible() {
        let session_id = SessionId::new("session-1");
        let turn_id = TurnId(0);
        let state = SessionState::Active;
        assert_eq!(session_id.as_str(), "session-1");
        assert_eq!(turn_id.0, 0);
        assert!(matches!(state, SessionState::Active));
    }

    #[test]
    fn backend_types_are_constructible() {
        let kind = BackendKind::OpenAiChatCompletions;
        let codex_kind = BackendKind::OpenAiCodexSubscription;
        let apple_kind = BackendKind::AppleFmBridge;
        let attach_mode = ServerAttachMode::AttachToExisting;
        let cache_mode = PrefixCacheMode::BackendDefault;
        assert!(matches!(kind, BackendKind::OpenAiChatCompletions));
        assert!(matches!(codex_kind, BackendKind::OpenAiCodexSubscription));
        assert!(matches!(apple_kind, BackendKind::AppleFmBridge));
        assert!(matches!(attach_mode, ServerAttachMode::AttachToExisting));
        assert!(matches!(cache_mode, PrefixCacheMode::BackendDefault));
    }

    #[test]
    fn admin_chat_bridge_types_are_serializable() {
        let request =
            AdminChatBridgeRequest::fake("request-1", 123, "admin@example.com", "hello admin chat");
        let encoded = serde_json::to_string(&request).expect("serialize request");
        let decoded: AdminChatBridgeRequest =
            serde_json::from_str(encoded.as_str()).expect("deserialize request");
        assert_eq!(decoded.request_id, "request-1");
        assert_eq!(decoded.tool_policy.mode, "admin_chat");

        let event = AdminChatBridgeEvent::TextDelta {
            run_id: String::from("run-1"),
            id: String::from("assistant-run-1"),
            delta: String::from("hello"),
        };
        let encoded = serde_json::to_string(&event).expect("serialize event");
        assert!(encoded.contains("\"type\":\"text_delta\""));
    }

    #[test]
    fn website_event_contract_serializes_stable_shape() {
        let event = ProbeWebsiteEvent::new(
            7,
            1_777_777_777_000,
            ProbeWebsiteEventType::ApprovalRequested,
            ProbeWebsiteEventActor {
                kind: String::from("probe"),
                id: Some(String::from("sess-1")),
                label: None,
            },
            ProbeWebsiteEventSource {
                kind: String::from("runtime"),
                id: Some(String::from("turn-0")),
                label: None,
            },
            ProbeWebsiteEventCorrelation {
                request_id: Some(String::from("request-1")),
                run_id: Some(String::from("run-1")),
                probe_session_id: Some(String::from("sess-1")),
                probe_turn_id: Some(String::from("turn-0")),
                ..ProbeWebsiteEventCorrelation::default()
            },
            Map::new(),
        );

        let encoded = serde_json::to_string(&event).expect("serialize website event");

        assert!(encoded.contains(PROBE_WEBSITE_EVENT_SCHEMA_VERSION));
        assert!(encoded.contains("\"eventType\":\"approval_requested\""));
        assert!(encoded.contains("\"sequence\":7"));
        assert!(encoded.contains("\"probeSessionId\":\"sess-1\""));
    }
}
