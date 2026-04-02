pub mod backend_profiles;
pub mod dataset_export;
pub mod harness;
pub mod long_context;
pub mod provider;
pub mod runtime;
pub mod server_control;
pub mod session_summary_artifacts;
pub mod session_store;
pub mod tools;

use probe_protocol::ProtocolDescriptor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeBootstrap {
    pub protocol: ProtocolDescriptor,
    pub crate_boundaries: Vec<&'static str>,
}

#[must_use]
pub fn runtime_bootstrap() -> RuntimeBootstrap {
    RuntimeBootstrap {
        protocol: ProtocolDescriptor::current(),
        crate_boundaries: vec![
            "probe-protocol",
            "probe-client",
            "probe-core",
            "probe-server",
            "probe-openai-auth",
            "probe-provider-apple-fm",
            "probe-provider-openai",
            "probe-cli",
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::backend_profiles::named_backend_profile;
    use super::runtime_bootstrap;
    use crate::session_store::FilesystemSessionStore;

    #[test]
    fn bootstrap_mentions_all_initial_crates() {
        let bootstrap = runtime_bootstrap();
        assert_eq!(bootstrap.protocol.version, probe_protocol::PROBE_PROTOCOL_VERSION);
        assert_eq!(bootstrap.crate_boundaries.len(), 8);
        assert!(bootstrap.crate_boundaries.contains(&"probe-cli"));
        assert!(bootstrap.crate_boundaries.contains(&"probe-client"));
        assert!(bootstrap.crate_boundaries.contains(&"probe-server"));
        assert!(bootstrap.crate_boundaries.contains(&"probe-openai-auth"));
        assert!(
            bootstrap
                .crate_boundaries
                .contains(&"probe-provider-apple-fm")
        );
    }

    #[test]
    fn filesystem_session_store_is_constructible() {
        let store = FilesystemSessionStore::new("/tmp/probe-test");
        assert!(store.root().ends_with("probe-test"));
    }

    #[test]
    fn named_psionic_profile_is_available() {
        let profile = named_backend_profile("psionic-qwen35-2b-q8-registry")
            .expect("expected canonical profile");
        assert_eq!(profile.model, "qwen3.5-2b-q8_0-registry.gguf");
    }
}
