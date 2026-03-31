use std::collections::BTreeMap;
use std::env;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use probe_protocol::backend::{BackendKind, BackendProfile};
use probe_protocol::session::{
    CacheSignal, SessionBackendTarget, SessionHarnessProfile, SessionId, SessionMetadata,
    SessionTurn, TranscriptItemKind, TurnObservability,
};
use probe_provider_openai::{ChatMessage, ChatToolCall};
use psionic_apple_fm::{
    APPLE_FM_TRANSCRIPT_TYPE, APPLE_FM_TRANSCRIPT_VERSION, AppleFmToolCallError, AppleFmTranscript,
    AppleFmTranscriptContent, AppleFmTranscriptEntry, AppleFmTranscriptPayload,
};

use crate::dataset_export::build_decision_summary;
use crate::provider::{
    PlainTextMessage, ProviderError, ProviderUsage, apple_fm_tool_loop_response,
    complete_plain_text, openai_tool_loop_response,
};
use crate::session_store::{FilesystemSessionStore, NewItem, NewSession, SessionStoreError};
use crate::tools::{
    ExecutedToolCall, ToolExecutionContext, ToolExecutionSession, ToolLongContextContext,
    ToolLoopConfig, ToolOracleContext,
};

const DEFAULT_PROBE_HOME_DIR: &str = ".probe";
const LIKELY_WARM_WALLCLOCK_RATIO_NUMERATOR: u64 = 80;
const LIKELY_WARM_WALLCLOCK_RATIO_DENOMINATOR: u64 = 100;

#[derive(Clone, Debug)]
enum AppleFmToolLoopInterruption {
    ApprovalPending {
        tool_name: String,
        call_id: String,
        reason: Option<String>,
    },
    CallbackBudgetExceeded {
        max_round_trips: usize,
    },
}

#[derive(Clone, Debug)]
struct AppleFmToolLoopRecorder {
    execution_session: ToolExecutionSession,
    records: Vec<ExecutedToolCall>,
    next_call_index: usize,
    max_callback_calls: usize,
    interruption: Option<AppleFmToolLoopInterruption>,
}

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
    pub usage: Option<ProviderUsage>,
    pub executed_tool_calls: usize,
    pub tool_results: Vec<ExecutedToolCall>,
}

