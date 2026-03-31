# OpenAI Streaming And Runtime Delta Events

Issue `#47` adds the first real streamed OpenAI-compatible provider path to
Probe plus the runtime event widening needed for live terminal surfaces.

## What Landed

- `probe-provider-openai` no longer rejects `stream: true`
- the provider now parses OpenAI-compatible SSE frames from
  `/v1/chat/completions`
- streamed assistant text is reassembled into the same final
  `ChatCompletionResponse` shape used by the blocking path
- streamed tool-call deltas are reassembled into final `tool_calls`
- terminal usage receipts are preserved when the stream includes them
- non-stream JSON fallback still works when a backend responds to a streaming
  request with a blocking JSON body

## Runtime Event Widening

`probe-core` now emits incremental OpenAI-stream events in addition to the
existing lifecycle and commit events:

- `AssistantStreamStarted`
- `TimeToFirstTokenObserved`
- `AssistantDelta`
- `ToolCallDelta`
- `AssistantStreamFinished`
- `ModelRequestFailed`

The preexisting events remain the durable turn-loop truth:

- `TurnStarted`
- `ModelRequestStarted`
- `ToolCallRequested`
- `ToolExecutionStarted`
- `ToolExecutionCompleted`
- `ToolRefused`
- `ToolPaused`
- `AssistantTurnCommitted`

That means streamed output is now visible to runtime consumers without
changing the append-only transcript contract or moving tool execution out of
Probe.

## Behavioral Contract

The landed shape is deliberately narrow:

- only the OpenAI-compatible backend lane streams in this issue
- Probe still owns tool execution, approvals, transcript persistence, and turn
  orchestration locally
- streamed tool-call deltas are advisory live events; the persisted transcript
  is still created from the final assembled response plus local tool outcomes
- if a backend answers a `stream: true` request with plain JSON instead of SSE,
  Probe falls back to that same blocking response body instead of retrying a
  second request

## Files

- `crates/probe-provider-openai/src/lib.rs`
- `crates/probe-core/src/provider.rs`
- `crates/probe-core/src/runtime.rs`
- `crates/probe-test-support/src/lib.rs`
- `crates/probe-tui/src/screens.rs`

## Validation

The retained coverage for this issue includes:

- provider tests for streamed text assembly
- provider tests for streamed tool-call assembly
- provider tests for JSON fallback on a streaming request
- runtime tests for streamed plain-text event emission
- runtime tests for streamed tool-call delta emission
- full `probe-core` lib and `probe-tui` test runs to verify compatibility with
  the widened event enum

The TUI does not render incremental streamed text yet in this issue. That UI
work lands on top of these runtime events in the next step.
