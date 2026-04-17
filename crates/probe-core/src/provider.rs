use std::env;
use std::fmt::{Display, Formatter};
use std::path::Path;
use std::sync::Arc;

use probe_openai_auth::{OpenAiAuthError, OpenAiCodexAuthController, OpenAiCodexRoute};
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
    OpenAiProviderClient, OpenAiProviderConfig, OpenAiProviderError, OpenAiRequestAuth,
};
use psionic_apple_fm::{
    AppleFmChatUsage, AppleFmErrorCode, AppleFmTextStreamEvent, AppleFmToolCallError,
    AppleFmTranscript, AppleFmUsageMeasurement, AppleFmUsageTruth,
};

const DEFAULT_OPENAI_API_BASE_URL: &str = "https://api.openai.com/v1";

#[derive(Clone, Debug)]
struct ResolvedCodexAttempt {
    provider_config: OpenAiProviderConfig,
    account_key: Option<String>,
}

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
pub type AppleFmStreamEventCallback<'a> = dyn FnMut(&AppleFmTextStreamEvent) + 'a;

#[derive(Clone, Copy, Debug, Default)]
pub struct OpenAiRequestContext<'a> {
    pub probe_home: Option<&'a Path>,
    pub session_id: Option<&'a str>,
}

#[derive(Debug)]
pub enum ProviderError {
    OpenAi(OpenAiProviderError),
    AppleFm(AppleFmProviderError),
    Auth(OpenAiAuthError),
    MissingCodexSubscriptionAuth {
        path: String,
    },
    MissingOpenAiApiKey {
        profile_name: String,
        env_var: String,
    },
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
            Self::Auth(error) => write!(f, "{error}"),
            Self::MissingCodexSubscriptionAuth { path } => write!(
                f,
                "codex subscription auth is missing at {path}; run `probe codex login --method browser` locally or `probe codex login --method headless` on worker machines"
            ),
            Self::MissingOpenAiApiKey {
                profile_name,
                env_var,
            } => write!(
                f,
                "backend profile `{profile_name}` requires bearer auth from env `{env_var}`; export it before running Probe against that OpenAI-compatible lane"
            ),
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
            Self::OpenAi(_)
            | Self::Auth(_)
            | Self::MissingCodexSubscriptionAuth { .. }
            | Self::MissingOpenAiApiKey { .. }
            | Self::UnsupportedFeature { .. } => None,
        }
    }
}

pub fn complete_plain_text(
    profile: &BackendProfile,
    messages: Vec<PlainTextMessage>,
) -> Result<PlainTextProviderResponse, ProviderError> {
    complete_plain_text_with_context(profile, messages, OpenAiRequestContext::default())
}

pub fn complete_plain_text_with_context(
    profile: &BackendProfile,
    messages: Vec<PlainTextMessage>,
    context: OpenAiRequestContext<'_>,
) -> Result<PlainTextProviderResponse, ProviderError> {
    match profile.kind {
        BackendKind::OpenAiChatCompletions | BackendKind::OpenAiCodexSubscription => {
            complete_openai_plain_text(profile, messages, context)
        }
        BackendKind::AppleFmBridge => complete_apple_fm_plain_text(profile, messages),
    }
}

fn complete_openai_plain_text(
    profile: &BackendProfile,
    messages: Vec<PlainTextMessage>,
    context: OpenAiRequestContext<'_>,
) -> Result<PlainTextProviderResponse, ProviderError> {
    complete_openai_plain_text_with_context_and_callback(profile, messages, context, None)
}

pub fn complete_openai_plain_text_with_callback(
    profile: &BackendProfile,
    messages: Vec<PlainTextMessage>,
    callback: Option<&mut OpenAiStreamChunkCallback<'_>>,
) -> Result<PlainTextProviderResponse, ProviderError> {
    complete_openai_plain_text_with_context_and_callback(
        profile,
        messages,
        OpenAiRequestContext::default(),
        callback,
    )
}

