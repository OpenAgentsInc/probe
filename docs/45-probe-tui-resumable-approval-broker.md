# Probe TUI Resumable Approval Broker

## Summary

Issue `#45` turns Probe's persisted approval pause into a real approval broker.

Probe already knew how to:

- pause a risky tool call
- persist the paused `tool_result`
- surface `RuntimeError::ToolApprovalPending`

What it did not know how to do was finish that turn honestly from the UI. This
issue closes that gap in both `probe-core` and `probe-tui`.

## Runtime

### Pending-approval records

`probe-protocol` and `probe-core::session_store` now persist dedicated
`PendingToolApproval` records per session.

Each record carries:

- session id
- tool call id
- tool name
- structured arguments
- risk class
- pause reason
- originating transcript turn indexes
- request timestamp
- optional resolution state and resolved timestamp

This moves approval truth out of ad-hoc UI state and into durable runtime data.

### Resolve and resume APIs

`ProbeRuntime` now exposes runtime-backed approval methods:

- list pending tool approvals for a session
- resolve one pending approval as `approved` or `rejected`
- resume the paused tool loop once the pending set is empty

The runtime now has explicit behavior for:

- missing approval ids
- already-resolved approvals
- normal `continue` attempts while approvals are still pending

### Replay correctness

Transcript replay now uses the latest `tool_result` per tool call id rather
than blindly replaying the first paused receipt forever.

That matters because approval resolution is append-only:

1. original paused `tool_result`
2. later approved or refused `tool_result`
3. resumed assistant turn

On resume, Probe now replays the resolved result, not the stale paused one.

## TUI

### Real approval overlay

The approval overlay no longer renders placeholder copy. It now opens from real
pending-runtime state and shows:

- tool name
- call id
- risk class
- originating turn indexes
- pause reason
- structured arguments

### Real approve or reject path

Submitting the overlay now queues a real background worker request that calls
back into `ProbeRuntime`.

The shell behavior is now:

1. tool pauses
2. pending approval records load
3. approval overlay takes focus
4. composer disables while approvals remain pending
5. approve or reject resolves the pending tool
6. Probe either opens the next pending approval or resumes the paused turn

## Tests

Coverage now proves:

- pause -> approve -> resume for the OpenAI-style tool loop
- pause -> reject -> resume for the OpenAI-style tool loop
- pending approval records persist and resolve durably in the session store
- the TUI approval overlay renders real pending-tool data
- approving from the TUI clears pending approval state and resumes the turn

Validation:

```bash
cargo test -p probe-core --lib -- --nocapture
cargo test -p probe-tui -- --nocapture
cargo test -p probe-cli --test cli_regressions -- --nocapture
cargo check --workspace
```
