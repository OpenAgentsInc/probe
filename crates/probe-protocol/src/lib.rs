pub const PROBE_PROTOCOL_VERSION: u32 = 1;
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

#[cfg(test)]
mod tests {
    use super::ProtocolDescriptor;

    #[test]
    fn current_descriptor_is_stable() {
        let descriptor = ProtocolDescriptor::current();
        assert_eq!(descriptor.runtime_name, "probe");
        assert_eq!(descriptor.version, 1);
    }
}
