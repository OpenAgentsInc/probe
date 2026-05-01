use probe_protocol::admin_chat::{
    AdminChatBridgeEvent, AdminChatBridgeRequest, AdminChatProviderMetadata,
    AdminChatRedactedDiagnostics, AdminChatUsageSnapshot,
};

const FAKE_BACKEND_PROFILE: &str = "openagents-admin-chat-fake";
const FAKE_MODEL: &str = "probe-admin-chat-fake-v1";

#[derive(Clone, Debug, PartialEq)]
pub struct AdminChatBridgeStream {
    pub events: Vec<AdminChatBridgeEvent>,
}

#[must_use]
pub fn fake_admin_chat_bridge_stream(request: &AdminChatBridgeRequest) -> AdminChatBridgeStream {
    let provider = provider_metadata(request);
    let probe_session_id = format!("probe-admin-chat.{}", request.run_id);
    let diagnostics = AdminChatRedactedDiagnostics {
        probe_session_id: probe_session_id.clone(),
        transcript_ref: format!(
            "probe://admin-chat/{}/{}",
            request.workspace, request.run_id
        ),
        request_id: Some(request.request_id.clone()),
        response_id: Some(format!("fake-response-{}", request.run_id)),
    };
    let usage = AdminChatUsageSnapshot {
        input_tokens: Some(tokenish_count(request.prompt.as_str())),
        output_tokens: Some(12),
        total_tokens: Some(tokenish_count(request.prompt.as_str()) + 12),
        raw: None,
    };
    let mut events = vec![
        AdminChatBridgeEvent::RunStarted {
            request_id: request.request_id.clone(),
            run_id: request.run_id.clone(),
            probe_session_id,
            provider: provider.clone(),
            tool_policy: request.tool_policy.clone(),
        },
        AdminChatBridgeEvent::ModelStreamStarted {
            run_id: request.run_id.clone(),
            provider: provider.clone(),
        },
    ];

    for delta in fake_text(request).split_inclusive(' ') {
        events.push(AdminChatBridgeEvent::TextDelta {
            run_id: request.run_id.clone(),
            id: format!("assistant-{}", request.run_id),
            delta: delta.to_string(),
        });
    }

    events.push(AdminChatBridgeEvent::UsageLimitsSnapshot {
        run_id: request.run_id.clone(),
        provider: provider.clone(),
        usage: Some(usage.clone()),
        limits: None,
    });
    events.push(AdminChatBridgeEvent::RunCompleted {
        run_id: request.run_id.clone(),
        status: String::from("succeeded"),
        provider,
        response_id: Some(format!("fake-response-{}", request.run_id)),
        usage: Some(usage),
        diagnostics,
    });

    AdminChatBridgeStream { events }
}

pub fn render_admin_chat_sse(events: &[AdminChatBridgeEvent]) -> Result<String, serde_json::Error> {
    let mut output = String::new();

    for event in events {
        output.push_str("data: ");
        output.push_str(serde_json::to_string(event)?.as_str());
        output.push_str("\n\n");
    }

    output.push_str("data: [DONE]\n\n");

    Ok(output)
}

fn provider_metadata(request: &AdminChatBridgeRequest) -> AdminChatProviderMetadata {
    AdminChatProviderMetadata {
        key: request.provider.key.clone(),
        mode: request.provider.mode.clone(),
        account_ref: request.provider.account_ref.clone(),
        label: request.provider.label.clone(),
        backend_family: String::from("fake"),
        backend_profile: String::from(FAKE_BACKEND_PROFILE),
        model: String::from(FAKE_MODEL),
    }
}

fn fake_text(request: &AdminChatBridgeRequest) -> String {
    let prompt = request
        .prompt
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let summary = prompt.chars().take(96).collect::<String>();

    format!(
        "Probe admin chat bridge fake response for {} run {}: {}",
        request.workspace, request.run_id, summary
    )
}

fn tokenish_count(text: &str) -> u64 {
    text.split_whitespace().count().max(1) as u64
}

#[cfg(test)]
mod tests {
    use super::{fake_admin_chat_bridge_stream, render_admin_chat_sse};
    use probe_protocol::admin_chat::{AdminChatBridgeEvent, AdminChatBridgeRequest};
    use serde_json::json;

    #[test]
    fn fake_bridge_stream_maps_to_laravel_persistence_events() {
        let mut request = AdminChatBridgeRequest::fake(
            "request-1",
            123,
            "admin@example.com",
            "Summarize provider state.",
        );
        request.run_id = String::from("run-123");
        request.provider.account_ref = Some(String::from("provider-account-opaque-1"));

        let stream = fake_admin_chat_bridge_stream(&request);

        assert!(matches!(
            stream.events.first(),
            Some(AdminChatBridgeEvent::RunStarted { .. })
        ));
        assert!(
            stream
                .events
                .iter()
                .any(|event| matches!(event, AdminChatBridgeEvent::ModelStreamStarted { .. }))
        );
        assert!(
            stream
                .events
                .iter()
                .any(|event| matches!(event, AdminChatBridgeEvent::TextDelta { .. }))
        );
        assert!(matches!(
            stream.events.last(),
            Some(AdminChatBridgeEvent::RunCompleted { .. })
        ));
    }

    #[test]
    fn fake_bridge_sse_does_not_echo_secret_shaped_metadata() {
        let mut request = AdminChatBridgeRequest::fake(
            "request-2",
            123,
            "admin@example.com",
            "Do not leak secrets.",
        );
        request
            .metadata
            .insert(String::from("api_key"), json!("sk-should-not-appear"));
        request
            .metadata
            .insert(String::from("refresh_token"), json!("refresh-secret"));

        let stream = fake_admin_chat_bridge_stream(&request);
        let rendered = render_admin_chat_sse(&stream.events).expect("render sse");

        assert!(rendered.contains("data: {\"type\":\"run_started\""));
        assert!(rendered.contains("data: [DONE]"));
        assert!(!rendered.contains("sk-should-not-appear"));
        assert!(!rendered.contains("refresh-secret"));
    }
}
