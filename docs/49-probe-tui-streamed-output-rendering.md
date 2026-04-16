# Probe TUI Streamed Output Rendering

## Why

Probe already had streamed backend truth in `probe-core`, but `probe-tui` was
still treating those events like disposable status pings. That made the chat
surface feel fake even when the runtime was honest:

- assistant deltas replaced the active cell with short previews instead of
  growing real text
- Apple FM snapshot updates rendered like transient notes instead of the
  current full snapshot
- streamed tool-call assembly disappeared into event logs until the final
  authoritative transcript rows arrived
- the shell had no compact always-visible backend or stream summary after the
  right rail was removed

## What Changed

- `crates/probe-tui/src/message.rs`
  - adds first-class stream app messages for start, first chunk, assistant
    delta, assistant snapshot, streamed tool-call assembly, finish, and failure
- `crates/probe-tui/src/worker.rs`
  - maps runtime stream events into those typed app messages instead of
    treating them as generic `ProbeRuntimeEvent` passthrough
- `crates/probe-tui/src/screens.rs`
  - adds a retained live-stream state for the chat screen
  - accumulates OpenAI-compatible assistant deltas into one active assistant
    cell
  - replaces Apple FM snapshot text with the latest full snapshot instead of
    inventing token deltas
  - retains streamed tool-call assembly in the active cell until the
    authoritative tool rows commit
  - preserves partial output on stream failure and only clears the live stream
    cell when the authoritative transcript rows land
  - classifies runtime failures into operator-facing next steps instead of
    dumping raw backend transport text into the title row
- `crates/probe-tui/src/app.rs`
  - reuses the input box header as a compact backend and stream state surface,
    avoiding a return to the old right-rail clutter

## Operator Contract

- `cargo probe` now appends streamed backend output visibly in the transcript
  pane
- OpenAI-compatible backends render incremental assistant text deltas
- Apple FM renders honest snapshot updates rather than fake token streaming
- streamed tool-call assembly stays visible before final `tool_call` and
  `tool_result` transcript rows commit
- failures keep any partial assistant output visible together with the backend
  error
- failure titles and next steps now stay typed and compact:
  - Codex auth failures point to `probe codex login`
  - local backend reachability failures tell the operator to start the target
    and retry
  - usage-limit failures surface the reset window when the backend provides it
- authoritative committed transcript rows still replace the live cell once the
  runtime session store has the durable turn

## Validation

- `cargo test -p probe-tui -- --nocapture`
- snapshot coverage for:
  - streamed delta rendering
  - Apple FM snapshot rendering
  - live tool-call assembly
  - failure retention
  - commit replacement of the live cell
