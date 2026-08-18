//! Gemini lowering and SSE stream parser, ported from the archived TS
//! `backends/gemini/protocol.ts`. The acceptance suite is the shared corpus
//! `fixtures/gemini/sse-stream.json`, itself ported from the archived
//! stream-parser tests: thought parts become reasoning deltas with their
//! signatures preserved, function calls normalize to tool-call events,
//! thoughts add to candidate tokens for inclusive output usage, and a
//! malformed payload fails without leaking its bytes.

use probe_core::contract::event::{Event, FinishReason};
use probe_core::contract::message::{ContentPart, Message, Role, ToolResultValue};
use probe_core::contract::request::Request;
use probe_core::contract::usage::Usage;

use crate::WireError;

/// Lower a neutral request to the `:streamGenerateContent` body.
pub fn lower_request(request: &Request) -> serde_json::Value {
    let mut body = serde_json::Map::new();

    let system_text: String = request
        .system
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|part| match part {
            ContentPart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !system_text.is_empty() {
        body.insert(
            "systemInstruction".into(),
            serde_json::json!({ "parts": [{ "text": system_text }] }),
        );
    }

    let contents: Vec<serde_json::Value> = request.messages.iter().map(lower_message).collect();
    body.insert("contents".into(), serde_json::Value::Array(contents));

    if !request.tools.is_empty() {
        let declarations: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                })
            })
            .collect();
        body.insert(
            "tools".into(),
            serde_json::json!([{ "functionDeclarations": declarations }]),
        );
    }

    if let Some(generation) = &request.generation {
        let mut config = serde_json::Map::new();
        if let Some(max) = generation.max_tokens {
            config.insert("maxOutputTokens".into(), max.into());
        }
        if let Some(temperature) = &generation.temperature {
            config.insert("temperature".into(), serde_json::Value::Number(temperature.clone()));
        }
        if let Some(top_p) = &generation.top_p {
            config.insert("topP".into(), serde_json::Value::Number(top_p.clone()));
        }
        if let Some(top_k) = generation.top_k {
            config.insert("topK".into(), top_k.into());
        }
        if let Some(stop) = &generation.stop {
            config.insert("stopSequences".into(), serde_json::json!(stop));
        }
        if !config.is_empty() {
            body.insert("generationConfig".into(), serde_json::Value::Object(config));
        }
    }

    serde_json::Value::Object(body)
}

fn thought_signature(part: &ContentPart) -> Option<&serde_json::Value> {
    let provider_metadata = match part {
        ContentPart::ToolCall { provider_metadata, .. }
        | ContentPart::Reasoning { provider_metadata, .. }
        | ContentPart::Text { provider_metadata, .. } => provider_metadata.as_ref(),
        _ => None,
    };
    provider_metadata?.get("google")?.get("thoughtSignature")
}

fn lower_message(message: &Message) -> serde_json::Value {
    let role = match message.role {
        Role::Assistant => "model",
        // Gemini carries tool responses in a user-role turn.
        _ => "user",
    };
    let parts: Vec<serde_json::Value> = message
        .content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text, .. } => serde_json::json!({ "text": text }),
            ContentPart::Reasoning { text, .. } => serde_json::json!({ "text": text, "thought": true }),
            ContentPart::ToolCall { name, input, .. } => {
                let mut lowered = serde_json::Map::new();
                lowered.insert("functionCall".into(), serde_json::json!({ "name": name, "args": input }));
                if let Some(signature) = thought_signature(part) {
                    lowered.insert("thoughtSignature".into(), signature.clone());
                }
                serde_json::Value::Object(lowered)
            }
            ContentPart::ToolResult { name, result, .. } => {
                let response = match result {
                    ToolResultValue::Json { value } => value.clone(),
                    ToolResultValue::Text { value } => serde_json::json!({ "output": value }),
                    ToolResultValue::Error { value } => serde_json::json!({ "error": value }),
                };
                serde_json::json!({ "functionResponse": { "name": name, "response": response } })
            }
            ContentPart::Media { media_type, data, .. } => {
                serde_json::json!({ "inlineData": { "mimeType": media_type, "data": data } })
            }
        })
        .collect();
    serde_json::json!({ "role": role, "parts": parts })
}