pub fn complete_openai_plain_text_with_context_and_callback(
    profile: &BackendProfile,
    messages: Vec<PlainTextMessage>,
    context: OpenAiRequestContext<'_>,
    callback: Option<&mut OpenAiStreamChunkCallback<'_>>,
) -> Result<PlainTextProviderResponse, ProviderError> {
    let request_messages = messages
        .iter()
        .map(PlainTextMessage::to_openai_message)
        .collect::<Vec<_>>();
    let mut callback = callback;
    let response =
        execute_openai_request_with_routing(profile, context, |provider, provider_config| {
            match callback.as_mut() {
                Some(callback) => {
                    let mut streaming_config = provider_config.clone();
                    streaming_config.stream = true;
                    let request = probe_provider_openai::ChatCompletionRequest::from_config(
                        &streaming_config,
                        request_messages.clone(),
                    );
                    provider.send_chat_completion_with_callback(&request, &mut **callback)
                }
                None => provider.chat_completion(request_messages.clone()),
            }
        })?;
    let assistant_text = response
        .first_message_text()
        .map(normalize_openai_assistant_text);
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
    complete_apple_fm_plain_text_with_callback(profile, messages, None)
}

pub fn complete_apple_fm_plain_text_with_callback(
    profile: &BackendProfile,
    messages: Vec<PlainTextMessage>,
    callback: Option<&mut AppleFmStreamEventCallback<'_>>,
) -> Result<PlainTextProviderResponse, ProviderError> {
    let provider_config = AppleFmProviderConfig::from_backend_profile(profile);
    let provider = AppleFmProviderClient::new(provider_config).map_err(ProviderError::AppleFm)?;
    let response = provider
        .chat_completion_with_callback(
            messages
                .iter()
                .map(PlainTextMessage::to_apple_fm_message)
                .collect(),
            callback,
        )
        .map_err(ProviderError::AppleFm)?;
    Ok(PlainTextProviderResponse {
        response_id: response.id,
        response_model: response.model,
        assistant_text: response
            .assistant_text
            .as_deref()
            .map(normalize_openai_assistant_text),
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
    openai_tool_loop_response_with_context(
        profile,
        messages,
        tools,
        tool_choice,
        parallel_tool_calls,
        OpenAiRequestContext::default(),
    )
}

pub fn openai_tool_loop_response_with_context(
    profile: &BackendProfile,
    messages: Vec<ChatMessage>,
    tools: Vec<ChatToolDefinitionEnvelope>,
    tool_choice: Option<probe_provider_openai::ChatToolChoice>,
    parallel_tool_calls: Option<bool>,
    context: OpenAiRequestContext<'_>,
) -> Result<ToolLoopProviderResponse, ProviderError> {
    openai_tool_loop_response_with_callback(
        profile,
        messages,
        tools,
        tool_choice,
        parallel_tool_calls,
        context,
        None,
    )
}

pub fn openai_tool_loop_response_with_callback(
    profile: &BackendProfile,
    messages: Vec<ChatMessage>,
    tools: Vec<ChatToolDefinitionEnvelope>,
    tool_choice: Option<probe_provider_openai::ChatToolChoice>,
    parallel_tool_calls: Option<bool>,
    context: OpenAiRequestContext<'_>,
    callback: Option<&mut OpenAiStreamChunkCallback<'_>>,
) -> Result<ToolLoopProviderResponse, ProviderError> {
    if !matches!(
        profile.kind,
        BackendKind::OpenAiChatCompletions | BackendKind::OpenAiCodexSubscription
    ) {
        return Err(ProviderError::UnsupportedFeature {
            backend: profile.kind,
            feature: "openai-style tool calls",
        });
    }

    let mut callback = callback;
    let response =
        execute_openai_request_with_routing(profile, context, |provider, provider_config| {
            let mut request = probe_provider_openai::ChatCompletionRequest::from_config(
                provider_config,
                messages.clone(),
            )
            .with_tools(tools.clone(), tool_choice.clone(), parallel_tool_calls);
            if callback.is_some() {
                request.stream = true;
            }
            match callback.as_mut() {
                Some(callback) => {
                    provider.send_chat_completion_with_callback(&request, &mut **callback)
                }
                None => provider.send_chat_completion(&request),
            }
        })?;
    let tool_calls = response.first_tool_calls().map(ToOwned::to_owned);
    let assistant_text = response
        .first_message_text()
        .map(normalize_openai_assistant_text);
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
    apple_fm_tool_loop_response_with_callback(
        profile,
        system_prompt,
        transcript,
        prompt,
        tools,
        callback,
        None,
    )
}

pub fn apple_fm_tool_loop_response_with_callback(
    profile: &BackendProfile,
    system_prompt: Option<&str>,
    transcript: AppleFmTranscript,
    prompt: &str,
    tools: Vec<AppleFmProviderToolDefinition>,
    callback: Arc<
        dyn Fn(AppleFmProviderToolCall) -> Result<String, AppleFmToolCallError> + Send + Sync,
    >,
    stream_callback: Option<&mut AppleFmStreamEventCallback<'_>>,
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
        .respond_in_session_with_tools_and_callback(
            system_prompt,
            transcript,
            prompt,
            tools,
            callback,
            stream_callback,
        )
        .map_err(ProviderError::AppleFm)?;
    Ok(plain_text_provider_response_from_apple_session(response))
}

fn plain_text_provider_response_from_apple_session(
    response: AppleFmProviderSessionResponse,
) -> PlainTextProviderResponse {
    PlainTextProviderResponse {
        response_id: response.id,
        response_model: response.model,
        assistant_text: Some(normalize_openai_assistant_text(
            response.assistant_text.as_str(),
        )),
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

fn execute_openai_request_with_routing<T>(
    profile: &BackendProfile,
    context: OpenAiRequestContext<'_>,
    mut execute: impl FnMut(
        &OpenAiProviderClient,
        &OpenAiProviderConfig,
    ) -> Result<T, OpenAiProviderError>,
) -> Result<T, ProviderError> {
    if profile.kind != BackendKind::OpenAiCodexSubscription {
        let provider_config = openai_provider_config_from_profile(profile, context)?;
        let provider =
            OpenAiProviderClient::new(provider_config.clone()).map_err(ProviderError::OpenAi)?;
        return execute(&provider, &provider_config).map_err(ProviderError::OpenAi);
    }

    let probe_home =
        context
            .probe_home
            .ok_or_else(|| ProviderError::MissingCodexSubscriptionAuth {
                path: String::from("PROBE_HOME/auth/openai-codex.json"),
            })?;
    let controller = OpenAiCodexAuthController::new(probe_home).map_err(ProviderError::Auth)?;
    let auth_path = controller.store().path().display().to_string();
    let attempts = codex_provider_attempts_from_profile(profile, context, &controller)?;
    if attempts.is_empty() {
        return Err(ProviderError::MissingCodexSubscriptionAuth { path: auth_path });
    }

    let mut last_rate_limit_error = None;
    for attempt in attempts {
        if let Some(account_key) = attempt.account_key.as_deref() {
            controller
                .mark_selected_account(account_key)
                .map_err(ProviderError::Auth)?;
        }
        let provider = OpenAiProviderClient::new(attempt.provider_config.clone())
            .map_err(ProviderError::OpenAi)?;
        match execute(&provider, &attempt.provider_config) {
            Ok(response) => return Ok(response),
            Err(OpenAiProviderError::HttpStatus { status: 429, body }) => {
                if let Some(account_key) = attempt.account_key.as_deref() {
                    controller
                        .mark_account_rate_limited(account_key, body.as_str())
                        .map_err(ProviderError::Auth)?;
                }
                last_rate_limit_error =
                    Some(ProviderError::OpenAi(OpenAiProviderError::HttpStatus {
                        status: 429,
                        body,
                    }));
            }
            Err(error) => return Err(ProviderError::OpenAi(error)),
        }
    }

    Err(last_rate_limit_error
        .unwrap_or(ProviderError::MissingCodexSubscriptionAuth { path: auth_path }))
}

fn codex_provider_attempts_from_profile(
    profile: &BackendProfile,
    context: OpenAiRequestContext<'_>,
    controller: &OpenAiCodexAuthController,
) -> Result<Vec<ResolvedCodexAttempt>, ProviderError> {
    let routing_plan = controller
        .routing_plan(Some(codex_api_key_env(profile)))
        .map_err(ProviderError::Auth)?;
    routing_plan
        .routes
        .iter()
        .map(|route| codex_provider_attempt_from_route(profile, context, route))
        .collect()
}

fn codex_provider_attempt_from_route(
    profile: &BackendProfile,
    context: OpenAiRequestContext<'_>,
    route: &OpenAiCodexRoute,
) -> Result<ResolvedCodexAttempt, ProviderError> {
    let mut config = OpenAiProviderConfig::from_backend_profile(profile);
    decorate_codex_provider_config(&mut config, context, None);
    match route {
        OpenAiCodexRoute::SubscriptionAccount(account) => {
            config.auth = OpenAiRequestAuth::BearerToken(account.record.access.clone());
            if let Some(account_id) = account.record.account_id.as_deref() {
                config
                    .extra_headers
                    .insert(String::from("ChatGPT-Account-Id"), account_id.to_string());
            }
            Ok(ResolvedCodexAttempt {
                provider_config: config,
                account_key: Some(account.account_key.clone()),
            })
        }
        OpenAiCodexRoute::ApiKeyFallback(route) => {
            config.base_url = String::from(DEFAULT_OPENAI_API_BASE_URL);
            config.auth = OpenAiRequestAuth::BearerToken(route.api_key.clone());
            config.extra_headers.remove("ChatGPT-Account-Id");
            Ok(ResolvedCodexAttempt {
                provider_config: config,
                account_key: None,
            })
        }
    }
}

fn decorate_codex_provider_config(
    config: &mut OpenAiProviderConfig,
    context: OpenAiRequestContext<'_>,
    account_id: Option<&str>,
) {
    config
        .extra_headers
        .insert(String::from("originator"), String::from("probe"));
    config
        .extra_headers
        .insert(String::from("User-Agent"), codex_user_agent());
    if let Some(account_id) = account_id {
        config
            .extra_headers
            .insert(String::from("ChatGPT-Account-Id"), account_id.to_string());
    }
    if let Some(session_id) = context.session_id {
        config
            .extra_headers
            .insert(String::from("session_id"), String::from(session_id));
    }
}

fn codex_api_key_env(profile: &BackendProfile) -> &str {
    if profile.api_key_env.trim().is_empty() {
        "PROBE_OPENAI_API_KEY"
    } else {
        profile.api_key_env.as_str()
    }
}

fn openai_provider_config_from_profile(
    profile: &BackendProfile,
    context: OpenAiRequestContext<'_>,
) -> Result<OpenAiProviderConfig, ProviderError> {
    let mut config = OpenAiProviderConfig::from_backend_profile(profile);
    if profile.kind != BackendKind::OpenAiCodexSubscription {
        if !profile.api_key_env.trim().is_empty() {
            let token = env::var(profile.api_key_env.as_str())
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ProviderError::MissingOpenAiApiKey {
                    profile_name: profile.name.clone(),
                    env_var: profile.api_key_env.clone(),
                })?;
            config.auth = OpenAiRequestAuth::BearerToken(token);
        }
        return Ok(config);
    }

    let probe_home =
        context
            .probe_home
            .ok_or_else(|| ProviderError::MissingCodexSubscriptionAuth {
                path: String::from("PROBE_HOME/auth/openai-codex.json"),
            })?;
    let controller = OpenAiCodexAuthController::new(probe_home).map_err(ProviderError::Auth)?;
    let auth_path = controller.store().path().display().to_string();
    let attempt = codex_provider_attempts_from_profile(profile, context, &controller)?
        .into_iter()
        .next()
        .ok_or(ProviderError::MissingCodexSubscriptionAuth { path: auth_path })?;
    Ok(attempt.provider_config)
}

fn codex_user_agent() -> String {
    format!(
        "probe/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Strips leading `response_id` / `model` / `response` rows that some providers echo ahead of the
/// real assistant body. Applied in [`normalize_openai_stream_display_text`] so streaming deltas
/// never surface that prefix (fixes one-frame flashes in the TUI before JSON envelope parsing).
#[must_use]
pub fn strip_leading_probe_response_noise(text: &str) -> &str {
    let mut slice = text;
    let mut stripped_any = false;
    loop {
        slice = slice.trim_start_matches(|c| c == '\n' || c == '\r');
        if slice.is_empty() {
            return "";
        }
        let (line, rest) = match slice.find('\n') {
            Some(i) => (&slice[..i], Some(&slice[i + 1..])),
            None => {
                return if is_partial_leading_response_noise_line(slice, stripped_any) {
                    ""
                } else {
                    slice
                };
            }
        };
        if is_leading_response_noise_line(line) {
            stripped_any = true;
            slice = rest.unwrap_or("");
            continue;
        }
        return slice;
    }
}

fn is_leading_response_noise_line(line: &str) -> bool {
    let s = normalized_leading_response_noise_candidate(line);
    s.starts_with("response_id:") || s.starts_with("model:") || s == "response"
}

fn is_partial_leading_response_noise_line(line: &str, stripped_any: bool) -> bool {
    let s = normalized_leading_response_noise_candidate(line);
    if s.is_empty() {
        return false;
    }

    "response_id:".starts_with(s)
        || "model:".starts_with(s)
        || s == "response"
        || (stripped_any && "response".starts_with(s))
}

fn normalized_leading_response_noise_candidate(line: &str) -> &str {
    let mut s = line.trim().trim_end_matches('\r');
    s = s.trim_start();
    if let Some(rest) = s.strip_prefix('•') {
        s = rest.trim_start();
    } else if let Some(rest) = s.strip_prefix('-') {
        s = rest.trim_start();
    } else if let Some(rest) = s.strip_prefix('*') {
        s = rest.trim_start();
    }
    s.trim_start()
}

#[must_use]
pub fn normalize_openai_assistant_text(raw: &str) -> String {
    normalize_openai_stream_display_text(raw)
}

#[must_use]
pub fn normalize_openai_stream_display_text(raw: &str) -> String {
    let head = strip_leading_probe_response_noise(raw);
    let middle = normalized_message_envelope_content(head)
        .or_else(|| partial_message_envelope_content(head));
    let text = match middle {
        Some(s) => s,
        None => head.to_string(),
    };
    strip_leading_probe_response_noise(text.as_str()).to_string()
}

fn normalized_message_envelope_content(raw: &str) -> Option<String> {
    let parsed = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    let object = parsed.as_object()?;
    if object.get("kind").and_then(|value| value.as_str()) != Some("message") {
        return None;
    }
    object
        .get("content")
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
}

fn partial_message_envelope_content(raw: &str) -> Option<String> {
    let trimmed = raw.trim_start();
    if !looks_like_message_envelope_prefix(trimmed) {
        return None;
    }

    let Some(content_key_index) = trimmed.find("\"content\"") else {
        return Some(String::new());
    };
    let after_key = &trimmed[content_key_index + "\"content\"".len()..];
    let colon_index = after_key.find(':')?;
    let after_colon = after_key[colon_index + 1..].trim_start();
    let quoted = after_colon.strip_prefix('"')?;
    Some(decode_partial_json_string(quoted))
}

fn looks_like_message_envelope_prefix(raw: &str) -> bool {
    let trimmed = raw.trim_start();
    trimmed.starts_with("{\"kind\":\"message")
        || trimmed.starts_with("{ \"kind\":\"message")
        || trimmed.starts_with("{\"kind\": \"message")
        || trimmed.starts_with("{ \"kind\": \"message")
}

fn decode_partial_json_string(raw: &str) -> String {
    let chars = raw.chars().collect::<Vec<_>>();
    let mut decoded = String::new();
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '"' => break,
            '\\' => {
                index += 1;
                if index >= chars.len() {
                    break;
                }
                match chars[index] {
                    '"' => decoded.push('"'),
                    '\\' => decoded.push('\\'),
                    '/' => decoded.push('/'),
                    'b' => decoded.push('\u{0008}'),
                    'f' => decoded.push('\u{000C}'),
                    'n' => decoded.push('\n'),
                    'r' => decoded.push('\r'),
                    't' => decoded.push('\t'),
                    'u' => {
                        if index + 4 >= chars.len() {
                            break;
                        }
                        let hex = chars[index + 1..=index + 4].iter().collect::<String>();
                        if let Ok(codepoint) = u32::from_str_radix(hex.as_str(), 16)
                            && let Some(value) = char::from_u32(codepoint)
                        {
                            decoded.push(value);
                            index += 4;
                        } else {
                            break;
                        }
                    }
                    other => decoded.push(other),
                }
            }
            character => decoded.push(character),
        }
        index += 1;
    }
    decoded
}

fn provider_usage_from_openai(usage: probe_provider_openai::ChatCompletionUsage) -> ProviderUsage {
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

#[cfg(test)]
mod tests {
    use std::fs;

    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use probe_test_support::{FakeHttpResponse, FakeOpenAiServer};
    use serde_json::json;
    use tempfile::tempdir;

    use crate::backend_profiles::openai_codex_subscription;

    use super::{
        DEFAULT_OPENAI_API_BASE_URL, OpenAiRequestAuth, OpenAiRequestContext, PlainTextMessage,
        ProviderError, complete_plain_text_with_context, normalize_openai_assistant_text,
        normalize_openai_stream_display_text, openai_provider_config_from_profile,
        strip_leading_probe_response_noise,
    };

    fn explicit_openai_env_profile(env_var: &str) -> BackendProfile {
        BackendProfile {
            name: String::from("test-openai-chat-profile"),
            kind: BackendKind::OpenAiChatCompletions,
            base_url: String::from("http://127.0.0.1:8080/v1"),
            model: String::from("tiny-qwen35"),
            reasoning_level: None,
            api_key_env: env_var.to_string(),
            timeout_secs: 30,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
            control_plane: None,
            psionic_mesh: None,
        }
    }

    #[test]
    fn openai_assistant_text_unwraps_message_envelope() {
        let raw = r#"{"kind":"message","content":"hello from qwen"}"#;
        assert_eq!(normalize_openai_assistant_text(raw), "hello from qwen");
    }

    #[test]
    fn openai_assistant_text_unwraps_truncated_message_envelope() {
        let raw = r#"{"kind":"message","content":"hello from qwen"#;
        assert_eq!(normalize_openai_assistant_text(raw), "hello from qwen");
    }

    #[test]
    fn openai_stream_display_text_suppresses_envelope_prefix_until_content_arrives() {
        assert_eq!(
            normalize_openai_stream_display_text(r#"{"kind":"message","#),
            ""
        );
        assert_eq!(
            normalize_openai_stream_display_text(r#"{"kind":"message","content":"hello\nwor"#),
            "hello\nwor"
        );
    }

    #[test]
    fn openai_stream_strips_provider_metadata_lines_before_envelope() {
        let raw = concat!(
            "response_id: resp_1\n",
            "model: gpt-test\n",
            "response\n",
            r#"{"kind":"message","content":"Hi"}"#
        );
        assert_eq!(normalize_openai_stream_display_text(raw), "Hi");
    }

    #[test]
    fn strip_leading_probe_response_noise_strips_bullets() {
        let raw = "• response_id: x\n- model: y\nresponse\n\nHi";
        assert_eq!(strip_leading_probe_response_noise(raw), "Hi");
    }

    #[test]
    fn openai_stream_display_text_hides_partial_leading_response_noise() {
        assert_eq!(normalize_openai_stream_display_text("response_"), "");
        assert_eq!(
            normalize_openai_stream_display_text("response_id: resp_1\nmod"),
            ""
        );
        assert_eq!(
            normalize_openai_stream_display_text("response_id: resp_1\nmodel: gpt-test\nres"),
            ""
        );
    }

    #[test]
    fn openai_stream_display_text_keeps_real_text_once_it_diverges_from_noise_prefix() {
        assert_eq!(normalize_openai_stream_display_text("modeling"), "modeling");
        assert_eq!(
            normalize_openai_stream_display_text("response_id: resp_1\nhello"),
            "hello"
        );
    }

    #[test]
    fn openai_assistant_text_leaves_plain_content_alone() {
        assert_eq!(
            normalize_openai_assistant_text("plain assistant text"),
            "plain assistant text"
        );
    }

    #[test]
    fn codex_backend_requires_subscription_auth_state() {
        let error = complete_plain_text_with_context(
            &openai_codex_subscription(),
            vec![PlainTextMessage::user("hello")],
            OpenAiRequestContext::default(),
        )
        .expect_err("codex request without auth should fail");

        assert!(matches!(
            error,
            ProviderError::MissingCodexSubscriptionAuth { .. }
        ));
    }

    #[test]
    fn openai_chat_backend_requires_explicit_api_key_env_when_configured() {
        let profile = explicit_openai_env_profile("PROBE_TEST_OPENAI_API_KEY_REQUIRED");
        let error = openai_provider_config_from_profile(&profile, OpenAiRequestContext::default())
            .expect_err("configured env-backed profile should fail without its bearer env");

        assert!(matches!(error, ProviderError::MissingOpenAiApiKey { .. }));
    }

    #[test]
    fn openai_chat_backend_uses_configured_api_key_env() {
        let profile = explicit_openai_env_profile("PROBE_TEST_OPENAI_API_KEY_PRESENT");
        // SAFETY: this test uses a unique env key and restores process state before exit.
        unsafe {
            std::env::set_var("PROBE_TEST_OPENAI_API_KEY_PRESENT", "probe-test-bearer");
        }

        let config = openai_provider_config_from_profile(&profile, OpenAiRequestContext::default())
            .expect("configured env-backed profile should resolve its bearer");

        assert_eq!(
            config.auth,
            OpenAiRequestAuth::BearerToken(String::from("probe-test-bearer"))
        );

        // SAFETY: this test removes the unique env key it created.
        unsafe {
            std::env::remove_var("PROBE_TEST_OPENAI_API_KEY_PRESENT");
        }
    }

    #[test]
    fn codex_backend_uses_api_key_fallback_when_all_accounts_are_limited() {
        let temp = tempdir().expect("temp dir");
        let auth_path = temp.path().join("auth/openai-codex.json");
        fs::create_dir_all(auth_path.parent().expect("auth dir")).expect("create auth dir");
        fs::write(
            &auth_path,
            serde_json::to_vec_pretty(&json!({
                "version": 2,
                "selected_account_key": "acct:acct-limited",
                "accounts": [
                    {
                        "key": "acct:acct-limited",
                        "refresh": "refresh-limited",
                        "access": "access-limited",
                        "expires": u64::MAX / 2,
                        "account_id": "acct-limited",
                        "added_at_ms": 10,
                        "last_selected_at_ms": 10,
                        "rate_limits": {
                            "fetched_at_ms": u64::MAX / 4,
                            "plan_type": "pro",
                            "allowed": false,
                            "limit_reached": true,
                            "primary_window": {
                                "used_percent": 100,
                                "limit_window_seconds": 604800,
                                "reset_after_seconds": 1800,
                                "reset_at": 1776793745u64
                            }
                        }
                    }
                ]
            }))
            .expect("serialize auth state"),
        )
        .expect("write auth state");
        let mut profile = openai_codex_subscription();
        profile.api_key_env = String::from("PROBE_TEST_CODEX_FALLBACK_API_KEY");
        let previous_api_key = std::env::var("PROBE_TEST_CODEX_FALLBACK_API_KEY").ok();
        // SAFETY: this test restores the prior process env state before exit.
        unsafe {
            std::env::set_var("PROBE_TEST_CODEX_FALLBACK_API_KEY", "probe-test-fallback");
        }
        let config = openai_provider_config_from_profile(
            &profile,
            OpenAiRequestContext {
                probe_home: Some(temp.path()),
                session_id: Some("sess-fallback"),
            },
        )
        .expect("codex provider config");
        assert_eq!(config.base_url, DEFAULT_OPENAI_API_BASE_URL);
        assert_eq!(
            config.auth,
            OpenAiRequestAuth::BearerToken(String::from("probe-test-fallback"))
        );
        assert!(
            !config.extra_headers.contains_key("ChatGPT-Account-Id"),
            "api key fallback must not send a ChatGPT account header"
        );
        // SAFETY: this test restores the prior process env state.
        unsafe {
            match previous_api_key {
                Some(value) => std::env::set_var("PROBE_TEST_CODEX_FALLBACK_API_KEY", value),
                None => std::env::remove_var("PROBE_TEST_CODEX_FALLBACK_API_KEY"),
            }
        }
    }

    #[test]
    fn codex_request_rotates_to_next_account_after_429() {
        let temp = tempdir().expect("temp dir");
        let auth_path = temp.path().join("auth/openai-codex.json");
        fs::create_dir_all(auth_path.parent().expect("auth dir")).expect("create auth dir");
        fs::write(
            &auth_path,
            serde_json::to_vec_pretty(&json!({
                "version": 2,
                "selected_account_key": "acct:acct-first",
                "accounts": [
                    {
                        "key": "acct:acct-first",
                        "label": "first",
                        "refresh": "refresh-first",
                        "access": "access-first",
                        "expires": u64::MAX / 2,
                        "account_id": "acct-first",
                        "added_at_ms": 10,
                        "last_selected_at_ms": 10,
                        "rate_limits": {
                            "fetched_at_ms": u64::MAX / 4,
                            "plan_type": "pro",
                            "allowed": true,
                            "limit_reached": false,
                            "primary_window": {
                                "used_percent": 12,
                                "limit_window_seconds": 604800,
                                "reset_after_seconds": 1800,
                                "reset_at": 1776793745u64
                            }
                        }
                    },
                    {
                        "key": "acct:acct-second",
                        "label": "second",
                        "refresh": "refresh-second",
                        "access": "access-second",
                        "expires": u64::MAX / 2,
                        "account_id": "acct-second",
                        "added_at_ms": 20,
                        "last_selected_at_ms": 20,
                        "rate_limits": {
                            "fetched_at_ms": u64::MAX / 4,
                            "plan_type": "pro",
                            "allowed": true,
                            "limit_reached": false,
                            "primary_window": {
                                "used_percent": 40,
                                "limit_window_seconds": 604800,
                                "reset_after_seconds": 1800,
                                "reset_at": 1776793745u64
                            }
                        }
                    }
                ]
            }))
            .expect("serialize auth state"),
        )
        .expect("write auth state");
        let server = FakeOpenAiServer::from_handler(|request| {
            assert_eq!(request.path, "/v1/responses");
            let raw = request.raw.to_ascii_lowercase();
            if raw.contains("chatgpt-account-id: acct-first") {
                FakeHttpResponse::json_status(
                    429,
                    json!({
                        "error": {
                            "type": "usage_limit_reached",
                            "plan_type": "pro",
                            "resets_in_seconds": 1200
                        }
                    }),
                )
            } else if raw.contains("chatgpt-account-id: acct-second") {
                FakeHttpResponse::json_ok(json!({
                    "id": "resp_second",
                    "model": "gpt-5.4",
                    "output": [
                        {
                            "type": "message",
                            "content": [
                                {"type": "output_text", "text": "hello from second"}
                            ]
                        }
                    ],
                    "usage": {
                        "input_tokens": 8,
                        "output_tokens": 4,
                        "total_tokens": 12
                    }
                }))
            } else {
                panic!("missing expected ChatGPT account header: {}", request.raw);
            }
        });
        let mut profile = openai_codex_subscription();
        profile.base_url = server.base_url().to_string();
        let response = complete_plain_text_with_context(
            &profile,
            vec![PlainTextMessage::user("hello")],
            OpenAiRequestContext {
                probe_home: Some(temp.path()),
                session_id: Some("sess-rotate"),
            },
        )
        .expect("rotated codex response");
        assert_eq!(
            response.assistant_text.as_deref(),
            Some("hello from second")
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains("chatgpt-account-id: acct-first")
        );
        assert!(
            requests[1]
                .to_ascii_lowercase()
                .contains("chatgpt-account-id: acct-second")
        );

        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(auth_path).expect("read auth state"))
                .expect("parse auth state");
        assert_eq!(
            stored["selected_account_key"].as_str(),
            Some("acct:acct-second")
        );
        assert_eq!(
            stored["accounts"][0]["rate_limits"]["limit_reached"].as_bool(),
            Some(true)
        );
    }
}
