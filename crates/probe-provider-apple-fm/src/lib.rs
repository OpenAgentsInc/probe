use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use std::{thread, thread::JoinHandle};

use probe_protocol::backend::BackendProfile;
use psionic_apple_fm::{
    AppleFmAsyncBridgeClient, AppleFmBridgeClient, AppleFmBridgeClientError,
    AppleFmBridgeStreamError, AppleFmChatCompletionRequest, AppleFmChatMessage,
    AppleFmChatMessageRole, AppleFmChatUsage, AppleFmErrorCode, AppleFmFoundationModelsError,
    AppleFmGenerationSchema, AppleFmSession, AppleFmSessionCreateRequest,
    AppleFmSessionRespondRequest, AppleFmSystemLanguageModel,
    AppleFmSystemLanguageModelAvailability, AppleFmTextStreamEvent, AppleFmToolCallError,
    AppleFmToolCallRequest, AppleFmToolCallResponse, AppleFmToolCallbackConfiguration,
    AppleFmToolDefinition, AppleFmTranscript, AppleFmTranscriptContent, AppleFmTranscriptEntry,
    AppleFmTranscriptPayload,
};
use reqwest::blocking::Client;
use tokio_stream::StreamExt;

static NEXT_TOOL_CALLBACK_TOKEN: AtomicU64 = AtomicU64::new(1);

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
            base_url: String::from("http://127.0.0.1:11435"),
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

#[derive(Clone, Debug, PartialEq)]
pub struct AppleFmProviderToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppleFmProviderToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppleFmProviderSessionResponse {
    pub id: String,
    pub session_id: String,
    pub model: String,
    pub assistant_text: String,
    pub usage: Option<AppleFmChatUsage>,
    pub transcript: Option<AppleFmTranscript>,
}

pub type AppleFmProviderStreamCallback<'a> = dyn FnMut(&AppleFmTextStreamEvent) + 'a;

#[derive(Debug)]
pub enum AppleFmProviderError {
    Build(String),
    ToolSchema { tool_name: String, message: String },
    Transcript(String),
    Request(AppleFmBridgeClientError),
    Stream(AppleFmBridgeStreamError),
    StreamProtocol(String),
}

