use probe_core::server_control::ServerOperatorSummary;
use probe_protocol::backend::BackendKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeFailureSummary {
    pub(crate) title: &'static str,
    pub(crate) summary: String,
    pub(crate) next_step: String,
    pub(crate) detail: Option<String>,
}

impl RuntimeFailureSummary {
    #[must_use]
    pub(crate) fn body_lines(&self) -> Vec<String> {
        let mut lines = vec![self.summary.clone()];
        if let Some(detail) = self.detail.as_deref() {
            lines.push(format!("detail: {detail}"));
        }
        lines.push(format!("next: {}", self.next_step));
        lines
    }
}

#[must_use]
pub(crate) fn classify_runtime_failure(
    error: &str,
    operator_backend: Option<&ServerOperatorSummary>,
) -> RuntimeFailureSummary {
    let normalized = error.trim();
    let lowered = normalized.to_ascii_lowercase();

    if lowered.contains("usage_limit_reached") || lowered.contains("http 429") {
        let reset = extract_reset_seconds(normalized).map(format_duration);
        let next_step = reset.as_ref().map_or_else(
            || String::from("Wait for the limit window to reset, then retry."),
            |reset| format!("Wait about {reset}, then retry."),
        );
        return RuntimeFailureSummary {
            title: "Usage Limit Reached",
            summary: String::from(
                "The active backend refused this turn because the current account hit its usage limit.",
            ),
            next_step,
            detail: reset.map(|reset| format!("reset window: about {reset}")),
        };
    }

    if is_auth_failure(lowered.as_str()) {
        let next_step = if operator_backend
            .is_some_and(|summary| summary.backend_kind == BackendKind::OpenAiCodexSubscription)
        {
            String::from("Run `probe codex login`, then retry the turn.")
        } else {
            String::from("Update backend credentials, then retry the turn.")
        };
        return RuntimeFailureSummary {
            title: "Authentication Needed",
            summary: String::from("Probe could not authenticate with the active backend."),
            next_step,
            detail: Some(preview(normalized, 120)),
        };
    }

    if is_backend_unavailable(lowered.as_str()) {
        let next_step = if operator_backend.is_some_and(|backend| !backend.is_remote_target()) {
            String::from("Start the local backend, then retry. Use /backend for target details.")
        } else {
            String::from("Retry when the target is reachable. Use /backend for target details.")
        };
        return RuntimeFailureSummary {
            title: "Backend Unavailable",
            summary: String::from("Probe could not reach the active backend."),
            next_step,
            detail: Some(preview(normalized, 120)),
        };
    }

    RuntimeFailureSummary {
        title: "Runtime Error",
        summary: preview(normalized, 140),
        next_step: String::from("Retry the turn. Use /backend if you need to inspect the target."),
        detail: None,
    }
}

fn is_auth_failure(lowered: &str) -> bool {
    lowered.contains("authenticated=false")
        || lowered.contains("authentication")
        || lowered.contains("unauthorized")
        || lowered.contains("api key")
        || lowered.contains("login")
        || lowered.contains("http 401")
        || lowered.contains("http 403")
        || lowered.contains("forbidden")
}

fn is_backend_unavailable(lowered: &str) -> bool {
    lowered.contains("connection refused")
        || lowered.contains("transport failed")
        || lowered.contains("connection dropped")
        || lowered.contains("error sending request")
        || lowered.contains("timed out")
        || lowered.contains("timeout")
        || lowered.contains("dns")
        || lowered.contains("connection reset")
        || lowered.contains("network is unreachable")
        || lowered.contains("temporarily unavailable")
        || lowered.contains("127.0.0.1")
        || lowered.contains("localhost")
}

fn extract_reset_seconds(value: &str) -> Option<u64> {
    extract_number_after(value, "\"resets_in_seconds\":")
        .or_else(|| extract_number_after(value, "resets_in_seconds:"))
}

fn extract_number_after(value: &str, marker: &str) -> Option<u64> {
    let start = value.find(marker)? + marker.len();
    let digits = value[start..]
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 && minutes > 0 {
        format!("{hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

fn preview(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let preview = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

#[cfg(test)]
mod tests {
    use probe_core::backend_profiles::{openai_codex_subscription, psionic_qwen35_2b_q8_registry};
    use probe_core::server_control::PsionicServerConfig;

    use super::classify_runtime_failure;

    #[test]
    fn usage_limit_failures_include_reset_guidance() {
        let summary = classify_runtime_failure(
            r#"backend returned http 429: {"error":{"type":"usage_limit_reached","resets_in_seconds":12525}}"#,
            None,
        );
        assert_eq!(summary.title, "Usage Limit Reached");
        assert_eq!(summary.next_step, "Wait about 3h 28m, then retry.");
    }

    #[test]
    fn codex_auth_failures_point_to_login() {
        let backend = PsionicServerConfig::from_backend_profile(&openai_codex_subscription())
            .operator_summary();
        let summary =
            classify_runtime_failure("backend returned http 401 unauthorized", Some(&backend));
        assert_eq!(summary.title, "Authentication Needed");
        assert!(summary.next_step.contains("probe codex login"));
    }

    #[test]
    fn local_backend_failures_explain_local_recovery() {
        let backend = PsionicServerConfig::from_backend_profile(&psionic_qwen35_2b_q8_registry())
            .operator_summary();
        let summary = classify_runtime_failure(
            "error sending request for url (http://127.0.0.1:8080/v1/chat/completions)",
            Some(&backend),
        );
        assert_eq!(summary.title, "Backend Unavailable");
        assert!(summary.next_step.contains("Start the local backend"));
        assert!(summary.next_step.contains("/backend"));
    }
}
