use std::env;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::time::Instant;

use probe_protocol::backend::BackendProfile;
use probe_protocol::session::{
    CacheSignal, SessionBackendTarget, SessionHarnessProfile, SessionId, SessionMetadata,
    SessionTurn, TranscriptItemKind, TurnObservability,
};
use probe_provider_openai::{
    ChatCompletionRequest, ChatCompletionUsage, ChatMessage, ChatToolCall, OpenAiProviderClient,
    OpenAiProviderConfig, OpenAiProviderError,
};

use crate::session_store::{FilesystemSessionStore, NewItem, NewSession, SessionStoreError};
use crate::tools::{ExecutedToolCall, ToolExecutionContext, ToolLoopConfig, ToolOracleContext};

const DEFAULT_PROBE_HOME_DIR: &str = ".probe";
const LIKELY_WARM_WALLCLOCK_RATIO_NUMERATOR: u64 = 80;
const LIKELY_WARM_WALLCLOCK_RATIO_DENOMINATOR: u64 = 100;

#[derive(Clone, Debug)]
pub struct PlainTextExecRequest {
    pub profile: BackendProfile,
    pub prompt: String,
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub system_prompt: Option<String>,
    pub harness_profile: Option<SessionHarnessProfile>,
    pub tool_loop: Option<ToolLoopConfig>,
}

#[derive(Clone, Debug)]
pub struct PlainTextResumeRequest {
    pub session_id: SessionId,
    pub profile: BackendProfile,
    pub prompt: String,
    pub tool_loop: Option<ToolLoopConfig>,
}

#[derive(Clone, Debug)]
pub struct PlainTextExecOutcome {
    pub session: SessionMetadata,
    pub turn: SessionTurn,
    pub assistant_text: String,
    pub response_id: String,
    pub response_model: String,
    pub usage: Option<ChatCompletionUsage>,
    pub executed_tool_calls: usize,
    pub tool_results: Vec<ExecutedToolCall>,
}

#[derive(Debug)]
pub enum RuntimeError {
    ProbeHomeUnavailable,
    CurrentDir(std::io::Error),
    SessionStore(SessionStoreError),
    ProviderBuild(OpenAiProviderError),
    ProviderRequest {
        session_id: SessionId,
        source: OpenAiProviderError,
    },
    MissingAssistantMessage {
        session_id: SessionId,
        response_id: String,
    },
    ToolApprovalPending {
        session_id: SessionId,
        tool_name: String,
        call_id: String,
        reason: Option<String>,
    },
    MaxToolRoundTrips {
        session_id: SessionId,
        max_round_trips: usize,
    },
    MalformedTranscript(String),
}

impl Display for RuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProbeHomeUnavailable => {
                write!(f, "failed to resolve probe home; set PROBE_HOME or HOME")
            }
            Self::CurrentDir(error) => write!(f, "failed to resolve current directory: {error}"),
            Self::SessionStore(error) => write!(f, "{error}"),
            Self::ProviderBuild(error) => write!(f, "{error}"),
            Self::ProviderRequest { session_id, source } => {
                write!(
                    f,
                    "backend request failed for session {}: {source}",
                    session_id.as_str()
                )
            }
            Self::MissingAssistantMessage {
                session_id,
                response_id,
            } => write!(
                f,
                "backend response {response_id} for session {} did not include assistant text",
                session_id.as_str()
            ),
            Self::ToolApprovalPending {
                session_id,
                tool_name,
                call_id,
                reason,
            } => write!(
                f,
                "session {} paused for approval on tool `{tool_name}` ({call_id}){}",
                session_id.as_str(),
                reason
                    .as_deref()
                    .map(|value| format!(": {value}"))
                    .unwrap_or_default()
            ),
            Self::MaxToolRoundTrips {
                session_id,
                max_round_trips,
            } => write!(
                f,
                "session {} exceeded the configured tool loop bound of {} model round trips",
                session_id.as_str(),
                max_round_trips
            ),
            Self::MalformedTranscript(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<SessionStoreError> for RuntimeError {
    fn from(value: SessionStoreError) -> Self {
        Self::SessionStore(value)
    }
}

#[derive(Clone, Debug)]
pub struct ProbeRuntime {
    session_store: FilesystemSessionStore,
}

impl ProbeRuntime {
    #[must_use]
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            session_store: FilesystemSessionStore::new(home),
        }
    }

    pub fn detect() -> Result<Self, RuntimeError> {
        Ok(Self::new(default_probe_home()?))
    }

    #[must_use]
    pub fn session_store(&self) -> &FilesystemSessionStore {
        &self.session_store
    }

    pub fn exec_plain_text(
        &self,
        request: PlainTextExecRequest,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        let session = self.session_store.create_session_with(
            NewSession::new(
                request
                    .title
                    .clone()
                    .unwrap_or_else(|| default_session_title(request.prompt.as_str())),
                request.cwd,
            )
            .with_system_prompt(request.system_prompt.clone())
            .with_harness_profile(request.harness_profile.clone())
            .with_backend(SessionBackendTarget {
                profile_name: request.profile.name.clone(),
                base_url: request.profile.base_url.clone(),
                model: request.profile.model.clone(),
            }),
        )?;

        self.run_plain_text_turn(session, request.profile, request.prompt, request.tool_loop)
    }

    pub fn continue_plain_text_session(
        &self,
        request: PlainTextResumeRequest,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        let session = self.session_store.read_metadata(&request.session_id)?;
        self.run_plain_text_turn(session, request.profile, request.prompt, request.tool_loop)
    }
}

