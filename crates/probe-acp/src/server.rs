//! The ACP v1 server as a sans-I/O state machine. Lines in; outgoing lines
//! and typed embedder events out. The embedder (probe-bin over stdio,
//! probe-wasm under a JS host) moves bytes, runs the model transport, and
//! executes tools; this module owns protocol lifecycle, session state,
//! permission plumbing, and the outgoing message budget.

use std::collections::BTreeMap;

use probe_core::permission::{PermissionDecision, PermissionRequest};

use crate::jsonrpc::{
    self, error_line, notification_line, request_line, result_line, ErrorObject, Incoming, RequestId,
    INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND,
};
use crate::types::{
    methods, AgentCapabilities, CancelParams, ContentBlock, InitializeParams, InitializeResult,
    PermissionOption, PermissionOptionKind, PermissionOutcome, PermissionToolCall, PromptParams,
    RequestPermissionParams, RequestPermissionResponse, SessionLoadParams, SessionNewParams,
    SessionNewResult, SessionNotification, SessionUpdate, StopReason, PROTOCOL_VERSION,
};

/// What the embedder must act on after feeding a line in.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerEvent {
    /// Start driving a turn for this prompt; report updates through
    /// `send_update` and finish with `finish_prompt` / `fail_prompt`.
    PromptStarted { session_id: String, cwd: String, prompt: String },
    /// The client cancelled; cancel the running turn, then finish the
    /// prompt with the `cancelled` stop reason promptly — the controller
    /// settles 5 seconds after cancelling and SIGTERMs the process.
    PromptCancelled { session_id: String },
    /// A permission decision arrived for a request made via
    /// `request_permission`.
    PermissionDecided { session_id: String, tool_call_id: String, decision: PermissionDecision },
}

#[derive(Debug)]
struct Session {
    cwd: String,
    prompt_request: Option<RequestId>,
}

/// Outgoing message budget. Lines larger than the client's cap are dropped
/// whole and silently by the controller (4 MiB); staying far below it is
/// the only defense.
#[derive(Debug, Clone, Copy)]
pub struct ServerConfig {
    pub max_line_bytes: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        // Half the controller's cap: headroom for framing and escaping.
        ServerConfig { max_line_bytes: 2 * 1024 * 1024 }
    }
}

pub struct AcpServer {
    config: ServerConfig,
    initialized: bool,
    sessions: BTreeMap<String, Session>,
    next_session: u64,
    next_outgoing_id: i64,
    pending_permissions: BTreeMap<String, (String, String)>,
}

impl AcpServer {
    pub fn new(config: ServerConfig) -> AcpServer {
        AcpServer {
            config,
            initialized: false,
            sessions: BTreeMap::new(),
            next_session: 0,
            next_outgoing_id: 0,
            pending_permissions: BTreeMap::new(),
        }
    }

    /// Feed one inbound line. Returns lines to write and events to act on.
    pub fn handle_line(&mut self, line: &str) -> (Vec<String>, Vec<ServerEvent>) {
        match jsonrpc::parse_line(line) {
            Incoming::Request { id, method, params } => self.handle_request(id, &method, params),
            Incoming::Notification { method, params } => (Vec::new(), self.handle_notification(&method, params)),
            Incoming::Response { id, result, error } => (Vec::new(), self.handle_response(id, result, error)),
            Incoming::Invalid { error } => {
                (vec![error_line(&RequestId::Number(0), &error)], Vec::new())
            }
        }
    }

