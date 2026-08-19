//! probe-wasm: a wasm-bindgen surface over the sans-I/O Engine. The ABI is
//! deliberately synchronous — line/event in, JSON command array out — so the
//! JavaScript host owns every async concern (fetch, timers, tool execution).
//! This avoids JSPI entirely, so the module runs on stable Node LTS, Bun,
//! and every browser with no flags.
//!
//! The host loop mirrors probe-bin exactly: feed protocol lines and provider
//! events in, act on WriteLine/StartStream/CancelStream/RunTool commands out.

use probe_acp::engine::{Engine, EngineConfig, HostCommand};
use probe_core::contract::event::Event;
use probe_core::contract::message::ToolResultValue;
use probe_core::contract::request::ModelRef;
use probe_core::permission::ToolKind;
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// JSON shape of a host command handed to JavaScript. Requests and
/// invocations are passed through as their canonical contract JSON.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum JsCommand {
    WriteLine { line: String },
    StartStream { request: serde_json::Value },
    CancelStream,
    RunTool { id: String, name: String, input: serde_json::Value },
}

fn to_js_commands(commands: Vec<HostCommand>) -> Vec<JsCommand> {
    commands
        .into_iter()
        .map(|command| match command {
            HostCommand::WriteLine(line) => JsCommand::WriteLine { line },
            HostCommand::StartStream(request) => {
                JsCommand::StartStream { request: serde_json::to_value(request).unwrap_or(serde_json::Value::Null) }
            }
            HostCommand::CancelStream => JsCommand::CancelStream,
            HostCommand::RunTool(invocation) => JsCommand::RunTool {
                id: invocation.id,
                name: invocation.name,
                input: invocation.input,
            },
        })
        .collect()
}

fn commands_to_json(commands: Vec<HostCommand>) -> String {
    serde_json::to_string(&to_js_commands(commands)).unwrap_or_else(|_| "[]".to_string())
}

/// The wasm-side engine. Construct once per ACP connection; call `handle_*`
/// and act on the returned JSON command array. Tool definitions and their
/// ACP kinds are provided by the host as JSON so the catalog stays a host
/// concern (the host has the filesystem; the core does not).
#[wasm_bindgen]
pub struct ProbeEngine {
    engine: Engine,
}

#[wasm_bindgen]
impl ProbeEngine {
    /// `config_json`: { provider, model, systemPrompt, tools: ToolDefinition[],
    /// toolKinds: { name: kind } }.
    #[wasm_bindgen(constructor)]
    pub fn new(config_json: &str) -> Result<ProbeEngine, JsError> {
        let raw: serde_json::Value = serde_json::from_str(config_json)
            .map_err(|error| JsError::new(&format!("invalid config JSON: {error}")))?;
        let model = ModelRef {
            provider: raw["provider"].as_str().unwrap_or("probe").to_string(),
            model: raw["model"].as_str().unwrap_or("default").to_string(),
        };
        let mut config = EngineConfig::new(model, raw["systemPrompt"].as_str().unwrap_or_default());
        if let Some(tools) = raw.get("tools") {
            config.tools = serde_json::from_value(tools.clone())
                .map_err(|error| JsError::new(&format!("invalid tools: {error}")))?;
        }
        if let Some(kinds) = raw["toolKinds"].as_object() {
            for (name, kind) in kinds {
                if let Ok(kind) = serde_json::from_value::<ToolKind>(kind.clone()) {
                    config.tool_kinds.insert(name.clone(), kind);
                }
            }
        }
        Ok(ProbeEngine { engine: Engine::new(config) })
    }

    /// One inbound protocol line. Returns the JSON command array.
    #[wasm_bindgen(js_name = handleLine)]
    pub fn handle_line(&mut self, line: &str) -> String {
        commands_to_json(self.engine.handle_line(line))
    }

    /// One neutral provider event (contract JSON from the host's transport).
    #[wasm_bindgen(js_name = onProviderEvent)]
    pub fn on_provider_event(&mut self, event_json: &str) -> Result<String, JsError> {
        let event: Event = serde_json::from_str(event_json)
            .map_err(|error| JsError::new(&format!("invalid event JSON: {error}")))?;
        Ok(commands_to_json(self.engine.on_provider_event(event)))
    }

    /// The transport failed (secret-free description).
    #[wasm_bindgen(js_name = onProviderFailure)]
    pub fn on_provider_failure(&mut self, message: &str) -> String {
        commands_to_json(self.engine.on_provider_failure(message))
    }

    /// A tool outcome (result-value contract JSON, e.g. {"type":"text",...}).
    #[wasm_bindgen(js_name = onToolOutcome)]
    pub fn on_tool_outcome(&mut self, tool_call_id: &str, result_json: &str) -> Result<String, JsError> {
        let result: ToolResultValue = serde_json::from_str(result_json)
            .map_err(|error| JsError::new(&format!("invalid tool result JSON: {error}")))?;
        Ok(commands_to_json(self.engine.on_tool_outcome(tool_call_id, result)))
    }
}

/// The Gemini and OpenAI-compatible lowerings are also useful from JS hosts
/// that run their own fetch; expose them as pure functions.
#[wasm_bindgen(js_name = lowerOpenAiRequest)]
pub fn lower_openai_request(request_json: &str) -> Result<String, JsError> {
    let request = serde_json::from_str(request_json)
        .map_err(|error| JsError::new(&format!("invalid request JSON: {error}")))?;
    Ok(serde_json::to_string(&probe_wire::openai::lower_request(&request)).unwrap())
}

/// Parse a complete OpenAI-compatible SSE body into contract events JSON.
#[wasm_bindgen(js_name = parseOpenAiSse)]
pub fn parse_openai_sse(body: &str) -> Result<String, JsError> {
    let events = probe_wire::openai::parse_sse(body).map_err(|error| JsError::new(&error.to_string()))?;
    Ok(serde_json::to_string(&events).unwrap())
}

#[wasm_bindgen(js_name = probeVersion)]
pub fn probe_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
