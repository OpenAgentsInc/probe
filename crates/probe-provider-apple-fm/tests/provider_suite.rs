use std::time::Duration;

use probe_provider_apple_fm::{
    AppleFmProviderClient, AppleFmProviderConfig, AppleFmProviderError, AppleFmProviderMessage,
};
use probe_test_support::{FakeAppleFmServer, FakeHttpResponse};
use serde_json::json;

#[test]
fn provider_suite_executes_plain_text_completion_against_bridge() {
    let server = FakeAppleFmServer::from_json_responses(vec![json!({
        "id": "apple_provider_suite",
        "model": "apple-foundation-model",
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": "hello from apple fm"},
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "total_tokens_detail": {"value": 11, "truth": "estimated"}
        }
    })]);
    let client = AppleFmProviderClient::new(AppleFmProviderConfig {
        base_url: server.base_url().to_string(),
        model: String::from("apple-foundation-model"),
        timeout: Duration::from_secs(5),
    })
    .expect("client");

    let response = client
        .chat_completion(vec![AppleFmProviderMessage::user("hello")])
        .expect("completion");

    assert_eq!(
        response.assistant_text.as_deref(),
        Some("hello from apple fm")
    );
    let requests = server.finish();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("POST /v1/chat/completions HTTP/1.1"));
}

#[test]
fn provider_suite_surfaces_typed_foundation_models_errors() {
    let server = FakeAppleFmServer::from_responses(vec![FakeHttpResponse::json_status(
        503,
        json!({
            "error": {
                "message": "Apple Intelligence is not enabled",
                "type": "assets_unavailable",
                "code": "assets_unavailable",
                "failure_reason": "Apple Intelligence is disabled",
                "recovery_suggestion": "Enable Apple Intelligence and retry"
            }
        }),
    )]);
    let client = AppleFmProviderClient::new(AppleFmProviderConfig {
        base_url: server.base_url().to_string(),
        model: String::from("apple-foundation-model"),
        timeout: Duration::from_secs(5),
    })
    .expect("client");

    let error = client
        .chat_completion(vec![AppleFmProviderMessage::user("hello")])
        .expect_err("expected typed error");

    assert!(matches!(error, AppleFmProviderError::Request(_)));
    let typed = error
        .foundation_models_error()
        .expect("typed foundation models error");
    assert_eq!(typed.kind.label(), "assets_unavailable");
    assert_eq!(
        typed.recovery_suggestion.as_deref(),
        Some("Enable Apple Intelligence and retry")
    );
}
