//! OpenAI-compatible chat-completions lowering and SSE parser. One lowering,
//! three deployments: Sarah's inference-grant provider proxy, local Psionic
//! serving, and any vanilla OpenAI-compatible endpoint.

use std::collections::BTreeMap;

use probe_core::contract::event::{Event, FinishReason};
use probe_core::contract::message::{ContentPart, Message, Role, ToolResultValue};
use probe_core::contract::request::{Request, ToolChoice};
use probe_core::contract::usage::Usage;

use crate::WireError;

/// Lower a neutral request to a streaming `/chat/completions` body.
pub fn lower_request(request: &Request) -> serde_json::Value {
    let mut messages: Vec<serde_json::Value> = Vec::new();
    for message in &request.system {
        messages.push(serde_json::json!({ "role": "system", "content": text_of(message) }));
    }
    for message in &request.messages {
        messages.extend(lower_message(message));
    }

    let mut body = serde_json::Map::new();
    body.insert("model".into(), serde_json::Value::String(request.model.model.clone()));
    body.insert("messages".into(), serde_json::Value::Array(messages));
    body.insert("stream".into(), serde_json::Value::Bool(true));
    body.insert("stream_options".into(), serde_json::json!({ "include_usage": true }));

    if !request.tools.is_empty() {
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input_schema,
                    }
                })
            })
            .collect();
        body.insert("tools".into(), serde_json::Value::Array(tools));
    }
    if let Some(choice) = &request.tool_choice {
        let lowered = match choice {
            ToolChoice::Auto => serde_json::json!("auto"),
            ToolChoice::None => serde_json::json!("none"),
            ToolChoice::Required => serde_json::json!("required"),
            ToolChoice::Tool { name } => {
                serde_json::json!({ "type": "function", "function": { "name": name } })
            }
        };
        body.insert("tool_choice".into(), lowered);
    }
    if let Some(generation) = &request.generation {
        if let Some(max) = generation.max_tokens {
            body.insert("max_tokens".into(), max.into());
        }
        if let Some(temperature) = &generation.temperature {
            body.insert("temperature".into(), serde_json::Value::Number(temperature.clone()));
        }
        if let Some(top_p) = &generation.top_p {
            body.insert("top_p".into(), serde_json::Value::Number(top_p.clone()));
        }
        if let Some(stop) = &generation.stop {
            body.insert("stop".into(), serde_json::json!(stop));
        }
    }
    serde_json::Value::Object(body)
}

fn text_of(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn lower_message(message: &Message) -> Vec<serde_json::Value> {
    match message.role {
        Role::Assistant => {
            let text = text_of(message);
            let tool_calls: Vec<serde_json::Value> = message
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::ToolCall { id, name, input, .. } => Some(serde_json::json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                        }
                    })),
                    _ => None,
                })
                .collect();
            let mut lowered = serde_json::Map::new();
            lowered.insert("role".into(), serde_json::json!("assistant"));
            lowered.insert(
                "content".into(),
                if text.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(text) },
            );
            if !tool_calls.is_empty() {
                lowered.insert("tool_calls".into(), serde_json::Value::Array(tool_calls));
            }
            vec![serde_json::Value::Object(lowered)]
        }
        Role::Tool => message
            .content
            .iter()
            .filter_map(|part| match part {
                ContentPart::ToolResult { id, result, .. } => {
                    let content = match result {
                        ToolResultValue::Text { value } | ToolResultValue::Error { value } => value.clone(),
                        ToolResultValue::Json { value } => serde_json::to_string(value).unwrap_or_default(),
                    };
                    Some(serde_json::json!({ "role": "tool", "tool_call_id": id, "content": content }))
                }
                _ => None,
            })
            .collect(),
        Role::User => vec![serde_json::json!({ "role": "user", "content": text_of(message) })],
        Role::System => vec![serde_json::json!({ "role": "system", "content": text_of(message) })],
    }
}

#[derive(Debug, Default)]
struct PendingToolCall {
    id: Option<String>,
    name: String,
    arguments: String,
}

/// Incremental SSE parse state for chat-completions streams.
#[derive(Debug, Default)]
pub struct OpenAiSseState {
    started: bool,
    done: bool,
    tool_calls: BTreeMap<u64, PendingToolCall>,
    finish_reason: Option<FinishReason>,
    usage: Option<Usage>,
}

impl OpenAiSseState {
    pub fn new() -> OpenAiSseState {
        OpenAiSseState::default()
    }

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
            .map_err(|_| WireError::malformed("chat-completions SSE data line was not valid JSON"))?;