    fn handle_request(
        &mut self,
        id: RequestId,
        method: &str,
        params: serde_json::Value,
    ) -> (Vec<String>, Vec<ServerEvent>) {
        if !self.initialized && method != methods::INITIALIZE {
            return (
                vec![error_line(
                    &id,
                    &ErrorObject { code: INVALID_PARAMS, message: "Not initialized. Call initialize first.".into(), data: None },
                )],
                Vec::new(),
            );
        }
        match method {
            methods::INITIALIZE => {
                let Ok(_params) = serde_json::from_value::<InitializeParams>(params) else {
                    return (self.invalid_params(&id, "Invalid initialize params"), Vec::new());
                };
                self.initialized = true;
                // Always answer with OUR protocol version; the client decides
                // compatibility. Auth methods stay empty: authority arrives
                // as a grant with the launch, never through an auth dance.
                let result = InitializeResult {
                    protocol_version: PROTOCOL_VERSION,
                    agent_capabilities: AgentCapabilities { load_session: true },
                    auth_methods: Vec::new(),
                };
                (vec![result_line(&id, &serde_json::to_value(result).unwrap())], Vec::new())
            }
            methods::SESSION_NEW => {
                let Ok(params) = serde_json::from_value::<SessionNewParams>(params) else {
                    return (self.invalid_params(&id, "Invalid session/new params"), Vec::new());
                };
                self.next_session += 1;
                let session_id = format!("sess_{}", self.next_session);
                self.sessions.insert(session_id.clone(), Session { cwd: params.cwd, prompt_request: None });
                let result = SessionNewResult { session_id };
                (vec![result_line(&id, &serde_json::to_value(result).unwrap())], Vec::new())
            }
            methods::SESSION_LOAD => {
                let Ok(params) = serde_json::from_value::<SessionLoadParams>(params) else {
                    return (self.invalid_params(&id, "Invalid session/load params"), Vec::new());
                };
                if !self.sessions.contains_key(&params.session_id) {
                    return (
                        vec![error_line(
                            &id,
                            &ErrorObject { code: INVALID_PARAMS, message: "unknown session".into(), data: None },
                        )],
                        Vec::new(),
                    );
                }
                if let Some(session) = self.sessions.get_mut(&params.session_id) {
                    session.cwd = params.cwd;
                }
                (vec![result_line(&id, &serde_json::json!(null))], Vec::new())
            }
            methods::SESSION_PROMPT => {
                let Ok(params) = serde_json::from_value::<PromptParams>(params) else {
                    return (self.invalid_params(&id, "Invalid session/prompt params"), Vec::new());
                };
                let Some(session) = self.sessions.get_mut(&params.session_id) else {
                    return (
                        vec![error_line(
                            &id,
                            &ErrorObject { code: INVALID_PARAMS, message: "unknown session".into(), data: None },
                        )],
                        Vec::new(),
                    );
                };
                if session.prompt_request.is_some() {
                    return (
                        vec![error_line(
                            &id,
                            &ErrorObject { code: INVALID_PARAMS, message: "a prompt is already running".into(), data: None },
                        )],
                        Vec::new(),
                    );
                }
                session.prompt_request = Some(id);
                let text: String = params
                    .prompt
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        ContentBlock::Other(_) => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let cwd = session.cwd.clone();
                (
                    Vec::new(),
                    vec![ServerEvent::PromptStarted { session_id: params.session_id, cwd, prompt: text }],
                )
            }
            other => (
                vec![error_line(
                    &id,
                    &ErrorObject { code: METHOD_NOT_FOUND, message: format!("method not found: {other}"), data: None },
                )],
                Vec::new(),
            ),
        }
    }

    fn handle_notification(&mut self, method: &str, params: serde_json::Value) -> Vec<ServerEvent> {
        if method != methods::SESSION_CANCEL {
            return Vec::new();
        }
        let Ok(params) = serde_json::from_value::<CancelParams>(params) else {
            return Vec::new();
        };
        if self.sessions.contains_key(&params.session_id) {
            vec![ServerEvent::PromptCancelled { session_id: params.session_id }]
        } else {
            Vec::new()
        }
    }

    fn handle_response(
        &mut self,
        id: RequestId,
        result: Option<serde_json::Value>,
        _error: Option<ErrorObject>,
    ) -> Vec<ServerEvent> {
        let key = match &id {
            RequestId::Number(number) => number.to_string(),
            RequestId::Text(text) => text.clone(),
        };
        let Some((session_id, tool_call_id)) = self.pending_permissions.remove(&key) else {
            return Vec::new();
        };
        let decision = match result.and_then(|value| serde_json::from_value::<RequestPermissionResponse>(value).ok()) {
            Some(RequestPermissionResponse { outcome: PermissionOutcome::Selected { option_id } }) => {
                if option_id.starts_with("allow") {
                    PermissionDecision::Allowed
                } else {
                    PermissionDecision::Denied
                }
            }
            // Cancelled, an error response, or an unreadable body all mean
            // the same thing here: not allowed.
            _ => PermissionDecision::Denied,
        };
        vec![ServerEvent::PermissionDecided { session_id, tool_call_id, decision }]
    }

    fn invalid_params(&self, id: &RequestId, message: &str) -> Vec<String> {
        vec![error_line(id, &ErrorObject { code: INVALID_PARAMS, message: message.into(), data: None })]
    }

    /// Emit one session update as a notification line, enforcing the line
    /// budget. Returns None if the update cannot fit even alone — the caller
    /// should have chunked via `mapping::updates_for_event`.
    pub fn send_update(&self, session_id: &str, update: SessionUpdate) -> Option<String> {
        let params = SessionNotification { session_id: session_id.to_string(), update };
        let line = notification_line(methods::SESSION_UPDATE, &serde_json::to_value(params).unwrap());
        if line.len() > self.config.max_line_bytes {
            return None;
        }
        Some(line)
    }

