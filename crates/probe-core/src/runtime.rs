use std::env;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

use probe_protocol::backend::BackendProfile;
use probe_protocol::session::{
    SessionBackendTarget, SessionId, SessionMetadata, SessionTurn, TranscriptItemKind,
};
use probe_provider_openai::{
    ChatCompletionUsage, ChatMessage, OpenAiProviderClient, OpenAiProviderConfig,
    OpenAiProviderError,
};

use crate::session_store::{FilesystemSessionStore, NewItem, NewSession, SessionStoreError};

const DEFAULT_PROBE_HOME_DIR: &str = ".probe";

#[derive(Clone, Debug)]
pub struct PlainTextExecRequest {
    pub profile: BackendProfile,
    pub prompt: String,
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub system_prompt: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PlainTextResumeRequest {
    pub session_id: SessionId,
    pub profile: BackendProfile,
    pub prompt: String,
}

#[derive(Clone, Debug)]
pub struct PlainTextExecOutcome {
    pub session: SessionMetadata,
    pub turn: SessionTurn,
    pub assistant_text: String,
    pub response_id: String,
    pub response_model: String,
    pub usage: Option<ChatCompletionUsage>,
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
            .with_backend(SessionBackendTarget {
                profile_name: request.profile.name.clone(),
                base_url: request.profile.base_url.clone(),
                model: request.profile.model.clone(),
            }),
        )?;

        self.run_plain_text_turn(session, request.profile, request.prompt)
    }

    pub fn continue_plain_text_session(
        &self,
        request: PlainTextResumeRequest,
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        let session = self.session_store.read_metadata(&request.session_id)?;
        self.run_plain_text_turn(session, request.profile, request.prompt)
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
    ) -> Result<PlainTextExecOutcome, RuntimeError> {
        let provider_config = OpenAiProviderConfig::from_backend_profile(&profile);
        let provider =
            OpenAiProviderClient::new(provider_config).map_err(RuntimeError::ProviderBuild)?;
        let mut messages = self.replay_messages(&session)?;
        messages.push(ChatMessage::user(prompt.clone()));

        let response =
            provider
                .chat_completion(messages)
                .map_err(|source| RuntimeError::ProviderRequest {
                    session_id: session.id.clone(),
                    source,
                });

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

        let assistant_text = response
            .first_message_text()
            .map(ToOwned::to_owned)
            .ok_or_else(|| RuntimeError::MissingAssistantMessage {
                session_id: session.id.clone(),
                response_id: response.id.clone(),
            })?;

        let turn = self.session_store.append_turn(
            &session.id,
            &[
                NewItem::new(TranscriptItemKind::UserMessage, prompt),
                NewItem::new(TranscriptItemKind::AssistantMessage, assistant_text.clone()),
            ],
        )?;

        let session = self.session_store.read_metadata(&session.id)?;
        Ok(PlainTextExecOutcome {
            session,
            turn,
            assistant_text,
            response_id: response.id,
            response_model: response.model,
            usage: response.usage,
        })
    }

    fn replay_messages(&self, session: &SessionMetadata) -> Result<Vec<ChatMessage>, RuntimeError> {
        let mut messages = Vec::new();
        if let Some(system_prompt) = &session.system_prompt {
            messages.push(ChatMessage::system(system_prompt.clone()));
        }

        for event in self.session_store.read_transcript(&session.id)? {
            for item in event.turn.items {
                match item.kind {
                    TranscriptItemKind::UserMessage => messages.push(ChatMessage::user(item.text)),
                    TranscriptItemKind::AssistantMessage => {
                        messages.push(ChatMessage::assistant(item.text))
                    }
                    TranscriptItemKind::ToolCall
                    | TranscriptItemKind::ToolResult
                    | TranscriptItemKind::Note => {}
                }
            }
        }

        Ok(messages)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use crate::backend_profiles::psionic_qwen35_2b_q8_registry;

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
            })
            .expect("exec should succeed");

        assert_eq!(outcome.assistant_text, "hello from probe exec");
        assert_eq!(outcome.response_id, "chatcmpl_exec_test");
        assert_eq!(outcome.response_model, "qwen3.5-2b-q8_0-registry.gguf");
        assert_eq!(outcome.turn.items.len(), 2);
        assert_eq!(outcome.session.title, "Exec Test");
        assert_eq!(
            outcome
                .session
                .backend
                .as_ref()
                .expect("backend metadata should exist")
                .profile_name,
            "psionic-qwen35-2b-q8-registry"
        );

        let transcript = runtime
            .session_store()
            .read_transcript(&outcome.session.id)
            .expect("read transcript");
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].turn.items[0].text, "say hello");
        assert_eq!(transcript[0].turn.items[1].text, "hello from probe exec");

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
            })
            .expect("first turn should succeed");

        let second = runtime
            .continue_plain_text_session(PlainTextResumeRequest {
                session_id: first.session.id.clone(),
                profile,
                prompt: String::from("second prompt"),
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

        handle.join().expect("server thread should exit cleanly");
    }
}