        let mut events = Vec::new();
        if !self.started {
            self.started = true;
            events.push(Event::StepStart { index: 0 });
        }
        let delta = &chunk["choices"][0]["delta"];
        if let Some(text) = delta["content"].as_str() {
            if !text.is_empty() {
                events.push(Event::TextDelta { id: "text-0".into(), text: text.into(), provider_metadata: None });
            }
        }
        if let Some(text) = delta["reasoning_content"].as_str() {
            if !text.is_empty() {
                events.push(Event::ReasoningDelta {
                    id: "reasoning-0".into(),
                    text: text.into(),
                    provider_metadata: None,
                });
            }
        }
        if let Some(calls) = delta["tool_calls"].as_array() {
            for call in calls {
                let index = call["index"].as_u64().unwrap_or(0);
                let pending = self.tool_calls.entry(index).or_default();
                if let Some(id) = call["id"].as_str() {
                    pending.id = Some(id.to_string());
                }
                if let Some(name) = call["function"]["name"].as_str() {
                    pending.name.push_str(name);
                }
                if let Some(arguments) = call["function"]["arguments"].as_str() {
                    pending.arguments.push_str(arguments);
                }
            }
        }
        if let Some(reason) = chunk["choices"][0]["finish_reason"].as_str() {
            self.finish_reason = Some(match reason {
                "stop" => FinishReason::Stop,
                "length" => FinishReason::Length,
                "tool_calls" => FinishReason::ToolCalls,
                "content_filter" => FinishReason::ContentFilter,
                _ => FinishReason::Unknown,
            });
        }
        if let Some(usage) = chunk.get("usage").filter(|value| value.is_object()) {
            self.usage = Some(lower_usage(usage));
        }
        Ok(events)
    }

    fn terminal_events(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        for (index, pending) in std::mem::take(&mut self.tool_calls) {
            let input = serde_json::from_str::<serde_json::Value>(&pending.arguments)
                .unwrap_or(serde_json::Value::String(pending.arguments.clone()));
            events.push(Event::ToolCall {
                id: pending.id.unwrap_or_else(|| format!("tool_{index}")),
                name: pending.name,
                input,
                provider_executed: None,
                provider_metadata: None,
            });
        }
        let reason = if events.iter().any(Event::is_tool_call) {
            FinishReason::ToolCalls
        } else {
            self.finish_reason.unwrap_or(FinishReason::Unknown)
        };
        events.push(Event::StepFinish { index: 0, reason, usage: None, provider_metadata: None });
        events.push(Event::Finish { reason, usage: self.usage.take(), provider_metadata: None });
        events
    }

    pub fn finish(mut self) -> Result<Vec<Event>, WireError> {
        if self.done {
            return Ok(Vec::new());
        }
        if !self.started {
            return Err(WireError::malformed("chat-completions stream ended before any data"));
        }
        self.done = true;
        Ok(self.terminal_events())
    }
}

fn lower_usage(usage: &serde_json::Value) -> Usage {
    Usage {
        input_tokens: usage["prompt_tokens"].as_u64(),
        output_tokens: usage["completion_tokens"].as_u64(),
        non_cached_input_tokens: None,
        cache_read_input_tokens: usage["prompt_tokens_details"]["cached_tokens"].as_u64(),
        cache_write_input_tokens: None,
        reasoning_tokens: usage["completion_tokens_details"]["reasoning_tokens"].as_u64(),
        total_tokens: usage["total_tokens"].as_u64(),
        provider_metadata: None,
    }
    .normalized()
}

/// Convenience: parse a complete SSE body.
pub fn parse_sse(body: &str) -> Result<Vec<Event>, WireError> {
    let mut state = OpenAiSseState::new();
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
    use probe_core::contract::request::ModelRef;

    #[test]
    fn text_stream_parses_to_deltas_and_finish() {
        let events = parse_sse(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"total_tokens\":7}}\n\n",
            "data: [DONE]\n\n",
        ))
        .unwrap();
        let types: Vec<&str> = events
            .iter()
            .map(|event| match event {
                Event::StepStart { .. } => "step-start",
                Event::TextDelta { .. } => "text-delta",
                Event::StepFinish { .. } => "step-finish",
                Event::Finish { .. } => "finish",
                _ => "other",
            })
            .collect();
        assert_eq!(types, vec!["step-start", "text-delta", "text-delta", "step-finish", "finish"]);
        let Event::Finish { reason, usage, .. } = events.last().unwrap() else { panic!() };
        assert_eq!(*reason, FinishReason::Stop);
        assert_eq!(usage.as_ref().unwrap().total_tokens, Some(7));
    }

    #[test]
    fn split_tool_call_arguments_reassemble_by_index() {
        let events = parse_sse(concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"comm\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"and\\\": \\\"ls\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        ))
        .unwrap();
        let tool_call = events.iter().find(|event| event.is_tool_call()).unwrap();
        let Event::ToolCall { id, name, input, .. } = tool_call else { panic!() };
        assert_eq!(id, "call_1");
        assert_eq!(name, "shell");
        assert_eq!(input, &serde_json::json!({ "command": "ls" }));
        let Event::Finish { reason, .. } = events.last().unwrap() else { panic!() };
        assert_eq!(*reason, FinishReason::ToolCalls);
    }

    #[test]
    fn lowers_the_full_tool_round_trip() {
        let mut request = Request::simple(
            ModelRef { provider: "openai".into(), model: "m".into() },
            "sys",
            "hi",
        );
        request.messages.push(Message::from_parts(
            Role::Assistant,
            vec![ContentPart::tool_call("call_1", "shell", serde_json::json!({ "command": "ls" }))],
        ));
        request.messages.push(Message::from_parts(
            Role::Tool,
            vec![ContentPart::tool_result("call_1", "shell", ToolResultValue::text("a.txt"))],
        ));
        let body = lower_request(&request);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(body["messages"][3]["role"], "tool");
        assert_eq!(body["messages"][3]["tool_call_id"], "call_1");
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn malformed_payloads_fail_without_leaking_bytes() {
        let error = parse_sse("data: {broken secret-key-value}\n\n").unwrap_err();
        assert_eq!(error.failure_class, "malformed_response");
        assert!(!error.message.contains("secret-key-value"));
    }
}