#[derive(Debug)]
pub enum RuntimeError {
    ProbeHomeUnavailable,
    CurrentDir(std::io::Error),
    SessionStore(SessionStoreError),
    ProviderRequest {
        session_id: SessionId,
        source: ProviderError,
    },
    MissingAssistantMessage {
        session_id: SessionId,
        response_id: String,
    },
    UnsupportedBackendFeature {
        session_id: SessionId,
        backend: BackendKind,
        feature: &'static str,
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
            Self::UnsupportedBackendFeature {
                session_id,
                backend,
                feature,
            } => write!(
                f,
                "session {} cannot use backend {:?} for {feature}",
                session_id.as_str(),
                backend
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
                "session {} exceeded the configured tool loop bound of {} controller round trips",
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
        let tool_loop = tool_loop.filter(|config| !config.registry.is_empty());
        if tool_loop.is_none() {
            return self.run_plain_completion_turn(session, profile, prompt);
        }
        match profile.kind {
            BackendKind::OpenAiChatCompletions => self.run_openai_tool_loop_turn(
                session,
                profile,
                prompt,
                tool_loop.expect("filtered"),
            ),
            BackendKind::AppleFmBridge => self.run_apple_fm_tool_loop_turn(
                session,
                profile,
                prompt,
                tool_loop.expect("filtered"),
            ),
        }
    }

    fn run_openai_tool_loop_turn(
        &self,
        session: SessionMetadata,
        profile: BackendProfile,
        prompt: String,
        tool_loop: ToolLoopConfig,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        let mut messages = self.replay_messages(&session)?;
        let mut pending_user_prompt = Some(prompt);
        let mut executed_tool_calls = 0_usize;
        let mut tool_results = Vec::new();
        let max_round_trips = tool_loop.max_model_round_trips;

        for _ in 0..max_round_trips {
            let next_user_prompt = pending_user_prompt.as_ref().cloned();
            if let Some(user_prompt) = next_user_prompt.as_ref() {
                messages.push(ChatMessage::user(user_prompt));
            }

            let request_started = Instant::now();
            let response = openai_tool_loop_response(
                &profile,
                messages.clone(),
                tool_loop.registry.declared_tools(),
                tool_loop.tool_choice.to_provider_choice(),
                Some(tool_loop.parallel_tool_calls),
            )
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
                self.build_turn_observability(&session.id, wallclock_ms, response.2.as_ref())?;

            if let Some(tool_calls) = response.4.as_ref()
                && !tool_calls.is_empty()
            {
                let tool_calls = tool_calls.clone();
                let _ = self.append_tool_call_turn(
                    &session.id,
                    pending_user_prompt.take(),
                    &tool_calls,
                    Some(observability),
                )?;

                let execution_context = self.build_tool_execution_context(
                    &session,
                    &tool_loop,
                    next_user_prompt.as_deref(),
                )?;
                let executed = tool_loop.registry.execute_batch(
                    &execution_context,
                    tool_calls.as_slice(),
                    &tool_loop.approval,
                );
                executed_tool_calls += executed.iter().filter(|tool| tool.was_executed()).count();
                tool_results.extend(executed.clone());
                let _ = self.append_tool_result_turn(&session.id, &executed)?;
                if let Some(paused) = executed.iter().find(|tool| tool.was_paused()) {
                    return Err(RuntimeError::ToolApprovalPending {
                        session_id: session.id.clone(),
                        tool_name: paused.name.clone(),
                        call_id: paused.call_id.clone(),
                        reason: paused.tool_execution.reason.clone(),
                    });
                }

                messages.push(ChatMessage::assistant_tool_calls(tool_calls.clone()));
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

            let assistant_text =
                response
                    .3
                    .clone()
                    .ok_or_else(|| RuntimeError::MissingAssistantMessage {
                        session_id: session.id.clone(),
                        response_id: response.0.clone(),
                    })?;
            let turn = self.append_assistant_turn(
                &session.id,
                pending_user_prompt.take(),
                assistant_text.clone(),
                Some(observability),
            )?;
            let session = self.session_store.read_metadata(&session.id)?;
            return Ok(PlainTextExecOutcome {
                session,
                turn,
                assistant_text,
                response_id: response.0,
                response_model: response.1,
                usage: response.2,
                executed_tool_calls,
                tool_results,
            });
        }

        let _ = self.session_store.append_turn(
            &session.id,
            &[NewItem::new(
                TranscriptItemKind::Note,
                format!(
                    "session exceeded the configured tool loop bound of {} controller round trips",
                    max_round_trips
                ),
            )],
        );
        Err(RuntimeError::MaxToolRoundTrips {
            session_id: session.id,
            max_round_trips,
        })
    }

    fn run_apple_fm_tool_loop_turn(
        &self,
        session: SessionMetadata,
        profile: BackendProfile,
        prompt: String,
        tool_loop: ToolLoopConfig,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        let transcript = self.replay_apple_fm_transcript(&session)?;
        let execution_context =
            self.build_tool_execution_context(&session, &tool_loop, Some(prompt.as_str()))?;
        let recorder = Arc::new(Mutex::new(AppleFmToolLoopRecorder::new(
            tool_loop
                .registry
                .execution_session(&execution_context, &tool_loop.approval),
            tool_loop.max_model_round_trips,
        )));
        let callback_recorder = Arc::clone(&recorder);
        let tool_definitions = tool_loop
            .registry
            .declared_tools()
            .into_iter()
            .map(
                |tool| probe_provider_apple_fm::AppleFmProviderToolDefinition {
                    name: tool.function.name,
                    description: tool.function.description,
                    parameters: tool.function.parameters,
                },
            )
            .collect::<Vec<_>>();

        let request_started = Instant::now();
        let response = apple_fm_tool_loop_response(
            &profile,
            session.system_prompt.as_deref(),
            transcript,
            prompt.as_str(),
            tool_definitions,
            Arc::new(move |tool_call| {
                callback_recorder
                    .lock()
                    .expect("apple fm tool recorder lock")
                    .handle_call(tool_call)
            }),
        )
        .map_err(|source| RuntimeError::ProviderRequest {
            session_id: session.id.clone(),
            source,
        });
        let wallclock_ms = elapsed_ms(request_started);

        let (tool_results, interruption) = {
            let recorder = recorder.lock().expect("apple fm tool recorder lock");
            (recorder.records.clone(), recorder.interruption.clone())
        };
        let executed_tool_calls = tool_results
            .iter()
            .filter(|tool| tool.was_executed())
            .count();

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                if !tool_results.is_empty() {
                    let _ = self.append_recorded_tool_call_turn(
                        &session.id,
                        Some(prompt.clone()),
                        &tool_results,
                        None,
                    );
                    let _ = self.append_tool_result_turn(&session.id, &tool_results);
                } else if matches!(
                    interruption,
                    Some(AppleFmToolLoopInterruption::CallbackBudgetExceeded { .. })
                ) {
                    let _ = self.session_store.append_turn(
                        &session.id,
                        &[NewItem::new(
                            TranscriptItemKind::UserMessage,
                            prompt.clone(),
                        )],
                    );
                }

                return match interruption {
                    Some(AppleFmToolLoopInterruption::ApprovalPending {
                        tool_name,
                        call_id,
                        reason,
                    }) => Err(RuntimeError::ToolApprovalPending {
                        session_id: session.id,
                        tool_name,
                        call_id,
                        reason,
                    }),
                    Some(AppleFmToolLoopInterruption::CallbackBudgetExceeded {
                        max_round_trips,
                    }) => {
                        let _ = self.session_store.append_turn(
                            &session.id,
                            &[NewItem::new(
                                TranscriptItemKind::Note,
                                format!(
                                    "session exceeded the configured tool loop bound of {} controller round trips",
                                    max_round_trips
                                ),
                            )],
                        );
                        Err(RuntimeError::MaxToolRoundTrips {
                            session_id: session.id,
                            max_round_trips,
                        })
                    }
                    None => {
                        let mut items = Vec::new();
                        if tool_results.is_empty() {
                            items.push(NewItem::new(TranscriptItemKind::UserMessage, prompt));
                        }
                        items.push(NewItem::new(TranscriptItemKind::Note, error.to_string()));
                        let _ = self.session_store.append_turn(&session.id, &items);
                        Err(error)
                    }
                };
            }
        };
        let observability =
            self.build_turn_observability(&session.id, wallclock_ms, response.usage.as_ref())?;
        if !tool_results.is_empty() {
            let _ = self.append_recorded_tool_call_turn(
                &session.id,
                Some(prompt),
                &tool_results,
                None,
            )?;
            let _ = self.append_tool_result_turn(&session.id, &tool_results)?;
            let turn = self.append_assistant_turn(
                &session.id,
                None,
                response.assistant_text.clone().ok_or_else(|| {
                    RuntimeError::MissingAssistantMessage {
                        session_id: session.id.clone(),
                        response_id: response.response_id.clone(),
                    }
                })?,
                Some(observability),
            )?;
            let session = self.session_store.read_metadata(&session.id)?;
            return Ok(PlainTextExecOutcome {
                session,
                turn,
                assistant_text: response.assistant_text.unwrap_or_default(),
                response_id: response.response_id,
                response_model: response.response_model,
                usage: response.usage,
                executed_tool_calls,
                tool_results,
            });
        }

        let assistant_text = response.assistant_text.clone().ok_or_else(|| {
            RuntimeError::MissingAssistantMessage {
                session_id: session.id.clone(),
                response_id: response.response_id.clone(),
            }
        })?;
        let turn = self.append_assistant_turn(
            &session.id,
            Some(prompt),
            assistant_text.clone(),
            Some(observability),
        )?;
        let session = self.session_store.read_metadata(&session.id)?;
        Ok(PlainTextExecOutcome {
            session,
            turn,
            assistant_text,
            response_id: response.response_id,
            response_model: response.response_model,
            usage: response.usage,
            executed_tool_calls,
            tool_results,
        })
    }

