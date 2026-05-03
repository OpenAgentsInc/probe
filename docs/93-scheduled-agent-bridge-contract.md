# Scheduled-Agent Bridge Contract

This is the stable Probe-first contract for website/Pylon scheduled agents.
`openagents.com` may schedule the work, own admin authorization, and persist
Laravel rows, but Probe owns the coding runtime boundary: sessions, turns,
tools, approvals, transcripts, artifacts, and backend routing.

Codex is a backend that Probe can select. It is not the website scheduler
contract.

The canonical Rust types live in
`probe_protocol::scheduled_bridge`. The website-safe event stream is
`probe_protocol::website_events`.

## Transport And Auth

The signed request envelope is
`ScheduledAgentBridgeSignedRequest`:

```json
{
  "auth": {
    "keyId": "openagents.com",
    "issuedAtMs": 1777777776000,
    "expiresAtMs": 1777777836000,
    "nonce": "nonce-success-100",
    "signature": "sha256=hex-hmac-sha256"
  },
  "request": {}
}
```

The signature context string is:

```text
probe-scheduled-agent-bridge-v1
```

The HMAC payload must include, in this order:

- signature context string
- `keyId`
- `issuedAtMs`
- `expiresAtMs`
- `nonce`
- canonical request JSON

Probe must reject invalid signatures, expired requests, requests issued too far
in the future, replayed nonces, missing required ids, unknown backend profiles,
and idempotency conflicts. Accepted nonces should be retained until expiration.

Do not send model-provider secrets, bearer tokens, refresh tokens, local file
paths, or raw provider account material through this bridge.

## Request JSON

Each request has schema version `probe.scheduled_agent_bridge.v1`:

```json
{
  "schemaVersion": "probe.scheduled_agent_bridge.v1",
  "requestId": "req-sched-success-100",
  "workspace": "openagents.com",
  "actor": {
    "webUserId": 42,
    "email": "admin@example.com",
    "role": "admin"
  },
  "conversation": {
    "conversationId": "conv-admin-100",
    "threadRef": "openagents-chat-thread-100"
  },
  "run": {
    "runId": "run-web-100",
    "scheduledRunId": "sched-run-100"
  },
  "schedule": {
    "scheduleId": "schedule-evolve-pylon-training",
    "name": "Evolve pylon training",
    "regularity": {
      "kind": "interval",
      "everySeconds": 7200,
      "timezone": "UTC"
    }
  },
  "wake": {
    "wakeId": "wake-100",
    "dueAtMs": 1777777777000,
    "attempt": 1
  },
  "orchestrationJob": {
    "orchestrationJobId": "orch-100",
    "queue": "scheduled-agents",
    "attempt": 1
  },
  "goal": {
    "masterGoal": "Evolve the pylon training code.",
    "phaseGoal": "Inspect current state and prepare the next safe patch."
  },
  "context": {
    "workspaceRef": "openagents://workspace/main",
    "repository": "OpenAgentsInc/openagents",
    "issueRefs": [
      "https://github.com/OpenAgentsInc/openagents/issues/4476"
    ],
    "deviceRefs": [
      "pylon://imac-pro-bertha"
    ],
    "memoryRefs": [
      "probe://memory/schedules/schedule-evolve-pylon-training"
    ],
    "stateSnapshotRefs": [],
    "instructions": [
      "Use Probe as the runtime boundary."
    ]
  },
  "backend": {
    "key": "probe-codex",
    "family": "codex",
    "profile": "openai-codex-subscription",
    "model": "gpt-5.4",
    "mode": "probe_backend",
    "accountRef": "probe://auth/openai-codex/default",
    "label": "Codex through Probe"
  },
  "toolPolicy": {
    "mode": "scheduled_agent",
    "allowedTools": [
      "read",
      "patch",
      "shell"
    ],
    "approvalRequired": true,
    "approvalMode": "admin_control_api"
  },
  "idempotencyKey": "sched-run-100:start",
  "metadata": {}
}
```

Website examples:

- resolve open admin-created issues every hour:
  `schedule.regularity.everySeconds=3600`,
  `goal.masterGoal="Resolve any open issues created by the admin."`,
  `backend.family="codex"` if Probe should use Codex for that phase
- evolve Pylon training every two hours:
  `schedule.regularity.everySeconds=7200`,
  `context.repository="OpenAgentsInc/openagents"`,
  `goal.masterGoal="Evolve the pylon training code."`

Pylon examples:

- distribute work to devices:
  `goal.masterGoal="Distribute work to pylons."`,
  `context.deviceRefs=["pylon://imac-pro-bertha"]`
- evolve device runtime:
  `goal.masterGoal="Evolve what is running on people's devices."`,
  `context.stateSnapshotRefs` points at website-safe acquaintance snapshots

## Response JSON

Accepted or terminal runtime responses use
`ScheduledAgentBridgeAcceptedResponse`:

```json
{
  "schemaVersion": "probe.scheduled_agent_bridge.v1",
  "requestId": "req-sched-success-100",
  "runId": "run-web-100",
  "scheduledRunId": "sched-run-100",
  "probeSessionId": "sess-success-100",
  "probeTurnId": "turn-success-100",
  "status": "completed",
  "backend": {
    "key": "probe-codex",
    "family": "codex",
    "profile": "openai-codex-subscription",
    "model": "gpt-5.4",
    "mode": "probe_backend",
    "accountRef": "probe://auth/openai-codex/default",
    "label": "Codex through Probe"
  },
  "transcriptRef": "probe://sessions/sess-success-100/transcript",
  "correlation": {
    "requestId": "req-sched-success-100",
    "workspace": "openagents.com",
    "conversationId": "conv-admin-100",
    "runId": "run-web-100",
    "scheduleId": "schedule-evolve-pylon-training",
    "wakeId": "wake-100",
    "scheduledRunId": "sched-run-100",
    "orchestrationJobId": "orch-100"
  },
  "diagnostics": {
    "redaction": "website_safe"
  }
}
```

