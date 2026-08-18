//! The agent loop as a sans-I/O state machine. The host drives it: provider
//! events, tool outcomes, permission decisions, and cancellation come in;
//! typed effects come out. The core never performs I/O, never prompts, and
//! never sees a credential.
//!
//! Discipline carried from Sarah's tool runtime: tool calls execute one at a
//! time, rounds are bounded (sixteen by default), denials become paired
//! `tool-error` + `tool-result` events (never silence), and cancellation is
//! an ordinary state transition from any state.

use std::collections::{BTreeMap, VecDeque};

use crate::contract::event::{Event, FinishReason};
use crate::contract::message::{ContentPart, Message, Role, ToolResultValue};
use crate::contract::request::Request;
use crate::contract::usage::{ProviderMetadata, Usage};
use crate::permission::{
    input_digest, PermissionDecision, PermissionPolicy, PermissionRequest, PolicyTable, ToolKind,
};

/// What the host must do next. Effects are emitted in order and are the
/// core's only voice.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnEffect {
    /// Send this request to the model transport and feed the resulting
    /// events back through `on_provider_event`.
    SendProviderRequest(Request),
    /// Ask the client for permission (over ACP `session/request_permission`)
    /// and feed the decision back through `on_permission_decision`.
    RequestPermission(PermissionRequest),
    /// Execute this tool and feed the outcome back through `on_tool_outcome`.
    ExecuteTool(ToolInvocation),
    /// A turn-synthesized event (tool results, denials) for the host's
    /// rendering surface. Provider events are already in the host's hands.
    Emit(Event),
    /// The turn is over. No further inputs will produce effects.
    Finished(TurnOutcome),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolInvocation {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TurnStatus {
    Completed { reason: FinishReason },
    Cancelled,
    ToolRoundLimit,
    ProviderError { message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TurnOutcome {
    pub status: TurnStatus,
    /// Concatenated assistant text across all rounds.
    pub text: String,
    pub usage: Usage,
    pub rounds: u64,
    pub tool_calls: u64,
}

#[derive(Debug, Clone)]
pub struct TurnConfig {
    pub max_tool_rounds: u64,
    pub policy: PolicyTable,
    /// ACP kind per tool name; unknown tools are `other` (which escalates
    /// under the default policy — unknown never silently widens authority).
    pub tool_kinds: BTreeMap<String, ToolKind>,
}

impl Default for TurnConfig {
    fn default() -> Self {
        TurnConfig {
            max_tool_rounds: 16,
            policy: PolicyTable::default(),
            tool_kinds: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct PendingCall {
    id: String,
    name: String,
    input: serde_json::Value,
    provider_metadata: Option<ProviderMetadata>,
}

#[derive(Debug, Clone)]
enum CurrentCall {
    AwaitingPermission(PendingCall),
    AwaitingExecution(PendingCall),
}

#[derive(Debug)]
enum Phase {
    Streaming,
    ProcessingTools,
    Finished,
}

pub struct Turn {
    config: TurnConfig,
    request: Request,
    phase: Phase,
    /// Ordered text accumulation, id-keyed so interleaved deltas reassemble.
    text_order: Vec<String>,
    text_by_id: BTreeMap<String, String>,
    all_text: String,
    round_calls: Vec<PendingCall>,
    queue: VecDeque<PendingCall>,
    current: Option<CurrentCall>,
    results: Vec<ContentPart>,
    usage: Usage,
    rounds: u64,
    tool_calls_total: u64,
}

impl Turn {
    pub fn new(config: TurnConfig, request: Request) -> Turn {
        Turn {
            config,
            request,
            phase: Phase::Streaming,
            text_order: Vec::new(),
            text_by_id: BTreeMap::new(),
            all_text: String::new(),
            round_calls: Vec::new(),
            queue: VecDeque::new(),
            current: None,
            results: Vec::new(),
            usage: Usage::default(),
            rounds: 0,
            tool_calls_total: 0,
        }
    }

    /// Start the turn: the first provider request.
    pub fn begin(&mut self) -> Vec<TurnEffect> {
        vec![TurnEffect::SendProviderRequest(self.request.clone())]
    }

    pub fn on_provider_event(&mut self, event: Event) -> Vec<TurnEffect> {
        if !matches!(self.phase, Phase::Streaming) {
            return Vec::new();
        }
        match event {
            Event::TextDelta { id, text, .. } => {
                if !self.text_by_id.contains_key(&id) {
                    self.text_order.push(id.clone());
                }
                self.text_by_id.entry(id).or_default().push_str(&text);
                Vec::new()
            }
            Event::ToolCall { id, name, input, provider_metadata, .. } => {
                self.round_calls.push(PendingCall { id, name, input, provider_metadata });
                Vec::new()
            }
            Event::ProviderError { message, .. } => self.finish(TurnStatus::ProviderError { message }),
            Event::StepFinish { usage, .. } => {
                if let Some(usage) = usage {
                    self.usage.accumulate(&usage.normalized());
                }
                Vec::new()
            }
            Event::Finish { reason, usage, .. } => {
                if let Some(usage) = usage {
                    self.usage.accumulate(&usage.normalized());
                }
                self.flush_round_text();
                if reason == FinishReason::ToolCalls && !self.round_calls.is_empty() {
                    self.phase = Phase::ProcessingTools;
                    self.queue = self.round_calls.iter().cloned().collect();
                    self.tool_calls_total += self.round_calls.len() as u64;
                    self.advance()
                } else {
                    self.finish(TurnStatus::Completed { reason })
                }
            }
            // Reasoning deltas, step starts, and provider tool results are
            // rendering concerns; the host already holds them.
            _ => Vec::new(),
        }
    }

    pub fn on_permission_decision(&mut self, tool_call_id: &str, decision: PermissionDecision) -> Vec<TurnEffect> {
        let Some(CurrentCall::AwaitingPermission(call)) = self.current.take() else {
            self.current = self.current.take();
            return Vec::new();
        };
        if call.id != tool_call_id {
            self.current = Some(CurrentCall::AwaitingPermission(call));
            return Vec::new();
        }
        match decision {
            PermissionDecision::Allowed => {
                let invocation = ToolInvocation {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    input: call.input.clone(),
                };
                self.current = Some(CurrentCall::AwaitingExecution(call));
                vec![TurnEffect::ExecuteTool(invocation)]
            }
            PermissionDecision::Denied => {
                let mut effects = self.record_failure(&call, &format!("permission denied: {}", call.name));
                effects.extend(self.advance());
                effects
            }
        }
    }

    pub fn on_tool_outcome(&mut self, tool_call_id: &str, result: ToolResultValue) -> Vec<TurnEffect> {
        let Some(CurrentCall::AwaitingExecution(call)) = self.current.take() else {
            self.current = self.current.take();
            return Vec::new();
        };
        if call.id != tool_call_id {
            self.current = Some(CurrentCall::AwaitingExecution(call));
            return Vec::new();
        }
        let mut effects = Vec::new();
        if let ToolResultValue::Error { value } = &result {
            effects.push(TurnEffect::Emit(Event::ToolError {
                id: call.id.clone(),
                name: call.name.clone(),
                message: value.clone(),
                provider_metadata: None,
            }));
        }
        effects.push(TurnEffect::Emit(Event::ToolResult {
            id: call.id.clone(),
            name: call.name.clone(),
            result: result.clone(),
            provider_executed: None,
            provider_metadata: None,
        }));
        self.results.push(ContentPart::tool_result(call.id.clone(), call.name.clone(), result));
        effects.extend(self.advance());
        effects
    }

    /// Cancellation is an ordinary transition from any state.
    pub fn cancel(&mut self) -> Vec<TurnEffect> {
        if matches!(self.phase, Phase::Finished) {
            return Vec::new();
        }
        self.flush_round_text();
        self.finish(TurnStatus::Cancelled)
    }

    fn advance(&mut self) -> Vec<TurnEffect> {
        let mut effects = Vec::new();
        while self.current.is_none() {
            let Some(call) = self.queue.pop_front() else {
                effects.extend(self.continue_round());
                return effects;
            };
            let kind = self.config.tool_kinds.get(&call.name).copied().unwrap_or(ToolKind::Other);
            match self.config.policy.policy_for(kind) {
                PermissionPolicy::Allow => {
                    let invocation = ToolInvocation {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        input: call.input.clone(),
                    };
                    self.current = Some(CurrentCall::AwaitingExecution(call));
                    effects.push(TurnEffect::ExecuteTool(invocation));
                }
                PermissionPolicy::ApprovalRequired => {
                    let request = PermissionRequest {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        kind,
                        title: call.name.clone(),
                        command: call
                            .input
                            .get("command")
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                        input_digest: input_digest(&call.input),
                    };
                    self.current = Some(CurrentCall::AwaitingPermission(call));
                    effects.push(TurnEffect::RequestPermission(request));
                }
                PermissionPolicy::Deny => {
                    effects.extend(self.record_failure(&call, &format!("tool denied by policy: {}", call.name)));
                }
            }
        }
        effects
    }

    fn continue_round(&mut self) -> Vec<TurnEffect> {
        self.rounds += 1;
        if self.rounds >= self.config.max_tool_rounds {
            return self.finish(TurnStatus::ToolRoundLimit);
        }
        let mut assistant_parts: Vec<ContentPart> = Vec::new();
        let round_text = self.take_round_text();
        if !round_text.is_empty() {
            assistant_parts.push(ContentPart::text(round_text));
        }
        for call in &self.round_calls {
            assistant_parts.push(ContentPart::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                input: call.input.clone(),
                provider_executed: None,
                metadata: None,
                provider_metadata: call.provider_metadata.clone(),
            });
        }
        self.request.messages.push(Message::from_parts(Role::Assistant, assistant_parts));
        self.request
            .messages
            .push(Message::from_parts(Role::Tool, std::mem::take(&mut self.results)));
        self.round_calls.clear();
        self.phase = Phase::Streaming;
        vec![TurnEffect::SendProviderRequest(self.request.clone())]
    }

    fn record_failure(&mut self, call: &PendingCall, message: &str) -> Vec<TurnEffect> {
        let [error, result] = Event::tool_failure(&call.id, &call.name, message);
        self.results
            .push(ContentPart::tool_result(call.id.clone(), call.name.clone(), ToolResultValue::error(message)));
        vec![TurnEffect::Emit(error), TurnEffect::Emit(result)]
    }

    fn flush_round_text(&mut self) {
        let round_text = self.take_round_text();
        self.all_text.push_str(&round_text);
    }

    fn collect_round_text(&self) -> String {
        self.text_order
            .iter()
            .filter_map(|id| self.text_by_id.get(id).map(String::as_str))
            .collect()
    }

    fn take_round_text(&mut self) -> String {
        let text = self.collect_round_text();
        self.text_order.clear();
        self.text_by_id.clear();
        text
    }

    fn finish(&mut self, status: TurnStatus) -> Vec<TurnEffect> {
        self.phase = Phase::Finished;
        vec![TurnEffect::Finished(TurnOutcome {
            status,
            text: {
                // Text already flushed for completed rounds; include any
                // unflushed remainder from an interrupted stream.
                let mut text = self.all_text.clone();
                text.push_str(&self.take_round_text());
                text
            },
            usage: self.usage.clone(),
            rounds: self.rounds,
            tool_calls: self.tool_calls_total,
        })]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::request::ModelRef;

    fn request() -> Request {
        Request::simple(
            ModelRef { provider: "test".into(), model: "m".into() },
            "sys",
            "hi",
        )
    }

    fn finish(reason: FinishReason) -> Event {
        Event::Finish { reason, usage: None, provider_metadata: None }
    }

    fn text_delta(id: &str, text: &str) -> Event {
        Event::TextDelta { id: id.into(), text: text.into(), provider_metadata: None }
    }

    fn tool_call(id: &str, name: &str, input: serde_json::Value) -> Event {
        Event::ToolCall { id: id.into(), name: name.into(), input, provider_executed: None, provider_metadata: None }
    }

    #[test]
    fn plain_completion_flows_to_finished() {
        let mut turn = Turn::new(TurnConfig::default(), request());
        assert!(matches!(turn.begin()[0], TurnEffect::SendProviderRequest(_)));
        assert!(turn.on_provider_event(text_delta("a", "Hel")).is_empty());
        assert!(turn.on_provider_event(text_delta("a", "lo")).is_empty());
        let effects = turn.on_provider_event(finish(FinishReason::Stop));
        match &effects[0] {
            TurnEffect::Finished(outcome) => {
                assert_eq!(outcome.text, "Hello");
                assert!(matches!(outcome.status, TurnStatus::Completed { reason: FinishReason::Stop }));
            }
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    #[test]
    fn allowed_tool_executes_serially_and_continues_with_results() {
        let mut config = TurnConfig::default();
        config.tool_kinds.insert("lookup".into(), ToolKind::Read);
        let mut turn = Turn::new(config, request());
        turn.begin();
        turn.on_provider_event(tool_call("t1", "lookup", serde_json::json!({ "q": 1 })));
        turn.on_provider_event(tool_call("t2", "lookup", serde_json::json!({ "q": 2 })));
        let effects = turn.on_provider_event(finish(FinishReason::ToolCalls));
        // Serial: only the first call is dispatched.
        assert_eq!(
            effects,
            vec![TurnEffect::ExecuteTool(ToolInvocation {
                id: "t1".into(),
                name: "lookup".into(),
                input: serde_json::json!({ "q": 1 })
            })]
        );
        let effects = turn.on_tool_outcome("t1", ToolResultValue::text("one"));
        assert!(matches!(effects.last(), Some(TurnEffect::ExecuteTool(inv)) if inv.id == "t2"));
        let effects = turn.on_tool_outcome("t2", ToolResultValue::text("two"));
        // Both results are in: continuation request carries assistant
        // tool-call parts and a tool message with both results.
        let TurnEffect::SendProviderRequest(continuation) = effects.last().unwrap() else {
            panic!("expected continuation request");
        };
        assert_eq!(continuation.messages.len(), 3);
        assert_eq!(continuation.messages[2].role, Role::Tool);
        assert_eq!(continuation.messages[2].content.len(), 2);
    }

    #[test]
    fn approval_required_suspends_until_a_decision_and_denial_pairs_error_and_result() {
        let mut turn = Turn::new(TurnConfig::default(), request());
        turn.begin();
        turn.on_provider_event(tool_call("t1", "shell", serde_json::json!({ "command": "rm -rf /" })));
        let effects = turn.on_provider_event(finish(FinishReason::ToolCalls));
        let TurnEffect::RequestPermission(permission) = &effects[0] else {
            panic!("expected RequestPermission");
        };
        assert_eq!(permission.command.as_deref(), Some("rm -rf /"));
        let effects = turn.on_permission_decision("t1", PermissionDecision::Denied);
        assert!(matches!(effects[0], TurnEffect::Emit(Event::ToolError { .. })));
        assert!(matches!(effects[1], TurnEffect::Emit(Event::ToolResult { .. })));
        // Denied result still reaches the model in the continuation.
        assert!(matches!(effects.last(), Some(TurnEffect::SendProviderRequest(_))));
    }

    #[test]
    fn round_limit_is_enforced() {
        let mut config = TurnConfig::default();
        config.max_tool_rounds = 1;
        config.tool_kinds.insert("lookup".into(), ToolKind::Read);
        let mut turn = Turn::new(config, request());
        turn.begin();
        turn.on_provider_event(tool_call("t1", "lookup", serde_json::json!({})));
        turn.on_provider_event(finish(FinishReason::ToolCalls));
        let effects = turn.on_tool_outcome("t1", ToolResultValue::text("x"));
        assert!(matches!(
            effects.last(),
            Some(TurnEffect::Finished(TurnOutcome { status: TurnStatus::ToolRoundLimit, .. }))
        ));
    }

    #[test]
    fn cancellation_finishes_from_any_state_and_later_inputs_are_ignored() {
        let mut turn = Turn::new(TurnConfig::default(), request());
        turn.begin();
        turn.on_provider_event(text_delta("a", "partial"));
        let effects = turn.cancel();
        match &effects[0] {
            TurnEffect::Finished(outcome) => {
                assert!(matches!(outcome.status, TurnStatus::Cancelled));
                assert_eq!(outcome.text, "partial");
            }
            other => panic!("expected Finished, got {other:?}"),
        }
        assert!(turn.on_provider_event(finish(FinishReason::Stop)).is_empty());
        assert!(turn.cancel().is_empty());
    }

    #[test]
    fn provider_error_is_terminal() {
        let mut turn = Turn::new(TurnConfig::default(), request());
        turn.begin();
        let effects = turn.on_provider_event(Event::ProviderError {
            message: "boom".into(),
            retryable: Some(false),
            provider_metadata: None,
        });
        assert!(matches!(
            &effects[0],
            TurnEffect::Finished(TurnOutcome { status: TurnStatus::ProviderError { message }, .. }) if message == "boom"
        ));
    }

    #[test]
    fn usage_accumulates_across_steps() {
        let mut turn = Turn::new(TurnConfig::default(), request());
        turn.begin();
        turn.on_provider_event(Event::Finish {
            reason: FinishReason::Stop,
            usage: Some(Usage { input_tokens: Some(5), output_tokens: Some(2), ..Usage::default() }),
            provider_metadata: None,
        });
        // finish() emitted; outcome carried normalized usage
        let mut turn2 = Turn::new(TurnConfig::default(), request());
        turn2.begin();
        let effects = turn2.on_provider_event(Event::Finish {
            reason: FinishReason::Stop,
            usage: Some(Usage { input_tokens: Some(5), output_tokens: Some(2), ..Usage::default() }),
            provider_metadata: None,
        });
        let TurnEffect::Finished(outcome) = &effects[0] else { panic!() };
        assert_eq!(outcome.usage.total_tokens, Some(7));
    }
}
