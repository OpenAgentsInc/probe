# Managed Runtime API

Probe now exposes `probe.managed_runtime.v1` for Laravel-owned managed agents.
This is the product control-plane contract above Probe runtime sessions. It is
not a browser chat bridge, and it does not make Codex the product boundary.

Canonical Rust types:

- `probe_protocol::managed_runtime`
- `probe_core::managed_runtime::ManagedRuntimeController`
- `RuntimeRequest::ManagedRuntime` on the existing JSONL server transport

## Ownership

Laravel owns durable product truth:

- managed agents and versions
- managed environments
- managed sessions and runs
- admin API/UI authorization
- work orders, outcomes, evidence, billing, and governance

Probe owns runtime truth:

- Probe session ids
- runtime status projection
- tool calls and tool results
- approval pause and resolution events
- transcript and artifact refs
- child-session hooks
- worker heartbeat events
- replayable append-only event history

Codex is only one possible Probe backend profile. Laravel should select a Probe
profile or capability key and let Probe route the actual backend.

## Transport

The managed API reuses the existing `probe-server` JSONL protocol:

```json
{
  "message_type": "request",
  "request_id": "req-1",
  "request": {
    "op": "managed_runtime",
    "request": {
      "op": "start_session",
      "schemaVersion": "probe.managed_runtime.v1"
    }
  }
}
```

The response is also wrapped under `managed_runtime`:

```json
{
  "message_type": "response",
  "request_id": "req-1",
  "status": "ok",
  "response": {
    "op": "managed_runtime",
    "response": {
      "op": "start_session",
      "schemaVersion": "probe.managed_runtime.v1"
    }
  }
}
```

This keeps Laravel on the same local daemon, hosted TCP, and future hosted
transport contract as first-party Probe clients.

## Operations

`ManagedRuntimeRequest` supports:

- `start_session`
- `resume_session`
- `interrupt_session`
- `cancel_session`
- `resolve_approval`
- `replay_events`
- `heartbeat`
- `record_child_session`

`start_session` creates a durable Probe session with profile, cwd,
system-prompt, harness, workspace-state, mounted-ref, optional initial prompt
metadata, and optional `ManagedEnvironmentConstraints` from the Laravel
environment record. It returns the Probe session id, transcript ref, current
status, and the first replayable events.

`resume_session` reconstructs current status from the persisted event log and
returns all events after a caller-provided sequence number.

`replay_events` returns the append-only event window for a Probe session.

`interrupt_session` and `cancel_session` append terminal control events to the
managed runtime log. Existing lower-level turn interrupt and queued-turn cancel
APIs remain available for direct turn-control mechanics.

`resolve_approval` appends an approval-resolution event. Tool replay and
execution continuation still belong to the normal Probe approval runtime.

`heartbeat` projects worker/session status back to Laravel without exposing
provider credentials. It can also carry the worker's current
`probe.managed_environment.v1` advertisement so Laravel can refresh scheduling
eligibility before dispatching more work.

`record_child_session` is the future multi-agent hook: parent and child Probe
session ids are explicit and correlated without requiring transcript prose.

## Event Log

Managed runtime events persist under each Probe session:

```text
<PROBE_HOME>/sessions/<session-id>/managed_runtime_events.jsonl
```

Each event is a `ManagedRuntimeEvent`:

```json
{
  "schemaVersion": "probe.managed_runtime.v1",
  "sequence": 1,
  "occurredAtMs": 1777777777000,
  "eventType": "session_started",
  "status": "running",
  "actor": {"kind": "laravel_admin", "id": "user-1"},
  "source": {"kind": "runtime", "id": "sess_..."},
  "session": {
    "probeSessionId": "sess_...",
    "managedSessionId": "managed-session-1"
  },
  "correlation": {
    "workspace": "openagents.com",
    "managedAgentId": "agent-1",
    "managedSessionId": "managed-session-1"
  },
  "artifactRefs": [],
  "payload": {"kind": "session_lifecycle"}
}
```

`sequence` is stable within one Probe session and is the replay cursor Laravel
should persist in `managed_session_events`.

## Payload Coverage

The payload enum covers the runtime facts Laravel needs to persist:

- session lifecycle
- turn lifecycle
- text deltas
- tool calls
- tool results
- custom tool results
- approvals, including redacted pending-approval argument summaries
- transcript refs
- artifact refs
- structured errors
- terminal states
- heartbeats
- child sessions
- status notes

Tool call, tool result, and approval events should use Probe's redacted
`argumentsSummary` and `outputSummary` fields for Laravel UI/history. Raw local
tool arguments stay in Probe's session store for approval resume and should not
be treated as the managed API display contract. Provider keys, bearer tokens,
refresh tokens, and local secret material must not be sent through this
contract.

## Restart Semantics

Probe reconstructs managed status by replaying the event log:

- last event status gives the status projection
- terminal statuses remain terminal after restart
- unresolved approval events produce `approval_paused`
- resolved approval events return the projection to `running`
- heartbeat events update the same status projection without mutating transcript
  truth

This means a hosted worker can restart or migrate as long as the same
`PROBE_HOME` session directory is available.

## Verification

Focused retained coverage:

```bash
cargo test -p probe-protocol -p probe-core managed_runtime -- --nocapture
cargo test -p probe-server \
  stdio_protocol_exposes_managed_runtime_start_replay_resume_and_cancel \
  -- --nocapture
```