    /// The standard permission options: a conservative one-shot pair. Never
    /// an "always" grant by default, and never anything named like a bypass
    /// — the controller refuses those outright.
    pub fn permission_options() -> Vec<PermissionOption> {
        vec![
            PermissionOption { option_id: "allow_once".into(), name: "Allow".into(), kind: PermissionOptionKind::AllowOnce },
            PermissionOption { option_id: "reject_once".into(), name: "Reject".into(), kind: PermissionOptionKind::RejectOnce },
        ]
    }

    /// Send a `session/request_permission` for a core permission request.
    /// The decision comes back as `ServerEvent::PermissionDecided`.
    pub fn request_permission(&mut self, session_id: &str, request: &PermissionRequest) -> String {
        self.next_outgoing_id += 1;
        let id = RequestId::Number(self.next_outgoing_id);
        self.pending_permissions
            .insert(self.next_outgoing_id.to_string(), (session_id.to_string(), request.tool_call_id.clone()));
        let mut raw_input = serde_json::Map::new();
        if let Some(command) = &request.command {
            raw_input.insert("command".into(), serde_json::Value::String(command.clone()));
        }
        raw_input.insert("inputDigest".into(), serde_json::Value::String(request.input_digest.clone()));
        let params = RequestPermissionParams {
            session_id: session_id.to_string(),
            tool_call: PermissionToolCall {
                tool_call_id: request.tool_call_id.clone(),
                title: Some(request.command.clone().unwrap_or_else(|| request.title.clone())),
                kind: Some(request.kind.as_str().to_string()),
                raw_input: Some(serde_json::Value::Object(raw_input)),
            },
            options: Self::permission_options(),
        };
        request_line(&id, methods::SESSION_REQUEST_PERMISSION, &serde_json::to_value(params).unwrap())
    }

    /// Answer the pending prompt with a stop reason.
    pub fn finish_prompt(&mut self, session_id: &str, stop_reason: StopReason) -> Option<String> {
        let session = self.sessions.get_mut(session_id)?;
        let id = session.prompt_request.take()?;
        Some(result_line(&id, &serde_json::json!({ "stopReason": stop_reason })))
    }

