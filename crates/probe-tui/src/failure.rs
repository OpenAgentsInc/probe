use crate::transcript::{TranscriptEntry, TranscriptRole};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeFailureSummary {
    pub(crate) title: &'static str,
    pub(crate) summary: String,
    pub(crate) next_step: String,
    pub(crate) detail: Option<String>,
}

impl RuntimeFailureSummary {
    #[must_use]
    pub(crate) fn transcript_body_lines(&self) -> Vec<String> {
        let mut lines = vec![self.summary.clone()];
        if let Some(detail) = self.detail.as_deref() {
            lines.push(format!("detail: {detail}"));
        }
        lines.push(format!("next: {}", self.next_step));
        lines
    }
}

#[must_use]
pub(crate) fn classify_runtime_failure(error: &str) -> RuntimeFailureSummary {
    let normalized = error.trim();
    let lowered = normalized.to_ascii_lowercase();

    if lowered.contains("usage_limit_reached") || lowered.contains("http 429") {
        let reset = extract_reset_seconds(normalized).map(format_duration);
        let next_step = reset.as_ref().map_or_else(
            || String::from("Wait for the limit to reset, or switch backend/model"),
            |reset| format!("Wait about {reset}, or switch backend/model"),
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
        return RuntimeFailureSummary {
            title: "Authentication Needed",
            summary: String::from(
                "Probe could not authenticate with the active backend or integration.",
            ),
            next_step: String::from(
                "Open /doctor, then re-run login or update backend credentials",
            ),
            detail: None,
        };
    }

    if is_mcp_failure(lowered.as_str()) {
        return RuntimeFailureSummary {
            title: "MCP Attach Failed",
            summary: String::from(
                "Probe could not attach an MCP server or load its tool inventory for this session.",
            ),
            next_step: String::from(
                "Open /mcp or /doctor to inspect the MCP server, then retry the next turn",
            ),
            detail: Some(preview(normalized, 120)),
        };
    }

    if is_backend_unavailable(lowered.as_str()) {
        return RuntimeFailureSummary {
            title: "Backend Unavailable",
            summary: String::from("Probe could not reach the active backend."),
            next_step: if lowered.contains("127.0.0.1") || lowered.contains("localhost") {
                String::from("Start the local backend, or switch lanes with Tab")
            } else {
                String::from("Retry the turn, or switch backends if this target is unavailable")
            },
            detail: Some(preview(normalized, 120)),
        };
    }

    RuntimeFailureSummary {
        title: "Runtime Error",
        summary: preview(normalized, 140),
        next_step: String::from("Retry the turn or switch lanes with Tab"),
        detail: None,
    }
}

#[must_use]
pub(crate) fn runtime_failure_transcript_entry(error: &str) -> TranscriptEntry {
    let summary = classify_runtime_failure(error);
    TranscriptEntry::new(
        TranscriptRole::Status,
        summary.title,
        summary.transcript_body_lines(),
    )
}

fn is_auth_failure(lowered: &str) -> bool {
    lowered.contains("authenticated=false")
        || lowered.contains("authentication")
        || lowered.contains("unauthorized")
        || lowered.contains("api key")
        || lowered.contains("login")
        || (lowered.contains("http 401"))
        || (lowered.contains("http 403"))
        || lowered.contains("forbidden")
}

fn is_mcp_failure(lowered: &str) -> bool {
    lowered.contains("mcp")
        && (lowered.contains("initialize")
            || lowered.contains("tools/list")
            || lowered.contains("attach")
            || lowered.contains("tool inventory")
            || lowered.contains("runtime server"))
}

fn is_backend_unavailable(lowered: &str) -> bool {
    lowered.contains("connection refused")
        || lowered.contains("transport failed")
        || lowered.contains("connection dropped")
        || lowered.contains("error sending request")
        || lowered.contains("timed out")
        || lowered.contains("timeout")
        || lowered.contains("dns")
        || lowered.contains("127.0.0.1")
        || lowered.contains("localhost")
        || lowered.contains("connection reset")
        || lowered.contains("network is unreachable")
        || lowered.contains("temporarily unavailable")
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
    use super::{classify_runtime_failure, runtime_failure_transcript_entry};

    #[test]
    fn usage_limit_failures_get_reset_guidance() {
        let summary = classify_runtime_failure(
            r#"backend returned http 429: {"error":{"type":"usage_limit_reached","resets_in_seconds":12525}}"#,
        );

        assert_eq!(summary.title, "Usage Limit Reached");
        assert!(summary.summary.contains("usage limit"));
        assert_eq!(
            summary.next_step,
            "Wait about 3h 28m, or switch backend/model"
        );
        assert_eq!(
            summary.detail.as_deref(),
            Some("reset window: about 3h 28m")
        );
    }

    #[test]
    fn auth_failures_are_called_out_cleanly() {
        let summary =
            classify_runtime_failure("backend returned http 401 unauthorized: login required");

        assert_eq!(summary.title, "Authentication Needed");
        assert!(summary.next_step.contains("/doctor"));
    }

    #[test]
    fn local_backend_failures_explain_the_backend_is_down() {
        let summary = classify_runtime_failure(
            "error sending request for url (http://127.0.0.1:8080/v1/chat/completions)",
        );

        assert_eq!(summary.title, "Backend Unavailable");
        assert_eq!(
            summary.next_step,
            "Start the local backend, or switch lanes with Tab"
        );
    }

    #[test]
    fn mcp_failures_keep_mcp_specific_recovery_copy() {
        let summary =
            classify_runtime_failure("MCP initialize failed while loading tools/list inventory");

        assert_eq!(summary.title, "MCP Attach Failed");
        assert!(summary.next_step.contains("/mcp"));
    }

    #[test]
    fn transcript_entries_use_normalized_failure_copy() {
        let entry = runtime_failure_transcript_entry(
            "backend returned http 401 unauthorized: login required",
        );

        assert_eq!(entry.title(), "Authentication Needed");
        assert_eq!(
            entry.body()[0],
            "Probe could not authenticate with the active backend or integration."
        );
        assert!(entry.body()[1].contains("next:"));
    }
}
