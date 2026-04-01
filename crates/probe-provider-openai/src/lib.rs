use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::io::{BufRead, BufReader};
use std::time::Duration;

use probe_protocol::backend::BackendProfile;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

const CODEX_MODEL_ALLOWLIST: &[&str] = &[
    "gpt-5.1-codex",
    "gpt-5.1-codex-max",
    "gpt-5.1-codex-mini",
    "gpt-5.2",
    "gpt-5.2-codex",
    "gpt-5.3-codex",
    "gpt-5.4",
    "gpt-5.4-mini",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenAiRequestAuth {
    None,
    BearerToken(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenAiTransport {
    ChatCompletions,
    CodexResponses,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiProviderConfig {
    pub base_url: String,
    pub model: String,
    pub auth: OpenAiRequestAuth,
    pub timeout: Duration,
    pub stream: bool,
    pub max_tokens: Option<u32>,
    pub transport: OpenAiTransport,
    pub extra_headers: BTreeMap<String, String>,
}

impl OpenAiProviderConfig {
    pub const DEFAULT_MAX_TOKENS: u32 = 1024;

    #[must_use]
    pub fn localhost(model: impl Into<String>) -> Self {
        Self {
            base_url: String::from("http://127.0.0.1:8080/v1"),
            model: model.into(),
            auth: OpenAiRequestAuth::BearerToken(String::from("dummy")),
            timeout: Duration::from_secs(30),
            stream: false,
            max_tokens: Some(Self::DEFAULT_MAX_TOKENS),
            transport: OpenAiTransport::ChatCompletions,
            extra_headers: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn request_endpoint(&self) -> String {
        match self.transport {
            OpenAiTransport::ChatCompletions => {
                format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
            }
            OpenAiTransport::CodexResponses => {
                format!("{}/responses", self.base_url.trim_end_matches('/'))
            }
        }
    }

    #[must_use]
    pub fn chat_completions_endpoint(&self) -> String {
        self.request_endpoint()
    }

    #[must_use]
    pub fn from_backend_profile(profile: &BackendProfile) -> Self {
        Self {
            base_url: profile.base_url.clone(),
            model: profile.model.clone(),
            auth: match profile.kind {
                probe_protocol::backend::BackendKind::OpenAiCodexSubscription => {
                    OpenAiRequestAuth::None
                }
                probe_protocol::backend::BackendKind::OpenAiChatCompletions
                | probe_protocol::backend::BackendKind::AppleFmBridge => {
                    OpenAiRequestAuth::BearerToken(String::from("dummy"))
                }
            },
            timeout: Duration::from_secs(profile.timeout_secs),
            stream: false,
            max_tokens: Some(Self::DEFAULT_MAX_TOKENS),
            transport: match profile.kind {
                probe_protocol::backend::BackendKind::OpenAiCodexSubscription => {
                    OpenAiTransport::CodexResponses
                }
                probe_protocol::backend::BackendKind::OpenAiChatCompletions
                | probe_protocol::backend::BackendKind::AppleFmBridge => {
                    OpenAiTransport::ChatCompletions
                }
            },
            extra_headers: BTreeMap::new(),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
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
            max_tokens: config.max_tokens,
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

#[derive(Clone, Debug, PartialEq, Serialize)]
struct CodexResponsesRequest {
    pub model: String,
    pub instructions: String,
    pub input: Vec<CodexInputItem>,
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<CodexToolDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<CodexToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    pub store: bool,
}

impl CodexResponsesRequest {
    #[must_use]
    fn from_chat_completion(request: &ChatCompletionRequest) -> Self {
        let mut instructions = Vec::new();
        let mut input = Vec::new();
        for message in &request.messages {
            match message.role.as_str() {
                "system" => {
                    if let Some(content) = message
                        .content
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        instructions.push(content.to_string());
                    }
                }
                "user" => {
                    if let Some(content) = message.content.clone() {
                        input.push(CodexInputItem::Message(CodexMessage {
                            role: String::from("user"),
                            content: vec![CodexMessageContent::InputText { text: content }],
                        }));
                    }
                }
                "assistant" => {
                    if let Some(content) = message.content.clone() {
                        input.push(CodexInputItem::Message(CodexMessage {
                            role: String::from("assistant"),
                            content: vec![CodexMessageContent::OutputText { text: content }],
                        }));
                    }
                    if let Some(tool_calls) = &message.tool_calls {
                        for tool_call in tool_calls {
                            input.push(CodexInputItem::FunctionCall(CodexFunctionCall {
                                kind: String::from("function_call"),
                                call_id: tool_call.id.clone(),
                                name: tool_call.function.name.clone(),
                                arguments: tool_call.function.arguments.clone(),
                            }));
                        }
                    }
                }
                "tool" => {
                    if let Some(tool_call_id) = message.tool_call_id.clone() {
                        input.push(CodexInputItem::FunctionCallOutput(
                            CodexFunctionCallOutput {
                                kind: String::from("function_call_output"),
                                call_id: tool_call_id,
                                output: message.content.clone().unwrap_or_default(),
                            },
                        ));
                    }
                }
                _ => {}
            }
        }

        Self {
            model: request.model.clone(),
            instructions: instructions.join("\n\n"),
            input,
            stream: request.stream,
            max_output_tokens: None,
            tools: request
                .tools
                .iter()
                .map(CodexToolDefinition::from_chat_tool)
                .collect(),
            tool_choice: request.tool_choice.as_ref().map(CodexToolChoice::from_chat),
            parallel_tool_calls: request.parallel_tool_calls,
            store: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
enum CodexInputItem {
    Message(CodexMessage),
    FunctionCall(CodexFunctionCall),
    FunctionCallOutput(CodexFunctionCallOutput),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct CodexMessage {
    pub role: String,
    pub content: Vec<CodexMessageContent>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type")]
enum CodexMessageContent {
    #[serde(rename = "input_text")]
    InputText { text: String },
    #[serde(rename = "output_text")]
    OutputText { text: String },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct CodexFunctionCall {
    #[serde(rename = "type")]
    pub kind: String,
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct CodexFunctionCallOutput {
    #[serde(rename = "type")]
    pub kind: String,
    pub call_id: String,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct CodexToolDefinition {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

impl CodexToolDefinition {
    #[must_use]
    fn from_chat_tool(tool: &ChatToolDefinitionEnvelope) -> Self {
        Self {
            kind: String::from("function"),
            name: tool.function.name.clone(),
            description: tool.function.description.clone(),
            parameters: tool.function.parameters.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
enum CodexToolChoice {
    Mode(String),
    Named(CodexNamedToolChoice),
}

impl CodexToolChoice {
    #[must_use]
    fn from_chat(choice: &ChatToolChoice) -> Self {
        match choice {
            ChatToolChoice::Mode(mode) => Self::Mode(mode.clone()),
            ChatToolChoice::Named(named) => Self::Named(CodexNamedToolChoice {
                kind: String::from("function"),
                name: named.function.name.clone(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct CodexNamedToolChoice {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct CodexResponsesApiResponse {
    pub id: String,
    pub model: String,
    #[serde(default)]
    pub output: Vec<CodexResponseOutputItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<CodexResponsesUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type")]
enum CodexResponseOutputItem {
    #[serde(rename = "message")]
    Message {
        #[serde(default)]
        content: Vec<CodexResponseMessageContent>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type")]
enum CodexResponseMessageContent {
    #[serde(rename = "output_text")]
    OutputText { text: String },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct CodexResponsesUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type")]
enum CodexResponseEvent {
    #[serde(rename = "response.created")]
    ResponseCreated {
        response: CodexStreamResponseCreated,
    },
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded { item: CodexStreamOutputItem },
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta { item_id: String, delta: String },
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta { item_id: String, delta: String },
    #[serde(rename = "response.completed")]
    ResponseCompleted {
        response: CodexStreamResponseCompleted,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct CodexStreamResponseCreated {
    pub id: String,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct CodexStreamResponseCompleted {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<CodexResponsesUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type")]
enum CodexStreamOutputItem {
    #[serde(rename = "message")]
    Message { id: String },
    #[serde(rename = "function_call")]
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
    },
    #[serde(other)]
    Unknown,
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
    UnsupportedCodexModel { model: String },
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
            Self::UnsupportedCodexModel { model } => {
                write!(
                    f,
                    "model `{model}` is not allowed for Codex subscription transport"
                )
            }
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
        match self.config.transport {
            OpenAiTransport::ChatCompletions => {
                if request.stream {
                    return self.send_streaming_chat_completion(request, on_chunk);
                }
                self.send_non_streaming_chat_completion(request)
            }
            OpenAiTransport::CodexResponses => {
                if request.stream {
                    return self.send_streaming_codex_response(request, on_chunk);
                }
                self.send_non_streaming_codex_response(request)
            }
        }
    }

    fn send_non_streaming_chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, OpenAiProviderError> {
        self.validate_request()?;
        let response = self
            .request_builder(self.http.post(self.config.request_endpoint()))
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

    fn send_non_streaming_codex_response(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, OpenAiProviderError> {
        self.validate_request()?;
        let request = CodexResponsesRequest::from_chat_completion(request);
        let response = self
            .request_builder(self.http.post(self.config.request_endpoint()))
            .json(&request)
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

        let response: CodexResponsesApiResponse = response
            .json()
            .map_err(|error| OpenAiProviderError::Decode(error.to_string()))?;
        Ok(chat_completion_from_codex_response(response))
    }

    fn send_streaming_chat_completion(
        &self,
        request: &ChatCompletionRequest,
        mut on_chunk: impl FnMut(&ChatCompletionChunk),
    ) -> Result<ChatCompletionResponse, OpenAiProviderError> {
        self.validate_request()?;
        let response = self
            .request_builder(
                self.http
                    .post(self.config.request_endpoint())
                    .header("accept", "text/event-stream"),
            )
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

    fn send_streaming_codex_response(
        &self,
        request: &ChatCompletionRequest,
        mut on_chunk: impl FnMut(&ChatCompletionChunk),
    ) -> Result<ChatCompletionResponse, OpenAiProviderError> {
        self.validate_request()?;
        let request = CodexResponsesRequest::from_chat_completion(request);
        let response = self
            .request_builder(
                self.http
                    .post(self.config.request_endpoint())
                    .header("accept", "text/event-stream"),
            )
            .json(&request)
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
        if content_type.starts_with("application/json") {
            let response: CodexResponsesApiResponse = response
                .json()
                .map_err(|error| OpenAiProviderError::Decode(error.to_string()))?;
            return Ok(chat_completion_from_codex_response(response));
        }

        let mut accumulator = ChatCompletionStreamAccumulator::default();
        let mut codex_state = CodexStreamState::default();
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
                    let event: CodexResponseEvent = serde_json::from_str(payload.as_str())
                        .map_err(|error| OpenAiProviderError::Decode(error.to_string()))?;
                    if let Some(chunk) = codex_state.chunk_from_event(event)? {
                        on_chunk(&chunk);
                        accumulator.ingest(chunk)?;
                    }
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
                let event: CodexResponseEvent = serde_json::from_str(payload.as_str())
                    .map_err(|error| OpenAiProviderError::Decode(error.to_string()))?;
                if let Some(chunk) = codex_state.chunk_from_event(event)? {
                    on_chunk(&chunk);
                    accumulator.ingest(chunk)?;
                }
            }
        }

        accumulator.finalize()
    }

    fn validate_request(&self) -> Result<(), OpenAiProviderError> {
        if self.config.transport == OpenAiTransport::CodexResponses
            && !model_allowed_for_codex(self.config.model.as_str())
        {
            return Err(OpenAiProviderError::UnsupportedCodexModel {
                model: self.config.model.clone(),
            });
        }
        Ok(())
    }

    fn request_builder(
        &self,
        builder: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        let builder = match &self.config.auth {
            OpenAiRequestAuth::None => builder,
            OpenAiRequestAuth::BearerToken(token) => builder.bearer_auth(token),
        };
        self.config
            .extra_headers
            .iter()
            .fold(builder, |builder, (name, value)| {
                builder.header(name, value)
            })
    }
}

fn model_allowed_for_codex(model: &str) -> bool {
    model.contains("codex") || CODEX_MODEL_ALLOWLIST.contains(&model)
}

fn chat_completion_from_codex_response(
    response: CodexResponsesApiResponse,
) -> ChatCompletionResponse {
    let mut assistant_text = String::new();
    let mut tool_calls = Vec::new();
    for item in response.output {
        match item {
            CodexResponseOutputItem::Message { content } => {
                for part in content {
                    if let CodexResponseMessageContent::OutputText { text } = part {
                        assistant_text.push_str(text.as_str());
                    }
                }
            }
            CodexResponseOutputItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                tool_calls.push(ChatToolCall {
                    id: call_id,
                    kind: String::from("function"),
                    function: ChatToolCallFunction { name, arguments },
                });
            }
            CodexResponseOutputItem::Unknown => {}
        }
    }

    ChatCompletionResponse {
        id: response.id,
        model: response.model,
        choices: vec![ChatCompletionChoice {
            index: 0,
            message: ChatMessage {
                role: String::from("assistant"),
                content: (!assistant_text.is_empty()).then_some(assistant_text),
                name: None,
                tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
                tool_call_id: None,
            },
            finish_reason: None,
        }],
        usage: response.usage.map(chat_completion_usage_from_codex),
    }
}

fn chat_completion_usage_from_codex(usage: CodexResponsesUsage) -> ChatCompletionUsage {
    ChatCompletionUsage {
        prompt_tokens: usage.input_tokens,
        completion_tokens: usage.output_tokens,
        total_tokens: usage
            .total_tokens
            .unwrap_or(usage.input_tokens.saturating_add(usage.output_tokens)),
    }
}

#[derive(Default)]
struct CodexStreamState {
    response_id: Option<String>,
    response_model: Option<String>,
    next_tool_index: usize,
    tool_items: BTreeMap<String, CodexStreamToolRef>,
}

struct CodexStreamToolRef {
    index: usize,
}

impl CodexStreamState {
    fn chunk_from_event(
        &mut self,
        event: CodexResponseEvent,
    ) -> Result<Option<ChatCompletionChunk>, OpenAiProviderError> {
        match event {
            CodexResponseEvent::ResponseCreated { response } => {
                self.response_id = Some(response.id.clone());
                self.response_model = Some(response.model.clone());
                Ok(Some(ChatCompletionChunk {
                    id: response.id,
                    model: response.model,
                    choices: Vec::new(),
                    usage: None,
                }))
            }
            CodexResponseEvent::OutputItemAdded { item } => match item {
                CodexStreamOutputItem::Message { .. } => {
                    let (id, model) = self.response_identity()?;
                    Ok(Some(ChatCompletionChunk {
                        id,
                        model,
                        choices: vec![ChatCompletionChunkChoice {
                            index: 0,
                            delta: ChatCompletionChunkDelta {
                                role: Some(String::from("assistant")),
                                content: None,
                                tool_calls: None,
                            },
                            finish_reason: None,
                        }],
                        usage: None,
                    }))
                }
                CodexStreamOutputItem::FunctionCall { id, call_id, name } => {
                    let (response_id, response_model) = self.response_identity()?;
                    let tool_index = self.next_tool_index;
                    self.next_tool_index += 1;
                    self.tool_items
                        .insert(id, CodexStreamToolRef { index: tool_index });
                    Ok(Some(ChatCompletionChunk {
                        id: response_id,
                        model: response_model,
                        choices: vec![ChatCompletionChunkChoice {
                            index: 0,
                            delta: ChatCompletionChunkDelta {
                                role: Some(String::from("assistant")),
                                content: None,
                                tool_calls: Some(vec![ChatCompletionChunkToolCall {
                                    index: tool_index,
                                    id: Some(call_id),
                                    kind: Some(String::from("function")),
                                    function: Some(ChatCompletionChunkToolCallFunctionDelta {
                                        name: Some(name),
                                        arguments: None,
                                    }),
                                }]),
                            },
                            finish_reason: None,
                        }],
                        usage: None,
                    }))
                }
                CodexStreamOutputItem::Unknown => Ok(None),
            },
            CodexResponseEvent::OutputTextDelta { item_id: _, delta } => {
                let (id, model) = self.response_identity()?;
                Ok(Some(ChatCompletionChunk {
                    id,
                    model,
                    choices: vec![ChatCompletionChunkChoice {
                        index: 0,
                        delta: ChatCompletionChunkDelta {
                            role: None,
                            content: Some(delta),
                            tool_calls: None,
                        },
                        finish_reason: None,
                    }],
                    usage: None,
                }))
            }
            CodexResponseEvent::FunctionCallArgumentsDelta { item_id, delta } => {
                let Some(tool_ref) = self.tool_items.get(item_id.as_str()) else {
                    return Ok(None);
                };
                let (id, model) = self.response_identity()?;
                Ok(Some(ChatCompletionChunk {
                    id,
                    model,
                    choices: vec![ChatCompletionChunkChoice {
                        index: 0,
                        delta: ChatCompletionChunkDelta {
                            role: None,
                            content: None,
                            tool_calls: Some(vec![ChatCompletionChunkToolCall {
                                index: tool_ref.index,
                                id: None,
                                kind: None,
                                function: Some(ChatCompletionChunkToolCallFunctionDelta {
                                    name: None,
                                    arguments: Some(delta),
                                }),
                            }]),
                        },
                        finish_reason: None,
                    }],
                    usage: None,
                }))
            }
            CodexResponseEvent::ResponseCompleted { response } => {
                let (id, model) = self.response_identity()?;
                Ok(Some(ChatCompletionChunk {
                    id,
                    model,
                    choices: vec![ChatCompletionChunkChoice {
                        index: 0,
                        delta: ChatCompletionChunkDelta::default(),
                        finish_reason: None,
                    }],
                    usage: response.usage.map(chat_completion_usage_from_codex),
                }))
            }
            CodexResponseEvent::Unknown => Ok(None),
        }
    }

    fn response_identity(&self) -> Result<(String, String), OpenAiProviderError> {
        Ok((
            self.response_id.clone().ok_or_else(|| {
                OpenAiProviderError::Decode(String::from(
                    "codex stream event arrived before response.created id",
                ))
            })?,
            self.response_model.clone().ok_or_else(|| {
                OpenAiProviderError::Decode(String::from(
                    "codex stream event arrived before response.created model",
                ))
            })?,
        ))
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
        let id = self.response_id.ok_or_else(|| {
            OpenAiProviderError::Decode(String::from("stream ended before emitting a response id"))
        })?;
        let model = self.response_model.ok_or_else(|| {
            OpenAiProviderError::Decode(String::from(
                "stream ended before emitting a response model",
            ))
        })?;
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
    use std::collections::BTreeMap;
    use std::time::Duration;

    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use probe_test_support::{FakeAppleFmServer, FakeHttpResponse, FakeOpenAiServer};

    use super::{
        ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ChatMessage,
        ChatToolCall, ChatToolCallFunction, ChatToolChoice, ChatToolDefinition,
        ChatToolDefinitionEnvelope, CodexResponsesRequest, OpenAiProviderClient,
        OpenAiProviderConfig, OpenAiProviderError, OpenAiRequestAuth, OpenAiTransport,
    };

    #[test]
    fn localhost_helper_uses_local_default() {
        let config = OpenAiProviderConfig::localhost("example.gguf");
        assert_eq!(config.base_url, "http://127.0.0.1:8080/v1");
        assert_eq!(
            config.auth,
            OpenAiRequestAuth::BearerToken(String::from("dummy"))
        );
        assert_eq!(
            config.chat_completions_endpoint(),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
        assert_eq!(config.transport, OpenAiTransport::ChatCompletions);
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
        assert_eq!(
            config.max_tokens,
            Some(OpenAiProviderConfig::DEFAULT_MAX_TOKENS)
        );
        assert_eq!(config.transport, OpenAiTransport::ChatCompletions);
    }

    #[test]
    fn codex_config_can_be_built_from_backend_profile() {
        let profile = BackendProfile {
            name: String::from("openai-codex-subscription"),
            kind: BackendKind::OpenAiCodexSubscription,
            base_url: String::from("https://chatgpt.com/backend-api/codex"),
            model: String::from("gpt-5.3-codex"),
            api_key_env: String::new(),
            timeout_secs: 60,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
        };
        let config = OpenAiProviderConfig::from_backend_profile(&profile);
        assert_eq!(
            config.request_endpoint(),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(config.transport, OpenAiTransport::CodexResponses);
        assert_eq!(config.auth, OpenAiRequestAuth::None);
    }

    fn streaming_config(base_url: &str) -> OpenAiProviderConfig {
        OpenAiProviderConfig {
            base_url: base_url.to_string(),
            model: String::from("tiny-qwen35"),
            auth: OpenAiRequestAuth::BearerToken(String::from("dummy")),
            timeout: Duration::from_secs(5),
            stream: true,
            max_tokens: Some(OpenAiProviderConfig::DEFAULT_MAX_TOKENS),
            transport: OpenAiTransport::ChatCompletions,
            extra_headers: BTreeMap::new(),
        }
    }

    fn codex_config(base_url: &str) -> OpenAiProviderConfig {
        let mut extra_headers = BTreeMap::new();
        extra_headers.insert(String::from("ChatGPT-Account-Id"), String::from("acct-123"));
        extra_headers.insert(String::from("originator"), String::from("probe"));
        extra_headers.insert(String::from("session_id"), String::from("sess-codex-123"));
        extra_headers.insert(String::from("User-Agent"), String::from("probe/test"));
        OpenAiProviderConfig {
            base_url: base_url.to_string(),
            model: String::from("gpt-5.3-codex"),
            auth: OpenAiRequestAuth::BearerToken(String::from("access-token")),
            timeout: Duration::from_secs(5),
            stream: false,
            max_tokens: Some(OpenAiProviderConfig::DEFAULT_MAX_TOKENS),
            transport: OpenAiTransport::CodexResponses,
            extra_headers,
        }
    }

    fn codex_streaming_config(base_url: &str) -> OpenAiProviderConfig {
        let mut config = codex_config(base_url);
        config.stream = true;
        config
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
            auth: OpenAiRequestAuth::BearerToken(String::from("dummy")),
            timeout: std::time::Duration::from_secs(5),
            stream: false,
            max_tokens: Some(OpenAiProviderConfig::DEFAULT_MAX_TOKENS),
            transport: OpenAiTransport::ChatCompletions,
            extra_headers: BTreeMap::new(),
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
            auth: OpenAiRequestAuth::BearerToken(String::from("dummy")),
            timeout: Duration::from_secs(5),
            stream: false,
            max_tokens: Some(OpenAiProviderConfig::DEFAULT_MAX_TOKENS),
            transport: OpenAiTransport::ChatCompletions,
            extra_headers: BTreeMap::new(),
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
    fn codex_request_lifts_system_prompt_into_instructions() {
        let request = ChatCompletionRequest::from_config(
            &codex_config("https://chatgpt.com/backend-api/codex"),
            vec![
                ChatMessage::system("You are helpful"),
                ChatMessage::user("hello from probe"),
            ],
        );

        let encoded = serde_json::to_value(CodexResponsesRequest::from_chat_completion(&request))
            .expect("serialize codex request");

        assert_eq!(encoded["instructions"], "You are helpful");
        assert_eq!(encoded["input"][0]["role"], "user");
        assert_eq!(encoded["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(
            encoded["input"][0]["content"][0]["text"],
            "hello from probe"
        );
        assert!(encoded.get("messages").is_none());
    }

    #[test]
    fn codex_request_translates_tool_replay_messages_to_responses_input() {
        let request = ChatCompletionRequest::from_config(
            &codex_config("https://chatgpt.com/backend-api/codex"),
            vec![
                ChatMessage::system("You are helpful"),
                ChatMessage::assistant_tool_calls(vec![ChatToolCall {
                    id: String::from("call_1"),
                    kind: String::from("function"),
                    function: ChatToolCallFunction {
                        name: String::from("read_file"),
                        arguments: String::from("{\"path\":\"README.md\"}"),
                    },
                }]),
                ChatMessage::tool("read_file", "call_1", "{\"contents\":\"hi\"}"),
            ],
        )
        .with_tools(
            vec![ChatToolDefinitionEnvelope {
                kind: String::from("function"),
                function: ChatToolDefinition {
                    name: String::from("read_file"),
                    description: Some(String::from("Read a file")),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"}
                        },
                        "required": ["path"]
                    })),
                },
            }],
            Some(ChatToolChoice::Mode(String::from("auto"))),
            Some(false),
        );

        let encoded = serde_json::to_value(CodexResponsesRequest::from_chat_completion(&request))
            .expect("serialize codex tool request");

        assert_eq!(encoded["instructions"], "You are helpful");
        assert_eq!(encoded["tools"][0]["type"], "function");
        assert_eq!(encoded["tools"][0]["name"], "read_file");
        assert_eq!(encoded["tool_choice"], "auto");
        assert_eq!(encoded["parallel_tool_calls"], false);
        assert_eq!(encoded["input"][0]["type"], "function_call");
        assert_eq!(encoded["input"][0]["call_id"], "call_1");
        assert_eq!(encoded["input"][0]["name"], "read_file");
        assert_eq!(encoded["input"][1]["type"], "function_call_output");
        assert_eq!(encoded["input"][1]["call_id"], "call_1");
        assert_eq!(encoded["input"][1]["output"], "{\"contents\":\"hi\"}");
    }

    #[test]
    fn codex_transport_rewrites_to_subscription_endpoint_and_injects_headers() {
        let server = FakeAppleFmServer::from_json_responses(vec![serde_json::json!({
            "id": "codex_resp_1",
            "model": "gpt-5.3-codex",
            "output": [
                {
                    "type": "message",
                    "id": "msg_1",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "hello from codex"}]
                }
            ],
            "usage": {
                "input_tokens": 3,
                "output_tokens": 4
            }
        })]);
        let client = OpenAiProviderClient::new(codex_config(
            format!("{}/backend-api/codex", server.base_url()).as_str(),
        ))
        .expect("client");

        let response = client
            .chat_completion(vec![
                ChatMessage::system("You are helpful"),
                ChatMessage::user("hello from probe"),
            ])
            .expect("codex response");

        assert_eq!(response.first_message_text(), Some("hello from codex"));
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        let request = requests[0].to_lowercase();
        let raw_request = &requests[0];
        assert!(request.contains("post /backend-api/codex/responses"));
        assert!(request.contains("bearer access-token"));
        assert!(request.contains("chatgpt-account-id: acct-123"));
        assert!(request.contains("originator: probe"));
        assert!(request.contains("user-agent: probe/test"));
        assert!(request.contains("session_id: sess-codex-123"));
        assert!(raw_request.contains("\"instructions\":\"You are helpful\""));
        assert!(raw_request.contains("\"type\":\"input_text\""));
        assert!(!raw_request.contains("\"messages\""));
    }

    #[test]
    fn codex_transport_decodes_function_call_responses() {
        let server = FakeAppleFmServer::from_json_responses(vec![serde_json::json!({
            "id": "codex_tool_resp_1",
            "model": "gpt-5.3-codex",
            "output": [
                {
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_1",
                    "name": "read_file",
                    "arguments": "{\"path\":\"README.md\"}",
                    "status": "completed"
                }
            ]
        })]);
        let client = OpenAiProviderClient::new(codex_config(
            format!("{}/backend-api/codex", server.base_url()).as_str(),
        ))
        .expect("client");

        let response = client
            .chat_completion(vec![
                ChatMessage::system("You are helpful"),
                ChatMessage::user("inspect README.md"),
            ])
            .expect("codex tool response");

        let tool_calls = response.first_tool_calls().expect("tool calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_1");
        assert_eq!(tool_calls[0].function.name, "read_file");
        assert_eq!(tool_calls[0].function.arguments, "{\"path\":\"README.md\"}");
    }

    #[test]
    fn codex_transport_rejects_models_outside_the_allowlist() {
        let server = FakeAppleFmServer::from_json_responses(vec![serde_json::json!({
            "id": "unused",
            "model": "gpt-4.1",
            "choices": []
        })]);
        let mut config = codex_config(format!("{}/backend-api/codex", server.base_url()).as_str());
        config.model = String::from("gpt-4.1");
        let client = OpenAiProviderClient::new(config).expect("client");

        let error = client
            .chat_completion(vec![ChatMessage::user("hello")])
            .expect_err("non-codex model should be rejected");

        match error {
            OpenAiProviderError::UnsupportedCodexModel { model } => {
                assert_eq!(model, "gpt-4.1");
            }
            other => panic!("unexpected error: {other}"),
        }
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
            auth: OpenAiRequestAuth::BearerToken(String::from("dummy")),
            timeout: std::time::Duration::from_secs(5),
            stream: false,
            max_tokens: Some(OpenAiProviderConfig::DEFAULT_MAX_TOKENS),
            transport: OpenAiTransport::ChatCompletions,
            extra_headers: BTreeMap::new(),
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
        let server =
            FakeOpenAiServer::from_responses(vec![FakeHttpResponse::text_event_stream(200, body)]);
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
            response
                .usage
                .expect("usage should be preserved")
                .total_tokens,
            5
        );
    }

    #[test]
    fn request_inherits_max_tokens_from_config() {
        let request = super::ChatCompletionRequest::from_config(
            &OpenAiProviderConfig::localhost("tiny-qwen35"),
            vec![ChatMessage::user("hello")],
        );
        assert_eq!(
            request.max_tokens,
            Some(OpenAiProviderConfig::DEFAULT_MAX_TOKENS)
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
        let server =
            FakeOpenAiServer::from_responses(vec![FakeHttpResponse::text_event_stream(200, body)]);
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
    fn codex_streaming_plain_text_requests_are_assembled_from_responses_events() {
        let body = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_stream_text\",\"model\":\"gpt-5.3-codex\",\"created_at\":1}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_1\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\"hello\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\" world\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\n\n",
            "data: [DONE]\n\n"
        );
        let server =
            FakeOpenAiServer::from_responses(vec![FakeHttpResponse::text_event_stream(200, body)]);
        let client = OpenAiProviderClient::new(codex_streaming_config(
            format!("{}/backend-api/codex", server.base_url()).as_str(),
        ))
        .expect("client");
        let mut seen_chunks = Vec::<ChatCompletionChunk>::new();

        let response = client
            .send_chat_completion_with_callback(
                &ChatCompletionRequest::from_config(
                    client.config(),
                    vec![
                        ChatMessage::system("You are helpful"),
                        ChatMessage::user("hello"),
                    ],
                ),
                |chunk| seen_chunks.push(chunk.clone()),
            )
            .expect("streaming codex completion");

        assert_eq!(response.first_message_text(), Some("hello world"));
        assert_eq!(seen_chunks.len(), 5);
        assert_eq!(
            response
                .usage
                .expect("usage should be preserved")
                .total_tokens,
            5
        );
    }

    #[test]
    fn codex_streaming_tool_call_requests_are_assembled_from_responses_events() {
        let body = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_stream_tool\",\"model\":\"gpt-5.3-codex\",\"created_at\":1}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"read_file\",\"arguments\":\"\",\"status\":\"in_progress\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"path\\\":\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"\\\"README.md\\\"}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}\n\n",
            "data: [DONE]\n\n"
        );
        let server =
            FakeOpenAiServer::from_responses(vec![FakeHttpResponse::text_event_stream(200, body)]);
        let client = OpenAiProviderClient::new(codex_streaming_config(
            format!("{}/backend-api/codex", server.base_url()).as_str(),
        ))
        .expect("client");

        let response = client
            .send_chat_completion(&ChatCompletionRequest::from_config(
                client.config(),
                vec![
                    ChatMessage::system("You are helpful"),
                    ChatMessage::user("inspect README.md"),
                ],
            ))
            .expect("streaming codex tool response");

        let tool_calls = response.first_tool_calls().expect("tool calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_1");
        assert_eq!(tool_calls[0].function.name, "read_file");
        assert_eq!(tool_calls[0].function.arguments, "{\"path\":\"README.md\"}");
    }

    #[test]
    fn streaming_requests_fall_back_to_non_streaming_when_backend_returns_json() {
        let server =
            FakeOpenAiServer::from_responses(vec![FakeHttpResponse::json_ok(serde_json::json!({
                "id": "chatcmpl_fallback",
                "model": "tiny-qwen35",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "fallback response"},
                    "finish_reason": "stop"
                }]
            }))]);
        let client =
            OpenAiProviderClient::new(streaming_config(server.base_url())).expect("client");

        let response = client
            .chat_completion(vec![ChatMessage::assistant("hello")])
            .expect("streaming request should fall back to non-streaming");
        assert_eq!(response.first_message_text(), Some("fallback response"));
    }

    #[test]
    fn codex_streaming_requests_fall_back_to_non_streaming_when_backend_returns_json() {
        let server =
            FakeOpenAiServer::from_responses(vec![FakeHttpResponse::json_ok(serde_json::json!({
                "id": "resp_fallback",
                "model": "gpt-5.3-codex",
                "output": [{
                    "type": "message",
                    "id": "msg_1",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "fallback response"}]
                }]
            }))]);
        let client = OpenAiProviderClient::new(codex_streaming_config(
            format!("{}/backend-api/codex", server.base_url()).as_str(),
        ))
        .expect("client");

        let response = client
            .chat_completion(vec![
                ChatMessage::system("You are helpful"),
                ChatMessage::assistant("hello"),
            ])
            .expect("codex streaming request should fall back to non-streaming");
        assert_eq!(response.first_message_text(), Some("fallback response"));
    }
}
