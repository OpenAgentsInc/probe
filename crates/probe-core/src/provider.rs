use std::fmt::{Display, Formatter};
use std::sync::Arc;

use probe_protocol::backend::{BackendKind, BackendProfile};
use probe_protocol::session::{
    BackendAvailabilityReceipt, BackendFailureReceipt, BackendTranscriptReceipt,
    BackendTurnReceipt, UsageMeasurement, UsageTruth,
};
use probe_provider_apple_fm::{
    AppleFmProviderClient, AppleFmProviderConfig, AppleFmProviderError, AppleFmProviderMessage,
    AppleFmProviderSessionResponse, AppleFmProviderToolCall, AppleFmProviderToolDefinition,
};
use probe_provider_openai::{
    ChatCompletionChunk, ChatMessage, ChatToolCall, ChatToolDefinitionEnvelope,
    OpenAiProviderClient, OpenAiProviderConfig, OpenAiProviderError,
};
use psionic_apple_fm::{
    AppleFmChatUsage, AppleFmErrorCode, AppleFmToolCallError, AppleFmTranscript,
    AppleFmUsageMeasurement, AppleFmUsageTruth,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlainTextMessageRole {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlainTextMessage {
    pub role: PlainTextMessageRole,
    pub content: String,
}

impl PlainTextMessage {
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: PlainTextMessageRole::System,
            content: content.into(),
        }
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: PlainTextMessageRole::User,
            content: content.into(),
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: PlainTextMessageRole::Assistant,
            content: content.into(),
        }
    }

    #[must_use]
    pub fn to_openai_message(&self) -> ChatMessage {
        match self.role {
            PlainTextMessageRole::System => ChatMessage::system(self.content.clone()),
            PlainTextMessageRole::User => ChatMessage::user(self.content.clone()),
            PlainTextMessageRole::Assistant => ChatMessage::assistant(self.content.clone()),
        }
    }

    #[must_use]
    pub fn to_apple_fm_message(&self) -> AppleFmProviderMessage {
        match self.role {
            PlainTextMessageRole::System => AppleFmProviderMessage::system(self.content.clone()),
            PlainTextMessageRole::User => AppleFmProviderMessage::user(self.content.clone()),
            PlainTextMessageRole::Assistant => {
                AppleFmProviderMessage::assistant(self.content.clone())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderUsageTruth {
    Exact,
    Estimated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderUsageMeasurement {
    pub value: u64,
    pub truth: ProviderUsageTruth,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderUsage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub prompt_tokens_detail: Option<ProviderUsageMeasurement>,
    pub completion_tokens_detail: Option<ProviderUsageMeasurement>,
    pub total_tokens_detail: Option<ProviderUsageMeasurement>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlainTextProviderResponse {
    pub response_id: String,
    pub response_model: String,
    pub assistant_text: Option<String>,
    pub usage: Option<ProviderUsage>,
    pub backend_receipt: Option<BackendTurnReceipt>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolLoopProviderResponse {
    pub response_id: String,
    pub response_model: String,
    pub usage: Option<ProviderUsage>,
    pub assistant_text: Option<String>,
    pub tool_calls: Option<Vec<ChatToolCall>>,
    pub backend_receipt: Option<BackendTurnReceipt>,
}

pub type OpenAiStreamChunkCallback<'a> = dyn FnMut(&ChatCompletionChunk) + 'a;

#[derive(Debug)]
pub enum ProviderError {
    OpenAi(OpenAiProviderError),
    AppleFm(AppleFmProviderError),
    UnsupportedFeature {
        backend: BackendKind,
        feature: &'static str,
    },
}

impl Display for ProviderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenAi(error) => write!(f, "{error}"),
            Self::AppleFm(error) => write!(f, "{error}"),
            Self::UnsupportedFeature { backend, feature } => {
                write!(f, "backend {:?} does not support {feature}", backend)
            }
        }
    }
}

impl std::error::Error for ProviderError {}

impl ProviderError {
    #[must_use]
    pub fn backend_turn_receipt(&self) -> Option<BackendTurnReceipt> {
        match self {
            Self::AppleFm(error) => {
                error
                    .foundation_models_error()
                    .map(|typed| BackendTurnReceipt {
                        failure: Some(BackendFailureReceipt {
                            family: String::from("apple_foundation_models"),
                            message: typed.message.clone(),
                            code: Some(typed.kind.label().to_string()),
                            retryable: Some(typed.is_retryable()),
                            failure_reason: typed.failure_reason.clone(),
                            recovery_suggestion: typed.recovery_suggestion.clone(),
                            refusal_explanation: typed.refusal_explanation.clone(),
                            tool_name: typed.tool_name.clone(),
                        }),
                        availability: matches!(typed.kind, AppleFmErrorCode::AssetsUnavailable)
                            .then(|| BackendAvailabilityReceipt {
                                ready: false,
                                reason_code: Some(typed.kind.label().to_string()),
                                message: typed
                                    .failure_reason
                                    .clone()
                                    .or_else(|| Some(typed.message.clone())),
                                platform: None,
                            }),
                        transcript: None,
                    })
            }
            Self::OpenAi(_) | Self::UnsupportedFeature { .. } => None,
        }
    }
}

pub fn complete_plain_text(
    profile: &BackendProfile,
    messages: Vec<PlainTextMessage>,
) -> Result<PlainTextProviderResponse, ProviderError> {
    match profile.kind {
        BackendKind::OpenAiChatCompletions => complete_openai_plain_text(profile, messages),
        BackendKind::AppleFmBridge => complete_apple_fm_plain_text(profile, messages),
    }
}

fn complete_openai_plain_text(
    profile: &BackendProfile,
    messages: Vec<PlainTextMessage>,
) -> Result<PlainTextProviderResponse, ProviderError> {
    complete_openai_plain_text_with_callback(profile, messages, None)
}

pub fn complete_openai_plain_text_with_callback(
    profile: &BackendProfile,
    messages: Vec<PlainTextMessage>,
    callback: Option<&mut OpenAiStreamChunkCallback<'_>>,
) -> Result<PlainTextProviderResponse, ProviderError> {
    let provider_config = OpenAiProviderConfig::from_backend_profile(profile);
    let provider = OpenAiProviderClient::new(provider_config).map_err(ProviderError::OpenAi)?;
    let request_messages = messages
        .iter()
        .map(PlainTextMessage::to_openai_message)
        .collect::<Vec<_>>();
    let response = match callback {
        Some(callback) => {
            let mut streaming_config = provider.config().clone();
            streaming_config.stream = true;
            let request =
                probe_provider_openai::ChatCompletionRequest::from_config(&streaming_config, request_messages);
            provider
                .send_chat_completion_with_callback(&request, callback)
                .map_err(ProviderError::OpenAi)?
        }
        None => provider
            .chat_completion(request_messages)
            .map_err(ProviderError::OpenAi)?,
    };
    let assistant_text = response.first_message_text().map(ToOwned::to_owned);
    Ok(PlainTextProviderResponse {
        response_id: response.id,
        response_model: response.model,
        assistant_text,
        usage: response.usage.map(provider_usage_from_openai),
        backend_receipt: None,
    })
}

fn complete_apple_fm_plain_text(
    profile: &BackendProfile,
    messages: Vec<PlainTextMessage>,
) -> Result<PlainTextProviderResponse, ProviderError> {
    let provider_config = AppleFmProviderConfig::from_backend_profile(profile);
    let provider = AppleFmProviderClient::new(provider_config).map_err(ProviderError::AppleFm)?;
    let response = provider
        .chat_completion(
            messages
                .iter()
                .map(PlainTextMessage::to_apple_fm_message)
                .collect(),
        )
        .map_err(ProviderError::AppleFm)?;
    Ok(PlainTextProviderResponse {
        response_id: response.id,
        response_model: response.model,
        assistant_text: response.assistant_text,
        usage: response.usage.as_ref().map(provider_usage_from_apple),
        backend_receipt: None,
    })
}

fn provider_usage_from_apple(usage: &AppleFmChatUsage) -> ProviderUsage {
    ProviderUsage {
        prompt_tokens: usage.prompt_tokens_best_effort(),
        completion_tokens: usage.completion_tokens_best_effort(),
        total_tokens: usage.total_tokens_best_effort(),
        prompt_tokens_detail: usage_measurement(
            usage.prompt_tokens,
            usage.prompt_tokens_detail.as_ref(),
        ),
        completion_tokens_detail: usage_measurement(
            usage.completion_tokens,
            usage.completion_tokens_detail.as_ref(),
        ),
        total_tokens_detail: usage_measurement(
            usage.total_tokens,
            usage.total_tokens_detail.as_ref(),
        ),
    }
}

fn usage_measurement(
    exact: Option<u64>,
    detail: Option<&AppleFmUsageMeasurement>,
) -> Option<ProviderUsageMeasurement> {
    match (exact, detail) {
        (_, Some(detail)) => Some(ProviderUsageMeasurement {
            value: detail.value,
            truth: match detail.truth {
                AppleFmUsageTruth::Exact => ProviderUsageTruth::Exact,
                AppleFmUsageTruth::Estimated => ProviderUsageTruth::Estimated,
            },
        }),
        (Some(value), None) => Some(ProviderUsageMeasurement {
            value,
            truth: ProviderUsageTruth::Exact,
        }),
        (None, None) => None,
    }
}

pub fn observability_usage_measurement(
    detail: Option<&ProviderUsageMeasurement>,
) -> Option<UsageMeasurement> {
    detail.map(|detail| UsageMeasurement {
        value: detail.value,
        truth: match detail.truth {
            ProviderUsageTruth::Exact => UsageTruth::Exact,
            ProviderUsageTruth::Estimated => UsageTruth::Estimated,
        },
    })
}

pub fn openai_tool_messages_from_plain_text(messages: &[PlainTextMessage]) -> Vec<ChatMessage> {
    messages
        .iter()
        .map(PlainTextMessage::to_openai_message)
        .collect()
}

pub fn openai_tool_loop_response(
    profile: &BackendProfile,
    messages: Vec<ChatMessage>,
    tools: Vec<ChatToolDefinitionEnvelope>,
    tool_choice: Option<probe_provider_openai::ChatToolChoice>,
    parallel_tool_calls: Option<bool>,
) -> Result<ToolLoopProviderResponse, ProviderError> {
    openai_tool_loop_response_with_callback(
        profile,
        messages,
        tools,
        tool_choice,
        parallel_tool_calls,
        None,
    )
}

pub fn openai_tool_loop_response_with_callback(
    profile: &BackendProfile,
    messages: Vec<ChatMessage>,
    tools: Vec<ChatToolDefinitionEnvelope>,
    tool_choice: Option<probe_provider_openai::ChatToolChoice>,
    parallel_tool_calls: Option<bool>,
    callback: Option<&mut OpenAiStreamChunkCallback<'_>>,
) -> Result<ToolLoopProviderResponse, ProviderError> {
    if profile.kind != BackendKind::OpenAiChatCompletions {
        return Err(ProviderError::UnsupportedFeature {
            backend: profile.kind,
            feature: "openai-style tool calls",
        });
    }

    let provider_config = OpenAiProviderConfig::from_backend_profile(profile);
    let provider =
        OpenAiProviderClient::new(provider_config.clone()).map_err(ProviderError::OpenAi)?;
    let mut request =
        probe_provider_openai::ChatCompletionRequest::from_config(&provider_config, messages)
            .with_tools(tools, tool_choice, parallel_tool_calls);
    if callback.is_some() {
        request.stream = true;
    }
    let response = match callback {
        Some(callback) => provider
            .send_chat_completion_with_callback(&request, callback)
            .map_err(ProviderError::OpenAi)?,
        None => provider
            .send_chat_completion(&request)
            .map_err(ProviderError::OpenAi)?,
    };
    let tool_calls = response.first_tool_calls().map(ToOwned::to_owned);
    let assistant_text = response.first_message_text().map(ToOwned::to_owned);
    Ok(ToolLoopProviderResponse {
        response_id: response.id,
        response_model: response.model,
        usage: response.usage.map(provider_usage_from_openai),
        assistant_text,
        tool_calls,
        backend_receipt: None,
    })
}

pub fn apple_fm_tool_loop_response(
    profile: &BackendProfile,
    system_prompt: Option<&str>,
    transcript: AppleFmTranscript,
    prompt: &str,
    tools: Vec<AppleFmProviderToolDefinition>,
    callback: Arc<
        dyn Fn(AppleFmProviderToolCall) -> Result<String, AppleFmToolCallError> + Send + Sync,
    >,
) -> Result<PlainTextProviderResponse, ProviderError> {
    if profile.kind != BackendKind::AppleFmBridge {
        return Err(ProviderError::UnsupportedFeature {
            backend: profile.kind,
            feature: "apple-fm session tool callbacks",
        });
    }

    let provider_config = AppleFmProviderConfig::from_backend_profile(profile);
    let provider = AppleFmProviderClient::new(provider_config).map_err(ProviderError::AppleFm)?;
    let response = provider
        .respond_in_session_with_tools(system_prompt, transcript, prompt, tools, callback)
        .map_err(ProviderError::AppleFm)?;
    Ok(plain_text_provider_response_from_apple_session(response))
}

fn plain_text_provider_response_from_apple_session(
    response: AppleFmProviderSessionResponse,
) -> PlainTextProviderResponse {
    PlainTextProviderResponse {
        response_id: response.id,
        response_model: response.model,
        assistant_text: Some(response.assistant_text),
        usage: response.usage.as_ref().map(provider_usage_from_apple),
        backend_receipt: response.transcript.and_then(|transcript| {
            transcript
                .to_json_string()
                .ok()
                .map(|payload| BackendTurnReceipt {
                    failure: None,
                    availability: None,
                    transcript: Some(BackendTranscriptReceipt {
                        format: String::from("foundation_models.transcript.v1"),
                        payload,
                    }),
                })
        }),
    }
}

fn provider_usage_from_openai(
    usage: probe_provider_openai::ChatCompletionUsage,
) -> ProviderUsage {
    ProviderUsage {
        prompt_tokens: Some(u64::from(usage.prompt_tokens)),
        completion_tokens: Some(u64::from(usage.completion_tokens)),
        total_tokens: Some(u64::from(usage.total_tokens)),
        prompt_tokens_detail: Some(ProviderUsageMeasurement {
            value: u64::from(usage.prompt_tokens),
            truth: ProviderUsageTruth::Exact,
        }),
        completion_tokens_detail: Some(ProviderUsageMeasurement {
            value: u64::from(usage.completion_tokens),
            truth: ProviderUsageTruth::Exact,
        }),
        total_tokens_detail: Some(ProviderUsageMeasurement {
            value: u64::from(usage.total_tokens),
            truth: ProviderUsageTruth::Exact,
        }),
    }
}
