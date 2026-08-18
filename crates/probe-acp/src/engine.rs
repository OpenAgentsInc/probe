//! The sans-I/O composition of the ACP server and the probe-core turn loop.
//! Both hosts wrap this one type: probe-bin drives it with threads and
//! stdio, probe-wasm hands it to a JavaScript host. Inputs are protocol
//! lines, provider events, and tool outcomes; outputs are typed host
//! commands. Nothing here touches a socket, a file, or a clock.

use probe_core::contract::event::Event;
use probe_core::contract::message::{Message, Role, ToolResultValue};
use probe_core::contract::request::{ModelRef, Request, ToolDefinition};
use probe_core::permission::{PolicyTable, ToolKind};
use probe_core::turn::{ToolInvocation, Turn, TurnConfig, TurnEffect, TurnStatus};

use crate::mapping::{stop_reason_for, updates_for_event, DEFAULT_TEXT_BUDGET_BYTES};
use crate::server::{AcpServer, ServerConfig, ServerEvent};

/// What the host must do, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum HostCommand {
    /// Write one framed line to the client (stdout for probe-bin).
    WriteLine(String),
    /// Run the model transport for this request and feed the resulting
    /// neutral events back through `on_provider_event` (transport failures
    /// through `on_provider_failure`).
    StartStream(Request),
    /// Abort the in-flight transport stream, if any.
    CancelStream,
    /// Execute this tool and feed the outcome back through
    /// `on_tool_outcome`.
    RunTool(ToolInvocation),
}

/// Everything the engine needs to build a turn for a prompt.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub server: ServerConfig,
    pub model: ModelRef,
    pub system_prompt: String,
    pub tools: Vec<ToolDefinition>,
    pub tool_kinds: std::collections::BTreeMap<String, ToolKind>,
    pub policy: PolicyTable,
    pub max_tool_rounds: u64,
    pub text_budget: usize,
}

impl EngineConfig {
    pub fn new(model: ModelRef, system_prompt: impl Into<String>) -> EngineConfig {
        EngineConfig {
            server: ServerConfig::default(),
            model,
            system_prompt: system_prompt.into(),
            tools: Vec::new(),
            tool_kinds: std::collections::BTreeMap::new(),
            policy: PolicyTable::default(),
            max_tool_rounds: 16,
            text_budget: DEFAULT_TEXT_BUDGET_BYTES,
        }
    }
}

struct ActiveTurn {
    session_id: String,
    turn: Turn,
    streaming: bool,
}

pub struct Engine {
    config: EngineConfig,
    server: AcpServer,
    active: Option<ActiveTurn>,
}

impl Engine {
    pub fn new(config: EngineConfig) -> Engine {
        let server = AcpServer::new(config.server);
        Engine { config, server, active: None }
    }

    /// One inbound protocol line.
    pub fn handle_line(&mut self, line: &str) -> Vec<HostCommand> {
        let (lines, events) = self.server.handle_line(line);
        let mut commands: Vec<HostCommand> = lines.into_iter().map(HostCommand::WriteLine).collect();
        for event in events {
            commands.extend(self.handle_server_event(event));
        }
        commands
    }

    /// A neutral event from the running transport stream. Renders the event
    /// to session updates AND advances the turn.
    pub fn on_provider_event(&mut self, event: Event) -> Vec<HostCommand> {
        let Some(active) = self.active.as_mut() else {
            return Vec::new();
        };
        let session_id = active.session_id.clone();
        let mut commands = self.render_event(&session_id, &event);
        let effects = match self.active.as_mut() {
            Some(active) => active.turn.on_provider_event(event),
            None => Vec::new(),
        };
        commands.extend(self.apply_effects(&session_id, effects));
        commands
    }

    /// The transport itself failed (network error, bad status). Terminal for
    /// the prompt; the detail must already be secret-free.
    pub fn on_provider_failure(&mut self, message: &str) -> Vec<HostCommand> {
        self.on_provider_event(Event::ProviderError {
            message: message.to_string(),
            retryable: None,
            provider_metadata: None,
        })
    }

    pub fn on_tool_outcome(&mut self, tool_call_id: &str, result: ToolResultValue) -> Vec<HostCommand> {
        let Some(active) = self.active.as_mut() else {
            return Vec::new();
        };
        let session_id = active.session_id.clone();
        let effects = active.turn.on_tool_outcome(tool_call_id, result);
        self.apply_effects(&session_id, effects)
    }

