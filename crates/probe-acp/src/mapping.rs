//! Pure mapping from the neutral probe-core event union onto ACP
//! `session/update` payloads, plus the chunking that keeps every framed
//! line under the client's message budget. The controller drops oversize
//! JSON-RPC lines WHOLE and SILENTLY (4 MiB cap), so exceeding the budget
//! is not an error — it is invisible loss. Chunk far below it.

use probe_core::contract::event::Event;
use probe_core::contract::message::ToolResultValue;
use probe_core::permission::ToolKind;
use probe_core::turn::TurnStatus;

use crate::types::{ContentBlock, SessionUpdate, StopReason, ToolCallContent, ToolCallStatus};

/// Default per-update text budget: far below the controller's 4 MiB line
/// cap, and small enough that a single update never dominates its 128 KiB
/// streamed-output window.
pub const DEFAULT_TEXT_BUDGET_BYTES: usize = 32 * 1024;

/// Split text into chunks of at most `budget` bytes on char boundaries.
pub fn bounded_chunks(text: &str, budget: usize) -> Vec<String> {
    assert!(budget > 0, "text budget must be positive");
    if text.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if current.len() + ch.len_utf8() > budget && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn tool_result_text(result: &ToolResultValue) -> String {
    match result {
        ToolResultValue::Text { value } | ToolResultValue::Error { value } => value.clone(),
        ToolResultValue::Json { value } => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn command_of(input: &serde_json::Value) -> Option<String> {
    input.get("command").and_then(|value| value.as_str()).map(str::to_string)
}

/// Map one neutral event to zero or more session updates. `kind_for` names
/// the ACP kind for a tool; the real command string rides in `rawInput` so
/// tier policies and the delegation rail can read it.
pub fn updates_for_event(
    event: &Event,
    kind_for: &dyn Fn(&str) -> ToolKind,
    text_budget: usize,
) -> Vec<SessionUpdate> {
    match event {
        Event::TextDelta { text, .. } => bounded_chunks(text, text_budget)
            .into_iter()
            .map(|chunk| SessionUpdate::AgentMessageChunk { content: ContentBlock::Text { text: chunk } })
            .collect(),
        Event::ReasoningDelta { text, .. } => bounded_chunks(text, text_budget)
            .into_iter()
            .map(|chunk| SessionUpdate::AgentThoughtChunk { content: ContentBlock::Text { text: chunk } })
            .collect(),
        Event::ToolCall { id, name, input, .. } => {
            let command = command_of(input);
            vec![SessionUpdate::ToolCall {
                tool_call_id: id.clone(),
                kind: Some(kind_for(name).as_str().to_string()),
                title: Some(command.clone().unwrap_or_else(|| name.clone())),
                raw_input: Some(input.clone()),
                status: Some(ToolCallStatus::InProgress),
            }]
        }
        Event::ToolResult { id, name, result, .. } => {
            let failed = matches!(result, ToolResultValue::Error { .. });
            let mut text = tool_result_text(result);
            let mut cut = text_budget.min(text.len());
            while cut < text.len() && !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            vec![SessionUpdate::ToolCallUpdate {
                tool_call_id: id.clone(),
                status: if failed { ToolCallStatus::Failed } else { ToolCallStatus::Completed },
                kind: Some(kind_for(name).as_str().to_string()),
                title: Some(name.clone()),
                content: if text.is_empty() { Vec::new() } else { vec![ToolCallContent::text(text)] },
            }]
        }
        // tool-error always precedes a paired tool-result carrying the same
        // message; the result mapping above renders the failure.
        Event::ToolError { .. } => Vec::new(),
        Event::StepStart { .. }
        | Event::StepFinish { .. }
        | Event::Finish { .. }
        | Event::ProviderError { .. } => Vec::new(),
    }
}

/// TurnStatus -> ACP stop reason, in the controller's vocabulary.
pub fn stop_reason_for(status: &TurnStatus) -> StopReason {
    use probe_core::contract::event::FinishReason;
    match status {
        TurnStatus::Cancelled => StopReason::Cancelled,
        TurnStatus::ToolRoundLimit => StopReason::MaxTurnRequests,
        TurnStatus::Completed { reason } => match reason {
            FinishReason::Length => StopReason::MaxTokens,
            FinishReason::ContentFilter => StopReason::Refusal,
            _ => StopReason::EndTurn,
        },
        // Provider errors are reported as JSON-RPC errors on the prompt
        // request, not a stop reason; this arm exists for total mapping.
        TurnStatus::ProviderError { .. } => StopReason::EndTurn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_for(_name: &str) -> ToolKind {
        ToolKind::Execute
    }

    #[test]
    fn text_deltas_chunk_under_the_budget_and_concatenate_back() {
        let text = "abcdefghij".repeat(10);
        let event = Event::TextDelta { id: "t".into(), text: text.clone(), provider_metadata: None };
        let updates = updates_for_event(&event, &kind_for, 16);
        assert!(updates.len() > 1);
        let mut rebuilt = String::new();
        for update in &updates {
            let SessionUpdate::AgentMessageChunk { content: ContentBlock::Text { text } } = update else {
                panic!("expected message chunk");
            };
            assert!(text.len() <= 16);
            rebuilt.push_str(text);
        }
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn tool_calls_surface_kind_title_and_raw_input_command() {
        let event = Event::ToolCall {
            id: "t1".into(),
            name: "shell".into(),
            input: serde_json::json!({ "command": "git status" }),
            provider_executed: None,
            provider_metadata: None,
        };
        let updates = updates_for_event(&event, &kind_for, DEFAULT_TEXT_BUDGET_BYTES);
        let SessionUpdate::ToolCall { kind, title, raw_input, .. } = &updates[0] else {
            panic!("expected tool_call");
        };
        assert_eq!(kind.as_deref(), Some("execute"));
        assert_eq!(title.as_deref(), Some("git status"));
        assert_eq!(raw_input.as_ref().unwrap()["command"], "git status");
    }

    #[test]
    fn error_results_render_failed_and_plain_results_completed() {
        let failed = Event::ToolResult {
            id: "t1".into(),
            name: "shell".into(),
            result: ToolResultValue::error("denied"),
            provider_executed: None,
            provider_metadata: None,
        };
        let updates = updates_for_event(&failed, &kind_for, DEFAULT_TEXT_BUDGET_BYTES);
        assert!(matches!(&updates[0], SessionUpdate::ToolCallUpdate { status: ToolCallStatus::Failed, .. }));
        let ok = Event::ToolResult {
            id: "t1".into(),
            name: "shell".into(),
            result: ToolResultValue::text("fine"),
            provider_executed: None,
            provider_metadata: None,
        };
        let updates = updates_for_event(&ok, &kind_for, DEFAULT_TEXT_BUDGET_BYTES);
        assert!(matches!(&updates[0], SessionUpdate::ToolCallUpdate { status: ToolCallStatus::Completed, .. }));
    }

    #[test]
    fn stop_reasons_map_to_the_controller_vocabulary() {
        use probe_core::contract::event::FinishReason;
        assert_eq!(stop_reason_for(&TurnStatus::Cancelled), StopReason::Cancelled);
        assert_eq!(stop_reason_for(&TurnStatus::ToolRoundLimit), StopReason::MaxTurnRequests);
        assert_eq!(
            stop_reason_for(&TurnStatus::Completed { reason: FinishReason::Length }),
            StopReason::MaxTokens
        );
        assert_eq!(
            stop_reason_for(&TurnStatus::Completed { reason: FinishReason::ContentFilter }),
            StopReason::Refusal
        );
        assert_eq!(
            stop_reason_for(&TurnStatus::Completed { reason: FinishReason::Stop }),
            StopReason::EndTurn
        );
    }
}