    fn run_plain_completion_turn(
        &self,
        session: SessionMetadata,
        profile: BackendProfile,
        prompt: String,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        let mut messages = self.replay_plain_text_messages(&session)?;
        messages.push(PlainTextMessage::user(prompt.clone()));

        let request_started = Instant::now();
        let response = complete_plain_text(&profile, messages).map_err(|source| {
            RuntimeError::ProviderRequest {
                session_id: session.id.clone(),
                source,
            }
        });
        let wallclock_ms = elapsed_ms(request_started);

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                let _ = self.session_store.append_turn(
                    &session.id,
                    &[
                        NewItem::new(TranscriptItemKind::UserMessage, prompt),
                        NewItem::new(TranscriptItemKind::Note, error.to_string()),
                    ],
                );
                return Err(error);
            }
        };
        let observability =
            self.build_turn_observability(&session.id, wallclock_ms, response.usage.as_ref())?;
        let assistant_text = response.assistant_text.clone().ok_or_else(|| {
            RuntimeError::MissingAssistantMessage {
                session_id: session.id.clone(),
                response_id: response.response_id.clone(),
            }
        })?;

        let turn = self.session_store.append_turn_with_observability(
            &session.id,
            &[
                NewItem::new(TranscriptItemKind::UserMessage, prompt),
                NewItem::new(TranscriptItemKind::AssistantMessage, assistant_text.clone()),
            ],
            Some(observability),
        )?;
        let session = self.session_store.read_metadata(&session.id)?;
        Ok(PlainTextExecOutcome {
            session,
            turn,
            assistant_text,
            response_id: response.response_id,
            response_model: response.response_model,
            usage: response.usage,
            executed_tool_calls: 0,
            tool_results: Vec::new(),
        })
    }

    fn build_tool_execution_context(
        &self,
        session: &SessionMetadata,
        tool_loop: &ToolLoopConfig,
        prompt: Option<&str>,
    ) -> Result<ToolExecutionContext, RuntimeError> {
        let transcript = self.session_store.read_transcript(&session.id)?;
        let session_summary = build_decision_summary(session, transcript.as_slice());
        let mut execution_context = ToolExecutionContext::new(session.cwd.clone());
        if let Some(oracle) = tool_loop.oracle.as_ref() {
            execution_context = execution_context.with_oracle(ToolOracleContext::new(
                oracle.profile.clone(),
                oracle.max_calls,
                session_summary.oracle_calls,
            ));
        }
        if let Some(long_context) = tool_loop.long_context.as_ref() {
            execution_context = execution_context.with_long_context(ToolLongContextContext::new(
                long_context.profile.clone(),
                long_context.max_calls,
                session_summary.long_context_calls,
                long_context.max_evidence_files,
                long_context.max_lines_per_file,
                prompt.map_or(0, |value| value.chars().count()),
                session_summary.files_listed.len(),
                session_summary.files_searched.len(),
                session_summary.files_read.len(),
                session_summary.too_many_turns,
                session_summary.oracle_calls,
            ));
        }
        Ok(execution_context)
    }

    fn replay_apple_fm_transcript(
        &self,
        session: &SessionMetadata,
    ) -> Result<AppleFmTranscript, RuntimeError> {
        let mut entries = Vec::new();

        for event in self.session_store.read_transcript(&session.id)? {
            let mut pending_tool_calls = Vec::new();
            let mut pending_tool_entry_id: Option<String> = None;
            for item in event.turn.items {
                match item.kind {
                    TranscriptItemKind::UserMessage => {
                        flush_apple_pending_tool_calls(
                            &mut entries,
                            &mut pending_tool_calls,
                            &mut pending_tool_entry_id,
                        );
                        entries.push(apple_fm_text_entry(
                            format!("turn-{}-user-{}", event.turn.index, item.sequence),
                            "user",
                            item.text,
                            BTreeMap::new(),
                        ));
                    }
                    TranscriptItemKind::AssistantMessage => {
                        flush_apple_pending_tool_calls(
                            &mut entries,
                            &mut pending_tool_calls,
                            &mut pending_tool_entry_id,
                        );
                        entries.push(apple_fm_text_entry(
                            format!("turn-{}-assistant-{}", event.turn.index, item.sequence),
                            "assistant",
                            item.text,
                            BTreeMap::new(),
                        ));
                    }
                    TranscriptItemKind::ToolCall => {
                        let name = item.name.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool call transcript items require a tool name",
                            ))
                        })?;
                        let arguments = item.arguments.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool call transcript items require structured arguments",
                            ))
                        })?;
                        if pending_tool_entry_id.is_none() {
                            pending_tool_entry_id =
                                Some(format!("turn-{}-assistant-tools", event.turn.index));
                        }
                        pending_tool_calls.push(serde_json::json!({
                            "name": name,
                            "arguments": arguments,
                        }));
                    }
                    TranscriptItemKind::ToolResult => {
                        flush_apple_pending_tool_calls(
                            &mut entries,
                            &mut pending_tool_calls,
                            &mut pending_tool_entry_id,
                        );
                        let name = item.name.ok_or_else(|| {
                            RuntimeError::MalformedTranscript(String::from(
                                "tool result transcript items require a tool name",
                            ))
                        })?;
                        let mut extra = BTreeMap::from([(
                            String::from("toolName"),
                            serde_json::Value::String(name),
                        )]);
                        if let Some(tool_call_id) = item.tool_call_id {
                            extra.insert(
                                String::from("toolCallId"),
                                serde_json::Value::String(tool_call_id),
                            );
                        }
                        entries.push(apple_fm_text_entry(
                            format!("turn-{}-tool-{}", event.turn.index, item.sequence),
                            "tool",
                            item.text,
                            extra,
                        ));
                    }
                    TranscriptItemKind::Note => {}
                }
            }
            flush_apple_pending_tool_calls(
                &mut entries,
                &mut pending_tool_calls,
                &mut pending_tool_entry_id,
            );
        }

        Ok(AppleFmTranscript {
            version: APPLE_FM_TRANSCRIPT_VERSION,
            transcript_type: APPLE_FM_TRANSCRIPT_TYPE.to_string(),
            transcript: AppleFmTranscriptPayload { entries },
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

    fn replay_plain_text_messages(
        &self,
        session: &SessionMetadata,
    ) -> Result<Vec<PlainTextMessage>, RuntimeError> {
        let mut messages = Vec::new();
        if let Some(system_prompt) = &session.system_prompt {
            messages.push(PlainTextMessage::system(system_prompt.clone()));
        }

        for event in self.session_store.read_transcript(&session.id)? {
            for item in event.turn.items {
                match item.kind {
                    TranscriptItemKind::UserMessage => {
                        messages.push(PlainTextMessage::user(item.text));
                    }
                    TranscriptItemKind::AssistantMessage => {
                        messages.push(PlainTextMessage::assistant(item.text));
                    }
                    TranscriptItemKind::Note => {}
                    TranscriptItemKind::ToolCall | TranscriptItemKind::ToolResult => {
                        return Err(RuntimeError::MalformedTranscript(String::from(
                            "plain-text backend replay does not support stored tool items",
                        )));
                    }
                }
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

    fn append_recorded_tool_call_turn(
        &self,
        session_id: &SessionId,
        user_prompt: Option<String>,
        executed_tool_calls: &[ExecutedToolCall],
        observability: Option<TurnObservability>,
    ) -> Result<SessionTurn, RuntimeError> {
        let mut items = Vec::new();
        if let Some(user_prompt) = user_prompt {
            items.push(NewItem::new(TranscriptItemKind::UserMessage, user_prompt));
        }
        for tool_call in executed_tool_calls {
            items.push(NewItem::tool_call(
                tool_call.name.clone(),
                tool_call.call_id.clone(),
                tool_call.arguments.clone(),
            ));
        }
        self.session_store
            .append_turn_with_observability(session_id, &items, observability)
            .map_err(RuntimeError::from)
    }

    fn append_assistant_turn(
        &self,
        session_id: &SessionId,
        user_prompt: Option<String>,
        assistant_text: String,
        observability: Option<TurnObservability>,
    ) -> Result<SessionTurn, RuntimeError> {
        let mut items = Vec::new();
        if let Some(user_prompt) = user_prompt {
            items.push(NewItem::new(TranscriptItemKind::UserMessage, user_prompt));
        }
        items.push(NewItem::new(
            TranscriptItemKind::AssistantMessage,
            assistant_text,
        ));
        self.session_store
            .append_turn_with_observability(session_id, &items, observability)
            .map_err(RuntimeError::from)
    }

    fn build_turn_observability(
        &self,
        session_id: &SessionId,
        wallclock_ms: u64,
        usage: Option<&ProviderUsage>,
    ) -> Result<TurnObservability, RuntimeError> {
        let prompt_tokens = usage.and_then(ProviderUsage::prompt_tokens_u32);
        let previous = if prompt_tokens.is_some() {
            self.last_prompt_bearing_observability(session_id)?
        } else {
            self.last_turn_observability(session_id)?
        };

        Ok(TurnObservability {
            wallclock_ms,
            model_output_ms: Some(wallclock_ms),
            prompt_tokens,
            completion_tokens: usage.and_then(ProviderUsage::completion_tokens_u32),
            total_tokens: usage.and_then(ProviderUsage::total_tokens_u32),
            completion_tokens_per_second_x1000: usage.and_then(|usage| {
                usage.completion_tokens_u32().and_then(|completion_tokens| {
                    completion_tokens_per_second_x1000(completion_tokens, wallclock_ms)
                })
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
}

impl AppleFmToolLoopRecorder {
    fn new(execution_session: ToolExecutionSession, max_callback_calls: usize) -> Self {
        Self {
            execution_session,
            records: Vec::new(),
            next_call_index: 0,
            max_callback_calls,
            interruption: None,
        }
    }

    fn handle_call(
        &mut self,
        tool_call: probe_provider_apple_fm::AppleFmProviderToolCall,
    ) -> Result<String, AppleFmToolCallError> {
        if self.next_call_index >= self.max_callback_calls {
            self.interruption = Some(AppleFmToolLoopInterruption::CallbackBudgetExceeded {
                max_round_trips: self.max_callback_calls,
            });
            return Err(AppleFmToolCallError::new(
                tool_call.name,
                format!(
                    "Probe controller-side Apple FM callback budget of {} round trips was exhausted",
                    self.max_callback_calls
                ),
            ));
        }

        self.next_call_index += 1;
        let call_id = format!("apple_fm_call_{}", self.next_call_index);
        let executed = self.execution_session.execute_named_call(
            call_id.clone(),
            tool_call.name.clone(),
            tool_call.arguments,
        );
        let output = serde_json::to_string(&executed.output)
            .unwrap_or_else(|_| String::from("{\"error\":\"tool output encode failed\"}"));
        if executed.was_paused() {
            self.interruption = Some(AppleFmToolLoopInterruption::ApprovalPending {
                tool_name: executed.name.clone(),
                call_id,
                reason: executed.tool_execution.reason.clone(),
            });
            self.records.push(executed.clone());
            return Err(AppleFmToolCallError::new(
                executed.name,
                executed
                    .tool_execution
                    .reason
                    .clone()
                    .unwrap_or_else(|| String::from("tool execution paused for approval")),
            ));
        }
        self.records.push(executed);
        Ok(output)
    }
}

fn apple_fm_text_entry(
    id: String,
    role: &str,
    text: String,
    extra: BTreeMap<String, serde_json::Value>,
) -> AppleFmTranscriptEntry {
    AppleFmTranscriptEntry {
        id: Some(id.clone()),
        role: role.to_string(),
        contents: vec![AppleFmTranscriptContent {
            content_type: String::from("text"),
            id: Some(format!("{id}-content")),
            extra: BTreeMap::from([(String::from("text"), serde_json::Value::String(text))]),
        }],
        extra,
    }
}

fn flush_apple_pending_tool_calls(
    entries: &mut Vec<AppleFmTranscriptEntry>,
    pending_tool_calls: &mut Vec<serde_json::Value>,
    pending_tool_entry_id: &mut Option<String>,
) {
    if pending_tool_calls.is_empty() {
        return;
    }
    let entry_id = pending_tool_entry_id
        .take()
        .unwrap_or_else(|| String::from("assistant-tools"));
    let mut extra = BTreeMap::new();
    extra.insert(
        String::from("toolCalls"),
        serde_json::Value::Array(std::mem::take(pending_tool_calls)),
    );
    entries.push(apple_fm_text_entry(
        entry_id,
        "assistant",
        String::new(),
        extra,
    ));
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
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use crate::backend_profiles::{psionic_apple_fm_bridge, psionic_qwen35_2b_q8_registry};
    use crate::tools::{ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction, ToolLoopConfig};
    use probe_protocol::session::{
        CacheSignal, SessionHarnessProfile, ToolApprovalState, ToolPolicyDecision,
        TranscriptItemKind,
    };
    use probe_test_support::{
        FakeAppleFmServer, FakeHttpRequest, FakeHttpResponse, FakeOpenAiServer,
        ProbeTestEnvironment,
    };
    use serde_json::json;

    use super::{
        PlainTextExecRequest, PlainTextResumeRequest, ProbeRuntime, default_session_title,
    };

    #[derive(Default)]
    struct AppleFmSessionBridgeState {
        callback_url: String,
        session_token: String,
        create_requests: Vec<serde_json::Value>,
        response_requests: Vec<serde_json::Value>,
    }

    struct ToolCallbackResponse {
        status_code: u16,
        body: String,
    }

    fn record_apple_session_create(
        state: &Arc<Mutex<AppleFmSessionBridgeState>>,
        request: &FakeHttpRequest,
        session_id: &str,
    ) -> FakeHttpResponse {
        let request_json: serde_json::Value =
            serde_json::from_str(request.body.as_str()).expect("session create json");
        let callback_url = request_json["tool_callback"]["url"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let session_token = request_json["tool_callback"]["session_token"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let mut guard = state.lock().expect("apple fm session bridge lock");
        guard.callback_url = callback_url;
        guard.session_token = session_token;
        guard.create_requests.push(request_json.clone());
        FakeHttpResponse::json_ok(json!({
            "session": {
                "id": session_id,
                "instructions": request_json["instructions"],
                "model": {
                    "id": "apple-foundation-model",
                    "use_case": "general",
                    "guardrails": "default"
                },
                "tools": request_json["tools"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|tool| json!({
                        "name": tool["name"],
                        "description": tool["description"]
                    }))
                    .collect::<Vec<_>>(),
                "is_responding": false,
                "transcript_json": serde_json::to_string(&request_json["transcript"])
                    .unwrap_or_else(|_| String::from("{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"))
            }
        }))
    }

    fn invoke_apple_tool_callback(
        state: &Arc<Mutex<AppleFmSessionBridgeState>>,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> ToolCallbackResponse {
        let (callback_url, session_token) = {
            let guard = state.lock().expect("apple fm session bridge lock");
            (guard.callback_url.clone(), guard.session_token.clone())
        };
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

    fn record_apple_response_request(
        state: &Arc<Mutex<AppleFmSessionBridgeState>>,
        request: &FakeHttpRequest,
    ) {
        let request_json: serde_json::Value =
            serde_json::from_str(request.body.as_str()).expect("session respond json");
        state
            .lock()
            .expect("apple fm session bridge lock")
            .response_requests
            .push(request_json);
    }

    #[test]
    fn default_title_is_trimmed_for_exec_prompts() {
        assert_eq!(default_session_title("  hello world  "), "hello world");
        assert_eq!(default_session_title(""), "Probe Session");
    }

    #[test]
    fn exec_plain_text_persists_session_and_transcript() {
        let server = FakeOpenAiServer::from_json_responses(vec![serde_json::json!({
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
        })]);

        let environment = ProbeTestEnvironment::new();
        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = String::from(server.base_url());

        let outcome = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("say hello"),
                title: Some(String::from("Exec Test")),
                cwd: environment.workspace().to_path_buf(),
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
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("say hello"));
    }

    #[test]
    fn apple_fm_exec_plain_text_persists_session_and_transcript() {
        let server = FakeAppleFmServer::from_json_responses(vec![serde_json::json!({
            "id": "apple_fm_exec_test",
            "model": "apple-foundation-model",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "hello from apple fm"},
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens_detail": {"value": 9, "truth": "estimated"},
                "completion_tokens_detail": {"value": 4, "truth": "estimated"},
                "total_tokens_detail": {"value": 13, "truth": "estimated"}
            }
        })]);

        let environment = ProbeTestEnvironment::new();
        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = String::from(server.base_url());

        let outcome = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("say hello"),
                title: Some(String::from("Apple FM Exec Test")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: Some(String::from("You are helpful")),
                harness_profile: None,
                tool_loop: None,
            })
            .expect("apple fm exec should succeed");

        assert_eq!(outcome.assistant_text, "hello from apple fm");
        assert_eq!(outcome.response_id, "apple_fm_exec_test");
        assert_eq!(outcome.response_model, "apple-foundation-model");
        assert_eq!(outcome.turn.items.len(), 2);
        assert_eq!(
            outcome
                .session
                .backend
                .as_ref()
                .expect("backend metadata")
                .profile_name,
            "psionic-apple-fm-bridge"
        );
        let observability = outcome
            .turn
            .observability
            .as_ref()
            .expect("observability should exist");
        assert_eq!(observability.prompt_tokens, Some(9));
        assert_eq!(observability.completion_tokens, Some(4));
        assert_eq!(observability.total_tokens, Some(13));

        let transcript = runtime
            .session_store()
            .read_transcript(&outcome.session.id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].turn.items[1].text, "hello from apple fm");

        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("POST /v1/chat/completions HTTP/1.1"));
        assert!(requests[0].contains("\"system\""));
        assert!(requests[0].contains("say hello"));
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
    fn apple_fm_resume_replays_prior_context() {
        let server = FakeAppleFmServer::from_json_responses(vec![
            serde_json::json!({
                "id": "apple_fm_first",
                "model": "apple-foundation-model",
                "choices": [
                    {
                        "index": 0,
                        "message": {"role": "assistant", "content": "first apple answer"},
                        "finish_reason": "stop"
                    }
                ]
            }),
            serde_json::json!({
                "id": "apple_fm_second",
                "model": "apple-foundation-model",
                "choices": [
                    {
                        "index": 0,
                        "message": {"role": "assistant", "content": "second apple answer"},
                        "finish_reason": "stop"
                    }
                ]
            }),
        ]);

        let temp = tempfile::tempdir().expect("temp dir");
        let runtime = ProbeRuntime::new(temp.path().join(".probe"));
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();

        let first = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile: profile.clone(),
                prompt: String::from("first prompt"),
                title: Some(String::from("Apple FM Chat")),
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

        assert_eq!(second.assistant_text, "second apple answer");
        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].contains("first prompt"));
        assert!(requests[1].contains("first apple answer"));
        assert!(requests[1].contains("second prompt"));
    }

    #[test]
    fn apple_fm_tool_loop_executes_probe_tools_through_session_callbacks() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let bridge_state = Arc::new(Mutex::new(AppleFmSessionBridgeState::default()));
        let captured_state = Arc::clone(&bridge_state);
        let server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => {
                    record_apple_session_create(&captured_state, &request, "sess-apple-tool-1")
                }
                ("POST", "/v1/sessions/sess-apple-tool-1/responses") => {
                    record_apple_response_request(&captured_state, &request);
                    let callback_response = invoke_apple_tool_callback(
                        &captured_state,
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
                    FakeHttpResponse::json_ok(json!({
                        "session": {
                            "id": "sess-apple-tool-1",
                            "instructions": "You are helpful",
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
                        "output": format!(
                            "tool-backed answer: {}",
                            callback_json["output"].as_str().unwrap_or_default()
                        ),
                        "usage": {
                            "total_tokens_detail": {"value": 21, "truth": "estimated"}
                        }
                    }))
                }
                ("DELETE", "/v1/sessions/sess-apple-tool-1") => {
                    FakeHttpResponse::json_ok(json!({}))
                }
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();

        let outcome = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("Use read_file on hello.txt and tell me the contents."),
                title: Some(String::from("Apple FM Tool Success")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: Some(String::from("You are helpful")),
                harness_profile: None,
                tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                    ProbeToolChoice::Required,
                    false,
                )),
            })
            .expect("apple fm tool loop should succeed");

        assert!(outcome.assistant_text.contains("hello world"));
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
        assert!(matches!(
            transcript[0].turn.items[1].kind,
            TranscriptItemKind::ToolCall
        ));
        assert!(matches!(
            transcript[1].turn.items[0].kind,
            TranscriptItemKind::ToolResult
        ));
        assert!(matches!(
            transcript[2].turn.items[0].kind,
            TranscriptItemKind::AssistantMessage
        ));
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
    }

    #[test]
    fn apple_fm_tool_loop_refusal_persists_probe_receipts() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let bridge_state = Arc::new(Mutex::new(AppleFmSessionBridgeState::default()));
        let captured_state = Arc::clone(&bridge_state);
        let server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => {
                    record_apple_session_create(&captured_state, &request, "sess-apple-refuse-1")
                }
                ("POST", "/v1/sessions/sess-apple-refuse-1/responses") => {
                    record_apple_response_request(&captured_state, &request);
                    let callback_response = invoke_apple_tool_callback(
                        &captured_state,
                        "apply_patch",
                        json!({
                            "path": "hello.txt",
                            "old_text": "world",
                            "new_text": "probe"
                        }),
                    );
                    assert_eq!(callback_response.status_code, 200);
                    let callback_json: serde_json::Value =
                        serde_json::from_str(callback_response.body.as_str())
                            .expect("callback json");
                    FakeHttpResponse::json_ok(json!({
                        "session": {
                            "id": "sess-apple-refuse-1",
                            "instructions": "You are helpful",
                            "model": {
                                "id": "apple-foundation-model",
                                "use_case": "general",
                                "guardrails": "default"
                            },
                            "tools": [{"name": "apply_patch"}],
                            "is_responding": false,
                            "transcript_json": "{\"version\":1,\"type\":\"FoundationModels.Transcript\",\"transcript\":{\"entries\":[]}}"
                        },
                        "model": "apple-foundation-model",
                        "output": format!(
                            "refused output seen: {}",
                            callback_json["output"].as_str().unwrap_or_default()
                        )
                    }))
                }
                ("DELETE", "/v1/sessions/sess-apple-refuse-1") => {
                    FakeHttpResponse::json_ok(json!({}))
                }
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();

        let outcome = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile,
                prompt: String::from("Patch hello.txt."),
                title: Some(String::from("Apple FM Tool Refusal")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: Some(String::from("You are helpful")),
                harness_profile: None,
                tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                    ProbeToolChoice::Required,
                    false,
                )),
            })
            .expect("apple fm refusal should complete");

        assert_eq!(outcome.executed_tool_calls, 0);
        assert_eq!(outcome.tool_results.len(), 1);
        assert_eq!(
            outcome.tool_results[0].tool_execution.policy_decision,
            ToolPolicyDecision::Refused
        );
        let transcript = runtime
            .session_store()
            .read_transcript(&outcome.session.id)
            .expect("read transcript");
        assert_eq!(
            transcript[1].turn.items[0]
                .tool_execution
                .as_ref()
                .expect("tool execution should persist")
                .policy_decision,
            ToolPolicyDecision::Refused
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
    }

    #[test]
    fn apple_fm_tool_loop_can_pause_for_approval() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let bridge_state = Arc::new(Mutex::new(AppleFmSessionBridgeState::default()));
        let captured_state = Arc::clone(&bridge_state);
        let server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => {
                    record_apple_session_create(&captured_state, &request, "sess-apple-pause-1")
                }
                ("POST", "/v1/sessions/sess-apple-pause-1/responses") => {
                    record_apple_response_request(&captured_state, &request);
                    let callback_response = invoke_apple_tool_callback(
                        &captured_state,
                        "apply_patch",
                        json!({
                            "path": "hello.txt",
                            "old_text": "world",
                            "new_text": "probe"
                        }),
                    );
                    assert_eq!(callback_response.status_code, 422);
                    let callback_error: serde_json::Value =
                        serde_json::from_str(callback_response.body.as_str())
                            .expect("callback error json");
                    FakeHttpResponse::json_status(
                        422,
                        json!({
                            "error": {
                                "message": format!(
                                    "tool '{}' failed: {}",
                                    callback_error["tool_name"].as_str().unwrap_or_default(),
                                    callback_error["underlying_error"].as_str().unwrap_or_default()
                                ),
                                "type": "tool_call_failed",
                                "code": "tool_call_failed",
                                "tool_name": callback_error["tool_name"],
                                "underlying_error": callback_error["underlying_error"]
                            }
                        }),
                    )
                }
                ("DELETE", "/v1/sessions/sess-apple-pause-1") => {
                    FakeHttpResponse::json_ok(json!({}))
                }
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();
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
                prompt: String::from("Patch hello.txt."),
                title: Some(String::from("Apple FM Tool Pause")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: Some(String::from("You are helpful")),
                harness_profile: None,
                tool_loop: Some(tool_loop),
            })
            .expect_err("apple fm pause should surface pending approval");

        assert!(matches!(
            error,
            super::RuntimeError::ToolApprovalPending { .. }
        ));
        let sessions = runtime
            .session_store()
            .list_sessions()
            .expect("list sessions");
        let transcript = runtime
            .session_store()
            .read_transcript(&sessions[0].id)
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
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
    }

    #[test]
    fn apple_fm_tool_loop_resume_reconstructs_session_transcript() {
        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let bridge_state = Arc::new(Mutex::new(AppleFmSessionBridgeState::default()));
        let captured_state = Arc::clone(&bridge_state);
        let response_count = Arc::new(Mutex::new(0_usize));
        let captured_responses = Arc::clone(&response_count);
        let server = FakeAppleFmServer::from_handler(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/v1/sessions") => {
                    let index = captured_state
                        .lock()
                        .expect("apple fm session bridge lock")
                        .create_requests
                        .len()
                        + 1;
                    record_apple_session_create(
                        &captured_state,
                        &request,
                        format!("sess-apple-resume-{index}").as_str(),
                    )
                }
                ("POST", path) if path.starts_with("/v1/sessions/sess-apple-resume-") => {
                    record_apple_response_request(&captured_state, &request);
                    let mut response_index = captured_responses
                        .lock()
                        .expect("apple fm response count lock");
                    *response_index += 1;
                    if *response_index == 1 {
                        let callback_response = invoke_apple_tool_callback(
                            &captured_state,
                            "read_file",
                            json!({
                                "path": "hello.txt",
                                "start_line": 1,
                                "max_lines": 10
                            }),
                        );
                        assert_eq!(callback_response.status_code, 200);
                        FakeHttpResponse::json_ok(json!({
                            "session": {
                                "id": "sess-apple-resume-1",
                                "instructions": "You are helpful",
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
                            "output": "first tool-backed answer"
                        }))
                    } else {
                        FakeHttpResponse::json_ok(json!({
                            "session": {
                                "id": "sess-apple-resume-2",
                                "instructions": "You are helpful",
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
                            "output": "second tool-backed answer"
                        }))
                    }
                }
                ("DELETE", path) if path.starts_with("/v1/sessions/sess-apple-resume-") => {
                    FakeHttpResponse::json_ok(json!({}))
                }
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_apple_fm_bridge();
        profile.base_url = server.base_url().to_string();
        let first = runtime
            .exec_plain_text(PlainTextExecRequest {
                profile: profile.clone(),
                prompt: String::from("Read hello.txt"),
                title: Some(String::from("Apple FM Tool Resume")),
                cwd: environment.workspace().to_path_buf(),
                system_prompt: Some(String::from("You are helpful")),
                harness_profile: None,
                tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                    ProbeToolChoice::Required,
                    false,
                )),
            })
            .expect("first apple fm tool turn should succeed");

        let second = runtime
            .continue_plain_text_session(PlainTextResumeRequest {
                session_id: first.session.id.clone(),
                profile,
                prompt: String::from("Summarize the previous read."),
                tool_loop: Some(ToolLoopConfig::coding_bootstrap(
                    ProbeToolChoice::Required,
                    false,
                )),
            })
            .expect("second apple fm tool turn should succeed");

        assert_eq!(second.assistant_text, "second tool-backed answer");
        let bridge_state = bridge_state.lock().expect("apple fm session bridge lock");
        assert_eq!(bridge_state.create_requests.len(), 2);
        let restored_entries =
            bridge_state.create_requests[1]["transcript"]["transcript"]["entries"]
                .as_array()
                .expect("restored entries");
        assert!(restored_entries.iter().any(|entry| {
            entry["toolCalls"]
                .as_array()
                .is_some_and(|calls| calls.iter().any(|call| call["name"] == "read_file"))
        }));
        assert!(restored_entries.iter().any(|entry| {
            entry["role"] == "tool"
                && entry["toolName"] == "read_file"
                && entry["contents"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("hello world")
        }));
        drop(bridge_state);
        let requests = server.finish();
        assert_eq!(requests.len(), 6);
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
        let server = FakeOpenAiServer::from_json_responses(vec![serde_json::json!({
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
        })]);

        let environment = ProbeTestEnvironment::new();
        environment.seed_coding_workspace();
        let runtime = ProbeRuntime::new(environment.probe_home().to_path_buf());
        let mut profile = psionic_qwen35_2b_q8_registry();
        profile.base_url = String::from(server.base_url());
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
                cwd: environment.workspace().to_path_buf(),
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
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("\"tool_choice\":\"required\""));
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
