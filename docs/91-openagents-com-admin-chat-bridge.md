# OpenAgents.com Admin Chat Bridge

This document defines the first internal Probe contract for the
`openagents.com` admin-only AI chat surface. It is not a public internet API and
it does not move WorkOS authorization into Probe.

## Ownership

- `openagents.com` owns WorkOS login, admin gating, conversation rows, website
  run rows, and React UI.
- Probe owns runtime execution, backend/provider routing, tool policy
  interpretation, approvals, runtime transcripts, and redacted diagnostics.
- The first safe ChatGPT/Codex subscription path is Probe-owned service or
  operator account auth through the existing `openai-codex-subscription`
  backend profile.
- Per-user OpenAI Platform API-key mode can be brokered by Laravel, but Probe
  should receive only an internal execution credential or short-lived bearer in
  a future signed transport. Raw refresh tokens should not be passed through the
  bridge.

## Request Shape

The canonical Rust type is
`probe_protocol::admin_chat::AdminChatBridgeRequest`.

```json
{
  "requestId": "uuid",
  "workspace": "openagents.com",
  "webUserId": 123,
  "webUserEmail": "admin@example.com",
  "conversationId": "conversation-id",
  "runId": "run-id",
  "prompt": "user text",
  "messages": [],
  "provider": {
    "key": "openai",
    "mode": "chatgpt_subscription|api_usage|service_api_key|fake",
    "accountRef": "opaque-provider-account-ref",
    "label": "redacted display label"
  },
  "toolPolicy": {
    "mode": "admin_chat",
    "allowedTools": [],
    "approvalRequired": true
  },
  "metadata": {}
}
```

The non-fake internal transport wraps that request in
`probe_protocol::admin_chat::AdminChatBridgeSignedRequest`:

```json
{
  "auth": {
    "keyId": "openagents.com",
    "issuedAtMs": 1777777777000,
    "expiresAtMs": 1777777837000,
    "nonce": "uuid-or-random-128-bit-value",
    "signature": "sha256=hex-hmac-sha256"
  },
  "request": {
    "requestId": "uuid",
    "workspace": "openagents.com",
    "webUserId": 123,
    "webUserEmail": "admin@example.com",
    "conversationId": "conversation-id",
    "runId": "run-id",
    "prompt": "user text",
    "messages": [],
    "provider": {
      "key": "openai",
      "mode": "service_api_key",
      "accountRef": "opaque-provider-account-ref",
      "label": "service fallback"
    },
    "toolPolicy": {
      "mode": "admin_chat",
      "allowedTools": [],
      "approvalRequired": true
    },
    "metadata": {
      "backendProfile": "openai-codex-subscription",
      "scheduleId": "optional-schedule-id",
      "wakeId": "optional-wake-id",
      "scheduledRunId": "optional-scheduled-run-id"
    }
  }
}
```

The signature payload is stable text:

```text
probe-admin-chat-bridge-v1
<keyId>
<issuedAtMs>
<expiresAtMs>
<nonce>
<canonical request JSON>
```

Probe verifies `signature` as HMAC-SHA256 with the shared secret from
`PROBE_ADMIN_CHAT_BRIDGE_SECRET` or a caller-selected secret env var. The secret
must be at least 32 bytes. Probe rejects expired requests, requests issued too
far in the future, invalid signatures, empty required fields, `fake` provider
mode on the signed path, unknown backend profiles, and replayed nonces. Accepted
nonces are persisted under `PROBE_HOME/admin-chat-bridge/nonces.json` until
their expiry time.

The initial admin-chat policy is default-deny: no filesystem, shell, network,
or repository tools are enabled unless Laravel sends an explicit policy and a
later bridge implementation maps that policy into Probe's approval model.

## Event Stream

The canonical Rust type is `probe_protocol::admin_chat::AdminChatBridgeEvent`.
The first local smoke path renders Server-Sent Events:

```text
data: {"type":"run_started",...}

data: {"type":"model_stream_started",...}

data: {"type":"text_delta",...}

data: {"type":"usage_limits_snapshot",...}

data: {"type":"run_completed",...}

data: [DONE]
```

Laravel can map these directly onto website persistence:

- `run_started` -> create/update website run
- `model_stream_started` -> persist provider/model metadata
- `text_delta` -> append assistant stream text
- `tool_call_started`, `tool_call_result`, `approval_requested` -> website
  run events when tools are later enabled
- `usage_limits_snapshot` -> usage/limit metadata
- `run_completed` / `run_failed` -> terminal website run state

The signed non-fake path currently accepts the request into a real Probe session
and appends the first Probe turn with the web prompt plus a bridge-acceptance
note. Its `run_completed` status is `accepted`, not model-completed. The event
diagnostics and JSON response include:

- `probeSessionId`
- `probeTurnId`
- selected backend family/profile/model
- transcript ref
- request, conversation, run, and optional schedule/wake correlation ids

Provider streaming, approval mapping, and child-session event expansion are the
next bridge layer and should build on this signed session/turn contract plus
the website-safe event schema in
`docs/92-website-safe-runtime-events.md`.

## Provider Modes

### `chatgpt_subscription`

Deferred for the hosted website bridge until the internal transport and account
selection rules are finalized. Probe already owns Codex subscription auth at
`PROBE_HOME/auth/openai-codex.json`; the website should call Probe through this
bridge instead of copying Codex/OpenCode auth caches or passing refresh tokens
around.

### `api_usage`

Supported as a contract mode. The first secure implementation should use a
Laravel token broker or internal execution credential so Probe does not persist
raw per-user API keys unless explicitly designed.

### `service_api_key`

Supported as the first practical service fallback. Probe can route through
`PROBE_OPENAI_API_KEY` or an injected worker secret and return only source
labels/metadata, never the key value.

### `fake`

Implemented now for tests and local smoke runs without OpenAI credentials.

## Local Smoke

Emit a complete fake SSE cycle:

```bash
cargo run -p probe-cli -- admin-chat-bridge fake \
  --prompt "Summarize the bridge contract."
```

Or provide a full request JSON:

```bash
cargo run -p probe-cli -- admin-chat-bridge fake \
  --request /path/to/admin-chat-request.json
```

The fake path deliberately does not echo request metadata, so secret-shaped
metadata in the request cannot appear in the stream.

## Signed Local Transport

Accept a signed non-fake request and emit SSE:

```bash
PROBE_ADMIN_CHAT_BRIDGE_SECRET='32-byte-minimum-shared-secret-value' \
  cargo run -p probe-cli -- admin-chat-bridge signed \
    --request /path/to/signed-admin-chat-request.json \
    --probe-home ~/.probe \
    --cwd /path/to/workspace
```

For adapter tests, emit a single JSON document containing the accepted response
and event array:

```bash
PROBE_ADMIN_CHAT_BRIDGE_SECRET='32-byte-minimum-shared-secret-value' \
  cargo run -p probe-cli -- admin-chat-bridge signed \
    --request /path/to/signed-admin-chat-request.json \
    --format json
```

Use `--secret-env OPENAGENTS_PROBE_BRIDGE_SECRET` if Laravel or Pylon injects
the shared secret under a different non-secret env-var name. Do not pass the raw
secret as a command-line argument and do not log it.

## Next Implementation Step

The next bridge layer should execute or attach the accepted session through the
Probe runtime, stream runtime deltas, expose approval ids, and surface
child-session/artifact refs over the same signed transport. A later server or
daemon endpoint can reuse this envelope over a private local socket,
Tailnet-only HTTP, or Forge-managed worker attachment.
