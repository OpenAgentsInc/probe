# Probe Runtime Event Stream And Live TUI Lifecycle

## Summary

Issue `#43` adds a typed per-turn runtime event surface in `probe-core` and
wires `probe-tui` to it.

Probe TUI no longer has to wait for a whole turn to finish before it can show
runtime progress. The shell now receives live events for:

- turn start
- model request start
- tool call request
- tool execution start
- tool completion
- tool refusal
- tool pause for approval
- assistant turn commit

This is the first real runtime-driven lifecycle lane for the Probe TUI.

## Runtime Surface

`probe-core` now exports:

- `RuntimeEvent`
- `RuntimeEventSink`
- `exec_plain_text_with_events`
- `continue_plain_text_session_with_events`

The old `exec_plain_text` and `continue_plain_text_session` APIs remain intact
for callers that only need the final outcome.

## OpenAI And Apple FM Coverage

The OpenAI tool-loop path now emits ordered runtime events around each tool
round.

The Apple FM callback-backed tool path emits comparable events through the
callback recorder where the bridge exposes enough structure to do so cleanly.

That keeps the event surface Probe-owned and backend-aware rather than pushing
provider-specific parsing into the TUI.

## TUI Follow-Through

`probe-tui` now forwards runtime events from the worker thread into typed
`AppMessage::ProbeRuntimeEvent` messages.

`ChatScreen` uses those events to drive:

- the explicit active-turn cell
- worker event-log lines
- right-rail runtime phase, round, and active-tool state

The submit path no longer injects a fake placeholder active turn from the UI
layer. Live progress now comes from the runtime event stream itself.

## Current Boundary

This issue does not change the committed transcript source of truth.

Committed rows still come from the persisted Probe transcript after the turn
finishes. Issue `#44` remains the place to harden and refine the transcript-row
mapping and presentation around that truth.

## Tests

Coverage now proves:

- successful tool-backed turns emit ordered runtime events
- paused tool turns emit the expected pause event before returning an error
- the TUI shows a live active cell from runtime events and retains the live
  lifecycle events in its event log
- refreshed snapshots cover the expanded runtime sidebar and event-driven
  active-turn state

Validation:

```bash
cargo test -p probe-core eventful_tool_loop_emits_ordered_events_for_successful_turn -- --nocapture
cargo test -p probe-core eventful_tool_loop_emits_pause_event_before_returning_error -- --nocapture
cargo test -p probe-tui -- --nocapture
cargo test -p probe-cli --test cli_regressions -- --nocapture
cargo check --workspace
```
