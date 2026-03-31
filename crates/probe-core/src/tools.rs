use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

use probe_provider_openai::{
    ChatNamedToolChoice, ChatNamedToolChoiceFunction, ChatToolCall, ChatToolChoice,
    ChatToolDefinition, ChatToolDefinitionEnvelope,
};

pub type ToolHandler = fn(&serde_json::Value) -> Result<serde_json::Value, ToolInvocationError>;

#[derive(Clone, Debug)]
struct RegisteredTool {
    definition: ChatToolDefinition,
    handler: ToolHandler,
}

#[derive(Clone, Debug)]
pub struct ToolRegistry {
    name: String,
    tools: BTreeMap<String, RegisteredTool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeToolChoice {
    None,
    Auto,
    Required,
    Named(String),
}

#[derive(Clone, Debug)]
pub struct ToolLoopConfig {
    pub registry: ToolRegistry,
    pub tool_choice: ProbeToolChoice,
    pub parallel_tool_calls: bool,
    pub max_model_round_trips: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutedToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub output: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolInvocationError {
    InvalidArguments(String),
    ExecutionFailed(String),
}

impl Display for ToolInvocationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidArguments(message) => write!(f, "{message}"),
            Self::ExecutionFailed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ToolInvocationError {}

impl ProbeToolChoice {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "none" => Ok(Self::None),
            "auto" => Ok(Self::Auto),
            "required" => Ok(Self::Required),
            _ => value
                .strip_prefix("named:")
                .map(|name| Self::Named(String::from(name)))
                .ok_or_else(|| {
                    String::from("tool choice must be one of: none, auto, required, named:<tool>")
                }),
        }
    }

    #[must_use]
    pub fn to_provider_choice(&self) -> Option<ChatToolChoice> {
        match self {
            Self::None => Some(ChatToolChoice::Mode(String::from("none"))),
            Self::Auto => Some(ChatToolChoice::Mode(String::from("auto"))),
            Self::Required => Some(ChatToolChoice::Mode(String::from("required"))),
            Self::Named(name) => Some(ChatToolChoice::Named(ChatNamedToolChoice {
                kind: String::from("function"),
                function: ChatNamedToolChoiceFunction { name: name.clone() },
            })),
        }
    }
}

impl ToolLoopConfig {
    #[must_use]
    pub fn weather_demo(tool_choice: ProbeToolChoice, parallel_tool_calls: bool) -> Self {
        Self {
            registry: ToolRegistry::weather_demo(),
            tool_choice,
            parallel_tool_calls,
            max_model_round_trips: 4,
        }
    }
}

impl ToolRegistry {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tools: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn weather_demo() -> Self {
        let parameters = serde_json::json!({
            "type": "object",
            "properties": {
                "city": {
                    "type": "string",
                    "description": "The city to look up"
                }
            },
            "required": ["city"],
            "additionalProperties": false
        });

        Self::new("weather").register(
            String::from("lookup_weather"),
            Some(String::from(
                "Look up the retained demo weather for a city.",
            )),
            Some(parameters),
            lookup_weather,
        )
    }

    #[must_use]
    pub fn register(
        mut self,
        name: String,
        description: Option<String>,
        parameters: Option<serde_json::Value>,
        handler: ToolHandler,
    ) -> Self {
        self.tools.insert(
            name.clone(),
            RegisteredTool {
                definition: ChatToolDefinition {
                    name,
                    description,
                    parameters,
                },
                handler,
            },
        );
        self
    }

    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    #[must_use]
    pub fn declared_tools(&self) -> Vec<ChatToolDefinitionEnvelope> {
        self.tools
            .values()
            .map(|tool| ChatToolDefinitionEnvelope {
                kind: String::from("function"),
                function: tool.definition.clone(),
            })
            .collect()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn execute_batch(&self, tool_calls: &[ChatToolCall]) -> Vec<ExecutedToolCall> {
        tool_calls
            .iter()
            .map(|tool_call| {
                let parsed_arguments = serde_json::from_str::<serde_json::Value>(
                    tool_call.function.arguments.as_str(),
                )
                .unwrap_or_else(|error| {
                    serde_json::json!({
                        "error": format!("invalid tool arguments json: {error}")
                    })
                });

                let output = self
                    .tools
                    .get(tool_call.function.name.as_str())
                    .map(|tool| (tool.handler)(&parsed_arguments))
                    .unwrap_or_else(|| {
                        Err(ToolInvocationError::ExecutionFailed(format!(
                            "undeclared tool `{}`",
                            tool_call.function.name
                        )))
                    })
                    .unwrap_or_else(|error| serde_json::json!({ "error": error.to_string() }));

                ExecutedToolCall {
                    call_id: tool_call.id.clone(),
                    name: tool_call.function.name.clone(),
                    arguments: parsed_arguments,
                    output,
                }
            })
            .collect()
    }
}

fn lookup_weather(arguments: &serde_json::Value) -> Result<serde_json::Value, ToolInvocationError> {
    let city = arguments
        .get("city")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ToolInvocationError::InvalidArguments(String::from(
                "lookup_weather requires a string `city` argument",
            ))
        })?;

    let payload = match city {
        "Paris" => serde_json::json!({
            "city": "Paris",
            "conditions": "sunny",
            "temperature_c": 18
        }),
        "Tokyo" => serde_json::json!({
            "city": "Tokyo",
            "conditions": "rainy",
            "temperature_c": 12
        }),
        other => serde_json::json!({
            "error": format!("unsupported city: {other}")
        }),
    };
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use probe_provider_openai::{ChatToolCall, ChatToolCallFunction};

    use super::{ProbeToolChoice, ToolLoopConfig, ToolRegistry};

    #[test]
    fn weather_demo_registry_declares_lookup_weather() {
        let registry = ToolRegistry::weather_demo();
        let tools = registry.declared_tools();
        assert_eq!(registry.name(), "weather");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].function.name, "lookup_weather");
    }

    #[test]
    fn weather_demo_executes_lookup_weather() {
        let registry = ToolRegistry::weather_demo();
        let results = registry.execute_batch(&[ChatToolCall {
            id: String::from("call_1"),
            kind: String::from("function"),
            function: ChatToolCallFunction {
                name: String::from("lookup_weather"),
                arguments: String::from("{\"city\":\"Paris\"}"),
            },
        }]);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "lookup_weather");
        assert_eq!(results[0].output["conditions"], "sunny");
    }

    #[test]
    fn probe_tool_choice_parses_named_mode() {
        let choice = ProbeToolChoice::parse("named:lookup_weather").expect("named choice");
        let config = ToolLoopConfig::weather_demo(choice.clone(), true);
        assert_eq!(config.registry.name(), "weather");
        assert!(matches!(choice, ProbeToolChoice::Named(name) if name == "lookup_weather"));
    }
}