/// Incremental SSE parse state — the explicit make/push/finish triple the
/// archived parser used, so split chunks and missing terminators are
/// first-class rather than accidents.
#[derive(Debug, Default)]
pub struct GeminiSseState {
    started: bool,
    done: bool,
    reasoning_count: u64,
    tool_count: u64,
    saw_tool_call: bool,
    finish_reason: Option<FinishReason>,
    usage: Option<Usage>,
}

impl GeminiSseState {
    pub fn new() -> GeminiSseState {
        GeminiSseState::default()
    }

    /// Feed one line of the SSE stream. Blank lines and non-data fields are
    /// ignored per the SSE framing.
    pub fn push_line(&mut self, line: &str) -> Result<Vec<Event>, WireError> {
        if self.done {
            return Ok(Vec::new());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let Some(payload) = trimmed.strip_prefix("data:").map(str::trim_start) else {
            return Ok(Vec::new());
        };
        if payload.is_empty() {
            return Ok(Vec::new());
        }
        if payload == "[DONE]" {
            self.done = true;
            return Ok(self.terminal_events());
        }
        let chunk: serde_json::Value = serde_json::from_str(payload)
            .map_err(|_| WireError::malformed("Gemini SSE data line was not valid JSON"))?;

        let mut events = Vec::new();
        if !self.started {
            self.started = true;
            events.push(Event::StepStart { index: 0 });
        }

        if let Some(parts) = chunk["candidates"][0]["content"]["parts"].as_array() {
            for part in parts {
                events.extend(self.event_for_part(part));
            }
        }
        if let Some(reason) = chunk["candidates"][0]["finishReason"].as_str() {
            self.finish_reason = Some(match reason {
                "STOP" => FinishReason::Stop,
                "MAX_TOKENS" => FinishReason::Length,
                "SAFETY" | "PROHIBITED_CONTENT" | "BLOCKLIST" => FinishReason::ContentFilter,
                _ => FinishReason::Unknown,
            });
        }
        if let Some(usage) = chunk.get("usageMetadata").filter(|value| value.is_object()) {
            self.usage = Some(lower_usage(usage));
        }
        Ok(events)
    }

    fn event_for_part(&mut self, part: &serde_json::Value) -> Vec<Event> {
        let signature_metadata = part.get("thoughtSignature").and_then(|value| value.as_str()).map(|signature| {
            let mut google = serde_json::Map::new();
            google.insert("thoughtSignature".into(), serde_json::Value::String(signature.into()));
            let mut metadata = serde_json::Map::new();
            metadata.insert("google".into(), serde_json::Value::Object(google));
            metadata
        });
        if let Some(function_call) = part.get("functionCall") {
            let id = format!("tool_{}", self.tool_count);
            self.tool_count += 1;
            self.saw_tool_call = true;
            return vec![Event::ToolCall {
                id,
                name: function_call["name"].as_str().unwrap_or_default().to_string(),
                input: function_call.get("args").cloned().unwrap_or(serde_json::Value::Null),
                provider_executed: None,
                provider_metadata: signature_metadata,
            }];
        }
        if let Some(text) = part["text"].as_str() {
            if part["thought"].as_bool() == Some(true) {
                let id = format!("reasoning-{}", self.reasoning_count);
                self.reasoning_count += 1;
                return vec![Event::ReasoningDelta {
                    id,
                    text: text.to_string(),
                    provider_metadata: signature_metadata,
                }];
            }
            return vec![Event::TextDelta {
                id: "text-0".into(),
                text: text.to_string(),
                provider_metadata: signature_metadata,
            }];
        }
        Vec::new()
    }

    fn terminal_events(&mut self) -> Vec<Event> {
        let reason = if self.saw_tool_call {
            FinishReason::ToolCalls
        } else {
            self.finish_reason.unwrap_or(FinishReason::Unknown)
        };
        vec![
            Event::StepFinish { index: 0, reason, usage: None, provider_metadata: None },
            Event::Finish { reason, usage: self.usage.take(), provider_metadata: None },
        ]
    }

    /// End of input. Emits the terminal events if `[DONE]` never arrived.
    pub fn finish(mut self) -> Result<Vec<Event>, WireError> {
        if self.done {
            return Ok(Vec::new());
        }
        if !self.started {
            return Err(WireError::malformed("Gemini stream ended before any data"));
        }
        self.done = true;
        Ok(self.terminal_events())
    }
}

/// Gemini `usageMetadata` -> normalized usage. Thoughts add to visible
/// candidate tokens for inclusive output accounting; cached input splits
/// into the non-cached remainder.
fn lower_usage(usage: &serde_json::Value) -> Usage {
    let prompt = usage["promptTokenCount"].as_u64();
    let candidates = usage["candidatesTokenCount"].as_u64();
    let thoughts = usage["thoughtsTokenCount"].as_u64();
    let cached = usage["cachedContentTokenCount"].as_u64();
    let output = match (candidates, thoughts) {
        (Some(candidates), Some(thoughts)) => Some(candidates + thoughts),
        (candidates, None) => candidates,
        (None, thoughts) => thoughts,
    };
    Usage {
        input_tokens: prompt,
        output_tokens: output,
        non_cached_input_tokens: match (prompt, cached) {
            (Some(prompt), Some(cached)) => Some(prompt.saturating_sub(cached)),
            _ => None,
        },
        cache_read_input_tokens: cached,
        cache_write_input_tokens: None,
        reasoning_tokens: thoughts,
        total_tokens: usage["totalTokenCount"].as_u64(),
        provider_metadata: None,
    }
    .normalized()
}

/// Convenience: parse a complete SSE body.
pub fn parse_sse(body: &str) -> Result<Vec<Event>, WireError> {
    let mut state = GeminiSseState::new();
    let mut events = Vec::new();
    for line in body.lines() {
        events.extend(state.push_line(line)?);
    }
    events.extend(state.finish()?);
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use probe_core::contract::request::{ModelRef, ToolDefinition};

    #[test]
    fn lowers_tool_results_and_signed_tool_calls() {
        let mut request = Request::simple(
            ModelRef { provider: "google".into(), model: "gemini".into() },
            "sys",
            "hi",
        );
        request.tools.push(ToolDefinition {
            name: "lookup".into(),
            description: "Lookup.".into(),
            input_schema: serde_json::Map::new(),
            output_schema: None,
        });
        let mut signature = serde_json::Map::new();
        signature.insert("google".into(), serde_json::json!({ "thoughtSignature": "sig_tool" }));
        request.messages.push(Message::from_parts(
            Role::Assistant,
            vec![ContentPart::ToolCall {
                id: "tool_0".into(),
                name: "lookup".into(),
                input: serde_json::json!({ "query": "weather" }),
                provider_executed: None,
                metadata: None,
                provider_metadata: Some(signature),
            }],
        ));
        request.messages.push(Message::from_parts(
            Role::Tool,
            vec![ContentPart::tool_result("tool_0", "lookup", ToolResultValue::text("sunny"))],
        ));
        let body = lower_request(&request);
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "sys");
        assert_eq!(body["tools"][0]["functionDeclarations"][0]["name"], "lookup");
        assert_eq!(body["contents"][1]["role"], "model");
        assert_eq!(body["contents"][1]["parts"][0]["thoughtSignature"], "sig_tool");
        assert_eq!(body["contents"][2]["parts"][0]["functionResponse"]["response"]["output"], "sunny");
    }
}
