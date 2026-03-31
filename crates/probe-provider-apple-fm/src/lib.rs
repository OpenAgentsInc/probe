use std::fmt::{Display, Formatter};
use std::time::Duration;

use probe_protocol::backend::BackendProfile;
use psionic_apple_fm::{
    AppleFmBridgeClient, AppleFmBridgeClientError, AppleFmChatCompletionRequest,
    AppleFmChatMessage, AppleFmChatMessageRole, AppleFmChatUsage, AppleFmFoundationModelsError,
    AppleFmSystemLanguageModelAvailability,
};
use reqwest::blocking::Client;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppleFmProviderConfig {
    pub base_url: String,
    pub model: String,
    pub timeout: Duration,
}

impl AppleFmProviderConfig {
    #[must_use]
    pub fn localhost(model: impl Into<String>) -> Self {
        Self {
            base_url: String::from("http://127.0.0.1:8081"),
            model: model.into(),
            timeout: Duration::from_secs(30),
        }
    }

    #[must_use]
    pub fn from_backend_profile(profile: &BackendProfile) -> Self {
        Self {
            base_url: profile.base_url.clone(),
            model: profile.model.clone(),
            timeout: Duration::from_secs(profile.timeout_secs),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppleFmProviderMessageRole {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppleFmProviderMessage {
    pub role: AppleFmProviderMessageRole,
    pub content: String,
}

impl AppleFmProviderMessage {
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: AppleFmProviderMessageRole::System,
            content: content.into(),
        }
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: AppleFmProviderMessageRole::User,
            content: content.into(),
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: AppleFmProviderMessageRole::Assistant,
            content: content.into(),
        }
    }

    fn into_bridge_message(self) -> AppleFmChatMessage {
        AppleFmChatMessage {
            role: match self.role {
                AppleFmProviderMessageRole::System => AppleFmChatMessageRole::System,
                AppleFmProviderMessageRole::User => AppleFmChatMessageRole::User,
                AppleFmProviderMessageRole::Assistant => AppleFmChatMessageRole::Assistant,
            },
            content: self.content,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppleFmProviderResponse {
    pub id: String,
    pub model: String,
    pub assistant_text: Option<String>,
    pub usage: Option<AppleFmChatUsage>,
}

#[derive(Debug)]
pub enum AppleFmProviderError {
    Build(String),
    Request(AppleFmBridgeClientError),
}

impl AppleFmProviderError {
    #[must_use]
    pub fn foundation_models_error(&self) -> Option<&AppleFmFoundationModelsError> {
        match self {
            Self::Request(error) => error.foundation_models_error(),
            Self::Build(_) => None,
        }
    }
}

impl Display for AppleFmProviderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Build(message) => write!(
                f,
                "failed to initialize Apple FM provider client: {message}"
            ),
            Self::Request(error) => {
                write!(f, "{error}")?;
                if let Some(typed) = error.foundation_models_error() {
                    write!(
                        f,
                        " [kind={} retryable={}]",
                        typed.kind.label(),
                        typed.is_retryable()
                    )?;
                    if let Some(reason) = typed.failure_reason.as_deref() {
                        write!(f, " failure_reason={reason}")?;
                    }
                    if let Some(suggestion) = typed.recovery_suggestion.as_deref() {
                        write!(f, " recovery_suggestion={suggestion}")?;
                    }
                    if let Some(explanation) = typed.refusal_explanation.as_deref() {
                        write!(f, " refusal_explanation={explanation}")?;
                    }
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for AppleFmProviderError {}

pub struct AppleFmProviderClient {
    config: AppleFmProviderConfig,
    client: AppleFmBridgeClient,
}

impl AppleFmProviderClient {
    pub fn new(config: AppleFmProviderConfig) -> Result<Self, AppleFmProviderError> {
        let http = Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|error| AppleFmProviderError::Build(error.to_string()))?;
        let client = AppleFmBridgeClient::with_http_client(config.base_url.clone(), http)
            .map_err(AppleFmProviderError::Request)?;
        Ok(Self { config, client })
    }

    #[must_use]
    pub fn config(&self) -> &AppleFmProviderConfig {
        &self.config
    }

    pub fn system_model_availability(
        &self,
    ) -> Result<AppleFmSystemLanguageModelAvailability, AppleFmProviderError> {
        self.client
            .system_model_availability()
            .map_err(AppleFmProviderError::Request)
    }

    pub fn chat_completion(
        &self,
        messages: Vec<AppleFmProviderMessage>,
    ) -> Result<AppleFmProviderResponse, AppleFmProviderError> {
        let request = AppleFmChatCompletionRequest {
            model: Some(self.config.model.clone()),
            messages: messages
                .into_iter()
                .map(AppleFmProviderMessage::into_bridge_message)
                .collect(),
            temperature: None,
            max_tokens: None,
            options: None,
            adapter: None,
            stream: false,
        };
        let response = self
            .client
            .chat_completion(&request)
            .map_err(AppleFmProviderError::Request)?;
        Ok(AppleFmProviderResponse {
            id: response
                .id
                .clone()
                .unwrap_or_else(|| String::from("apple_fm_chat_completion")),
            model: response.model.clone(),
            assistant_text: response.first_text_content().map(ToOwned::to_owned),
            usage: response.usage.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use probe_test_support::{FakeAppleFmServer, FakeHttpResponse};
    use serde_json::json;

    use super::{
        AppleFmProviderClient, AppleFmProviderConfig, AppleFmProviderError, AppleFmProviderMessage,
    };

    #[test]
    fn localhost_helper_uses_local_default() {
        let config = AppleFmProviderConfig::localhost("apple-foundation-model");
        assert_eq!(config.base_url, "http://127.0.0.1:8081");
        assert_eq!(config.model, "apple-foundation-model");
    }

    #[test]
    fn config_can_be_built_from_backend_profile() {
        let profile = BackendProfile {
            name: String::from("psionic-apple-fm-bridge"),
            kind: BackendKind::AppleFmBridge,
            base_url: String::from("http://127.0.0.1:8081"),
            model: String::from("apple-foundation-model"),
            api_key_env: String::new(),
            timeout_secs: 45,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
        };
        let config = AppleFmProviderConfig::from_backend_profile(&profile);
        assert_eq!(config.base_url, "http://127.0.0.1:8081");
        assert_eq!(config.model, "apple-foundation-model");
        assert_eq!(config.timeout, Duration::from_secs(45));
    }

    #[test]
    fn client_executes_plain_text_completion_against_local_bridge() {
        let server = FakeAppleFmServer::from_json_responses(vec![json!({
            "id": "apple_fm_test",
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

        assert_eq!(response.id, "apple_fm_test");
        assert_eq!(
            response.assistant_text.as_deref(),
            Some("hello from apple fm")
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("POST /v1/chat/completions HTTP/1.1"));
    }

    #[test]
    fn client_surfaces_typed_foundation_models_errors() {
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
            .expect_err("request should fail");

        match error {
            AppleFmProviderError::Request(inner) => {
                let typed = inner.foundation_models_error().expect("typed error");
                assert_eq!(typed.kind.label(), "assets_unavailable");
                assert!(typed.is_retryable());
            }
            other => panic!("unexpected error: {other}"),
        }
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
    }

    #[test]
    fn client_reports_system_model_availability() {
        let server = FakeAppleFmServer::from_responses(vec![FakeHttpResponse::json_status(
            200,
            json!({
                "status": "ok",
                "model_available": true,
                "version": "1.0",
                "platform": "macOS"
            }),
        )]);

        let client = AppleFmProviderClient::new(AppleFmProviderConfig {
            base_url: server.base_url().to_string(),
            model: String::from("apple-foundation-model"),
            timeout: Duration::from_secs(5),
        })
        .expect("client");
        let availability = client
            .system_model_availability()
            .expect("availability should succeed");

        assert!(availability.available);
        assert_eq!(availability.model.id, "apple-foundation-model");
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("GET /health HTTP/1.1"));
    }
}
