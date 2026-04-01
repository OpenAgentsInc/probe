# Probe Client And First-Party Adoption

`probe-client` is the shared first-party client layer for `probe-server`.

Its job is narrow:

- spawn the local server seam
- run the startup handshake
- send typed runtime requests
- drain typed streamed events until the matching final response arrives
- centralize shutdown behavior

This is the layer that moves Probe's own clients onto the server contract
instead of letting `probe exec`, `probe chat`, and the TUI each wire
`probe-core` directly.

## What The Crate Owns

`probe-client` now owns:

- local child-process spawn for `probe-server`
- explicit fallback to the hidden `probe-cli __internal-probe-server` mode when
  a standalone sibling `probe-server` binary is not present
- protocol request ids and JSONL framing
- handshake on startup
- typed request and response handling
- inline event draining during active turn or approval requests
- shutdown on drop

## Why The API Adapts Back Into Core Value Types

The first migration keeps the caller-facing API pragmatic.

`probe-client` speaks the server protocol on the wire, but it converts that
protocol back into the existing Probe runtime value types where possible:

- `PlainTextExecOutcome`
- `ResolvePendingToolApprovalOutcome`
- `RuntimeEvent`
- `SessionMetadata`
- transcript and approval records from `probe-protocol`

That lets the CLI and TUI move onto the server contract without rewriting every
rendering and reporting path at the same time.

The important boundary change is transport ownership, not whether first-party
callers keep using familiar Rust structs after the conversion step.

## First-Party Consumers Now On The Shared Client

The primary session loop for these surfaces now goes through `probe-client`:

- `probe exec`
- `probe chat`
- `probe tui`

What changed:

- `probe exec` now starts a session through `probe-server`, submits the turn,
  and prints the converted outcome
- `probe chat` now lists or inspects stored sessions through `probe-client`
  instead of reading the session store directly for the primary chat flow
- the TUI worker now drives turns and approval continuations through the shared
  client and uses inspect or approval APIs for transcript and pending-approval
  refresh

## Backpressure And Queueing Posture

The first client cut is intentionally simple:

- one in-flight request per `ProbeClient`
- no second internal event queue inside the client
- streamed events are drained inline until the matching response arrives

That means the shared client does not invent a second buffering layer that
could silently diverge from the server's own lifecycle ordering.

Current implications:

- `lossless` versus `best_effort` event classes still come from the protocol
- first-party clients can coalesce `best_effort` progress updates if they want
- queued follow-up turns are still a separate later contract, so active sessions
  return `session_busy` instead of pretending a queue exists

## Hidden Internal Server Mode

The repo now supports two local spawn paths for the same server contract:

- standalone `probe-server`
- hidden `probe-cli __internal-probe-server`

The hidden path exists for practical local development and test runs where
`probe-cli` is built but a standalone sibling `probe-server` binary has not
been materialized yet.

That fallback does not create a second protocol. It only changes how the same
`probe-server::server::run_stdio_server` entrypoint is launched.

## Verification

The migration is covered at two levels:

- `crates/probe-client/src/lib.rs`
  - a real client test against a real `probe-server` child
- `crates/probe-cli/tests/binary_e2e.rs`
  - process-level `chat` and `tui` coverage now running through the shared
    client and server seam