Stable statuses:

- `accepted`
- `running`
- `approval_required`
- `completed`
- `cancelled`
- `failed`

## Event Schema

Use `ProbeWebsiteEventBatch` with schema version `probe.website_event.v1`.
Events must include correlation ids for request, conversation, run, schedule,
wake, scheduled run, Probe session, and Probe turn. The stable event type set
is documented in `docs/92-website-safe-runtime-events.md`.

Scheduled-agent adapters must persist these events append-only and treat
`sequence` as ordered within the Probe bridge response or stream.

## Approval Schema

Approvals use `ScheduledAgentBridgeApproval` and the matching
`approval_requested` website event:

```json
{
  "approvalId": "approval-200",
  "status": "pending",
  "actionRef": "probe://sessions/sess-approval-200/approvals/approval-200",
  "riskClass": "write",
  "toolName": "shell",
  "callId": "call-shell-200",
  "summary": "Run the focused test command before applying the patch.",
  "requestedAtMs": 1777778879000,
  "payloadSummary": {
    "argumentsHash": "sha256-200",
    "argumentsPreview": "php artisan test --compact tests/Feature/ExampleTest.php"
  }
}
```

Stable approval statuses:

- `pending`
- `approved`
- `rejected`
- `cancelled`

Admin control APIs should approve, reject, inspect, or cancel by stable ids
such as `approvalId`, `probeTurnId`, `scheduledRunId`, and `actionRef`.

## Artifact Refs

Artifact refs use `ProbeWebsiteArtifactRef`; they are durable refs, not raw
local paths:

```json
{
  "kind": "retained_session_summary",
  "resourceRef": "probe://sessions/sess-success-100/artifacts/retained_session_summary_v1.json",
  "stableDigest": "sha256-0000000000000000000000000000000000000000000000000000000000000101",
  "label": "Retained session summary",
  "updatedAtMs": 1777777779000
}
```

Persist transcript, retained-session summary, accepted-patch summary,
verification-pack, child-session, and acquaintance snapshot refs. Do not
persist raw transcript bodies into website scheduler tables unless a separate
redaction and retention policy explicitly authorizes that.

## Error Codes

Errors use `ScheduledAgentBridgeErrorResponse`:

```json
{
  "schemaVersion": "probe.scheduled_agent_bridge.v1",
  "requestId": "req-sched-failure-500",
  "error": {
    "code": "runtime.backend_failed",
    "message": "Probe could not start the selected backend profile.",
    "category": "runtime",
    "retryable": true,
    "diagnostics": {
      "backendProfile": "openai-codex-subscription",
      "redaction": "website_safe"
    }
  }
}
```

Stable error code families:

- `auth.invalid_signature`
- `auth.expired`
- `auth.replay`
- `request.invalid`
- `request.idempotency_conflict`
- `backend.unavailable`
- `runtime.backend_failed`
- `runtime.cancelled`
- `runtime.failed`

Diagnostic fields are safe for debugging but are not dispatch keys. Dispatch on
`error.code`, `retryable`, and persisted correlation ids.

## Idempotency

`requestId` identifies the bridge request. `idempotencyKey` identifies the
runtime-start intent. Retrying the same start with the same idempotency key
should return the existing accepted response or current terminal state. Reusing
the same key for a different schedule, wake, scheduled run, or backend must
fail with `request.idempotency_conflict`.

Admin control mutations also need idempotency keys, but those live on the
website admin Sanctum API. Probe observes the resulting control action refs and
runtime events.

## Stable Versus Diagnostic Fields

Stable fields:

- `schemaVersion`
- `requestId`
- `workspace`
- all ids under `conversation`, `run`, `schedule`, `wake`, and
  `orchestrationJob`
- `goal.masterGoal`
- `goal.phaseGoal`
- `backend.key`
- `backend.family`
- `backend.profile`
- `backend.model`
- `backend.mode`
- `toolPolicy.mode`
- `toolPolicy.approvalRequired`
- `idempotencyKey`
- response `status`
- `probeSessionId`
- `probeTurnId`
- `transcriptRef`
- event `sequence`
- event `eventType`
- event `correlation`
- approval `approvalId`
- approval `status`
- error `code`
- error `retryable`

Diagnostic-only fields:

- human labels
- previews
- counts
- hashes that summarize non-contract payloads
- backend display labels
- `metadata`
- `diagnostics`

## Fixtures

Mirror these JSON fixtures in `openagents.com` adapter tests:

- `crates/probe-protocol/tests/fixtures/scheduled_agent_bridge/success.json`
- `crates/probe-protocol/tests/fixtures/scheduled_agent_bridge/approval_pause.json`
- `crates/probe-protocol/tests/fixtures/scheduled_agent_bridge/child_session.json`
- `crates/probe-protocol/tests/fixtures/scheduled_agent_bridge/cancellation.json`
- `crates/probe-protocol/tests/fixtures/scheduled_agent_bridge/runtime_failure.json`
- `crates/probe-protocol/tests/fixtures/scheduled_agent_bridge/auth_failure.json`

The contract test is:

```bash
cargo test -p probe-protocol --test scheduled_bridge_contract -- --nocapture
```

