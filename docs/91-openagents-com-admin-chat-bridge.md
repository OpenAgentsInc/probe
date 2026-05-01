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

## Next Implementation Step

The next non-fake bridge should be a signed internal transport owned by Probe's
server/daemon layer. It should expose this same protocol over a private local
socket, Tailnet-only HTTP, or Forge-managed worker attachment, then map provider
mode to the existing Probe backend profiles and Codex subscription routing.
