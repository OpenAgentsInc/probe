//! The flat streaming event union, ported from the archived TS
//! `llm/events.ts`. Deltas are id-keyed so interleaved streams reassemble;
//! a failed tool dispatch emits BOTH `tool-error` and `tool-result`, so a
//! strict consumer never sees an unpaired call.

use serde::{Deserialize, Serialize};

use super::message::ToolResultValue;
use super::usage::{ProviderMetadata, Usage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Error,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum Event {
    StepStart {
        index: u64,
    },
    TextDelta {
        id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    ReasoningDelta {
        id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_executed: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    ToolResult {
        id: String,
        name: String,
        result: ToolResultValue,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_executed: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    ToolError {
        id: String,
        name: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    ProviderError {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        retryable: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    StepFinish {
        index: u64,
        reason: FinishReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    Finish {
        reason: FinishReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
}

impl Event {
    pub fn is_tool_call(&self) -> bool {
        matches!(self, Event::ToolCall { .. })
    }

    /// The paired error+result emission for a failed tool dispatch.
    pub fn tool_failure(id: &str, name: &str, message: &str) -> [Event; 2] {
        [
            Event::ToolError {
                id: id.to_string(),
                name: name.to_string(),
                message: message.to_string(),
                provider_metadata: None,
            },
            Event::ToolResult {
                id: id.to_string(),
                name: name.to_string(),
                result: ToolResultValue::error(message),
                provider_executed: None,
                provider_metadata: None,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_tags_match_the_ts_vocabulary() {
        let tags: Vec<String> = [
            Event::StepStart { index: 0 },
            Event::TextDelta { id: "t".into(), text: "x".into(), provider_metadata: None },
            Event::Finish { reason: FinishReason::ToolCalls, usage: None, provider_metadata: None },
        ]
        .iter()
        .map(|event| {
            serde_json::to_value(event).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
        assert_eq!(tags, vec!["step-start", "text-delta", "finish"]);
        assert_eq!(
            serde_json::to_value(FinishReason::ToolCalls).unwrap(),
            serde_json::json!("tool_calls")
        );
    }

    #[test]
    fn tool_failure_emits_paired_error_and_result() {
        let [error, result] = Event::tool_failure("tool_2", "lookup", "lookup unavailable");
        assert!(matches!(error, Event::ToolError { .. }));
        match result {
            Event::ToolResult { result, .. } => {
                assert_eq!(result, ToolResultValue::error("lookup unavailable"));
            }
            other => panic!("expected tool-result, got {other:?}"),
        }
    }
}