    /// Fail the pending prompt with a JSON-RPC error (provider errors,
    /// missing grants). The detail names the failure; it never carries a
    /// secret — redaction happened in the core.
    pub fn fail_prompt(&mut self, session_id: &str, message: &str) -> Option<String> {
        let session = self.sessions.get_mut(session_id)?;
        let id = session.prompt_request.take()?;
        Some(error_line(&id, &ErrorObject { code: INTERNAL_ERROR, message: message.into(), data: None }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use probe_core::permission::ToolKind;

    fn server() -> AcpServer {
        AcpServer::new(ServerConfig::default())
    }

    fn initialize(server: &mut AcpServer) {
        let (lines, _) = server.handle_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}}}"#,
        );
        let value: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(value["result"]["protocolVersion"], 1);
        assert_eq!(value["result"]["agentCapabilities"]["loadSession"], true);
        assert_eq!(value["result"]["authMethods"], serde_json::json!([]));
    }

    fn new_session(server: &mut AcpServer) -> String {
        let (lines, _) = server.handle_line(
            r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/work/repo","mcpServers":[]}}"#,
        );
        let value: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        value["result"]["sessionId"].as_str().unwrap().to_string()
    }

    #[test]
    fn requests_before_initialize_are_refused() {
        let mut server = server();
        let (lines, events) =
            server.handle_line(r#"{"jsonrpc":"2.0","id":9,"method":"session/new","params":{"cwd":"/x"}}"#);
        assert!(events.is_empty());
        let value: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert!(value["error"]["message"].as_str().unwrap().contains("Not initialized"));
    }

    #[test]
    fn full_lifecycle_reaches_prompt_started_and_finishes_with_end_turn() {
        let mut server = server();
        initialize(&mut server);
        let session_id = new_session(&mut server);
        let (lines, events) = server.handle_line(&format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"sessionId":"{session_id}","prompt":[{{"type":"text","text":"fix the bug"}}]}}}}"#
        ));
        assert!(lines.is_empty());
        assert_eq!(
            events,
            vec![ServerEvent::PromptStarted {
                session_id: session_id.clone(),
                cwd: "/work/repo".into(),
                prompt: "fix the bug".into()
            }]
        );
        let line = server.finish_prompt(&session_id, StopReason::EndTurn).unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["id"], 3);
        assert_eq!(value["result"]["stopReason"], "end_turn");
        // Finishing twice is impossible: the request id is consumed.
        assert!(server.finish_prompt(&session_id, StopReason::EndTurn).is_none());
    }

    #[test]
    fn cancel_notification_produces_the_cancel_event_and_cancelled_stop_reason() {
        let mut server = server();
        initialize(&mut server);
        let session_id = new_session(&mut server);
        server.handle_line(&format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"sessionId":"{session_id}","prompt":[{{"type":"text","text":"work"}}]}}}}"#
        ));
        let (lines, events) = server.handle_line(&format!(
            r#"{{"jsonrpc":"2.0","method":"session/cancel","params":{{"sessionId":"{session_id}"}}}}"#
        ));
        assert!(lines.is_empty());
        assert_eq!(events, vec![ServerEvent::PromptCancelled { session_id: session_id.clone() }]);
        let line = server.finish_prompt(&session_id, StopReason::Cancelled).unwrap();
        assert!(line.contains("cancelled"));
    }

    #[test]
    fn permission_flow_round_trips_and_options_never_look_like_a_bypass() {
        let mut server = server();
        initialize(&mut server);
        let session_id = new_session(&mut server);
        let request = PermissionRequest {
            tool_call_id: "t1".into(),
            tool_name: "shell".into(),
            kind: ToolKind::Execute,
            title: "shell".into(),
            command: Some("git status".into()),
            input_digest: "fnv1a:0".into(),
        };
        let line = server.request_permission(&session_id, &request);
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["method"], "session/request_permission");
        assert_eq!(value["params"]["toolCall"]["rawInput"]["command"], "git status");
        let options = value["params"]["options"].as_array().unwrap();
        assert!(options.iter().any(|option| option["kind"] == "allow_once"));
        for option in options {
            for field in ["optionId", "name", "kind"] {
                let text = option[field].as_str().unwrap().to_lowercase();
                assert!(!text.contains("bypass"), "option {field} may not look like a bypass: {text}");
            }
        }
        let request_id = value["id"].clone();
        let (_, events) = server.handle_line(&format!(
            r#"{{"jsonrpc":"2.0","id":{request_id},"result":{{"outcome":{{"outcome":"selected","optionId":"allow_once"}}}}}}"#
        ));
        assert_eq!(
            events,
            vec![ServerEvent::PermissionDecided {
                session_id: session_id.clone(),
                tool_call_id: "t1".into(),
                decision: PermissionDecision::Allowed
            }]
        );
    }

    #[test]
    fn permission_cancelled_outcome_is_a_denial() {
        let mut server = server();
        initialize(&mut server);
        let session_id = new_session(&mut server);
        let request = PermissionRequest {
            tool_call_id: "t1".into(),
            tool_name: "shell".into(),
            kind: ToolKind::Execute,
            title: "shell".into(),
            command: None,
            input_digest: "fnv1a:0".into(),
        };
        let line = server.request_permission(&session_id, &request);
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        let request_id = value["id"].clone();
        let (_, events) = server.handle_line(&format!(
            r#"{{"jsonrpc":"2.0","id":{request_id},"result":{{"outcome":{{"outcome":"cancelled"}}}}}}"#
        ));
        assert!(matches!(
            &events[0],
            ServerEvent::PermissionDecided { decision: PermissionDecision::Denied, .. }
        ));
    }

    #[test]
    fn session_load_accepts_known_ids_and_refuses_unknown_ones() {
        let mut server = server();
        initialize(&mut server);
        let session_id = new_session(&mut server);
        let (lines, _) = server.handle_line(&format!(
            r#"{{"jsonrpc":"2.0","id":4,"method":"session/load","params":{{"sessionId":"{session_id}","cwd":"/work/other","mcpServers":[]}}}}"#
        ));
        let value: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert!(value.get("error").is_none());
        let (lines, _) = server.handle_line(
            r#"{"jsonrpc":"2.0","id":5,"method":"session/load","params":{"sessionId":"sess_missing","cwd":"/x","mcpServers":[]}}"#,
        );
        let value: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(value["error"]["message"], "unknown session");
    }

    #[test]
    fn unknown_methods_get_method_not_found() {
        let mut server = server();
        initialize(&mut server);
        let (lines, _) = server.handle_line(r#"{"jsonrpc":"2.0","id":6,"method":"fs/read_text_file","params":{}}"#);
        let value: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(value["error"]["code"], -32601);
    }

    #[test]
    fn oversize_updates_are_refused_rather_than_silently_lost() {
        let mut server = AcpServer::new(ServerConfig { max_line_bytes: 200 });
        server.initialized = true;
        let update = SessionUpdate::AgentMessageChunk {
            content: ContentBlock::Text { text: "x".repeat(500) },
        };
        assert!(server.send_update("sess_1", update).is_none());
        let small = SessionUpdate::AgentMessageChunk { content: ContentBlock::Text { text: "ok".into() } };
        let line = server.send_update("sess_1", small).unwrap();
        assert!(line.len() <= 200);
    }
}