pub fn default_probe_home() -> Result<PathBuf, RuntimeError> {
    if let Ok(path) = env::var("PROBE_HOME") {
        return Ok(PathBuf::from(path));
    }

    if let Ok(home) = env::var("HOME") {
        return Ok(PathBuf::from(home).join(DEFAULT_PROBE_HOME_DIR));
    }

    Err(RuntimeError::ProbeHomeUnavailable)
}

pub fn current_working_dir() -> Result<PathBuf, RuntimeError> {
    env::current_dir().map_err(RuntimeError::CurrentDir)
}

fn default_session_title(prompt: &str) -> String {
    let trimmed = prompt.trim();
    let mut title = trimmed.chars().take(48).collect::<String>();
    if trimmed.chars().count() > 48 {
        title.push_str("...");
    }
    if title.is_empty() {
        String::from("Probe Session")
    } else {
        title
    }
}

impl ProbeRuntime {
    fn run_plain_text_turn(
        &self,
        session: SessionMetadata,
        profile: BackendProfile,
        prompt: String,
        tool_loop: Option<ToolLoopConfig>,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        let provider_config = OpenAiProviderConfig::from_backend_profile(&profile);
        let provider = OpenAiProviderClient::new(provider_config.clone())
            .map_err(RuntimeError::ProviderBuild)?;
        let mut messages = self.replay_messages(&session)?;
        let mut pending_user_prompt = Some(prompt);
        let mut executed_tool_calls = 0_usize;
        let mut tool_results = Vec::new();
        let tool_loop = tool_loop.filter(|config| !config.registry.is_empty());
        let max_round_trips = tool_loop
            .as_ref()
            .map(|config| config.max_model_round_trips)
            .unwrap_or(1);

        for _ in 0..max_round_trips {
            let next_user_prompt = pending_user_prompt.as_ref().cloned();
            if let Some(user_prompt) = next_user_prompt {
                messages.push(ChatMessage::user(user_prompt));
            }

            let request_started = Instant::now();
            let response = if let Some(tool_loop) = tool_loop.as_ref() {
                let request =
                    ChatCompletionRequest::from_config(&provider_config, messages.clone())
                        .with_tools(
                            tool_loop.registry.declared_tools(),
                            tool_loop.tool_choice.to_provider_choice(),
                            Some(tool_loop.parallel_tool_calls),
                        );
                provider.send_chat_completion(&request)
            } else {
                provider.chat_completion(messages.clone())
            }
            .map_err(|source| RuntimeError::ProviderRequest {
                session_id: session.id.clone(),
                source,
            });
            let wallclock_ms = elapsed_ms(request_started);

            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    let mut items = Vec::new();
                    if let Some(user_prompt) = pending_user_prompt.take() {
                        items.push(NewItem::new(TranscriptItemKind::UserMessage, user_prompt));
                    }
                    items.push(NewItem::new(TranscriptItemKind::Note, error.to_string()));
                    let _ = self.session_store.append_turn(&session.id, &items);
                    return Err(error);
                }
            };
            let observability =
                self.build_turn_observability(&session.id, wallclock_ms, response.usage.as_ref())?;

