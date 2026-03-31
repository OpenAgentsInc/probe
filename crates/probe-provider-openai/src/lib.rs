use std::fmt::{Display, Formatter};
use std::time::Duration;

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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: String::from("system"),
            content: content.into(),
        }
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: String::from("user"),
            content: content.into(),
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: String::from("assistant"),
            content: content.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub stream: bool,
}

impl ChatCompletionRequest {
    #[must_use]
    pub fn from_config(config: &OpenAiProviderConfig, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: config.model.clone(),
            messages,
            stream: config.stream,
        }
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
        self.choices.first().map(|choice| choice.message.content.as_str())
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
                write!(f, "streaming is not implemented in the initial provider client")
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::{
        ChatCompletionResponse, ChatMessage, OpenAiProviderClient, OpenAiProviderConfig,
        OpenAiProviderError,
    };

    #[test]
    fn localhost_helper_uses_local_default() {
        let config = OpenAiProviderConfig::localhost("example.gguf");
        assert_eq!(config.base_url, "http://127.0.0.1:8080/v1");
        assert_eq!(config.api_key, "dummy");
        assert_eq!(config.chat_completions_endpoint(), "http://127.0.0.1:8080/v1/chat/completions");
    }

    #[test]
    fn client_executes_plain_text_chat_completion_against_local_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let body = serde_json::json!({
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

        let config = OpenAiProviderConfig {
            base_url: format!("http://{address}/v1"),
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
        handle.join().expect("server thread");
    }

    #[test]
    fn http_status_errors_surface_response_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("listener addr");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let body = String::from("{\"error\":\"bad request\"}");
            let response = format!(
                "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let config = OpenAiProviderConfig {
            base_url: format!("http://{address}/v1"),
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

        handle.join().expect("server thread");
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
