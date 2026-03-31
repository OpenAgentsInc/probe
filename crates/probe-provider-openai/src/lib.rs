use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::io::{BufRead, BufReader};
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

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub model: String,
    pub choices: Vec<ChatCompletionChunkChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ChatCompletionUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ChatCompletionChunkChoice {
    pub index: u32,
    #[serde(default)]
    pub delta: ChatCompletionChunkDelta,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct ChatCompletionChunkDelta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ChatCompletionChunkToolCall>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ChatCompletionChunkToolCall {
    pub index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<ChatCompletionChunkToolCallFunctionDelta>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ChatCompletionChunkToolCallFunctionDelta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

#[derive(Debug)]
pub enum OpenAiProviderError {
    BuildClient(String),
    Transport(String),
    HttpStatus { status: u16, body: String },
    InvalidStreamContentType { content_type: String },
    Decode(String),
}

impl Display for OpenAiProviderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BuildClient(message) => write!(f, "failed to build http client: {message}"),
            Self::Transport(message) => write!(f, "request transport failed: {message}"),
            Self::HttpStatus { status, body } => {
                write!(f, "backend returned http {status}: {body}")
            }
            Self::InvalidStreamContentType { content_type } => {
                write!(
                    f,
                    "backend returned non-stream content type for streaming request: {content_type}"
                )
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
        self.send_chat_completion_with_callback(request, |_| {})
    }

    pub fn send_chat_completion_with_callback(
        &self,
        request: &ChatCompletionRequest,
        on_chunk: impl FnMut(&ChatCompletionChunk),
    ) -> Result<ChatCompletionResponse, OpenAiProviderError> {
        if request.stream {
            return self.send_streaming_chat_completion(request, on_chunk);
        }

        self.send_non_streaming_chat_completion(request)
    }

    fn send_non_streaming_chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, OpenAiProviderError> {
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

    fn send_streaming_chat_completion(
        &self,
        request: &ChatCompletionRequest,
        mut on_chunk: impl FnMut(&ChatCompletionChunk),
    ) -> Result<ChatCompletionResponse, OpenAiProviderError> {
        let response = self
            .http
            .post(self.config.chat_completions_endpoint())
            .bearer_auth(self.config.api_key.as_str())
            .header("accept", "text/event-stream")
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

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        if !content_type.starts_with("text/event-stream") {
            return if content_type.starts_with("application/json") {
                response
                    .json()
                    .map_err(|error| OpenAiProviderError::Decode(error.to_string()))
            } else {
                Err(OpenAiProviderError::InvalidStreamContentType { content_type })
            };
        }

        let mut accumulator = ChatCompletionStreamAccumulator::default();
        let mut reader = BufReader::new(response);
        let mut line = String::new();
        let mut event_data = Vec::new();
        loop {
            line.clear();
            let bytes = reader
                .read_line(&mut line)
                .map_err(|error| OpenAiProviderError::Transport(error.to_string()))?;
            if bytes == 0 {
                break;
            }

            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                if !event_data.is_empty() {
                    let payload = event_data.join("\n");
                    if payload == "[DONE]" {
                        break;
                    }
                    let chunk: ChatCompletionChunk = serde_json::from_str(payload.as_str())
                        .map_err(|error| OpenAiProviderError::Decode(error.to_string()))?;
                    on_chunk(&chunk);
                    accumulator.ingest(chunk)?;
                    event_data.clear();
                }
                continue;
            }

            if let Some(data) = trimmed.strip_prefix("data:") {
                event_data.push(data.trim_start().to_string());
            }
        }

        if !event_data.is_empty() {
            let payload = event_data.join("\n");
            if payload != "[DONE]" {
                let chunk: ChatCompletionChunk = serde_json::from_str(payload.as_str())
                    .map_err(|error| OpenAiProviderError::Decode(error.to_string()))?;
                on_chunk(&chunk);
                accumulator.ingest(chunk)?;
            }
        }

        accumulator.finalize()
    }
}

#[derive(Default)]
struct ChatCompletionStreamAccumulator {
    response_id: Option<String>,
    response_model: Option<String>,
    usage: Option<ChatCompletionUsage>,
    choices: BTreeMap<u32, ChatCompletionStreamChoiceBuilder>,
}

#[derive(Default)]
struct ChatCompletionStreamChoiceBuilder {
    role: Option<String>,
    content: String,
    tool_calls: BTreeMap<usize, ChatCompletionStreamToolCallBuilder>,
    finish_reason: Option<String>,
}

#[derive(Default)]
struct ChatCompletionStreamToolCallBuilder {
    id: Option<String>,
    kind: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ChatCompletionStreamAccumulator {
    fn ingest(&mut self, chunk: ChatCompletionChunk) -> Result<(), OpenAiProviderError> {
        if self.response_id.is_none() {
            self.response_id = Some(chunk.id.clone());
        }
        if self.response_model.is_none() {
            self.response_model = Some(chunk.model.clone());
        }
        if chunk.usage.is_some() {
            self.usage = chunk.usage.clone();
        }

        for choice in chunk.choices {
            let builder = self.choices.entry(choice.index).or_default();
            if let Some(role) = choice.delta.role {
                builder.role = Some(role);
            }
            if let Some(content) = choice.delta.content {
                builder.content.push_str(content.as_str());
            }
            if let Some(tool_calls) = choice.delta.tool_calls {
                for tool_call in tool_calls {
                    let tool_builder = builder.tool_calls.entry(tool_call.index).or_default();
                    if let Some(id) = tool_call.id {
                        tool_builder.id = Some(id);
                    }
                    if let Some(kind) = tool_call.kind {
                        tool_builder.kind = Some(kind);
                    }
                    if let Some(function) = tool_call.function {
                        if let Some(name) = function.name {
                            tool_builder.name = Some(name);
                        }
                        if let Some(arguments) = function.arguments {
                            tool_builder.arguments.push_str(arguments.as_str());
                        }
                    }
                }
            }
            if choice.finish_reason.is_some() {
                builder.finish_reason = choice.finish_reason;
            }
        }

        Ok(())
    }

    fn finalize(self) -> Result<ChatCompletionResponse, OpenAiProviderError> {
        let id = self
            .response_id
            .ok_or_else(|| OpenAiProviderError::Decode(String::from("stream ended before emitting a response id")))?;
        let model = self
            .response_model
            .ok_or_else(|| OpenAiProviderError::Decode(String::from("stream ended before emitting a response model")))?;
        let mut choices = Vec::new();
        for (index, builder) in self.choices {
            let mut tool_calls = Vec::new();
            for (_tool_index, tool_builder) in builder.tool_calls {
                let id = tool_builder.id.ok_or_else(|| {
                    OpenAiProviderError::Decode(String::from(
                        "streamed tool call ended before emitting a call id",
                    ))
                })?;
                let kind = tool_builder
                    .kind
                    .unwrap_or_else(|| String::from("function"));
                let name = tool_builder.name.ok_or_else(|| {
                    OpenAiProviderError::Decode(String::from(
                        "streamed tool call ended before emitting a function name",
                    ))
                })?;
                tool_calls.push(ChatToolCall {
                    id,
                    kind,
                    function: ChatToolCallFunction {
                        name,
                        arguments: tool_builder.arguments,
                    },
                });
            }
            choices.push(ChatCompletionChoice {
                index,
                message: ChatMessage {
                    role: builder.role.unwrap_or_else(|| String::from("assistant")),
                    content: (!builder.content.is_empty()).then_some(builder.content),
                    name: None,
                    tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
                    tool_call_id: None,
                },
                finish_reason: builder.finish_reason,
            });
        }

        Ok(ChatCompletionResponse {
            id,
            model,
            choices,
            usage: self.usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use probe_test_support::{FakeHttpResponse, FakeOpenAiServer};

    use super::{
        ChatCompletionChunk, ChatCompletionResponse, ChatMessage, ChatToolCall,
        ChatToolCallFunction, OpenAiProviderClient, OpenAiProviderConfig,
        OpenAiProviderError,
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

    fn streaming_config(base_url: &str) -> OpenAiProviderConfig {
        OpenAiProviderConfig {
            base_url: base_url.to_string(),
            model: String::from("tiny-qwen35"),
            api_key: String::from("dummy"),
            timeout: Duration::from_secs(5),
            stream: true,
        }
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
                                    "name": "read_file",
                                    "arguments": "{\"path\":\"README.md\"}"
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
            .chat_completion(vec![ChatMessage::user("inspect README.md")])
            .expect("tool call response");

        let tool_calls = response.first_tool_calls().expect("tool calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "read_file");
        assert_eq!(tool_calls[0].function.arguments, "{\"path\":\"README.md\"}");
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("inspect README.md"));
    }

    #[test]
    fn assistant_tool_call_messages_are_constructible() {
        let message = ChatMessage::assistant_tool_calls(vec![ChatToolCall {
            id: String::from("call_1"),
            kind: String::from("function"),
            function: ChatToolCallFunction {
                name: String::from("read_file"),
                arguments: String::from("{\"path\":\"README.md\"}"),
            },
        }]);

        assert_eq!(message.role, "assistant");
        assert!(message.content.is_none());
        assert_eq!(
            message.tool_calls.as_ref().expect("tool calls")[0]
                .function
                .name,
            "read_file"
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
    fn streaming_plain_text_requests_are_assembled_from_sse_chunks() {
        let body = concat!(
            "data: {\"id\":\"chatcmpl_stream_text\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hello\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_stream_text\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_stream_text\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n"
        );
        let server = FakeOpenAiServer::from_responses(vec![FakeHttpResponse::text_event_stream(
            200, body,
        )]);
        let client =
            OpenAiProviderClient::new(streaming_config(server.base_url())).expect("client");
        let mut seen_chunks = Vec::<ChatCompletionChunk>::new();

        let response = client
            .send_chat_completion_with_callback(
                &super::ChatCompletionRequest::from_config(
                    client.config(),
                    vec![ChatMessage::user("hello")],
                ),
                |chunk| seen_chunks.push(chunk.clone()),
            )
            .expect("streaming chat completion");

        assert_eq!(seen_chunks.len(), 3);
        assert_eq!(response.first_message_text(), Some("hello world"));
        assert_eq!(
            response.usage.expect("usage should be preserved").total_tokens,
            5
        );
    }

    #[test]
    fn streaming_tool_call_requests_are_assembled_from_sse_chunks() {
        let body = concat!(
            "data: {\"id\":\"chatcmpl_stream_tool\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_stream_tool\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"README.md\\\"}\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_stream_tool\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let server = FakeOpenAiServer::from_responses(vec![FakeHttpResponse::text_event_stream(
            200, body,
        )]);
        let client =
            OpenAiProviderClient::new(streaming_config(server.base_url())).expect("client");

        let response = client
            .chat_completion(vec![ChatMessage::user("inspect README.md")])
            .expect("streaming tool call response");

        let tool_calls = response.first_tool_calls().expect("tool calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_1");
        assert_eq!(tool_calls[0].function.name, "read_file");
        assert_eq!(tool_calls[0].function.arguments, "{\"path\":\"README.md\"}");
    }

    #[test]
    fn streaming_requests_fall_back_to_non_streaming_when_backend_returns_json() {
        let server = FakeOpenAiServer::from_responses(vec![FakeHttpResponse::json_ok(
            serde_json::json!({
                "id": "chatcmpl_fallback",
                "model": "tiny-qwen35",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "fallback response"},
                    "finish_reason": "stop"
                }]
            }),
        )]);
        let client =
            OpenAiProviderClient::new(streaming_config(server.base_url())).expect("client");

        let response = client
            .chat_completion(vec![ChatMessage::assistant("hello")])
            .expect("streaming request should fall back to non-streaming");
        assert_eq!(response.first_message_text(), Some("fallback response"));
    }
}