            if let Some(tool_calls) = response.first_tool_calls()
                && !tool_calls.is_empty()
            {
                let tool_calls = tool_calls.to_vec();
                let tool_call_turn = self.append_tool_call_turn(
                    &session.id,
                    pending_user_prompt.take(),
                    &tool_calls,
                    Some(observability),
                )?;
                let _ = tool_call_turn;

                let Some(tool_loop) = tool_loop.as_ref() else {
                    return Err(RuntimeError::MissingAssistantMessage {
                        session_id: session.id.clone(),
                        response_id: response.id,
                    });
                };
                let oracle_calls_used =
                    self.count_tool_results_named(&session.id, "consult_oracle")?;
                let mut execution_context = ToolExecutionContext::new(session.cwd.clone());
                if let Some(oracle) = tool_loop.oracle.as_ref() {
                    execution_context = execution_context.with_oracle(ToolOracleContext::new(
                        oracle.profile.clone(),
                        oracle.max_calls,
                        oracle_calls_used,
                    ));
                }
                let executed = tool_loop.registry.execute_batch(
                    &execution_context,
                    &tool_calls,
                    &tool_loop.approval,
                );
                executed_tool_calls += executed.iter().filter(|tool| tool.was_executed()).count();
                tool_results.extend(executed.clone());
                let tool_result_turn = self.append_tool_result_turn(&session.id, &executed)?;
                let _ = tool_result_turn;
                if let Some(paused) = executed.iter().find(|tool| tool.was_paused()) {
                    return Err(RuntimeError::ToolApprovalPending {
                        session_id: session.id.clone(),
                        tool_name: paused.name.clone(),
                        call_id: paused.call_id.clone(),
                        reason: paused.tool_execution.reason.clone(),
                    });
                }

                messages.push(ChatMessage::assistant_tool_calls(tool_calls));
                for tool_result in executed {
                    messages.push(ChatMessage::tool(
                        tool_result.name,
                        tool_result.call_id,
                        serde_json::to_string(&tool_result.output).unwrap_or_else(|_| {
                            String::from("{\"error\":\"tool output encode failed\"}")
                        }),
                    ));
                }
                continue;
            }

            let assistant_text = response
                .first_message_text()
                .map(ToOwned::to_owned)
                .ok_or_else(|| RuntimeError::MissingAssistantMessage {
                    session_id: session.id.clone(),
                    response_id: response.id.clone(),
                })?;

