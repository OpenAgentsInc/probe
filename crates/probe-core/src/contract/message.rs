//! Messages and content parts, ported from the archived TS `llm/messages.ts`.
//! The JSON encoding is the contract: tagged unions on `"type"`, camelCase
//! fields, optionals omitted when absent. The shared fixture corpus pins it.

use serde::{Deserialize, Serialize};

use super::usage::ProviderMetadata;

pub type Metadata = serde_json::Map<String, serde_json::Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheHintKind {
    Ephemeral,
    Persistent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheHint {
    #[serde(rename = "type")]
    pub kind: CacheHintKind,
    /// Kept as a raw JSON number so integral values re-encode exactly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<serde_json::Number>,
}

/// The three shapes a tool result can take. `error` is a first-class value —
/// a failed tool still produces a result the model can read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolResultValue {
    Json { value: serde_json::Value },
    Text { value: String },
    Error { value: String },
}

impl ToolResultValue {
    /// Port of `makeProbeLlmToolResultValue`: an already-shaped value passes
    /// through; otherwise it is wrapped as JSON.
    pub fn from_value(value: serde_json::Value) -> ToolResultValue {
        if let Ok(shaped) = serde_json::from_value::<ToolResultValue>(value.clone()) {
            return shaped;
        }
        ToolResultValue::Json { value }
    }

    pub fn error(message: impl Into<String>) -> ToolResultValue {
        ToolResultValue::Error { value: message.into() }
    }

    pub fn text(value: impl Into<String>) -> ToolResultValue {
        ToolResultValue::Text { value: value.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum ContentPart {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache: Option<CacheHint>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Metadata>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    Media {
        media_type: String,
        /// Base64 or provider-native string payload. The archived TS type
        /// also admitted raw bytes, which have no JSON encoding; the wire
        /// contract is the string form.
        data: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Metadata>,
    },
    Reasoning {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Metadata>,
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
        metadata: Option<Metadata>,
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
        cache: Option<CacheHint>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Metadata>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
}

impl ContentPart {
    pub fn text(text: impl Into<String>) -> ContentPart {
        ContentPart::Text {
            text: text.into(),
            cache: None,
            metadata: None,
            provider_metadata: None,
        }
    }

    pub fn tool_call(id: impl Into<String>, name: impl Into<String>, input: serde_json::Value) -> ContentPart {
        ContentPart::ToolCall {
            id: id.into(),
            name: name.into(),
            input,
            provider_executed: None,
            metadata: None,
            provider_metadata: None,
        }
    }

    pub fn tool_result(id: impl Into<String>, name: impl Into<String>, result: ToolResultValue) -> ContentPart {
        ContentPart::ToolResult {
            id: id.into(),
            name: name.into(),
            result,
            provider_executed: None,
            cache: None,
            metadata: None,
            provider_metadata: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Message {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub role: Role,
    pub content: Vec<ContentPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl Message {
    pub fn text(role: Role, text: impl Into<String>) -> Message {
        Message {
            id: None,
            role,
            content: vec![ContentPart::text(text)],
            metadata: None,
        }
    }

    pub fn from_parts(role: Role, content: Vec<ContentPart>) -> Message {
        Message { id: None, role, content, metadata: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_parts_round_trip_the_ts_wire_shape() {
        let json = serde_json::json!({
            "type": "tool-result",
            "id": "tool_3",
            "name": "lookup",
            "result": { "type": "json", "value": { "answer": 42 } }
        });
        let part: ContentPart = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&part).unwrap(), json);
    }

    #[test]
    fn pre_shaped_tool_result_values_pass_through() {
        let shaped = serde_json::json!({ "type": "error", "value": "boom" });
        assert_eq!(ToolResultValue::from_value(shaped), ToolResultValue::error("boom"));
        let raw = serde_json::json!({ "ok": true });
        assert_eq!(
            ToolResultValue::from_value(raw.clone()),
            ToolResultValue::Json { value: raw }
        );
    }
}
