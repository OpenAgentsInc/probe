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
        crate_boundaries: vec!["probe-protocol", "probe-core", "probe-provider-openai", "probe-cli"],
    }
}

#[cfg(test)]
mod tests {
    use super::runtime_bootstrap;

    #[test]
    fn bootstrap_mentions_all_initial_crates() {
        let bootstrap = runtime_bootstrap();
        assert_eq!(bootstrap.protocol.version, 1);
        assert_eq!(bootstrap.crate_boundaries.len(), 4);
        assert!(bootstrap.crate_boundaries.contains(&"probe-cli"));
    }
}
