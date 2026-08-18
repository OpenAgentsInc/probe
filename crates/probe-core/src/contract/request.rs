//! Provider-neutral requests and tool definitions, ported from the archived
//! TS `llm/request.ts` and `llm/tool.ts`. No provider names, URLs, or
//! credentials appear here — providers get a lowering in `probe-wire`.

use serde::{Deserialize, Serialize};

use super::message::{Message, Role};

pub type JsonSchema = serde_json::Map<String, serde_json::Value>;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerationOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Raw JSON numbers so integral and fractional inputs re-encode exactly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<serde_json::Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<serde_json::Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    Tool { name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: JsonSchema,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<JsonSchema>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub model: ModelRef,
    pub system: Vec<Message>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<GenerationOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Request {
    /// Port of `makeProbeLlmRequest`'s common case: a system string and a
    /// user prompt.
    pub fn simple(model: ModelRef, system: impl Into<String>, prompt: impl Into<String>) -> Request {
        Request {
            id: None,
            model,
            system: vec![Message::text(Role::System, system)],
            messages: vec![Message::text(Role::User, prompt)],
            tools: Vec::new(),
            tool_choice: None,
            generation: None,
            provider_options: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_carry_no_backend_specific_fields() {
        let request = Request::simple(
            ModelRef { provider: "test-provider".into(), model: "test-model".into() },
            "You are concise.",
            "Say hello.",
        );
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(!encoded.contains("apple_fm"));
        assert!(!encoded.contains("gemini"));
        assert!(encoded.contains("\"system\""));
    }

    #[test]
    fn tool_choice_uses_the_ts_tagged_shape() {
        assert_eq!(
            serde_json::to_value(ToolChoice::Tool { name: "lookup".into() }).unwrap(),
            serde_json::json!({ "type": "tool", "name": "lookup" })
        );
        assert_eq!(
            serde_json::to_value(ToolChoice::None).unwrap(),
            serde_json::json!({ "type": "none" })
        );
    }
}
