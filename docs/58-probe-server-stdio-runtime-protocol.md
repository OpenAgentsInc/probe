# Probe Server Stdio Runtime Protocol

`probe-server` is the first local-first multi-client control plane for Probe.

Phase 1 keeps the transport intentionally narrow:

- one local child process
- JSON Lines over stdio
- typed requests, responses, and streamed events from
  `crates/probe-protocol/src/runtime.rs`

This is the first cut that lets Probe clients supervise the runtime through a
stable contract instead of linking directly to `probe-core` session and turn
internals.

## Message Framing

Each frame is one JSON object per line.

Client-to-server frames are:

- `message_type=request`

Server-to-client frames are:

- `message_type=response`
- `message_type=event`

Every request carries a `request_id`. All streamed events and the final response
for that operation echo the same `request_id`.

## Core Operations

The Phase 1 stdio contract currently ships these typed operations:

- `initialize`
- `start_session`
- `resume_session`
- `list_sessions`
- `inspect_session`
- `start_turn`
- `continue_turn`
- `interrupt_turn`
- `list_pending_approvals`
- `resolve_pending_approval`
- `shutdown`

Session creation is explicit. A client can create a durable session first, then
submit turns against that stored session id.

## Turn Semantics

`start_turn` and `continue_turn` both target an existing session id and stream
typed lifecycle events until a final response arrives.

The final response is one of:

- `completed`
- `paused`

`paused` means Probe hit an approval gate and persisted one or more pending
approvals. The operator does not need to infer pause state from stderr text.

The first cut is honest about what it does not support yet:

- queued follow-up turns are not implemented
- a second turn request for the same active session returns `session_busy`
- `interrupt_turn` is in the contract, but the current runtime has no
  cooperative preemption path yet, so active sessions return `unsupported`

## Event Delivery Classes

The protocol marks every streamed event as either:

- `lossless`
- `best_effort`

Current policy:

- `lossless`
  - turn started
  - model request started
  - stream started and finished
  - time-to-first-token observed
  - tool requested, started, completed, refused, or paused
  - model request failed
  - assistant turn committed
  - pending approvals updated
- `best_effort`
  - assistant text deltas
  - assistant snapshot replacements
  - streamed tool-call assembly deltas

Clients should treat `best_effort` events as coalescible progress updates and
should not build correctness on receiving every single one. `lossless` events
are the durable lifecycle edges.

## Tool Loop Shape

The server protocol does not expose `probe-core::tools::ToolLoopConfig`
directly.

Instead it ships a serializable `ToolLoopRecipe` that carries:

- tool set
- tool choice
- round-trip bound
- approval policy
- optional oracle profile
- optional long-context profile

`probe-server` reconstructs the runtime-local `ToolLoopConfig` from that recipe.
That keeps the protocol stable even though runtime-local registries and
execution handlers are not serializable.

## Session Snapshots

`resume_session` and `inspect_session` return a `SessionSnapshot` with:

- current session metadata
- full stored transcript
- currently pending approvals

That gives a reattaching client enough state to rebuild the visible session
without linking against the filesystem session store directly.

## Example

Request:

```json
{"message_type":"request","request_id":"req-1","request":{"op":"initialize","client_name":"probe-cli","client_version":"0.1.0","protocol_version":1}}
```

Event:

```json
{"message_type":"event","request_id":"req-2","event":{"kind":"runtime_progress","delivery":"best_effort","event":{"kind":"assistant_delta","session_id":"sess_...","round_trip":1,"delta":"hello"}}}
```

Response:

```json
{"message_type":"response","request_id":"req-2","status":"ok","response":{"op":"continue_turn","status":"completed","session":{"id":"sess_..."},"assistant_text":"hello from probe-server","response_id":"chatcmpl_...","response_model":"tiny-qwen35","executed_tool_calls":0,"tool_results":[]}}
```

## Current Boundary

This stdio protocol is the canonical Phase 1 local server seam.

It is intentionally not yet:

- a detached local daemon
- a queueing runtime
- a remote Probe worker transport
- a multi-tenant or browser-facing API

Those layers only become worth adding after first-party clients use this same
contract end to end.