    fn handle_server_event(&mut self, event: ServerEvent) -> Vec<HostCommand> {
        match event {
            ServerEvent::PromptStarted { session_id, prompt, .. } => {
                if self.active.is_some() {
                    // One delegation per process; the server already refuses
                    // concurrent prompts per session.
                    return self
                        .server
                        .fail_prompt(&session_id, "another prompt is already running")
                        .map(HostCommand::WriteLine)
                        .into_iter()
                        .collect();
                }
                let request = Request {
                    id: None,
                    model: self.config.model.clone(),
                    system: vec![Message::text(Role::System, self.config.system_prompt.clone())],
                    messages: vec![Message::text(Role::User, prompt)],
                    tools: self.config.tools.clone(),
                    tool_choice: None,
                    generation: None,
                    provider_options: None,
                };
                let mut turn_config = TurnConfig::default();
                turn_config.max_tool_rounds = self.config.max_tool_rounds;
                turn_config.policy = self.config.policy.clone();
                turn_config.tool_kinds = self.config.tool_kinds.clone();
                let mut turn = Turn::new(turn_config, request);
                let effects = turn.begin();
                self.active = Some(ActiveTurn { session_id: session_id.clone(), turn, streaming: false });
                self.apply_effects(&session_id, effects)
            }
            ServerEvent::PromptCancelled { session_id } => {
                let Some(active) = self.active.as_mut() else {
                    return Vec::new();
                };
                if active.session_id != session_id {
                    return Vec::new();
                }
                let mut commands = Vec::new();
                if active.streaming {
                    commands.push(HostCommand::CancelStream);
                }
                let effects = active.turn.cancel();
                commands.extend(self.apply_effects(&session_id, effects));
                commands
            }
            ServerEvent::PermissionDecided { session_id, tool_call_id, decision } => {
                let Some(active) = self.active.as_mut() else {
                    return Vec::new();
                };
                if active.session_id != session_id {
                    return Vec::new();
                }
                let effects = active.turn.on_permission_decision(&tool_call_id, decision);
                self.apply_effects(&session_id, effects)
            }
        }
    }

    fn apply_effects(&mut self, session_id: &str, effects: Vec<TurnEffect>) -> Vec<HostCommand> {
        let mut commands = Vec::new();
        for effect in effects {
            match effect {
                TurnEffect::SendProviderRequest(request) => {
                    if let Some(active) = self.active.as_mut() {
                        active.streaming = true;
                    }
                    commands.push(HostCommand::StartStream(request));
                }
                TurnEffect::RequestPermission(request) => {
                    commands.push(HostCommand::WriteLine(self.server.request_permission(session_id, &request)));
                }
                TurnEffect::ExecuteTool(invocation) => {
                    commands.push(HostCommand::RunTool(invocation));
                }
                TurnEffect::Emit(event) => {
                    commands.extend(self.render_event(session_id, &event));
                }
                TurnEffect::Finished(outcome) => {
                    self.active = None;
                    let line = match &outcome.status {
                        TurnStatus::ProviderError { message } => self
                            .server
                            .fail_prompt(session_id, &format!("provider error: {message}")),
                        status => self.server.finish_prompt(session_id, stop_reason_for(status)),
                    };
                    commands.extend(line.map(HostCommand::WriteLine));
                }
            }
        }
        commands
    }

