//! Native model transports. The pure lowering/parsing lives in probe-wire;
//! this module owns the I/O half: HTTP streaming, cancellation, and the
//! stub transports used by tests and the controller conformance drive.

use std::sync::atomic::{AtomicBool, Ordering};

use probe_core::contract::event::{Event, FinishReason};
use probe_core::contract::request::Request;
use probe_core::contract::usage::Usage;

/// A transport runs one request to completion, emitting neutral events. It
/// must observe `cancelled` promptly. Errors are secret-free descriptions.
pub trait Transport: Send + Sync {
    fn run(
        &self,
        request: &Request,
        emit: &mut dyn FnMut(Event),
        cancelled: &AtomicBool,
    ) -> Result<(), String>;
}

/// Deterministic stub: streams a fixed greeting and finishes. Used by the
/// probe-bin integration tests and the controller end-to-end drive.
pub struct StubTransport;

impl Transport for StubTransport {
    fn run(
        &self,
        _request: &Request,
        emit: &mut dyn FnMut(Event),
        cancelled: &AtomicBool,
    ) -> Result<(), String> {
        emit(Event::StepStart { index: 0 });
        for chunk in ["Hello from probe. ", "The zerobase runtime is alive."] {
            if cancelled.load(Ordering::SeqCst) {
                return Ok(());
            }
            emit(Event::TextDelta { id: "text-0".into(), text: chunk.into(), provider_metadata: None });
        }
        emit(Event::Finish {
            reason: FinishReason::Stop,
            usage: Some(Usage { input_tokens: Some(12), output_tokens: Some(11), ..Usage::default() }),
            provider_metadata: None,
        });
        Ok(())
    }
}

/// Stub that streams slowly so cancellation paths are deterministic to test.
pub struct SlowStubTransport;

impl Transport for SlowStubTransport {
    fn run(
        &self,
        _request: &Request,
        emit: &mut dyn FnMut(Event),
        cancelled: &AtomicBool,
    ) -> Result<(), String> {
        emit(Event::StepStart { index: 0 });
        for index in 0..100 {
            if cancelled.load(Ordering::SeqCst) {
                return Ok(());
            }
            emit(Event::TextDelta {
                id: "text-0".into(),
                text: format!("chunk {index} "),
                provider_metadata: None,
            });
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        emit(Event::Finish { reason: FinishReason::Stop, usage: None, provider_metadata: None });
        Ok(())
    }
}

/// Stub that requests one shell tool call on the first round, then completes
/// on the continuation — exercises the whole permission + tool loop without
/// a model.
pub struct ToolStubTransport;

impl Transport for ToolStubTransport {
    fn run(
        &self,
        request: &Request,
        emit: &mut dyn FnMut(Event),
        _cancelled: &AtomicBool,
    ) -> Result<(), String> {
        emit(Event::StepStart { index: 0 });
        let has_tool_results = request
            .messages
            .iter()
            .any(|message| message.role == probe_core::contract::message::Role::Tool);
        if has_tool_results {
            emit(Event::TextDelta { id: "text-1".into(), text: "Tool round complete.".into(), provider_metadata: None });
            emit(Event::Finish { reason: FinishReason::Stop, usage: None, provider_metadata: None });
        } else {
            emit(Event::ToolCall {
                id: "tool_0".into(),
                name: "shell".into(),
                input: serde_json::json!({ "command": "git status" }),
                provider_executed: None,
                provider_metadata: None,
            });
            emit(Event::Finish { reason: FinishReason::ToolCalls, usage: None, provider_metadata: None });
        }
        Ok(())
    }
}

// ── HTTP transports (the I/O half; lowerings live in probe-wire) ─────────

use std::io::{BufRead, BufReader};
use std::sync::Arc;

use probe_core::redact::SecretSet;

fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(20)))
        // Streams are long-lived; bound reads per-chunk, not globally.
        .timeout_global(None)
        .build()
        .into()
}

/// Stream an SSE response line by line into a parser callback, observing
/// cancellation between lines. Never surfaces response body bytes in errors.
fn stream_sse_lines(
    response: &mut ureq::http::Response<ureq::Body>,
    cancelled: &AtomicBool,
    mut on_line: impl FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    let reader = BufReader::new(response.body_mut().as_reader());
    for line in reader.lines() {
        if cancelled.load(Ordering::SeqCst) {
            return Ok(());
        }
        let line = line.map_err(|error| format!("stream read failed: {}", error.kind()))?;
        on_line(&line)?;
    }
    Ok(())
}

/// OpenAI-compatible chat-completions transport: Sarah's inference-grant
/// proxy, local Psionic serving, or any vanilla endpoint. The bearer is the
/// delegation grant when one was injected at spawn.
pub struct OpenAiCompatibleTransport {
    chat_url: String,
    bearer: Option<String>,
}

