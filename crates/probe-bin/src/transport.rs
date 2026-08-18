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

/// OpenAI-compatible and Gemini transports land with the probe-wire phase
/// (#209); until then selecting them is an honest startup failure.
pub fn build_openai(
    url: &Option<String>,
    grant: &Option<String>,
) -> Result<std::sync::Arc<dyn Transport>, String> {
    let _ = (url, grant);
    Err("openai transport lands with probe-wire (#209)".to_string())
}

pub fn build_gemini(
    secrets: &mut probe_core::redact::SecretSet,
) -> Result<std::sync::Arc<dyn Transport>, String> {
    let _ = secrets;
    Err("gemini transport lands with probe-wire (#209)".to_string())
}
