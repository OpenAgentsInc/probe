use std::env;

use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};

pub const PSIONIC_APPLE_FM_BRIDGE_PROFILE: &str = "psionic-apple-fm-bridge";
pub const PSIONIC_APPLE_FM_ORACLE_PROFILE: &str = "psionic-apple-fm-oracle";
pub const PSIONIC_APPLE_FM_MODEL: &str = "apple-foundation-model";
pub const DEFAULT_APPLE_FM_BRIDGE_BASE_URL: &str = "http://127.0.0.1:11435";
pub const OPENAI_CODEX_SUBSCRIPTION_PROFILE: &str = "openai-codex-subscription";
pub const OPENAI_CODEX_SUBSCRIPTION_MODEL: &str = "gpt-5.4";
pub const OPENAI_CODEX_SUBSCRIPTION_REASONING_LEVEL: &str = "backend_default";
pub const DEFAULT_OPENAI_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
pub const PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE: &str = "psionic-qwen35-2b-q8-registry";
pub const PSIONIC_QWEN35_2B_Q8_ORACLE_PROFILE: &str = "psionic-qwen35-2b-q8-oracle";
pub const PSIONIC_QWEN35_2B_Q8_LONG_CONTEXT_PROFILE: &str = "psionic-qwen35-2b-q8-long-context";
pub const PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL: &str = "qwen3.5-2b-q8_0-registry.gguf";

const APPLE_FM_BASE_URL_ENV_KEYS: [&str; 2] =
    ["PROBE_APPLE_FM_BASE_URL", "OPENAGENTS_APPLE_FM_BASE_URL"];

#[must_use]
pub fn named_backend_profile(name: &str) -> Option<BackendProfile> {
    match name {
        OPENAI_CODEX_SUBSCRIPTION_PROFILE => Some(openai_codex_subscription()),
        PSIONIC_APPLE_FM_BRIDGE_PROFILE => Some(psionic_apple_fm_bridge()),
        PSIONIC_APPLE_FM_ORACLE_PROFILE => Some(psionic_apple_fm_oracle()),
        PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE => Some(psionic_qwen35_2b_q8_registry()),
        PSIONIC_QWEN35_2B_Q8_ORACLE_PROFILE => Some(psionic_qwen35_2b_q8_oracle()),
        PSIONIC_QWEN35_2B_Q8_LONG_CONTEXT_PROFILE => Some(psionic_qwen35_2b_q8_long_context()),
        _ => None,
    }
}

#[must_use]
pub fn openai_codex_subscription() -> BackendProfile {
    BackendProfile {
        name: String::from(OPENAI_CODEX_SUBSCRIPTION_PROFILE),
        kind: BackendKind::OpenAiCodexSubscription,
        base_url: String::from(DEFAULT_OPENAI_CODEX_BASE_URL),
        model: String::from(OPENAI_CODEX_SUBSCRIPTION_MODEL),
        api_key_env: String::new(),
        timeout_secs: 60,
        attach_mode: ServerAttachMode::AttachToExisting,
        prefix_cache_mode: PrefixCacheMode::BackendDefault,
    }
}

#[must_use]
pub const fn default_reasoning_level_for_backend(
    backend_kind: BackendKind,
) -> Option<&'static str> {
    match backend_kind {
        BackendKind::OpenAiCodexSubscription => Some(OPENAI_CODEX_SUBSCRIPTION_REASONING_LEVEL),
        BackendKind::OpenAiChatCompletions | BackendKind::AppleFmBridge => None,
    }
}

#[must_use]
pub fn psionic_apple_fm_bridge() -> BackendProfile {
    BackendProfile {
        name: String::from(PSIONIC_APPLE_FM_BRIDGE_PROFILE),
        kind: BackendKind::AppleFmBridge,
        base_url: resolved_apple_fm_bridge_base_url(),
        model: String::from(PSIONIC_APPLE_FM_MODEL),
        api_key_env: String::new(),
        timeout_secs: 45,
        attach_mode: ServerAttachMode::AttachToExisting,
        prefix_cache_mode: PrefixCacheMode::BackendDefault,
    }
}

