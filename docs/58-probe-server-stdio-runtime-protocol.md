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

Phase 2 now reuses this same typed contract over the local `probe-daemon`
socket transport. The message shapes stay the same; only the transport and the
reported runtime capabilities change.

Phase 3 now also reuses this contract over a hosted TCP JSONL transport. That
adds a remote-facing control-plane lane without inventing a second request or
response schema.

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

The shared protocol currently ships these typed operations:

- `initialize`
- `start_session`
- `resume_session`
- `list_sessions`
- `list_detached_sessions`
- `read_detached_session_log`
- `inspect_session`
- `inspect_detached_session`
- `watch_detached_session`
- `start_turn`
- `continue_turn`
- `queue_turn`
- `inspect_session_turns`
- `interrupt_turn`
- `cancel_queued_turn`
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

Queued follow-up work is now explicit.

- `queue_turn` accepts a follow-up prompt even when another turn is already
  running
- the accepted record persists queued-turn state as `queued`, `running`,
  `completed`, `failed`, `cancelled`, or `timed_out`
- every queued turn carries per-turn author metadata plus queue position
- detached supervision now also persists per-turn progress timestamps and
  execution deadlines for watchdog decisions
- `inspect_session_turns` returns the current active turn, the queued follow-up
  list, and recent terminal turns for a session
- `cancel_queued_turn` can remove queued work before it starts and appends an
  honest note into the transcript

The direct turn and queue contract is still honest about its limits:

- direct `start_turn` and `continue_turn` requests remain single-turn request or
  response flows, so a second direct turn request still returns `session_busy`
- stdio transport keeps queued background progress on explicit inspect calls
- daemon transport adds detached log replay and watch subscriptions on top of
  the same request and response model
- `interrupt_turn` can cancel an approval-paused active turn, reject its
  pending approvals, and then let the queue continue
- `interrupt_turn` still cannot preempt an in-flight model call that is inside
  the current runtime execution path, so those requests return `unsupported`

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

Detached daemon watch now reuses the same distinction through:

- `DetachedSessionEventTruth::Authoritative`
- `DetachedSessionEventTruth::BestEffort`

That mapping keeps detached logs and subscriptions honest about what can be
used as lifecycle truth versus transient progress.

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
- explicit runtime-owner metadata when Probe knows whether the session is owned
  by a foreground child, local daemon, or hosted control plane
- typed mounted knowledge-pack and eval-pack refs when the session started with
  explicit Forge-facing context mounts
- typed workspace provenance when Probe knows how the session booted, including
  boot mode, prepared baseline status, snapshot refs, execution-host metadata,
  and any explicit fallback note
- typed branch posture when the cwd is inside a git repo
- typed forge-agnostic delivery posture derived from that branch state
- typed child-session summaries when the session has delegated children,
  including initiator identity, delegation purpose, and terminal closure
  summaries when the child finishes
- full stored transcript
- currently pending approvals

That gives a reattaching client enough state to rebuild the visible session
without linking against the filesystem session store directly.

`start_session` now also accepts `mounted_refs` for typed `knowledge_pack` and
`eval_pack` context mounts. Those refs are persisted on the session itself and
also projected through daemon detached-session summaries. Unsupported mount
kinds are refused explicitly with protocol code
`unsupported_session_mount_kind`.

Queued-turn lifecycle state intentionally lives beside that snapshot instead of
inside it. `SessionSnapshot` remains the transcript plus delegated-child,
transcript, and approval view, while `inspect_session_turns` is the typed queue
and control view.

`spawn_child_session` is now part of the same contract. It lets a client create
a child session with an explicit parent link, conservative same-repo guardrails,
and bounded delegation depth or count. Detached daemon transports also emit
`child_session_updated` events back onto the parent session log when the child
is created or its detached status changes, and those child summaries now carry
runtime-owned initiator, purpose, and terminal closure fields instead of
forcing hosts to infer delegated ownership from transcript prose.

Detached daemon transports now also emit `workspace_state_updated` when Probe
can resolve git-owned branch and delivery posture for the session workspace.
That keeps branch name, upstream tracking, divergence, and compare posture on
the typed server seam instead of making clients scrape `git status` or ad hoc
shell output.

That same event now also carries the typed workspace-provenance payload when it
is available, so detached consumers can receive boot mode, baseline, snapshot,
and execution-host changes on the same event stream.

## Example

Request:

```json
{"message_type":"request","request_id":"req-1","request":{"op":"initialize","client_name":"probe-cli","client_version":"0.1.0","protocol_version":11}}
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

- a multi-tenant or browser-facing API

Detached local daemon transport now exists in Phase 2, but it is still the same
runtime protocol rather than a second API surface.

Hosted TCP transport now exists as the narrow first remote control-plane lane in
Phase 3. It gives Probe:

- a TCP JSONL request or response path for remote Rust consumers
- explicit hosted runtime-owner identity in session and detached-session state
- typed hosted workspace provenance in session and detached-session state,
  including prepared baseline status, snapshot refs, execution-host metadata,
  and explicit fresh-start fallback notes
- the same queue, inspect, attach, and shutdown semantics as local transports

It still does not claim:

- remote worker scheduling
- browser-facing HTTP or SSE APIs
- multi-tenant auth or policy surfaces
- actual prepared-workspace pools or snapshot restore execution

The main detached-only additions on top of this contract are documented in:

- `61-probe-daemon-and-local-socket-transport.md`
- `62-daemon-owned-detached-session-registry.md`
- `63-detached-session-watch-and-log-subscriptions.md`
- `65-detached-watchdog-and-timeout-policy.md`
- `69-typed-session-mount-contract.md`
