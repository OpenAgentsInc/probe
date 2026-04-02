# Probe Client And First-Party Adoption

`probe-client` is the shared first-party client layer for `probe-server`.

Its job is narrow:

- own the Probe server transport seam
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
- local daemon-socket attach for `probe-daemon`
- hosted TCP attach for remote Probe control planes
- shared local-daemon autostart when the socket is missing
- explicit fallback to the hidden `probe-cli __internal-probe-server` mode when
  a standalone sibling `probe-server` binary is not present
- protocol request ids and JSONL framing
- handshake on startup
- typed request and response handling
- inline event draining during active turn or approval requests
- queued-turn submission and inspection helpers
- detached-session listing and inspection helpers for daemon mode
- detached-session log replay and watch helpers for daemon mode
- shutdown on drop

Transport selection is now explicit:

- `SpawnStdio`
- `LocalDaemon`
- `HostedTcp`

The shutdown contract differs by transport:

- spawned child transports still shut down with the client
- daemon transports only close the client connection on drop
- hosted TCP transports also only close the client connection on drop

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
- `probe chat` now uses the daemon transport for its interactive session loop
  and auto-starts the local daemon when needed
- the TUI worker now drives turns and approval continuations through the shared
  client and uses inspect or approval APIs for transcript and pending-approval
  refresh
- `probe tui --resume <session-id>` now attaches to a daemon-owned detached
  session through that same shared client seam before a new prompt runs

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
- queued follow-up turns now go through explicit queue APIs instead of being
  smuggled through direct turn requests
- direct `start_turn` and `continue_turn` requests still return `session_busy`
  when a session already has active or queued work
- queued background progress can now be polled through
  `inspect_session_turns` or watched through the detached daemon event stream

## Hidden Internal Server Mode

The repo now supports two local spawn paths for the same server contract:

- standalone `probe-server`
- hidden `probe-cli __internal-probe-server`

The hidden path exists for practical local development and test runs where
`probe-cli` is built but a standalone sibling `probe-server` binary has not
been materialized yet.

That fallback does not create a second protocol. It only changes how the same
`probe-server::server::run_stdio_server` entrypoint is launched.

The new daemon path keeps that same rule. `probe-client` now chooses between a
spawned stdio child, the local daemon socket, and the hosted TCP transport, but
it still speaks the same typed request, event, and response contract either
way.

The same crate now owns the daemon autostart helper for first-party clients:

- prefer a sibling `probe-daemon` binary when one exists
- allow `PROBE_DAEMON_BIN` override for explicit launch paths
- otherwise fall back to `probe-cli __internal-probe-daemon`

The shared client also now exposes the first daemon-owned session helpers:

- explicit session creation through `start_session`
- `list_detached_sessions`
- `inspect_detached_session`
- `read_detached_session_log`
- `watch_detached_session`

That keeps later `probe ps` and attach work on the same shared client seam
instead of growing a second daemon-only control client. The shipped operator
CLI in `64-daemon-operator-cli.md` now sits on that same shared seam.

Hosted TCP is intentionally narrower today:

- no auto-start helper
- no auth layer in `probe-client`
- no browser-facing HTTP adapter

It is only the first remote Rust-to-Rust attach lane above the same Probe
runtime contract.

Hosted callers can now also seed `StartSessionRequest.workspace_state` when
they already know the intended prepared baseline, snapshot restore ref, or
execution host. Probe normalizes that into stored session truth instead of
forcing the caller to keep its own shadow provenance model.

## New Shared Control Calls

The shared client now exposes the first queued-turn and control helpers on top
of the wire protocol:

- queue a follow-up turn for an existing session
- inspect active, queued, and recent terminal turn-control records
- cancel a queued turn before execution
- interrupt an approval-paused active turn

That keeps the queue and approval control surface typed at the client boundary
instead of pushing those details back into ad-hoc filesystem reads.

## Verification

The migration is covered at two levels:

- `crates/probe-client/src/lib.rs`
  - a real client test against a real `probe-server` child
  - a real hosted TCP transport test against a real `probe-server` listener
  - hosted workspace-provenance coverage for prepared baseline hydration,
    detached-summary projection, and explicit fresh-start fallback when a
    requested prepared baseline is missing
- `crates/probe-cli/tests/binary_e2e.rs`
  - process-level `chat` and `tui` coverage now running through the shared
    client and server seam, including detached daemon reattach for both
    first-party interactive surfaces