    fn render_event(&mut self, session_id: &str, event: &Event) -> Vec<HostCommand> {
        if matches!(event, Event::Finish { .. } | Event::StepFinish { .. }) {
            if let Some(active) = self.active.as_mut() {
                if matches!(event, Event::Finish { .. }) {
                    active.streaming = false;
                }
            }
        }
        let kinds = self.config.tool_kinds.clone();
        let kind_for = move |name: &str| kinds.get(name).copied().unwrap_or(ToolKind::Other);
        updates_for_event(event, &kind_for, self.config.text_budget)
            .into_iter()
            .filter_map(|update| self.server.send_update(session_id, update))
            .map(HostCommand::WriteLine)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use probe_core::contract::event::FinishReason;

    fn engine_with_tool() -> Engine {
        let mut config = EngineConfig::new(
            ModelRef { provider: "stub".into(), model: "stub".into() },
            "You are probe.",
        );
        config.tool_kinds.insert("read_file".into(), ToolKind::Read);
        config.tool_kinds.insert("shell".into(), ToolKind::Execute);
        Engine::new(config)
    }

    fn drive_to_prompt(engine: &mut Engine) -> (String, Request) {
        engine.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}"#);
        let commands = engine.handle_line(
            r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/work","mcpServers":[]}}"#,
        );
        let HostCommand::WriteLine(line) = &commands[0] else { panic!() };
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        let session_id = value["result"]["sessionId"].as_str().unwrap().to_string();
        let commands = engine.handle_line(&format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"sessionId":"{session_id}","prompt":[{{"type":"text","text":"do the thing"}}]}}}}"#
        ));
        let HostCommand::StartStream(request) = commands.last().unwrap().clone() else {
            panic!("expected StartStream, got {commands:?}");
        };
        (session_id, request)
    }

    #[test]
    fn prompt_starts_a_stream_with_system_and_user_messages() {
        let mut engine = engine_with_tool();
        let (_, request) = drive_to_prompt(&mut engine);
        assert_eq!(request.system[0].role, Role::System);
        assert_eq!(request.messages.len(), 1);
    }

    #[test]
    fn text_stream_renders_updates_and_finishes_end_turn() {
        let mut engine = engine_with_tool();
        let (_, _) = drive_to_prompt(&mut engine);
        let commands = engine.on_provider_event(Event::TextDelta {
            id: "text-0".into(),
            text: "Hello".into(),
            provider_metadata: None,
        });
        let HostCommand::WriteLine(update) = &commands[0] else { panic!() };
        assert!(update.contains("agent_message_chunk"));
        let commands = engine.on_provider_event(Event::Finish {
            reason: FinishReason::Stop,
            usage: None,
            provider_metadata: None,
        });
        let HostCommand::WriteLine(response) = commands.last().unwrap() else { panic!() };
        let value: serde_json::Value = serde_json::from_str(response).unwrap();
        assert_eq!(value["result"]["stopReason"], "end_turn");
        assert_eq!(value["id"], 3);
    }

    #[test]
    fn allowed_tool_round_trips_through_run_tool_and_continues() {
        let mut engine = engine_with_tool();
        drive_to_prompt(&mut engine);
        engine.on_provider_event(Event::ToolCall {
            id: "t1".into(),
            name: "read_file".into(),
            input: serde_json::json!({ "path": "a.txt" }),
            provider_executed: None,
            provider_metadata: None,
        });
        let commands = engine.on_provider_event(Event::Finish {
            reason: FinishReason::ToolCalls,
            usage: None,
            provider_metadata: None,
        });
        assert!(commands.iter().any(|command| matches!(command, HostCommand::RunTool(inv) if inv.id == "t1")));
        let commands = engine.on_tool_outcome("t1", ToolResultValue::text("contents"));
        // Rendered result update + continuation stream.
        assert!(commands.iter().any(|command| matches!(command, HostCommand::WriteLine(line) if line.contains("tool_call_update"))));
        assert!(commands.iter().any(|command| matches!(command, HostCommand::StartStream(_))));
    }

    #[test]
    fn execute_tools_escalate_to_a_permission_request_line() {
        let mut engine = engine_with_tool();
        drive_to_prompt(&mut engine);
        engine.on_provider_event(Event::ToolCall {
            id: "t1".into(),
            name: "shell".into(),
            input: serde_json::json!({ "command": "git status" }),
            provider_executed: None,
            provider_metadata: None,
        });
        let commands = engine.on_provider_event(Event::Finish {
            reason: FinishReason::ToolCalls,
            usage: None,
            provider_metadata: None,
        });
        let permission_line = commands.iter().find_map(|command| match command {
            HostCommand::WriteLine(line) if line.contains("session/request_permission") => Some(line.clone()),
            _ => None,
        });
        let line = permission_line.expect("permission request line");
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        // The client answers; a selected allow flows back into RunTool.
        let commands = engine.handle_line(&format!(
            r#"{{"jsonrpc":"2.0","id":{},"result":{{"outcome":{{"outcome":"selected","optionId":"allow_once"}}}}}}"#,
            value["id"]
        ));
        assert!(commands.iter().any(|command| matches!(command, HostCommand::RunTool(inv) if inv.id == "t1")));
    }

    #[test]
    fn cancel_aborts_the_stream_and_answers_cancelled() {
        let mut engine = engine_with_tool();
        let (session_id, _) = drive_to_prompt(&mut engine);
        let commands = engine.handle_line(&format!(
            r#"{{"jsonrpc":"2.0","method":"session/cancel","params":{{"sessionId":"{session_id}"}}}}"#
        ));
        assert!(commands.contains(&HostCommand::CancelStream));
        let HostCommand::WriteLine(response) = commands.last().unwrap() else { panic!() };
        assert!(response.contains("cancelled"));
    }

    #[test]
    fn provider_failure_fails_the_prompt_with_a_json_rpc_error() {
        let mut engine = engine_with_tool();
        drive_to_prompt(&mut engine);
        let commands = engine.on_provider_failure("connection refused");
        let HostCommand::WriteLine(response) = commands.last().unwrap() else { panic!() };
        let value: serde_json::Value = serde_json::from_str(response).unwrap();
        assert!(value["error"]["message"].as_str().unwrap().contains("connection refused"));
    }
}
