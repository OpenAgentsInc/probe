use probe_core::server_control::ServerOperatorSummary;
use probe_protocol::backend::BackendKind;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeFailureSummary {
    pub(crate) title: &'static str,
    pub(crate) summary: String,
    pub(crate) next_step: String,
    pub(crate) metadata: Vec<String>,
}

impl RuntimeFailureSummary {
    #[must_use]
    pub(crate) fn body_lines(&self) -> Vec<String> {
        let mut lines = vec![self.summary.clone()];
        lines.extend(self.metadata.iter().cloned());
        lines.push(format!("next: {}", self.next_step));
        lines
    }
}

#[must_use]
pub(crate) fn summarize_runtime_note(
    note: &str,
    operator_backend: Option<&ServerOperatorSummary>,
) -> Option<RuntimeFailureSummary> {
    looks_like_runtime_failure(note).then(|| classify_runtime_failure(note, operator_backend))
}

#[must_use]
pub(crate) fn classify_runtime_failure(
    error: &str,
    operator_backend: Option<&ServerOperatorSummary>,
) -> RuntimeFailureSummary {
    let normalized = error.trim();
    let lowered = normalized.to_ascii_lowercase();
    let payload = extract_embedded_payload(normalized);
    let common_metadata = common_metadata_lines(normalized);

    if lowered.contains("usage_limit_reached") || lowered.contains("http 429") {
        let reset = extract_reset_seconds(normalized).map(format_duration);
        let next_step = reset.as_ref().map_or_else(
            || String::from("Wait for the limit window to reset, then retry."),
            |reset| format!("Wait about {reset}, then retry."),
        );
        let mut metadata = common_metadata;
        metadata.extend(error_payload_metadata_lines(payload.as_ref(), true));
        return RuntimeFailureSummary {
            title: "Usage Limit Reached",
            summary: String::from(
                "The active backend refused this turn because the current account hit its usage limit.",
            ),
            next_step,
            metadata,
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
        let mut metadata = common_metadata;
        if metadata.is_empty() {
            metadata.extend(error_payload_metadata_lines(payload.as_ref(), false));
        }
        if metadata.is_empty() {
            metadata.push(format!("detail: {}", preview(normalized, 120)));
        }
        return RuntimeFailureSummary {
            title: "Authentication Needed",
            summary: String::from("Probe could not authenticate with the active backend."),
            next_step,
            metadata,
        };
    }

    if is_backend_unavailable(lowered.as_str()) {
        let next_step = if operator_backend.is_some_and(|backend| !backend.is_remote_target()) {
            String::from("Start the local backend, then retry. Use /backend for target details.")
        } else {
            String::from("Retry when the target is reachable. Use /backend for target details.")
        };
        let mut metadata = common_metadata;
        if metadata.is_empty() {
            metadata.push(format!("detail: {}", preview(normalized, 120)));
        }
        return RuntimeFailureSummary {
            title: "Backend Unavailable",
            summary: String::from("Probe could not reach the active backend."),
            next_step,
            metadata,
        };
    }

    let mut metadata = common_metadata;
    if metadata.is_empty() {
        metadata.extend(error_payload_metadata_lines(payload.as_ref(), false));
    }
    if metadata.is_empty() {
        metadata.push(format!("detail: {}", preview(normalized, 140)));
    }
    RuntimeFailureSummary {
        title: "Runtime Error",
        summary: String::from("Probe could not finish the requested turn."),
        next_step: String::from("Retry the turn. Use /backend if you need to inspect the target."),
        metadata,
    }
}

fn looks_like_runtime_failure(value: &str) -> bool {
    let lowered = value.trim().to_ascii_lowercase();
    lowered.contains("backend request failed")
        || lowered.contains("backend returned http")
        || lowered.contains("usage_limit_reached")
        || is_auth_failure(lowered.as_str())
        || is_backend_unavailable(lowered.as_str())
        || lowered.contains("missing assistant message")
        || lowered.contains("provider request")
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

fn common_metadata_lines(value: &str) -> Vec<String> {
    let mut metadata = Vec::new();
    if let Some(session_id) = extract_session_id(value) {
        metadata.push(format!("session: {session_id}"));
    }
    if let Some(status) = extract_http_status(value) {
        metadata.push(format!("status: {status}"));
    }
    metadata
}

fn error_payload_metadata_lines(
    payload: Option<&Value>,
    include_usage_limit_fields: bool,
) -> Vec<String> {
    let Some(object) = payload.and_then(payload_error_object) else {
        return Vec::new();
    };

    let mut metadata = Vec::new();
    if let Some(kind) = object.get("type").and_then(Value::as_str) {
        metadata.push(format!("type: {kind}"));
    }
    if let Some(message) = object.get("message").and_then(Value::as_str) {
        metadata.push(format!("message: {message}"));
    }
    if include_usage_limit_fields {
        if let Some(plan) = object.get("plan_type").and_then(Value::as_str) {
            metadata.push(format!("plan: {plan}"));
        }
        if let Some(reset_seconds) = object.get("resets_in_seconds").and_then(Value::as_u64) {
            metadata.push(format!(
                "reset_in: about {}",
                format_duration(reset_seconds)
            ));
        }
    }
    metadata
}

fn extract_reset_seconds(value: &str) -> Option<u64> {
    extract_number_after(value, "\"resets_in_seconds\":")
        .or_else(|| extract_number_after(value, "resets_in_seconds:"))
}

fn extract_session_id(value: &str) -> Option<String> {
    let marker = "for session ";
    let start = value.find(marker)? + marker.len();
    let session_id = value[start..]
        .chars()
        .take_while(|character| !matches!(character, ':' | ' ' | '\n' | '\r' | '\t'))
        .collect::<String>();
    (!session_id.is_empty()).then_some(session_id)
}

fn extract_http_status(value: &str) -> Option<u16> {
    let marker = "http ";
    let start = value.find(marker)? + marker.len();
    let digits = value[start..]
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
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

fn extract_embedded_payload(value: &str) -> Option<Value> {
    let start = value.find("{\"error\"").or_else(|| value.find('{'))?;
    serde_json::from_str::<Value>(&value[start..]).ok()
}

fn payload_error_object(payload: &Value) -> Option<&serde_json::Map<String, Value>> {
    payload
        .get("error")
        .and_then(Value::as_object)
        .or_else(|| payload.as_object())
}

fn format_duration(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 && hours > 0 {
        format!("{days}d {hours}h")
    } else if days > 0 {
        format!("{days}d")
    } else if hours > 0 && minutes > 0 {
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

    use super::{classify_runtime_failure, summarize_runtime_note};

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
    fn usage_limit_failures_surface_structured_metadata() {
        let summary = classify_runtime_failure(
            r#"backend request failed for session sess_123: backend returned http 429: {"error":{"type":"usage_limit_reached","message":"The usage limit has been reached","plan_type":"pro","resets_in_seconds":451864}}"#,
            None,
        );
        assert_eq!(summary.title, "Usage Limit Reached");
        assert!(
            summary
                .metadata
                .contains(&String::from("session: sess_123"))
        );
        assert!(summary.metadata.contains(&String::from("status: 429")));
        assert!(
            summary
                .metadata
                .contains(&String::from("type: usage_limit_reached"))
        );
        assert!(summary.metadata.contains(&String::from("plan: pro")));
        assert!(
            summary
                .metadata
                .contains(&String::from("reset_in: about 5d 5h"))
        );
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

    #[test]
    fn plain_notes_do_not_get_failure_treatment() {
        assert!(
            summarize_runtime_note(
                "session exceeded the configured tool loop bound of 8 controller round trips",
                None
            )
            .is_none()
        );
    }
}
