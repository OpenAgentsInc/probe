use std::time::Duration;

use std::collections::BTreeMap;

use probe_provider_openai::{
    ChatMessage, OpenAiProviderClient, OpenAiProviderConfig, OpenAiProviderError,
    OpenAiRequestAuth, OpenAiTransport,
};
use probe_test_support::{FakeHttpResponse, FakeOpenAiServer};
use serde_json::json;

#[test]
fn provider_suite_executes_plain_text_chat_completion() {
    let server = FakeOpenAiServer::from_json_responses(vec![json!({
        "id": "chatcmpl_provider_suite",
        "model": "tiny-qwen35",
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": "hello from provider suite"},
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 3,
            "completion_tokens": 4,
            "total_tokens": 7
        }
    })]);
    let client = OpenAiProviderClient::new(OpenAiProviderConfig {
        base_url: server.base_url().to_string(),
        model: String::from("tiny-qwen35"),
        reasoning_level: None,
        auth: OpenAiRequestAuth::BearerToken(String::from("dummy")),
        timeout: Duration::from_secs(5),
        stream: false,
        max_tokens: Some(OpenAiProviderConfig::DEFAULT_MAX_TOKENS),
        transport: OpenAiTransport::ChatCompletions,
        extra_headers: BTreeMap::new(),
    })
    .expect("client");

    let response = client
        .chat_completion(vec![ChatMessage::user("hello")])
        .expect("chat completion");

    assert_eq!(
        response.first_message_text(),
        Some("hello from provider suite")
    );
    let requests = server.finish();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("POST /v1/chat/completions"));
}

#[test]
fn provider_suite_surfaces_http_status_errors_with_body_context() {
    let server = FakeOpenAiServer::from_responses(vec![FakeHttpResponse::json_status(
        400,
        json!({"error": "bad request"}),
    )]);
    let client = OpenAiProviderClient::new(OpenAiProviderConfig {
        base_url: server.base_url().to_string(),
        model: String::from("tiny-qwen35"),
        reasoning_level: None,
        auth: OpenAiRequestAuth::BearerToken(String::from("dummy")),
        timeout: Duration::from_secs(5),
        stream: false,
        max_tokens: Some(OpenAiProviderConfig::DEFAULT_MAX_TOKENS),
        transport: OpenAiTransport::ChatCompletions,
        extra_headers: BTreeMap::new(),
    })
    .expect("client");

    let error = client
        .chat_completion(vec![ChatMessage::system("fail")])
        .expect_err("expected http status error");

    match error {
        OpenAiProviderError::HttpStatus { status, body } => {
            assert_eq!(status, 400);
            assert!(body.contains("bad request"));
        }
        other => panic!("unexpected error: {other}"),
    }
}