impl AppleFmProviderError {
    #[must_use]
    pub fn foundation_models_error(&self) -> Option<&AppleFmFoundationModelsError> {
        match self {
            Self::Request(error) => error.foundation_models_error(),
            Self::Stream(AppleFmBridgeStreamError::FoundationModels { error }) => Some(error),
            Self::Build(_)
            | Self::ToolSchema { .. }
            | Self::Transcript(_)
            | Self::Stream(_)
            | Self::StreamProtocol(_) => None,
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
            Self::ToolSchema { tool_name, message } => {
                write!(
                    f,
                    "failed to build Apple FM tool schema for `{tool_name}`: {message}"
                )
            }
            Self::Transcript(message) => {
                write!(
                    f,
                    "failed to decode Apple FM transcript snapshot: {message}"
                )
            }
            Self::Stream(error) => write!(f, "{error}"),
            Self::StreamProtocol(message) => {
                write!(
                    f,
                    "Apple FM stream ended without a terminal response: {message}"
                )
            }
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

struct AppleFmProviderToolCallbackServer {
    callback_url: String,
    session_token: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

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
        self.chat_completion_with_callback(messages, None)
    }

    pub fn chat_completion_with_callback(
        &self,
        messages: Vec<AppleFmProviderMessage>,
        callback: Option<&mut AppleFmProviderStreamCallback<'_>>,
    ) -> Result<AppleFmProviderResponse, AppleFmProviderError> {
        if let Some(callback) = callback {
            return self.stream_plain_text_session(messages, callback);
        }

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

    pub fn respond_in_session_with_tools(
        &self,
        system_prompt: Option<&str>,
        transcript: AppleFmTranscript,
        prompt: &str,
        tools: Vec<AppleFmProviderToolDefinition>,
        callback: Arc<
            dyn Fn(AppleFmProviderToolCall) -> Result<String, AppleFmToolCallError> + Send + Sync,
        >,
    ) -> Result<AppleFmProviderSessionResponse, AppleFmProviderError> {
        self.respond_in_session_with_tools_and_callback(
            system_prompt,
            transcript,
            prompt,
            tools,
            callback,
            None,
        )
    }

    pub fn respond_in_session_with_tools_and_callback(
        &self,
        system_prompt: Option<&str>,
        transcript: AppleFmTranscript,
        prompt: &str,
        tools: Vec<AppleFmProviderToolDefinition>,
        callback: Arc<
            dyn Fn(AppleFmProviderToolCall) -> Result<String, AppleFmToolCallError> + Send + Sync,
        >,
        stream_callback: Option<&mut AppleFmProviderStreamCallback<'_>>,
    ) -> Result<AppleFmProviderSessionResponse, AppleFmProviderError> {
        let model = AppleFmSystemLanguageModel {
            id: self.config.model.clone(),
            ..Default::default()
        };
        let tool_definitions = tools
            .into_iter()
            .map(provider_tool_definition)
            .collect::<Result<Vec<_>, _>>()?;
        let callback_server = AppleFmProviderToolCallbackServer::new(callback)?;
        let session_request = AppleFmSessionCreateRequest {
            instructions: system_prompt.map(ToOwned::to_owned),
            model: Some(model),
            tools: tool_definitions,
            adapter: None,
            tool_callback: Some(callback_server.configuration()),
            transcript_json: None,
            transcript: Some(transcript),
        };
        let session = create_session_with_transcript_fallback(&self.client, &session_request)
            .map_err(AppleFmProviderError::Request)?;
        let respond_request = AppleFmSessionRespondRequest {
            prompt: prompt.to_string(),
            options: None,
            adapter: None,
        };
        let response = match stream_callback {
            Some(callback) => self
                .stream_session_response(session.id.as_str(), &respond_request, callback)
                .and_then(plain_text_provider_session_from_stream),
            None => {
                self.client
                    .respond_in_session(session.id.as_str(), &respond_request)
                    .map_err(AppleFmProviderError::Request)
                    .and_then(|response| {
                        Ok(AppleFmProviderSessionResponse {
                            id: response.session.id.clone(),
                            session_id: response.session.id.clone(),
                            model: response.model,
                            assistant_text: response.output,
                            usage: response.usage,
                            transcript: response.session.transcript().map_err(|error| {
                                AppleFmProviderError::Transcript(error.to_string())
                            })?,
                        })
                    })
            }
        };
        let _ = self.client.delete_session(session.id.as_str());
        drop(callback_server);
        response
    }

    fn stream_plain_text_session(
        &self,
        messages: Vec<AppleFmProviderMessage>,
        callback: &mut AppleFmProviderStreamCallback<'_>,
    ) -> Result<AppleFmProviderResponse, AppleFmProviderError> {
        let seed = apple_fm_stream_seed_from_messages(messages);
        let session_request = AppleFmSessionCreateRequest {
            instructions: seed.instructions,
            model: Some(AppleFmSystemLanguageModel {
                id: self.config.model.clone(),
                ..Default::default()
            }),
            tools: Vec::new(),
            adapter: None,
            tool_callback: None,
            transcript_json: None,
            transcript: Some(seed.transcript),
        };
        let session = create_session_with_transcript_fallback(&self.client, &session_request)
            .map_err(AppleFmProviderError::Request)?;
        let terminal = self.stream_session_response(
            session.id.as_str(),
            &AppleFmSessionRespondRequest {
                prompt: seed.prompt,
                options: None,
                adapter: None,
            },
            callback,
        );
        let _ = self.client.delete_session(session.id.as_str());
        let terminal = terminal?;
        Ok(AppleFmProviderResponse {
            id: terminal
                .session
                .as_ref()
                .map(|session| session.id.clone())
                .unwrap_or_else(|| session.id.clone()),
            model: terminal.model,
            assistant_text: Some(terminal.output),
            usage: terminal.usage,
        })
    }

    fn stream_session_response(
        &self,
        session_id: &str,
        request: &AppleFmSessionRespondRequest,
        callback: &mut AppleFmProviderStreamCallback<'_>,
    ) -> Result<AppleFmTextStreamEvent, AppleFmProviderError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| AppleFmProviderError::Build(error.to_string()))?;
        let base_url = self.config.base_url.clone();
        runtime.block_on(async {
            let client =
                AppleFmAsyncBridgeClient::new(base_url).map_err(AppleFmProviderError::Request)?;
            let mut stream = client
                .stream_session_response(session_id, request)
                .await
                .map_err(AppleFmProviderError::Request)?;
            let mut terminal = None;
            while let Some(event) = stream.next().await {
                let event = event.map_err(AppleFmProviderError::Stream)?;
                callback(&event);
                if event.is_terminal() {
                    terminal = Some(event);
                }
            }
            terminal.ok_or_else(|| {
                AppleFmProviderError::StreamProtocol(format!(
                    "session `{session_id}` stream ended before a terminal snapshot"
                ))
            })
        })
    }
}

fn plain_text_provider_session_from_stream(
    terminal: AppleFmTextStreamEvent,
) -> Result<AppleFmProviderSessionResponse, AppleFmProviderError> {
    let session = terminal.session.ok_or_else(|| {
        AppleFmProviderError::StreamProtocol(String::from(
            "terminal stream event did not include final session state",
        ))
    })?;
    let transcript = session
        .transcript()
        .map_err(|error| AppleFmProviderError::Transcript(error.to_string()))?;
    Ok(AppleFmProviderSessionResponse {
        id: session.id.clone(),
        session_id: session.id.clone(),
        model: terminal.model,
        assistant_text: terminal.output,
        usage: terminal.usage,
        transcript,
    })
}

struct AppleFmStreamPlainTextSeed {
    instructions: Option<String>,
    transcript: AppleFmTranscript,
    prompt: String,
}

fn apple_fm_stream_seed_from_messages(
    messages: Vec<AppleFmProviderMessage>,
) -> AppleFmStreamPlainTextSeed {
    let mut instructions = Vec::new();
    let mut conversation = Vec::new();
    for message in messages {
        match message.role {
            AppleFmProviderMessageRole::System => instructions.push(message.content),
            AppleFmProviderMessageRole::User | AppleFmProviderMessageRole::Assistant => {
                conversation.push(message)
            }
        }
    }

    let prompt = if conversation
        .last()
        .is_some_and(|message| message.role == AppleFmProviderMessageRole::User)
    {
        conversation
            .pop()
            .map(|message| message.content)
            .unwrap_or_default()
    } else {
        String::new()
    };

    AppleFmStreamPlainTextSeed {
        instructions: (!instructions.is_empty()).then(|| instructions.join("\n\n")),
        transcript: build_apple_fm_provider_transcript(conversation),
        prompt,
    }
}

fn build_apple_fm_provider_transcript(messages: Vec<AppleFmProviderMessage>) -> AppleFmTranscript {
    AppleFmTranscript {
        version: AppleFmTranscript::default().version,
        transcript_type: AppleFmTranscript::default().transcript_type,
        transcript: AppleFmTranscriptPayload {
            entries: messages
                .into_iter()
                .enumerate()
                .map(|(index, message)| {
                    apple_fm_provider_text_entry(
                        format!("probe-message-{index}"),
                        match message.role {
                            AppleFmProviderMessageRole::System => "system",
                            AppleFmProviderMessageRole::User => "user",
                            AppleFmProviderMessageRole::Assistant => "assistant",
                        },
                        message.content,
                    )
                })
                .collect(),
        },
    }
}

fn apple_fm_provider_text_entry(id: String, role: &str, text: String) -> AppleFmTranscriptEntry {
    AppleFmTranscriptEntry {
        id: Some(id.clone()),
        role: role.to_string(),
        contents: vec![AppleFmTranscriptContent {
            content_type: String::from("text"),
            id: Some(format!("{id}-content")),
            extra: BTreeMap::from([(String::from("text"), serde_json::Value::String(text))]),
        }],
        extra: BTreeMap::new(),
    }
}

fn provider_tool_definition(
    definition: AppleFmProviderToolDefinition,
) -> Result<AppleFmToolDefinition, AppleFmProviderError> {
    let tool_name = definition.name.clone();
    let schema = apple_fm_generation_schema_for_tool(
        definition
            .parameters
            .unwrap_or_else(default_empty_tool_schema),
        tool_name.as_str(),
    )?;
    Ok(AppleFmToolDefinition::new(
        tool_name,
        definition.description,
        schema,
    ))
}

fn apple_fm_generation_schema_for_tool(
    schema: serde_json::Value,
    tool_name: &str,
) -> Result<AppleFmGenerationSchema, AppleFmProviderError> {
    let tool_name = tool_name.to_string();
    let normalized = normalize_tool_schema_for_apple_fm(schema, tool_name.as_str());
    let schema: AppleFmGenerationSchema =
        serde_json::from_value(normalized).map_err(|error| AppleFmProviderError::ToolSchema {
            tool_name: tool_name.clone(),
            message: error.to_string(),
        })?;
    schema
        .validate()
        .map_err(|error| AppleFmProviderError::ToolSchema {
            tool_name,
            message: error.to_string(),
        })?;
    Ok(schema)
}

fn normalize_tool_schema_for_apple_fm(
    mut schema: serde_json::Value,
    tool_name: &str,
) -> serde_json::Value {
    let Some(schema_object) = schema.as_object_mut() else {
        return schema;
    };
    schema_object.remove("$schema");
    let property_order = schema_object
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|properties| {
            properties
                .keys()
                .map(|key| serde_json::Value::String(key.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    schema_object
        .entry("title".to_string())
        .or_insert(serde_json::Value::String(format!("{tool_name}_arguments")));
    schema_object
        .entry("x-order".to_string())
        .or_insert(serde_json::Value::Array(property_order));
    schema_object
        .entry("required".to_string())
        .or_insert(serde_json::Value::Array(Vec::new()));
    schema
}

fn default_empty_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

fn create_session_with_transcript_fallback(
    client: &AppleFmBridgeClient,
    request: &AppleFmSessionCreateRequest,
) -> Result<AppleFmSession, AppleFmBridgeClientError> {
    match client.create_session(request) {
        Ok(session) => Ok(session),
        Err(error)
            if request_has_transcript(request) && should_retry_without_transcript(&error) =>
        {
            let mut retry_request = request.clone();
            retry_request.transcript = None;
            retry_request.transcript_json = None;
            match client.create_session(&retry_request) {
                Ok(session) => Ok(session),
                Err(_) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

fn request_has_transcript(request: &AppleFmSessionCreateRequest) -> bool {
    request.transcript.is_some() || request.transcript_json.is_some()
}

fn should_retry_without_transcript(error: &AppleFmBridgeClientError) -> bool {
    let Some(error) = error.foundation_models_error() else {
        return false;
    };
    error.kind == AppleFmErrorCode::InvalidRequest
        && error.message.to_ascii_lowercase().contains("invalid json")
}

impl AppleFmProviderToolCallbackServer {
    fn new(
        callback: Arc<
            dyn Fn(AppleFmProviderToolCall) -> Result<String, AppleFmToolCallError> + Send + Sync,
        >,
    ) -> Result<Self, AppleFmProviderError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| AppleFmProviderError::Build(error.to_string()))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| AppleFmProviderError::Build(error.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|error| AppleFmProviderError::Build(error.to_string()))?
            .port();
        let callback_url = format!("http://127.0.0.1:{port}/tool-call");
        let session_token = format!(
            "probe-apple-fm-tool-session-{}",
            NEXT_TOOL_CALLBACK_TOKEN.fetch_add(1, Ordering::Relaxed)
        );
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let callback_thread = Arc::clone(&callback);
        let expected_token = session_token.clone();
        let thread = thread::spawn(move || {
            while !stop_thread.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = handle_tool_callback_connection(
                            &mut stream,
                            expected_token.as_str(),
                            callback_thread.as_ref(),
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            callback_url,
            session_token,
            stop,
            thread: Some(thread),
        })
    }

    fn configuration(&self) -> AppleFmToolCallbackConfiguration {
        AppleFmToolCallbackConfiguration {
            url: self.callback_url.clone(),
            session_token: self.session_token.clone(),
        }
    }
}

impl Drop for AppleFmProviderToolCallbackServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(
            self.callback_url
                .strip_prefix("http://")
                .and_then(|value| value.split_once('/').map(|(authority, _)| authority))
                .unwrap_or("127.0.0.1:0"),
        );
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn handle_tool_callback_connection(
    stream: &mut TcpStream,
    expected_session_token: &str,
    callback: &(
         dyn Fn(AppleFmProviderToolCall) -> Result<String, AppleFmToolCallError> + Send + Sync
     ),
) -> Result<(), String> {
    let request = match read_http_request(stream) {
        Ok(request) => request,
        Err(error) => {
            write_json_response(
                stream,
                400,
                &serde_json::json!({
                    "error": {
                        "message": error,
                        "type": "invalid_request",
                        "code": "invalid_request"
                    }
                }),
            )?;
            return Ok(());
        }
    };
    if request.method != "POST" || request.path != "/tool-call" {
        write_json_response(
            stream,
            404,
            &serde_json::json!({
                "error": {
                    "message": format!("Not found: {} {}", request.method, request.path),
                    "type": "not_found",
                    "code": "not_found"
                }
            }),
        )?;
        return Ok(());
    }
    let request: AppleFmToolCallRequest = match serde_json::from_slice(request.body.as_slice()) {
        Ok(request) => request,
        Err(error) => {
            write_json_response(
                stream,
                400,
                &serde_json::json!({
                    "error": {
                        "message": error.to_string(),
                        "type": "invalid_request",
                        "code": "invalid_request"
                    }
                }),
            )?;
            return Ok(());
        }
    };
    if request.session_token != expected_session_token {
        write_json_response(
            stream,
            422,
            &AppleFmToolCallError::new(
                request.tool_name,
                "unexpected Probe Apple FM callback session token",
            ),
        )?;
        return Ok(());
    }
    match callback(AppleFmProviderToolCall {
        name: request.tool_name.clone(),
        arguments: request.arguments.content,
    }) {
        Ok(output) => write_json_response(stream, 200, &AppleFmToolCallResponse { output })?,
        Err(error) => write_json_response(stream, 422, &error)?,
    }
    Ok(())
}

struct CallbackHttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn read_http_request(stream: &mut TcpStream) -> Result<CallbackHttpRequest, String> {
    let mut buffer = Vec::new();
    let mut header_end = None;
    let mut content_length = 0usize;
    loop {
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if header_end.is_none()
            && let Some(index) = find_header_end(buffer.as_slice())
        {
            header_end = Some(index);
            content_length = parse_content_length(&buffer[..index])?;
        }
        if let Some(index) = header_end {
            let body_end = index + 4 + content_length;
            if buffer.len() >= body_end {
                break;
            }
        }
    }
    let header_end = header_end.ok_or("missing HTTP header terminator".to_string())?;
    let header_text =
        String::from_utf8(buffer[..header_end].to_vec()).map_err(|error| error.to_string())?;
    let mut header_lines = header_text.split("\r\n");
    let request_line = header_lines
        .next()
        .ok_or("missing tool callback request line".to_string())?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or("missing tool callback method".to_string())?
        .to_string();
    let path = request_parts
        .next()
        .ok_or("missing tool callback path".to_string())?
        .to_string();
    let body_start = header_end + 4;
    let body_end = body_start + content_length;
    if buffer.len() < body_end {
        return Err("incomplete HTTP body".to_string());
    }
    Ok(CallbackHttpRequest {
        method,
        path,
        body: buffer[body_start..body_end].to_vec(),
    })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(headers: &[u8]) -> Result<usize, String> {
    let headers = String::from_utf8(headers.to_vec()).map_err(|error| error.to_string())?;
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            return value
                .trim()
                .parse::<usize>()
                .map_err(|error| error.to_string());
        }
    }
    Ok(0)
}

fn write_json_response<T: serde::Serialize>(
    stream: &mut TcpStream,
    status_code: u16,
    body: &T,
) -> Result<(), String> {
    let body = serde_json::to_string(body).map_err(|error| error.to_string())?;
    let response = format!(
        "HTTP/1.1 {status_code} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use probe_test_support::{FakeAppleFmServer, FakeHttpResponse};
    use psionic_apple_fm::AppleFmTranscript;
    use serde_json::json;

    use super::{
        AppleFmProviderClient, AppleFmProviderConfig, AppleFmProviderError, AppleFmProviderMessage,
        AppleFmProviderToolCall, AppleFmProviderToolDefinition, provider_tool_definition,
    };

    struct ToolCallbackResponse {
        status_code: u16,
        body: String,
    }

    fn invoke_tool_callback(
        callback_url: &str,
        session_token: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> ToolCallbackResponse {
        let url = callback_url
            .strip_prefix("http://")
            .expect("callback url should be http");
        let (authority, path) = url
            .split_once('/')
            .expect("callback url should include path");
        let body = json!({
            "session_token": session_token,
            "tool_name": tool_name,
            "arguments": {
                "content": arguments,
                "is_complete": true
            }
        })
        .to_string();
        let mut stream = TcpStream::connect(authority).expect("connect tool callback");
        let request = format!(
            "POST /{} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            path,
            authority,
            body.len(),
            body
        );
        stream
            .write_all(request.as_bytes())
            .expect("write tool callback request");
        stream.flush().expect("flush tool callback request");
        stream
            .shutdown(Shutdown::Write)
            .expect("close tool callback request writer");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read tool callback response");
        let (head, body) = response
            .split_once("\r\n\r\n")
            .expect("tool callback response should include body");
        let status_code = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u16>().ok())
            .expect("tool callback status code");
        ToolCallbackResponse {
            status_code,
            body: body.to_string(),
        }
    }

    #[test]
    fn localhost_helper_uses_local_default() {
        let config = AppleFmProviderConfig::localhost("apple-foundation-model");
        assert_eq!(config.base_url, "http://127.0.0.1:11435");
        assert_eq!(config.model, "apple-foundation-model");
    }

    #[test]
    fn config_can_be_built_from_backend_profile() {
        let profile = BackendProfile {
            name: String::from("psionic-apple-fm-bridge"),
            kind: BackendKind::AppleFmBridge,
            base_url: String::from("http://127.0.0.1:11435"),
            model: String::from("apple-foundation-model"),
            reasoning_level: None,
            api_key_env: String::new(),
            timeout_secs: 45,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
            control_plane: None,
            psionic_mesh: None,
        };
        let config = AppleFmProviderConfig::from_backend_profile(&profile);
        assert_eq!(config.base_url, "http://127.0.0.1:11435");
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
    fn client_streams_plain_text_completion_through_session_snapshots() {
        let server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => FakeHttpResponse::json_ok(json!({
                    "session": {
                        "id": "sess-stream-plain-1",
                        "instructions": request.body,
                        "model": {
                            "id": "apple-foundation-model",
                            "use_case": "general",
                            "guardrails": "default"
                        },
                        "tools": [],
                        "is_responding": false,
                        "transcript_json": "{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"
                    }
                })),
                ("POST", "/v1/sessions/sess-stream-plain-1/responses/stream") => {
                    FakeHttpResponse::text_event_stream(
                        200,
                        concat!(
                            "event: snapshot\n",
                            "data: {\"kind\":\"snapshot\",\"model\":\"apple-foundation-model\",\"output\":\"hello\"}\n\n",
                            "event: completed\n",
                            "data: {\"kind\":\"completed\",\"model\":\"apple-foundation-model\",\"output\":\"hello world\",\"session\":{\"id\":\"sess-stream-plain-1\",\"model\":{\"id\":\"apple-foundation-model\",\"use_case\":\"general\",\"guardrails\":\"default\"},\"tools\":[],\"is_responding\":false,\"transcript_json\":\"{\\\"version\\\":1,\\\"type\\\":\\\"FoundationModels.Transcript\\\",\\\"transcript\\\":{\\\"entries\\\":[]}}\"},\"usage\":{\"total_tokens_detail\":{\"value\":11,\"truth\":\"estimated\"}}}\n\n",
                        ),
                    )
                }
                ("DELETE", "/v1/sessions/sess-stream-plain-1") => {
                    FakeHttpResponse::json_ok(json!({}))
                }
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let client = AppleFmProviderClient::new(AppleFmProviderConfig {
            base_url: server.base_url().to_string(),
            model: String::from("apple-foundation-model"),
            timeout: Duration::from_secs(5),
        })
        .expect("client");
        let mut seen = Vec::new();
        let response = client
            .chat_completion_with_callback(
                vec![AppleFmProviderMessage::user("hello")],
                Some(&mut |event| seen.push(event.output.clone())),
            )
            .expect("streamed completion");

        assert_eq!(
            seen,
            vec![String::from("hello"), String::from("hello world")]
        );
        assert_eq!(response.id, "sess-stream-plain-1");
        assert_eq!(response.assistant_text.as_deref(), Some("hello world"));
        assert_eq!(
            response
                .usage
                .as_ref()
                .and_then(|usage| usage.total_tokens_detail.as_ref())
                .map(|detail| detail.value),
            Some(11)
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
        assert!(requests[1].contains("/responses/stream"));
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

    #[test]
    fn client_runs_session_tool_callbacks_against_the_bridge() {
        let callback_payloads = Arc::new(Mutex::new(Vec::new()));
        let captured_payloads = Arc::clone(&callback_payloads);
        let bridge_state = Arc::new(Mutex::new((String::new(), String::new())));
        let captured_state = Arc::clone(&bridge_state);
        let server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => {
                    let request_json: serde_json::Value =
                        serde_json::from_str(request.body.as_str()).expect("session create json");
                    let callback = request_json["tool_callback"]["url"]
                        .as_str()
                        .expect("callback url")
                        .to_string();
                    let session_token = request_json["tool_callback"]["session_token"]
                        .as_str()
                        .expect("session token")
                        .to_string();
                    let mut state = captured_state.lock().expect("bridge state lock");
                    state.0 = callback;
                    state.1 = session_token;
                    FakeHttpResponse::json_ok(json!({
                        "session": {
                            "id": "sess-tool-1",
                            "instructions": request_json["instructions"],
                            "model": {
                                "id": "apple-foundation-model",
                                "use_case": "general",
                                "guardrails": "default"
                            },
                            "tools": [{"name": "read_file"}],
                            "is_responding": false,
                            "transcript_json": "{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"
                        }
                    }))
                }
                ("POST", "/v1/sessions/sess-tool-1/responses") => {
                    let (callback, session_token) = {
                        let state = bridge_state.lock().expect("bridge state lock");
                        (state.0.clone(), state.1.clone())
                    };
                    let callback_response = invoke_tool_callback(
                        callback.as_str(),
                        session_token.as_str(),
                        "read_file",
                        json!({
                            "path": "hello.txt",
                            "start_line": 1,
                            "max_lines": 10
                        }),
                    );
                    assert_eq!(
                        callback_response.status_code, 200,
                        "{}",
                        callback_response.body
                    );
                    let callback_json: serde_json::Value =
                        serde_json::from_str(callback_response.body.as_str())
                            .expect("callback json");
                    captured_payloads
                        .lock()
                        .expect("callback payload lock")
                        .push(
                            callback_json["output"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                        );
                    FakeHttpResponse::json_ok(json!({
                        "session": {
                            "id": "sess-tool-1",
                            "instructions": "You are a helper",
                            "model": {
                                "id": "apple-foundation-model",
                                "use_case": "general",
                                "guardrails": "default"
                            },
                            "tools": [{"name": "read_file"}],
                            "is_responding": false,
                            "transcript_json": "{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"
                        },
                        "model": "apple-foundation-model",
                        "output": "tool-backed answer",
                        "usage": {
                            "total_tokens_detail": {"value": 21, "truth": "estimated"}
                        }
                    }))
                }
                ("DELETE", "/v1/sessions/sess-tool-1") => FakeHttpResponse::json_ok(json!({})),
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let client = AppleFmProviderClient::new(AppleFmProviderConfig {
            base_url: server.base_url().to_string(),
            model: String::from("apple-foundation-model"),
            timeout: Duration::from_secs(5),
        })
        .expect("client");
        let response = client
            .respond_in_session_with_tools(
                Some("You are a helper"),
                psionic_apple_fm::AppleFmTranscript::default(),
                "read hello.txt",
                vec![AppleFmProviderToolDefinition {
                    name: String::from("read_file"),
                    description: Some(String::from("Read a file.")),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"},
                            "start_line": {"type": "integer"},
                            "max_lines": {"type": "integer"}
                        },
                        "required": ["path"],
                        "additionalProperties": false
                    })),
                }],
                Arc::new(|tool_call: AppleFmProviderToolCall| {
                    Ok(json!({
                        "tool": tool_call.name,
                        "arguments": tool_call.arguments
                    })
                    .to_string())
                }),
            )
            .expect("tool session response");

        assert_eq!(response.session_id, "sess-tool-1");
        assert_eq!(response.assistant_text, "tool-backed answer");
        let callback_payloads = callback_payloads.lock().expect("callback payload lock");
        assert_eq!(callback_payloads.len(), 1);
        assert!(callback_payloads[0].contains("\"read_file\""));
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
        assert!(requests[0].contains("POST /v1/sessions HTTP/1.1"));
        assert!(requests[1].contains("POST /v1/sessions/sess-tool-1/responses HTTP/1.1"));
        assert!(requests[2].contains("DELETE /v1/sessions/sess-tool-1 HTTP/1.1"));
    }

    #[test]
    fn client_streams_tool_backed_session_responses_with_snapshot_semantics() {
        let callback_payloads = Arc::new(Mutex::new(Vec::new()));
        let captured_payloads = Arc::clone(&callback_payloads);
        let bridge_state = Arc::new(Mutex::new((String::new(), String::new())));
        let captured_state = Arc::clone(&bridge_state);
        let server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => {
                    let request_json: serde_json::Value =
                        serde_json::from_str(request.body.as_str()).expect("session create json");
                    let callback = request_json["tool_callback"]["url"]
                        .as_str()
                        .expect("callback url")
                        .to_string();
                    let session_token = request_json["tool_callback"]["session_token"]
                        .as_str()
                        .expect("session token")
                        .to_string();
                    let mut state = captured_state.lock().expect("bridge state lock");
                    state.0 = callback;
                    state.1 = session_token;
                    FakeHttpResponse::json_ok(json!({
                        "session": {
                            "id": "sess-stream-tool-1",
                            "instructions": request_json["instructions"],
                            "model": {
                                "id": "apple-foundation-model",
                                "use_case": "general",
                                "guardrails": "default"
                            },
                            "tools": [{"name": "read_file"}],
                            "is_responding": false,
                            "transcript_json": "{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"
                        }
                    }))
                }
                ("POST", "/v1/sessions/sess-stream-tool-1/responses/stream") => {
                    let (callback, session_token) = {
                        let state = bridge_state.lock().expect("bridge state lock");
                        (state.0.clone(), state.1.clone())
                    };
                    let callback_response = invoke_tool_callback(
                        callback.as_str(),
                        session_token.as_str(),
                        "read_file",
                        json!({
                            "path": "hello.txt",
                            "start_line": 1,
                            "max_lines": 10
                        }),
                    );
                    assert_eq!(callback_response.status_code, 200);
                    let callback_json: serde_json::Value =
                        serde_json::from_str(callback_response.body.as_str())
                            .expect("callback json");
                    captured_payloads
                        .lock()
                        .expect("callback payload lock")
                        .push(
                            callback_json["output"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                        );
                    FakeHttpResponse::text_event_stream(
                        200,
                        concat!(
                            "event: snapshot\n",
                            "data: {\"kind\":\"snapshot\",\"model\":\"apple-foundation-model\",\"output\":\"reading hello.txt\"}\n\n",
                            "event: completed\n",
                            "data: {\"kind\":\"completed\",\"model\":\"apple-foundation-model\",\"output\":\"tool-backed answer\",\"session\":{\"id\":\"sess-stream-tool-1\",\"instructions\":\"You are a helper\",\"model\":{\"id\":\"apple-foundation-model\",\"use_case\":\"general\",\"guardrails\":\"default\"},\"tools\":[{\"name\":\"read_file\"}],\"is_responding\":false,\"transcript_json\":\"{\\\"version\\\":1,\\\"type\\\":\\\"FoundationModels.Transcript\\\",\\\"transcript\\\":{\\\"entries\\\":[]}}\"},\"usage\":{\"total_tokens_detail\":{\"value\":21,\"truth\":\"estimated\"}}}\n\n",
                        ),
                    )
                }
                ("DELETE", "/v1/sessions/sess-stream-tool-1") => {
                    FakeHttpResponse::json_ok(json!({}))
                }
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let client = AppleFmProviderClient::new(AppleFmProviderConfig {
            base_url: server.base_url().to_string(),
            model: String::from("apple-foundation-model"),
            timeout: Duration::from_secs(5),
        })
        .expect("client");
        let mut snapshots = Vec::new();
        let response = client
            .respond_in_session_with_tools_and_callback(
                Some("You are a helper"),
                AppleFmTranscript::default(),
                "read hello.txt",
                vec![AppleFmProviderToolDefinition {
                    name: String::from("read_file"),
                    description: Some(String::from("Read a file.")),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"}
                        },
                        "required": ["path"],
                        "additionalProperties": false
                    })),
                }],
                Arc::new(|tool_call: AppleFmProviderToolCall| {
                    Ok(json!({
                        "tool": tool_call.name,
                        "arguments": tool_call.arguments
                    })
                    .to_string())
                }),
                Some(&mut |event| snapshots.push(event.output.clone())),
            )
            .expect("streamed tool session response");

        assert_eq!(
            snapshots,
            vec![
                String::from("reading hello.txt"),
                String::from("tool-backed answer")
            ]
        );
        assert_eq!(response.session_id, "sess-stream-tool-1");
        assert_eq!(response.assistant_text, "tool-backed answer");
        assert_eq!(
            callback_payloads
                .lock()
                .expect("callback payload lock")
                .len(),
            1
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
        assert!(requests[1].contains("/responses/stream"));
    }

    #[test]
    fn tool_schema_normalization_adds_root_title_and_order_without_property_titles() {
        let tool_definition = provider_tool_definition(AppleFmProviderToolDefinition {
            name: String::from("lookup_secret"),
            description: Some(String::from("Look up a secret value.")),
            parameters: Some(json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "key": {"type": "string"},
                    "namespace": {"type": "string"}
                },
                "additionalProperties": false
            })),
        })
        .expect("tool definition should normalize");

        let schema_json = tool_definition.arguments_schema.clone_json_value();
        assert_eq!(schema_json["title"], "lookup_secret_arguments");
        assert_eq!(schema_json["x-order"], json!(["key", "namespace"]));
        assert_eq!(schema_json["required"], json!([]));
        assert!(schema_json.get("$schema").is_none());
        assert!(schema_json["properties"]["key"].get("title").is_none());
        assert!(
            schema_json["properties"]["namespace"]
                .get("title")
                .is_none()
        );
    }

    #[test]
    fn session_create_retries_once_without_transcript_on_invalid_json() {
        let create_request_bodies = Arc::new(Mutex::new(Vec::new()));
        let captured_create_request_bodies = Arc::clone(&create_request_bodies);
        let server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => {
                    let request_json: serde_json::Value =
                        serde_json::from_str(request.body.as_str()).expect("session create json");
                    captured_create_request_bodies
                        .lock()
                        .expect("request body lock")
                        .push(request_json.clone());
                    if request_json.get("transcript").is_some() {
                        FakeHttpResponse::json_status(
                            400,
                            json!({
                                "error": {
                                    "message": "Invalid JSON in transcript restore payload",
                                    "type": "invalid_request",
                                    "code": "invalid_request"
                                }
                            }),
                        )
                    } else {
                        FakeHttpResponse::json_ok(json!({
                            "session": {
                                "id": "sess-retry-1",
                                "instructions": request_json["instructions"],
                                "model": {
                                    "id": "apple-foundation-model",
                                    "use_case": "general",
                                    "guardrails": "default"
                                },
                                "tools": [{"name": "read_file"}],
                                "is_responding": false,
                                "transcript_json": "{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"
                            }
                        }))
                    }
                }
                ("POST", "/v1/sessions/sess-retry-1/responses") => {
                    FakeHttpResponse::json_ok(json!({
                        "session": {
                            "id": "sess-retry-1",
                            "instructions": "You are a helper",
                            "model": {
                                "id": "apple-foundation-model",
                                "use_case": "general",
                                "guardrails": "default"
                            },
                            "tools": [{"name": "read_file"}],
                            "is_responding": false,
                            "transcript_json": "{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"
                        },
                        "model": "apple-foundation-model",
                        "output": "tool-backed answer after retry",
                        "usage": {
                            "total_tokens_detail": {"value": 18, "truth": "estimated"}
                        }
                    }))
                }
                ("DELETE", "/v1/sessions/sess-retry-1") => FakeHttpResponse::json_ok(json!({})),
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let client = AppleFmProviderClient::new(AppleFmProviderConfig {
            base_url: server.base_url().to_string(),
            model: String::from("apple-foundation-model"),
            timeout: Duration::from_secs(5),
        })
        .expect("client");
        let response = client
            .respond_in_session_with_tools(
                Some("You are a helper"),
                AppleFmTranscript::default(),
                "read hello.txt",
                vec![AppleFmProviderToolDefinition {
                    name: String::from("read_file"),
                    description: Some(String::from("Read a file.")),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"}
                        },
                        "required": ["path"],
                        "additionalProperties": false
                    })),
                }],
                Arc::new(|tool_call: AppleFmProviderToolCall| {
                    Ok(json!({
                        "tool": tool_call.name,
                        "arguments": tool_call.arguments
                    })
                    .to_string())
                }),
            )
            .expect("tool session response");

        assert_eq!(response.session_id, "sess-retry-1");
        assert_eq!(response.assistant_text, "tool-backed answer after retry");

        let create_request_bodies = create_request_bodies.lock().expect("request body lock");
        assert_eq!(create_request_bodies.len(), 2);
        assert!(create_request_bodies[0].get("transcript").is_some());
        assert!(create_request_bodies[1].get("transcript").is_none());
        assert!(create_request_bodies[1].get("transcript_json").is_none());

        let requests = server.finish();
        assert_eq!(requests.len(), 4);
        assert!(requests[0].contains("POST /v1/sessions HTTP/1.1"));
        assert!(requests[1].contains("POST /v1/sessions HTTP/1.1"));
        assert!(requests[2].contains("POST /v1/sessions/sess-retry-1/responses HTTP/1.1"));
        assert!(requests[3].contains("DELETE /v1/sessions/sess-retry-1 HTTP/1.1"));
    }
}