            let mut items = Vec::new();
            if let Some(user_prompt) = pending_user_prompt.take() {
                items.push(NewItem::new(TranscriptItemKind::UserMessage, user_prompt));
            }
            items.push(NewItem::new(
                TranscriptItemKind::AssistantMessage,
                assistant_text.clone(),
            ));
            let turn = self.session_store.append_turn_with_observability(
                &session.id,
                &items,
                Some(observability),
            )?;
            let session = self.session_store.read_metadata(&session.id)?;
            return Ok(PlainTextExecOutcome {
                session,
                turn,
                assistant_text,
                response_id: response.id,
                response_model: response.model,
                usage: response.usage,
                executed_tool_calls,
                tool_results,
            });
        }

        let _ = self.session_store.append_turn(
            &session.id,
            &[NewItem::new(
                TranscriptItemKind::Note,
                format!(
                    "session exceeded the configured tool loop bound of {} model round trips",
                    max_round_trips
                ),
            )],
        );
        Err(RuntimeError::MaxToolRoundTrips {
            session_id: session.id,
            max_round_trips,
        })
    }

    fn replay_messages(&self, session: &SessionMetadata) -> Result<Vec<ChatMessage>, RuntimeError> {
        let mut messages = Vec::new();
        if let Some(system_prompt) = &session.system_prompt {
            messages.push(ChatMessage::system(system_prompt.clone()));
        }

        for event in self.session_store.read_transcript(&session.id)? {
            let mut pending_tool_calls = Vec::new();
            for item in event.turn.items {
                match item.kind {
                    TranscriptItemKind::UserMessage => {
                        if !pending_tool_calls.is_empty() {
                            messages.push(ChatMessage::assistant_tool_calls(std::mem::take(
                                &mut pending_tool_calls,
                            )));
                        }
                        messages.push(ChatMessage::user(item.text));
                    }
                    TranscriptItemKind::AssistantMessage => {
                        if !pending_tool_calls.is_empty() {
                            messages.push(ChatMessage::assistant_tool_calls(std::mem::take(
                                &mut pending_tool_calls,
                            )));
                        }
                        messages.push(ChatMessage::assistant(item.text))
                    }
                    TranscriptItemKind::ToolCall => {
                        let name = item.name.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool call transcript items require a tool name",
                            ))
                        })?;
                        let tool_call_id = item.tool_call_id.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool call transcript items require a tool_call_id",
                            ))
                        })?;
                        let arguments = item.arguments.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool call transcript items require structured arguments",
                            ))
                        })?;
                        pending_tool_calls.push(ChatToolCall {
                            id: tool_call_id,
                            kind: String::from("function"),
                            function: probe_provider_openai::ChatToolCallFunction {
                                name,
                                arguments: serde_json::to_string(&arguments).map_err(|error| {
                                    RuntimeError::MalformedTranscript(format!(
                                        "failed to encode stored tool arguments: {error}"
                                    ))
                                })?,
                            },
                        });
                    }
                    TranscriptItemKind::ToolResult => {
                        if !pending_tool_calls.is_empty() {
                            messages.push(ChatMessage::assistant_tool_calls(std::mem::take(
                                &mut pending_tool_calls,
                            )));
                        }
                        let name = item.name.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool result transcript items require a tool name",
                            ))
                        })?;
                        let tool_call_id = item.tool_call_id.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool result transcript items require a tool_call_id",
                            ))
                        })?;
                        messages.push(ChatMessage::tool(name, tool_call_id, item.text));
                    }
                    TranscriptItemKind::Note => {
                        if !pending_tool_calls.is_empty() {
                            messages.push(ChatMessage::assistant_tool_calls(std::mem::take(
                                &mut pending_tool_calls,
                            )));
                        }
                    }
                }
            }
            if !pending_tool_calls.is_empty() {
                messages.push(ChatMessage::assistant_tool_calls(pending_tool_calls));
            }
        }

        Ok(messages)
    }

    fn append_tool_call_turn(
        &self,
        session_id: &SessionId,
        user_prompt: Option<String>,
        tool_calls: &[ChatToolCall],
        observability: Option<TurnObservability>,
    ) -> Result<SessionTurn, RuntimeError> {
        let mut items = Vec::new();
        if let Some(user_prompt) = user_prompt {
            items.push(NewItem::new(TranscriptItemKind::UserMessage, user_prompt));
        }
        for tool_call in tool_calls {
            let arguments =
                serde_json::from_str::<serde_json::Value>(tool_call.function.arguments.as_str())
                    .map_err(|error| {
                        RuntimeError::MalformedTranscript(format!(
                            "tool call `{}` returned non-JSON arguments: {error}",
                            tool_call.function.name
                        ))
                    })?;
            items.push(NewItem::tool_call(
                tool_call.function.name.clone(),
                tool_call.id.clone(),
                arguments,
            ));
        }
        self.session_store
            .append_turn_with_observability(session_id, &items, observability)
            .map_err(RuntimeError::from)
    }

    fn append_tool_result_turn(
        &self,
        session_id: &SessionId,
        executed_tool_calls: &[crate::tools::ExecutedToolCall],
    ) -> Result<SessionTurn, RuntimeError> {
        let items = executed_tool_calls
            .iter()
            .map(|tool_call| {
                NewItem::tool_result(
                    tool_call.name.clone(),
                    tool_call.call_id.clone(),
                    serde_json::to_string(&tool_call.output).unwrap_or_else(|_| {
                        String::from("{\"error\":\"tool output encode failed\"}")
                    }),
                    tool_call.tool_execution.clone(),
                )
            })
            .collect::<Vec<_>>();
        self.session_store
            .append_turn(session_id, &items)
            .map_err(RuntimeError::from)
    }

    fn build_turn_observability(
        &self,
        session_id: &SessionId,
        wallclock_ms: u64,
        usage: Option<&ChatCompletionUsage>,
    ) -> Result<TurnObservability, RuntimeError> {
        let prompt_tokens = usage.map(|usage| usage.prompt_tokens);
        let previous = if prompt_tokens.is_some() {
            self.last_prompt_bearing_observability(session_id)?
        } else {
            self.last_turn_observability(session_id)?
        };

        Ok(TurnObservability {
            wallclock_ms,
            model_output_ms: Some(wallclock_ms),
            prompt_tokens,
            completion_tokens: usage.map(|usage| usage.completion_tokens),
            total_tokens: usage.map(|usage| usage.total_tokens),
            completion_tokens_per_second_x1000: usage.and_then(|usage| {
                completion_tokens_per_second_x1000(usage.completion_tokens, wallclock_ms)
            }),
            cache_signal: infer_cache_signal(previous.as_ref(), prompt_tokens, wallclock_ms),
        })
    }

    fn last_turn_observability(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<TurnObservability>, RuntimeError> {
        let transcript = self.session_store.read_transcript(session_id)?;
        Ok(transcript
            .into_iter()
            .rev()
            .find_map(|event| event.turn.observability))
    }

    fn last_prompt_bearing_observability(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<TurnObservability>, RuntimeError> {
        let transcript = self.session_store.read_transcript(session_id)?;
        Ok(transcript.into_iter().rev().find_map(|event| {
            let observability = event.turn.observability?;
            observability.prompt_tokens.map(|_| observability)
        }))
    }

    fn count_tool_results_named(
        &self,
        session_id: &SessionId,
        tool_name: &str,
    ) -> Result<usize, RuntimeError> {
        let transcript = self.session_store.read_transcript(session_id)?;
        Ok(transcript
            .into_iter()
            .flat_map(|event| event.turn.items.into_iter())
            .filter(|item| {
                item.kind == TranscriptItemKind::ToolResult
                    && item.name.as_deref() == Some(tool_name)
                    && item
                        .tool_execution
                        .as_ref()
                        .map(|execution| {
                            matches!(
                                execution.policy_decision,
                                probe_protocol::session::ToolPolicyDecision::AutoAllow
                                    | probe_protocol::session::ToolPolicyDecision::Approved
                            )
                        })
                        .unwrap_or(false)
            })
            .count())
    }
}

fn infer_cache_signal(
    previous: Option<&TurnObservability>,
    current_prompt_tokens: Option<u32>,
    current_wallclock_ms: u64,
) -> CacheSignal {
    let Some(previous) = previous else {
        return CacheSignal::ColdStart;
    };
    let Some(current_prompt_tokens) = current_prompt_tokens else {
        return CacheSignal::Unknown;
    };
    let Some(previous_prompt_tokens) = previous.prompt_tokens else {
        return CacheSignal::Unknown;
    };
    if previous.wallclock_ms == 0 || current_prompt_tokens < previous_prompt_tokens {
        return CacheSignal::NoClearSignal;
    }
    if current_wallclock_ms.saturating_mul(LIKELY_WARM_WALLCLOCK_RATIO_DENOMINATOR)
        <= previous
            .wallclock_ms
            .saturating_mul(LIKELY_WARM_WALLCLOCK_RATIO_NUMERATOR)
    {
        CacheSignal::LikelyWarm
    } else {
        CacheSignal::NoClearSignal
    }
}

fn completion_tokens_per_second_x1000(completion_tokens: u32, model_output_ms: u64) -> Option<u64> {
    if completion_tokens == 0 || model_output_ms == 0 {
        return None;
    }
    Some((u64::from(completion_tokens) * 1_000_000) / model_output_ms)
}

fn elapsed_ms(started: Instant) -> u64 {
    let elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    elapsed_ms.max(1)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    use crate::backend_profiles::psionic_qwen35_2b_q8_registry;
    use crate::tools::{ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction, ToolLoopConfig};
    use probe_protocol::session::{
        CacheSignal, SessionHarnessProfile, ToolApprovalState, ToolPolicyDecision,
        TranscriptItemKind,
    };

    use super::{
        PlainTextExecRequest, PlainTextResumeRequest, ProbeRuntime, default_session_title,
    };

    #[test]
    fn default_title_is_trimmed_for_exec_prompts() {
        assert_eq!(default_session_title("  hello world  "), "hello world");
        assert_eq!(default_session_title(""), "Probe Session");
    }

    #[test]
    fn exec_plain_text_persists_session_and_transcript() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let body = serde_json::json!({
                "id": "chatcmpl_exec_test",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [
                    {
                        "index": 0,
                        "message": {"role": "assistant", "content": "hello from probe exec"},
                        "finish_reason": "stop"
                    }
                ],
                "usage": {
                    "prompt_tokens": 6,
                    "completion_tokens": 5,
                    "total_tokens": 11
                }
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let temp = tempfile::tempdir().expect("temp dir");
        let runtime = ProbeRuntime::new(temp.path().join(".probe"));
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = format!("http://{address}/v1");

        let outcome = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("say hello"),
                title: Some(String::from("Exec Test")),
                cwd: temp.path().to_path_buf(),
                system_prompt: None,
                harness_profile: Some(SessionHarnessProfile {
                    name: String::from("coding_bootstrap_default"),
                    version: String::from("v1"),
                }),
                tool_loop: None,
            })
            .expect("exec should succeed");

        assert_eq!(outcome.assistant_text, "hello from probe exec");
        assert_eq!(outcome.response_id, "chatcmpl_exec_test");
        assert_eq!(outcome.response_model, "qwen3.5-2b-q8_0-registry.gguf");
        assert_eq!(outcome.turn.items.len(), 2);
        assert_eq!(outcome.session.title, "Exec Test");
        let observability = outcome
            .turn
            .observability
            .as_ref()
            .expect("observability should be recorded");
        assert!(observability.wallclock_ms > 0);
        assert_eq!(
            observability.model_output_ms,
            Some(observability.wallclock_ms)
        );
        assert_eq!(observability.prompt_tokens, Some(6));
        assert_eq!(observability.completion_tokens, Some(5));
        assert_eq!(observability.total_tokens, Some(11));
        assert!(
            observability
                .completion_tokens_per_second_x1000
                .expect("throughput should be computed")
                > 0
        );
        assert!(matches!(observability.cache_signal, CacheSignal::ColdStart));
        assert_eq!(
            outcome
                .session
                .backend
                .as_ref()
                .expect("backend metadata should exist")
                .profile_name,
            "psionic-qwen35-2b-q8-registry"
        );
        assert_eq!(
            outcome
                .session
                .harness_profile
                .as_ref()
                .expect("harness profile should persist")
                .name,
            "coding_bootstrap_default"
        );

        let transcript = runtime
            .session_store()
            .read_transcript(&outcome.session.id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].turn.items[0].text, "say hello");
        assert_eq!(transcript[0].turn.items[1].text, "hello from probe exec");
        assert_eq!(
            transcript[0]
                .turn
                .observability
                .as_ref()
                .expect("observability should persist")
                .prompt_tokens,
            Some(6)
        );

        handle.join().expect("server thread should exit cleanly");
    }

    #[test]
    fn continue_plain_text_session_replays_prior_context() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            for expected_response in ["first answer", "second answer"] {
                let (mut stream, _) = listener.accept().expect("accept connection");
                let mut buffer = [0_u8; 8192];
                let bytes = stream.read(&mut buffer).expect("read request");
                let request_body = String::from_utf8_lossy(&buffer[..bytes]);
                if expected_response == "second answer" {
                    assert!(request_body.contains("first prompt"));
                    assert!(request_body.contains("first answer"));
                    assert!(request_body.contains("second prompt"));
                    thread::sleep(Duration::from_millis(5));
                } else {
                    thread::sleep(Duration::from_millis(60));
                }
                let body = serde_json::json!({
                    "id": format!("chatcmpl_{expected_response}"),
                    "model": "qwen3.5-2b-q8_0-registry.gguf",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": expected_response},
                            "finish_reason": "stop"
                        }
                    ],
                    "usage": {
                        "prompt_tokens": 10,
                        "completion_tokens": 4,
                        "total_tokens": 14
                    }
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });

        let temp = tempfile::tempdir().expect("temp dir");
        let runtime = ProbeRuntime::new(temp.path().join(".probe"));
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = format!("http://{address}/v1");

        let first = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile: profile.clone(),
                prompt: String::from("first prompt"),
                title: Some(String::from("Interactive Test")),
                cwd: temp.path().to_path_buf(),
                system_prompt: Some(String::from("You are helpful")),
                harness_profile: None,
                tool_loop: None,
            })
            .expect("first turn should succeed");

        let second = runtime
            .continue_plain_text_session(PlainTextResumeRequest {
                session_id: first.session.id.clone(),
                profile,
                prompt: String::from("second prompt"),
                tool_loop: None,
            })
            .expect("second turn should succeed");

        assert_eq!(second.assistant_text, "second answer");
        assert_eq!(second.turn.index, 1);
        assert_eq!(
            second
                .session
                .system_prompt
                .as_deref()
                .expect("system prompt should be persisted"),
            "You are helpful"
        );

        let transcript = runtime
            .session_store()
            .read_transcript(&first.session.id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[1].turn.items[0].text, "second prompt");
        assert!(matches!(
            transcript[0]
                .turn
                .observability
                .as_ref()
                .expect("first turn observability should exist")
                .cache_signal,
            CacheSignal::ColdStart
        ));
        assert!(matches!(
            transcript[1]
                .turn
                .observability
                .as_ref()
                .expect("second turn observability should exist")
                .cache_signal,
            CacheSignal::LikelyWarm
        ));

        handle.join().expect("server thread should exit cleanly");
    }

    #[test]
    fn tool_loop_executes_required_single_tool_and_replays_result() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            for step in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept connection");
                let mut buffer = [0_u8; 8192];
                let bytes = stream.read(&mut buffer).expect("read request");
                let request_body = String::from_utf8_lossy(&buffer[..bytes]);
                if step == 0 {
                    assert!(request_body.contains("\"tools\""));
                    assert!(request_body.contains("\"tool_choice\":\"required\""));
                    assert!(request_body.contains("\"parallel_tool_calls\":false"));
                } else {
                    assert!(request_body.contains("\"tool_calls\""));
                    assert!(request_body.contains("\"lookup_weather\""));
                    assert!(request_body.contains("Paris"));
                    assert!(request_body.contains("temperature_c"));
                }
                let body = if step == 0 {
                    serde_json::json!({
                        "id": "chatcmpl_tool_required",
                        "model": "qwen3.5-2b-q8_0-registry.gguf",
                        "choices": [
                            {
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "tool_calls": [
                                        {
                                            "id": "call_weather_1",
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
                    })
                } else {
                    serde_json::json!({
                        "id": "chatcmpl_tool_final",
                        "model": "qwen3.5-2b-q8_0-registry.gguf",
                        "choices": [
                            {
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "content": "Paris is sunny at 18C."
                                },
                                "finish_reason": "stop"
                            }
                        ],
                        "usage": {
                            "prompt_tokens": 20,
                            "completion_tokens": 6,
                            "total_tokens": 26
                        }
                    })
                }
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });

        let temp = tempfile::tempdir().expect("temp dir");
        let runtime = ProbeRuntime::new(temp.path().join(".probe"));
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = format!("http://{address}/v1");

        let outcome = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("what is the weather in Paris?"),
                title: Some(String::from("Tool Test")),
                cwd: temp.path().to_path_buf(),
                system_prompt: None,
                harness_profile: None,
                tool_loop: Some(ToolLoopConfig::weather_demo(
                    ProbeToolChoice::Required,
                    false,
                )),
            })
            .expect("tool loop should succeed");

        assert_eq!(outcome.assistant_text, "Paris is sunny at 18C.");
        assert_eq!(outcome.executed_tool_calls, 1);
        assert_eq!(outcome.tool_results.len(), 1);
        assert_eq!(
            outcome.tool_results[0].tool_execution.policy_decision,
            ToolPolicyDecision::AutoAllow
        );

        let transcript = runtime
            .session_store()
            .read_transcript(&outcome.session.id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 3);
        assert!(transcript[0].turn.observability.is_some());
        assert!(transcript[1].turn.observability.is_none());
        assert!(transcript[2].turn.observability.is_some());
        assert!(matches!(
            transcript[0].turn.items[1].kind,
            TranscriptItemKind::ToolCall
        ));
        assert!(matches!(
            transcript[1].turn.items[0].kind,
            TranscriptItemKind::ToolResult
        ));
        assert_eq!(
            transcript[1].turn.items[0]
                .tool_execution
                .as_ref()
                .expect("tool execution should persist")
                .approval_state,
            ToolApprovalState::NotRequired
        );
        assert_eq!(transcript[2].turn.items[0].text, "Paris is sunny at 18C.");

        handle.join().expect("server thread should exit cleanly");
    }

    #[test]
    fn tool_loop_can_pause_for_approval() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let body = serde_json::json!({
                "id": "chatcmpl_pause_test",
                "model": "qwen3.5-2b-q8_0-registry.gguf",
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "tool_calls": [
                                {
                                    "id": "call_patch_1",
                                    "type": "function",
                                    "function": {
                                        "name": "apply_patch",
                                        "arguments": "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}"
                                    }
                                }
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }
                ]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("hello.txt"), "hello world\n").expect("write hello");
        let runtime = ProbeRuntime::new(temp.path().join(".probe"));
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = format!("http://{address}/v1");
        let mut tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Required, false);
        tool_loop.approval = ToolApprovalConfig {
            allow_write_tools: false,
            allow_network_shell: false,
            allow_destructive_shell: false,
            denied_action: ToolDeniedAction::Pause,
        };

        let error = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("patch hello.txt"),
                title: Some(String::from("Pause Test")),
                cwd: temp.path().to_path_buf(),
                system_prompt: None,
                harness_profile: None,
                tool_loop: Some(tool_loop),
            })
            .expect_err("tool loop should pause");

        assert!(matches!(
            error,
            super::RuntimeError::ToolApprovalPending { .. }
        ));

        let sessions = runtime
            .session_store()
            .list_sessions()
            .expect("list sessions");
        let session_id = sessions[0].id.clone();
        let transcript = runtime
            .session_store()
            .read_transcript(&session_id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 2);
        assert_eq!(
            transcript[1].turn.items[0]
                .tool_execution
                .as_ref()
                .expect("tool execution should persist")
                .policy_decision,
            ToolPolicyDecision::Paused
        );

        handle.join().expect("server thread should exit cleanly");
    }

    #[test]
    fn tool_loop_executes_parallel_tool_batches() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            for step in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept connection");
                let mut buffer = [0_u8; 8192];
                let bytes = stream.read(&mut buffer).expect("read request");
                let request_body = String::from_utf8_lossy(&buffer[..bytes]);
                if step == 0 {
                    assert!(request_body.contains("\"parallel_tool_calls\":true"));
                } else {
                    assert!(request_body.contains("\"call_weather_paris\""));
                    assert!(request_body.contains("\"call_weather_tokyo\""));
                    assert!(request_body.contains("Tokyo"));
                }
                let body = if step == 0 {
                    serde_json::json!({
                        "id": "chatcmpl_parallel_tools",
                        "model": "qwen3.5-2b-q8_0-registry.gguf",
                        "choices": [
                            {
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "tool_calls": [
                                        {
                                            "id": "call_weather_paris",
                                            "type": "function",
                                            "function": {
                                                "name": "lookup_weather",
                                                "arguments": "{\"city\":\"Paris\"}"
                                            }
                                        },
                                        {
                                            "id": "call_weather_tokyo",
                                            "type": "function",
                                            "function": {
                                                "name": "lookup_weather",
                                                "arguments": "{\"city\":\"Tokyo\"}"
                                            }
                                        }
                                    ]
                                },
                                "finish_reason": "tool_calls"
                            }
                        ]
                    })
                } else {
                    serde_json::json!({
                        "id": "chatcmpl_parallel_final",
                        "model": "qwen3.5-2b-q8_0-registry.gguf",
                        "choices": [
                            {
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "content": "Paris is sunny at 18C. Tokyo is rainy at 12C."
                                },
                                "finish_reason": "stop"
                            }
                        ]
                    })
                }
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });

        let temp = tempfile::tempdir().expect("temp dir");
        let runtime = ProbeRuntime::new(temp.path().join(".probe"));
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = format!("http://{address}/v1");

        let outcome = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("check Paris and Tokyo"),
                title: Some(String::from("Parallel Tool Test")),
                cwd: temp.path().to_path_buf(),
                system_prompt: None,
                harness_profile: None,
                tool_loop: Some(ToolLoopConfig::weather_demo(
                    ProbeToolChoice::Required,
                    true,
                )),
            })
            .expect("parallel tool loop should succeed");

        assert_eq!(outcome.executed_tool_calls, 2);
        assert_eq!(
            outcome.assistant_text,
            "Paris is sunny at 18C. Tokyo is rainy at 12C."
        );

        let transcript = runtime
            .session_store()
            .read_transcript(&outcome.session.id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 3);
        assert_eq!(transcript[0].turn.items.len(), 3);
        assert_eq!(transcript[1].turn.items.len(), 2);
        assert!(transcript[0].turn.observability.is_some());
        assert!(transcript[1].turn.observability.is_none());
        assert!(transcript[2].turn.observability.is_some());

        handle.join().expect("server thread should exit cleanly");
    }
}