impl OpenAiCompatibleTransport {
    pub fn new(base_or_chat_url: String, bearer: Option<String>) -> OpenAiCompatibleTransport {
        let chat_url = if base_or_chat_url.contains("/chat/completions") {
            base_or_chat_url
        } else {
            format!("{}/chat/completions", base_or_chat_url.trim_end_matches('/'))
        };
        OpenAiCompatibleTransport { chat_url, bearer }
    }
}

impl Transport for OpenAiCompatibleTransport {
    fn run(
        &self,
        request: &Request,
        emit: &mut dyn FnMut(Event),
        cancelled: &AtomicBool,
    ) -> Result<(), String> {
        let body = probe_wire::openai::lower_request(request);
        let mut http_request = http_agent().post(&self.chat_url).header("content-type", "application/json");
        if let Some(bearer) = &self.bearer {
            http_request = http_request.header("authorization", &format!("Bearer {bearer}"));
        }
        let mut response = http_request
            .send_json(&body)
            .map_err(|error| match error {
                ureq::Error::StatusCode(code) => format!("provider returned HTTP {code}"),
                other => format!("provider request failed: {other}"),
            })?;

        let mut state = probe_wire::openai::OpenAiSseState::new();
        stream_sse_lines(&mut response, cancelled, |line| {
            for event in state.push_line(line).map_err(|error| error.to_string())? {
                emit(event);
            }
            Ok(())
        })?;
        if cancelled.load(Ordering::SeqCst) {
            return Ok(());
        }
        for event in state.finish().map_err(|error| error.to_string())? {
            emit(event);
        }
        Ok(())
    }
}

/// Direct Gemini transport (API key). The Omega-broker rewrite from the
/// archived backend is intentionally not carried until an issuer exists.
pub struct GeminiTransport {
    base_url: String,
    api_key: String,
}

impl GeminiTransport {
    pub fn new(base_url: String, api_key: String) -> GeminiTransport {
        GeminiTransport { base_url: base_url.trim_end_matches('/').to_string(), api_key }
    }
}

impl Transport for GeminiTransport {
    fn run(
        &self,
        request: &Request,
        emit: &mut dyn FnMut(Event),
        cancelled: &AtomicBool,
    ) -> Result<(), String> {
        let url = format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
            self.base_url, request.model.model
        );
        let body = probe_wire::gemini::lower_request(request);
        let mut response = http_agent()
            .post(&url)
            .header("content-type", "application/json")
            .header("x-goog-api-key", &self.api_key)
            .send_json(&body)
            .map_err(|error| match error {
                ureq::Error::StatusCode(code) => format!("provider returned HTTP {code}"),
                other => format!("provider request failed: {other}"),
            })?;

        let mut state = probe_wire::gemini::GeminiSseState::new();
        stream_sse_lines(&mut response, cancelled, |line| {
            for event in state.push_line(line).map_err(|error| error.to_string())? {
                emit(event);
            }
            Ok(())
        })?;
        if cancelled.load(Ordering::SeqCst) {
            return Ok(());
        }
        for event in state.finish().map_err(|error| error.to_string())? {
            emit(event);
        }
        Ok(())
    }
}

pub fn build_openai(
    url: &Option<String>,
    grant: &Option<String>,
) -> Result<Arc<dyn Transport>, String> {
    let url = url
        .clone()
        .ok_or_else(|| "openai transport selected but PROBE_INFERENCE_URL is not set".to_string())?;
    if url.contains("openagents.com") && grant.is_none() {
        return Err(
            "inference grant missing: PROBE_INFERENCE_URL points at a Sarah proxy but \
             PROBE_INFERENCE_GRANT is not set (grants are delegation-scoped and injected by the \
             controller at spawn)"
                .to_string(),
        );
    }
    Ok(Arc::new(OpenAiCompatibleTransport::new(url, grant.clone())))
}

pub fn build_gemini(secrets: &mut SecretSet) -> Result<Arc<dyn Transport>, String> {
    let key = std::env::var("GOOGLE_GENERATIVE_AI_API_KEY")
        .or_else(|_| std::env::var("GEMINI_API_KEY"))
        .map_err(|_| "gemini transport selected but no API key env is set".to_string())?;
    secrets.register(key.clone());
    let endpoint = probe_wire::provenance::ResolvedEndpoint::from_env(
        "PROBE_GEMINI_BASE_URL",
        "https://generativelanguage.googleapis.com",
    );
    eprintln!(
        "probe-bin: gemini endpoint {} ({})",
        endpoint.base_url,
        serde_json::to_string(&endpoint.source).unwrap_or_default()
    );
    Ok(Arc::new(GeminiTransport::new(endpoint.base_url, key)))
}