#[must_use]
pub fn psionic_apple_fm_oracle() -> BackendProfile {
    BackendProfile {
        name: String::from(PSIONIC_APPLE_FM_ORACLE_PROFILE),
        kind: BackendKind::AppleFmBridge,
        base_url: resolved_apple_fm_bridge_base_url(),
        model: String::from(PSIONIC_APPLE_FM_MODEL),
        api_key_env: String::new(),
        timeout_secs: 30,
        attach_mode: ServerAttachMode::AttachToExisting,
        prefix_cache_mode: PrefixCacheMode::BackendDefault,
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

#[must_use]
pub fn psionic_qwen35_2b_q8_long_context() -> BackendProfile {
    BackendProfile {
        name: String::from(PSIONIC_QWEN35_2B_Q8_LONG_CONTEXT_PROFILE),
        kind: BackendKind::OpenAiChatCompletions,
        base_url: String::from("http://127.0.0.1:8080/v1"),
        model: String::from(PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL),
        api_key_env: String::from("PROBE_OPENAI_API_KEY"),
        timeout_secs: 60,
        attach_mode: ServerAttachMode::AttachToExisting,
        prefix_cache_mode: PrefixCacheMode::BackendDefault,
    }
}

fn resolved_apple_fm_bridge_base_url() -> String {
    resolve_apple_fm_bridge_base_url_with(|key| {
        env::var(key).ok().and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    })
}

fn resolve_apple_fm_bridge_base_url_with(
    mut read_env: impl FnMut(&str) -> Option<String>,
) -> String {
    APPLE_FM_BASE_URL_ENV_KEYS
        .iter()
        .find_map(|key| read_env(key))
        .unwrap_or_else(|| String::from(DEFAULT_APPLE_FM_BRIDGE_BASE_URL))
}

#[cfg(test)]
mod tests {
    use probe_protocol::backend::{PrefixCacheMode, ServerAttachMode};

    use super::{
        DEFAULT_APPLE_FM_BRIDGE_BASE_URL, DEFAULT_OPENAI_CODEX_BASE_URL,
        OPENAI_CODEX_SUBSCRIPTION_MODEL, OPENAI_CODEX_SUBSCRIPTION_PROFILE,
        OPENAI_CODEX_SUBSCRIPTION_REASONING_LEVEL, PSIONIC_APPLE_FM_BRIDGE_PROFILE,
        PSIONIC_APPLE_FM_MODEL, PSIONIC_APPLE_FM_ORACLE_PROFILE,
        PSIONIC_QWEN35_2B_Q8_LONG_CONTEXT_PROFILE, PSIONIC_QWEN35_2B_Q8_ORACLE_PROFILE,
        PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL, PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE,
        default_reasoning_level_for_backend, named_backend_profile, openai_codex_subscription,
        psionic_apple_fm_bridge, psionic_apple_fm_oracle, psionic_qwen35_2b_q8_long_context,
        psionic_qwen35_2b_q8_oracle, psionic_qwen35_2b_q8_registry,
        resolve_apple_fm_bridge_base_url_with,
    };

    #[test]
    fn canonical_codex_subscription_profile_is_stable() {
        let profile = openai_codex_subscription();
        assert_eq!(profile.name, OPENAI_CODEX_SUBSCRIPTION_PROFILE);
        assert_eq!(
            profile.kind,
            probe_protocol::backend::BackendKind::OpenAiCodexSubscription
        );
        assert_eq!(profile.base_url, DEFAULT_OPENAI_CODEX_BASE_URL);
        assert_eq!(profile.model, OPENAI_CODEX_SUBSCRIPTION_MODEL);
        assert_eq!(profile.api_key_env, "");
        assert_eq!(profile.timeout_secs, 60);
    }

    #[test]
    fn codex_subscription_profile_is_available_by_name() {
        let profile = named_backend_profile(OPENAI_CODEX_SUBSCRIPTION_PROFILE)
            .expect("codex subscription profile");
        assert_eq!(profile.base_url, DEFAULT_OPENAI_CODEX_BASE_URL);
        assert_eq!(profile.model, OPENAI_CODEX_SUBSCRIPTION_MODEL);
    }

    #[test]
    fn codex_subscription_reasoning_level_is_stable() {
        assert_eq!(
            default_reasoning_level_for_backend(
                probe_protocol::backend::BackendKind::OpenAiCodexSubscription
            ),
            Some(OPENAI_CODEX_SUBSCRIPTION_REASONING_LEVEL)
        );
    }

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
    fn apple_fm_bridge_profile_is_available_by_name() {
        let profile =
            named_backend_profile(PSIONIC_APPLE_FM_BRIDGE_PROFILE).expect("apple fm profile");
        assert_eq!(profile.name, PSIONIC_APPLE_FM_BRIDGE_PROFILE);
        assert_eq!(profile.base_url, DEFAULT_APPLE_FM_BRIDGE_BASE_URL);
        assert_eq!(profile.model, PSIONIC_APPLE_FM_MODEL);
        assert_eq!(profile.api_key_env, "");
        assert_eq!(psionic_apple_fm_bridge().model, PSIONIC_APPLE_FM_MODEL);
    }

    #[test]
    fn apple_fm_oracle_profile_is_available_by_name() {
        let profile =
            named_backend_profile(PSIONIC_APPLE_FM_ORACLE_PROFILE).expect("apple fm oracle");
        assert_eq!(profile.model, PSIONIC_APPLE_FM_MODEL);
        assert_eq!(profile.timeout_secs, 30);
        assert_eq!(
            psionic_apple_fm_oracle().name,
            PSIONIC_APPLE_FM_ORACLE_PROFILE
        );
    }

    #[test]
    fn oracle_profile_is_available_by_name() {
        let profile =
            named_backend_profile(PSIONIC_QWEN35_2B_Q8_ORACLE_PROFILE).expect("oracle profile");
        assert_eq!(profile.model, PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL);
        assert_eq!(profile.timeout_secs, 30);
        assert_eq!(
            psionic_qwen35_2b_q8_oracle().name,
            PSIONIC_QWEN35_2B_Q8_ORACLE_PROFILE
        );
    }

    #[test]
    fn long_context_profile_is_available_by_name() {
        let profile = named_backend_profile(PSIONIC_QWEN35_2B_Q8_LONG_CONTEXT_PROFILE)
            .expect("long-context profile");
        assert_eq!(profile.model, PSIONIC_QWEN35_2B_Q8_REGISTRY_MODEL);
        assert_eq!(profile.timeout_secs, 60);
        assert_eq!(
            psionic_qwen35_2b_q8_long_context().name,
            PSIONIC_QWEN35_2B_Q8_LONG_CONTEXT_PROFILE
        );
    }

    #[test]
    fn apple_fm_bridge_uses_probe_specific_base_url_override_first() {
        let base_url = resolve_apple_fm_bridge_base_url_with(|key| match key {
            "PROBE_APPLE_FM_BASE_URL" => Some(String::from("http://127.0.0.1:19091")),
            "OPENAGENTS_APPLE_FM_BASE_URL" => Some(String::from("http://127.0.0.1:11435")),
            _ => None,
        });
        assert_eq!(base_url, "http://127.0.0.1:19091");
    }

    #[test]
    fn apple_fm_bridge_falls_back_to_openagents_override() {
        let base_url = resolve_apple_fm_bridge_base_url_with(|key| match key {
            "PROBE_APPLE_FM_BASE_URL" => None,
            "OPENAGENTS_APPLE_FM_BASE_URL" => Some(String::from("http://127.0.0.1:11435")),
            _ => None,
        });
        assert_eq!(base_url, "http://127.0.0.1:11435");
    }

    #[test]
    fn apple_fm_bridge_uses_default_when_no_override_is_set() {
        let base_url = resolve_apple_fm_bridge_base_url_with(|_| None);
        assert_eq!(base_url, DEFAULT_APPLE_FM_BRIDGE_BASE_URL);
    }
}
