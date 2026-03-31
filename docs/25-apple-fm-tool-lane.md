# Apple FM Tool Lane

## Purpose

Probe now supports session-backed Apple FM coding turns without moving the
controller loop into Psionic and without discarding Probe's own transcript and
approval model.

## What Landed

Probe now projects the built-in coding tools into Apple FM session tools:

- `read_file`
- `list_files`
- `code_search`
- `shell`
- `apply_patch`
- `consult_oracle`
- `analyze_repository`

The Apple FM provider now creates a session with:

- Apple FM tool definitions derived from the same Probe tool registry
- a Probe-owned local callback URL
- the current session instructions and reconstructed transcript snapshot

The bridge then calls back into Probe for each Apple FM tool use instead of
forcing the OpenAI `tool_calls` wire shape onto the backend.

The provider now also hardens two backend-specific compatibility edges:

- Probe normalizes the root tool-argument schema for Apple FM before session
  registration so strict bridge/runtime schema decoding does not fail on
  missing root `title`, `x-order`, or `required` fields
- Probe retries `create_session` once without transcript restore when Apple FM
  returns a typed `invalid_request` / `Invalid JSON` error for the replay
  payload

## Controller Truth

Probe transcript storage remains authoritative.

For Apple FM coding turns, Probe rebuilds an Apple transcript snapshot from its
own persisted items on each turn:

- user and assistant messages replay as Apple text entries
- stored tool calls replay as assistant entries with `toolCalls`
- stored tool results replay as Apple `tool` entries
- Probe-local notes remain controller-only and are not replayed into Apple FM

Resume therefore stays honest without depending on a long-lived bridge session
id. Probe restores the Apple FM session view from Probe-owned transcript state
before the next turn starts.

## Policy And Receipts

Apple FM tool callbacks now run through the same Probe approval and execution
policy used by the OpenAI/Qwen lane.

That means:

- read-only tools still auto-allow
- write, network, and destructive classes still follow the active approval
  policy
- refused Apple FM tool calls persist as first-class Probe tool-result records
  and return a structured denial payload to the model
- paused Apple FM tool calls persist as first-class Probe tool-result records
  and stop the turn with `ToolApprovalPending`

The Probe transcript still records tool calls and tool results in the existing
Probe item model rather than replacing it with raw Apple transcript storage.

## Explicit Bound

Apple FM can invoke several callback tools inside one backend response.

Probe keeps that bounded explicitly by reusing the current
`max_model_round_trips` controller bound as the maximum Apple FM callback count
allowed inside one Probe turn. If the bridge exceeds that bound, Probe stops
the turn instead of hiding an unbounded callback loop under the backend.

## Remaining Differences

Apple FM still differs from the OpenAI/Qwen lane in a few deliberate ways:

- Apple FM sessions are attach-only; Probe still only supervises managed launch
  for the OpenAI-compatible server lane
- Apple FM callback sequencing is session-native; Probe does not claim explicit
  OpenAI-style parallel tool-call control for this lane
- Probe rebuilds Apple session continuity from Probe transcript state each turn
  instead of persisting backend session ids as controller truth

## Validation

Probe now has retained Rust coverage for the Apple FM tool lane across:

- successful read-only tool execution
- explicit refusal receipts
- approval-pause receipts
- resume replay through reconstructed Apple transcript state
- tool-schema normalization for Apple FM session tools
- one-shot transcriptless retry on narrow invalid-JSON restore failures
