pub mod backend;
pub mod runtime;
pub mod session;

use std::path::{Path, PathBuf};

pub const PROBE_PROTOCOL_VERSION: u32 = 11;
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
    use super::backend::{BackendKind, PrefixCacheMode, ServerAttachMode};
    use super::session::{SessionId, SessionState, TurnId};

    #[test]
    fn current_descriptor_is_stable() {
        let descriptor = ProtocolDescriptor::current();
        assert_eq!(descriptor.runtime_name, "probe");
        assert_eq!(descriptor.version, 11);
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
}
