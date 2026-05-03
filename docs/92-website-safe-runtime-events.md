# Website-Safe Runtime Events

Probe exposes a stable website-facing event contract so `openagents.com` and
Pylon adapters do not need to parse internal `RuntimeEvent` variants directly.

The canonical protocol types live in `probe_protocol::website_events`.

## Event Shape

Each event is a `ProbeWebsiteEvent`:

```json
{
  "schemaVersion": "probe.website_event.v1",
  "sequence": 1,
  "occurredAtMs": 1777777777000,
  "eventType": "run_started",
  "actor": {
    "kind": "probe",
    "id": "sess_..."
  },
  "source": {
    "kind": "runtime",
    "id": "turn-0"
  },
  "correlation": {
    "requestId": "website-request-id",
    "workspace": "openagents.com",
    "conversationId": "conversation-id",
    "runId": "website-run-id",
    "scheduleId": "optional-schedule-id",
    "wakeId": "optional-wake-id",
    "scheduledRunId": "optional-scheduled-run-id",
    "probeSessionId": "sess_...",
    "probeTurnId": "turn-0"
  },
  "artifactRefs": [],
  "payload": {}
}
```

Sequences are assigned by the exporter/adapter. Payloads are JSON objects with
bounded, redacted summaries and stable hashes for sensitive or large values.

## Event Types

The first stable event type set is:

- `run_started`
- `text_delta`
- `tool_call_started`
- `tool_call_completed`
- `approval_requested`
- `approval_resolved`
- `child_session_started`
- `child_session_updated`
- `artifact_ref`
- `runtime_progress`
- `run_completed`
- `run_failed`
- `run_cancelled`

## Artifact Refs

Artifact refs avoid raw local paths. Use durable Probe refs and stable digests:

```json
{
  "kind": "transcript",
  "resourceRef": "probe://sessions/sess_.../transcript",
  "stableDigest": "sha256-hex",
  "label": "Probe transcript"
}
```

Supported artifact kinds:

- `transcript`
- `retained_session_summary`
- `accepted_patch_summary`
- `verification_pack`
- `other`

## Redaction Posture

Website-safe events must not include:

- model-provider keys
- bearer tokens or refresh/access tokens
- assignment nonces
- raw local filesystem paths
- raw tool arguments or raw tool output blobs

The core mapper emits previews plus stable hashes for prompts, assistant text,
errors, tool arguments, and tool output. Tool events expose tool name, call id,
risk class, policy decision, approval state, counts, and redacted
`argumentsSummary` / `outputSummary` objects rather than raw arguments or raw
outputs.

Pending approvals returned by `probe-server` are also API-safe: their
`arguments` field is replaced with the redacted summary before the approval is
sent over stdio, detached-session watch events, or session snapshots. The local
Probe session store still keeps raw arguments so approval resume can execute
the exact originally requested call.

## Core Mapper

Use `probe_core::website_events::runtime_event_to_website_event` to convert
runtime events into this contract. Additional helpers create:

- `approval_resolved_event`
- `child_session_event`
- `artifact_ref_event`
- `run_cancelled_event`
- `transcript_artifact_ref`
- `summary_artifact_ref`
- `verification_pack_artifact_ref`

The signed admin-chat bridge can attach this event stream after accepting a
session, and later bridge layers can reuse the same contract for live runtime
deltas, approvals, child sessions, and summary artifacts.
