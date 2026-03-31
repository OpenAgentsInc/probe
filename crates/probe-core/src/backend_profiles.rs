use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};

pub const PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE: &str = "psionic-qwen35-2b-q8-registry";
pub const PSIONIC_QWEN35_2B_Q8_ORACLE_PROFILE: &str = "psionic-qwen35-2b-q8-oracle";
pub const PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL: &str = "qwen3.5-2b-q8_0-registry.gguf";

#[must_use]
pub fn named_backend_profile(name: &str) -> Option<BackendProfile> {
    match name {
        PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE => Some(psionic_qwen35_2b_q8_registry()),
        PSIONIC_QWEN35_2B_Q8_ORACLE_PROFILE => Some(psionic_qwen35_2b_q8_oracle()),
        _ => None,
    }
}

#[must_use]
pub fn psionic_qwen35_2b_q8_registry() -> BackendProfile {
    BackendProfile {
        name: String::from(PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE),
        kind: BackendKind::OpenAiChatCompletions,
        base_url: String::from("http://127.0.0.1:8080/v1"),
        model: String::from(PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL),
        api_key_env: String::from("PROBE_OPENAI_API_KEY"),
        timeout_secs: 45,
        attach_mode: ServerAttachMode::AttachToExisting,
        prefix_cache_mode: PrefixCacheMode::BackendDefault,
    }
}

#[must_use]
pub fn psionic_qwen35_2b_q8_oracle() -> BackendProfile {
    BackendProfile {
        name: String::from(PSIONIC_QWEN35_2B_Q8_ORACLE_PROFILE),
        kind: BackendKind::OpenAiChatCompletions,
        base_url: String::from("http://127.0.0.1:8080/v1"),
        model: String::from(PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL),
        api_key_env: String::from("PROBE_OPENAI_API_KEY"),
        timeout_secs: 30,
        attach_mode: ServerAttachMode::AttachToExisting,
        prefix_cache_mode: PrefixCacheMode::BackendDefault,
    }
}

#[cfg(test)]
mod tests {
    use probe_protocol::backend::{PrefixCacheMode, ServerAttachMode};

    use super::{
        PSIONIC_QWEN35_2B_Q8_ORACLE_PROFILE, PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL,
        PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE, named_backend_profile, psionic_qwen35_2b_q8_oracle,
        psionic_qwen35_2b_q8_registry,
    };

    #[test]
    fn canonical_psionic_profile_is_stable() {
        let profile = psionic_qwen35_2b_q8_registry();
        assert_eq!(profile.name, PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE);
        assert_eq!(profile.base_url, "http://127.0.0.1:8080/v1");
        assert_eq!(profile.model, PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL);
        assert_eq!(profile.api_key_env, "PROBE_OPENAI_API_KEY");
        assert_eq!(profile.timeout_secs, 45);
        assert!(matches!(
            profile.attach_mode,
            ServerAttachMode::AttachToExisting
        ));
        assert!(matches!(
            profile.prefix_cache_mode,
            PrefixCacheMode::BackendDefault
        ));
    }

    #[test]
    fn canonical_profile_is_available_by_name() {
        let profile =
            named_backend_profile(PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE).expect("expected profile");
        assert_eq!(profile.model, PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL);
    }

    #[test]
    fn oracle_profile_is_available_by_name() {
        let profile =
            named_backend_profile(PSIONIC_QWEN35_2B_Q8_ORACLE_PROFILE).expect("oracle profile");
        assert_eq!(profile.model, PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL);
        assert_eq!(profile.timeout_secs, 30);
        assert_eq!(psionic_qwen35_2b_q8_oracle().name, PSIONIC_QWEN35_2B_Q8_ORACLE_PROFILE);
    }
}
