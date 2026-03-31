# Apple FM Streaming And Snapshot Events

Issue `#48` adds the first honest Apple Foundation Models streaming lane to
Probe.

## Operator Contract

Apple FM does not pretend to be the same as the OpenAI-compatible Qwen lane.

The landed Probe contract is:

- Apple FM streams through the existing session-first bridge contract
- streamed Apple FM output arrives as full response snapshots
- Probe exposes those snapshots explicitly through runtime events
- final transcript persistence still happens only after the terminal snapshot
- tool execution, approvals, transcript replay, and transcript restore remain
  local Probe responsibilities

This issue does **not** invent fake token deltas for Apple FM.

## What Landed

- `probe-provider-apple-fm` now has a streamed plain-text path built on:
  - session creation
  - `psionic-apple-fm` async `stream_session_response(...)`
  - final session cleanup
- tool-backed Apple FM turns can now stream through the same session-response
  endpoint while keeping the existing callback-driven local tool runtime
- transcript restore fallback on invalid JSON, typed availability facts, and
  typed Foundation Models errors still work under the streamed path
- `probe-core` now emits Apple-FM-specific incremental truth through:
  - `AssistantStreamStarted`
  - `TimeToFirstTokenObserved`
  - `AssistantSnapshot`
  - `AssistantStreamFinished`
  - existing local tool lifecycle and assistant commit events

## Behavioral Details

- plain-text Apple FM streaming now uses a temporary session instead of the
  blocking compatibility chat path
- streamed tool-backed Apple FM turns still create real callback-backed
  sessions with Probe-owned tool execution
- the final `AppleFmProviderSessionResponse` is assembled from the terminal
  snapshot event, including final usage and transcript state
- blocking Apple FM behavior remains available for non-streaming callers

## Files

- `Cargo.toml`
- `crates/probe-provider-apple-fm/Cargo.toml`
- `crates/probe-provider-apple-fm/src/lib.rs`
- `crates/probe-core/src/provider.rs`
- `crates/probe-core/src/runtime.rs`
- `crates/probe-tui/src/screens.rs`

## Validation

- `cargo test -p probe-provider-apple-fm -- --nocapture`
- `cargo test -p probe-core --lib -- --nocapture`
- `cargo test -p probe-tui -- --nocapture`
- `cargo check --workspace`

The next issue uses these snapshot events to make the TUI visibly append live
backend output instead of only reflecting final committed turns.
