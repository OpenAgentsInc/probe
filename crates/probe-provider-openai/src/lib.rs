use std::fmt::{Display, Formatter};
use std::time::Duration;

use probe_protocol::backend::BackendProfile;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiProviderConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub timeout: Duration,
    pub stream: bool,
}

impl OpenAiProviderConfig {
    #[must_use]
    pub fn localhost(model: impl Into<String>) -> Self {
        Self {
            base_url: String::from("http://127.0.0.1:8080/v1"),
            model: model.into(),
            api_key: String::from("dummy"),
            timeout: Duration::from_secs(30),
            stream: false,
        }
    }

    #[must_use]
    pub fn chat_completions_endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    #[must_use]
    pub fn from_backend_profile(profile: &BackendProfile) -> Self {
        Self {
            base_url: profile.base_url.clone(),
            model: profile.model.clone(),
            api_key: String::from("dummy"),
            timeout: Duration::from_secs(profile.timeout_secs),
            stream: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ChatToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: String::from("system"),
            content: Some(content.into()),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: String::from("user"),
            content: Some(content.into()),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: String::from("assistant"),
            content: Some(content.into()),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[must_use]
    pub fn assistant_tool_calls(tool_calls: Vec<ChatToolCall>) -> Self {
        Self {
            role: String::from("assistant"),
            content: None,
            name: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }

    #[must_use]
    pub fn tool(
        name: impl Into<String>,
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: String::from("tool"),
            content: Some(content.into()),
            name: Some(name.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatToolDefinitionEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ChatToolDefinition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatToolDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChatToolChoice {
    Mode(String),
    Named(ChatNamedToolChoice),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatNamedToolChoice {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ChatNamedToolChoiceFunction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatNamedToolChoiceFunction {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ChatToolCallFunction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ChatToolDefinitionEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ChatToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
}

impl ChatCompletionRequest {
    #[must_use]
    pub fn from_config(config: &OpenAiProviderConfig, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: config.model.clone(),
            messages,
            stream: config.stream,
            tools: Vec::new(),
            tool_choice: None,
            parallel_tool_calls: None,
        }
    }

    #[must_use]
    pub fn with_tools(
        mut self,
        tools: Vec<ChatToolDefinitionEnvelope>,
        tool_choice: Option<ChatToolChoice>,
        parallel_tool_calls: Option<bool>,
    ) -> Self {
        self.tools = tools;
        self.tool_choice = tool_choice;
        self.parallel_tool_calls = parallel_tool_calls;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ChatCompletionChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ChatCompletionUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub model: String,
    pub choices: Vec<ChatCompletionChoice>,
    pub usage: Option<ChatCompletionUsage>,
}

impl ChatCompletionResponse {
    #[must_use]
    pub fn first_message_text(&self) -> Option<&str> {
        self.choices
            .first()
            .and_then(|choice| choice.message.content.as_deref())
    }

    #[must_use]
    pub fn first_tool_calls(&self) -> Option<&[ChatToolCall]> {
        self.choices
            .first()
            .and_then(|choice| choice.message.tool_calls.as_deref())
    }
}

#[derive(Debug)]
pub enum OpenAiProviderError {
    UnsupportedStreaming,
    BuildClient(String),
    Transport(String),
    HttpStatus { status: u16, body: String },
    Decode(String),
}

impl Display for OpenAiProviderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedStreaming => {
                write!(
                    f,
                    "streaming is not implemented in the initial provider client"
                )
            }
            Self::BuildClient(message) => write!(f, "failed to build http client: {message}"),
            Self::Transport(message) => write!(f, "request transport failed: {message}"),
            Self::HttpStatus { status, body } => {
                write!(f, "backend returned http {status}: {body}")
            }
            Self::Decode(message) => write!(f, "failed to decode backend response: {message}"),
        }
    }
}

impl std::error::Error for OpenAiProviderError {}

pub struct OpenAiProviderClient {
    config: OpenAiProviderConfig,
    http: Client,
}

impl OpenAiProviderClient {
    pub fn new(config: OpenAiProviderConfig) -> Result<Self, OpenAiProviderError> {
        let http = Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|error| OpenAiProviderError::BuildClient(error.to_string()))?;
        Ok(Self { config, http })
    }

    #[must_use]
    pub fn config(&self) -> &OpenAiProviderConfig {
        &self.config
    }

    pub fn chat_completion(
        &self,
        messages: Vec<ChatMessage>,
    ) -> Result<ChatCompletionResponse, OpenAiProviderError> {
        let request = ChatCompletionRequest::from_config(&self.config, messages);
        self.send_chat_completion(&request)
    }

    pub fn send_chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, OpenAiProviderError> {
        if request.stream {
            return Err(OpenAiProviderError::UnsupportedStreaming);
        }

        let response = self
            .http
            .post(self.config.chat_completions_endpoint())
            .bearer_auth(self.config.api_key.as_str())
            .json(request)
            .send()
            .map_err(|error| OpenAiProviderError::Transport(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .unwrap_or_else(|error| format!("failed to read error body: {error}"));
            return Err(OpenAiProviderError::HttpStatus {
                status: status.as_u16(),
                body,
            });
        }

        response
            .json()
            .map_err(|error| OpenAiProviderError::Decode(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use probe_test_support::{FakeHttpResponse, FakeOpenAiServer};

    use super::{
        ChatCompletionResponse, ChatMessage, ChatToolCall, ChatToolCallFunction,
        OpenAiProviderClient, OpenAiProviderConfig, OpenAiProviderError,
    };

    #[test]
    fn localhost_helper_uses_local_default() {
        let config = OpenAiProviderConfig::localhost("example.gguf");
        assert_eq!(config.base_url, "http://127.0.0.1:8080/v1");
        assert_eq!(config.api_key, "dummy");
        assert_eq!(
            config.chat_completions_endpoint(),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
    }

    #[test]
    fn config_can_be_built_from_backend_profile() {
        let profile = BackendProfile {
            name: String::from("psionic-qwen35-2b-q8-registry"),
            kind: BackendKind::OpenAiChatCompletions,
            base_url: String::from("http://127.0.0.1:8080/v1"),
            model: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            api_key_env: String::from("PROBE_OPENAI_API_KEY"),
            timeout_secs: 45,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
        };
        let config = OpenAiProviderConfig::from_backend_profile(&profile);
        assert_eq!(config.base_url, "http://127.0.0.1:8080/v1");
        assert_eq!(config.model, "qwen3.5-2b-q8_0-registry.gguf");
        assert_eq!(config.timeout, Duration::from_secs(45));
        assert!(!config.stream);
    }

    #[test]
    fn client_executes_plain_text_chat_completion_against_local_endpoint() {
        let server = FakeOpenAiServer::from_json_responses(vec![serde_json::json!({
            "id": "chatcmpl_test",
            "model": "tiny-qwen35",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "hello from backend"},
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 3,
                "completion_tokens": 4,
                "total_tokens": 7
            }
        })]);

        let config = OpenAiProviderConfig {
            base_url: String::from(server.base_url()),
            model: String::from("tiny-qwen35"),
            api_key: String::from("dummy"),
            timeout: std::time::Duration::from_secs(5),
            stream: false,
        };
        let client = OpenAiProviderClient::new(config).expect("client");
        let response: ChatCompletionResponse = client
            .chat_completion(vec![ChatMessage::user("hello")])
            .expect("chat completion");

        assert_eq!(response.first_message_text(), Some("hello from backend"));
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("POST /v1/chat/completions"));
    }

    #[test]
    fn client_decodes_tool_call_responses() {
        let server = FakeOpenAiServer::from_json_responses(vec![serde_json::json!({
            "id": "chatcmpl_tools",
            "model": "tiny-qwen35",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "type": "function",
                                "function": {
                                    "name": "lookup_weather",
                                    "arguments": "{\"city\":\"Paris\"}"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        })]);

        let config = OpenAiProviderConfig {
            base_url: String::from(server.base_url()),
            model: String::from("tiny-qwen35"),
            api_key: String::from("dummy"),
            timeout: Duration::from_secs(5),
            stream: false,
        };
        let client = OpenAiProviderClient::new(config).expect("client");
        let response = client
            .chat_completion(vec![ChatMessage::user("weather in Paris")])
            .expect("tool call response");

        let tool_calls = response.first_tool_calls().expect("tool calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "lookup_weather");
        assert_eq!(tool_calls[0].function.arguments, "{\"city\":\"Paris\"}");
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("weather in Paris"));
    }

    #[test]
    fn assistant_tool_call_messages_are_constructible() {
        let message = ChatMessage::assistant_tool_calls(vec![ChatToolCall {
            id: String::from("call_1"),
            kind: String::from("function"),
            function: ChatToolCallFunction {
                name: String::from("lookup_weather"),
                arguments: String::from("{\"city\":\"Tokyo\"}"),
            },
        }]);

        assert_eq!(message.role, "assistant");
        assert!(message.content.is_none());
        assert_eq!(
            message.tool_calls.as_ref().expect("tool calls")[0]
                .function
                .name,
            "lookup_weather"
        );
    }

    #[test]
    fn http_status_errors_surface_response_body() {
        let server = FakeOpenAiServer::from_responses(vec![FakeHttpResponse::json_status(
            400,
            serde_json::json!({"error": "bad request"}),
        )]);

        let config = OpenAiProviderConfig {
            base_url: String::from(server.base_url()),
            model: String::from("tiny-qwen35"),
            api_key: String::from("dummy"),
            timeout: std::time::Duration::from_secs(5),
            stream: false,
        };
        let client = OpenAiProviderClient::new(config).expect("client");
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
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("\"fail\""));
    }

    #[test]
    fn streaming_requests_fail_explicitly_for_now() {
        let client = OpenAiProviderClient::new(OpenAiProviderConfig {
            base_url: String::from("http://127.0.0.1:8080/v1"),
            model: String::from("tiny-qwen35"),
            api_key: String::from("dummy"),
            timeout: std::time::Duration::from_secs(5),
            stream: true,
        })
        .expect("client");

        let error = client
            .chat_completion(vec![ChatMessage::assistant("hello")])
            .expect_err("streaming should be rejected for now");
        assert!(matches!(error, OpenAiProviderError::UnsupportedStreaming));
    }
}
