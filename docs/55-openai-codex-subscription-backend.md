# OpenAI Codex Subscription Backend

This doc records the first dedicated Probe transport for ChatGPT/Codex
subscription-backed inference.

The auth flow lives in [54-openai-codex-subscription-auth.md](54-openai-codex-subscription-auth.md).
This doc covers the backend and request path that consumes that stored auth
state.

## Scope

Probe now ships a first-class backend profile named
`openai-codex-subscription`.

That profile is separate from the existing
`BackendKind::OpenAiChatCompletions` path because the subscription lane does
not target a local `/v1/chat/completions` server and does not rely on
`PROBE_OPENAI_API_KEY`.

## Canonical Profile

- profile name: `openai-codex-subscription`
- backend kind: `openai_codex_subscription`
- base URL: `https://chatgpt.com/backend-api/codex`
- request endpoint: `https://chatgpt.com/backend-api/codex/responses`
- default model: `gpt-5.4`
- reasoning level: `backend_default`
- attach mode: `attach`
- saved backend config path: `PROBE_HOME/server/openai-codex-subscription.json`

The current Rust implementation for that profile lives in:

- `crates/probe-protocol/src/backend.rs`
- `crates/probe-core/src/backend_profiles.rs`
- `crates/probe-core/src/server_control.rs`

## Request Construction

Runtime-owned request construction now happens in:

- `crates/probe-core/src/provider.rs`
- `crates/probe-core/src/runtime.rs`
- `crates/probe-provider-openai/src/lib.rs`

The Codex transport differs from the local OpenAI-compatible path in four ways:

1. It loads versioned multi-account OAuth state from
   `PROBE_HOME/auth/openai-codex.json`.
2. It refreshes saved accounts independently when they are expired.
3. It refreshes or reuses cached usage snapshots from
   `GET /backend-api/wham/usage`.
4. It ranks non-expired accounts by remaining headroom and picks the best one.
5. It rewrites the request target to `/backend-api/codex/responses`.
6. It injects Codex-specific headers.

Current header set:

- `authorization: Bearer <access token>`
- `ChatGPT-Account-Id: <account id>` when present in stored auth state
- `originator: probe`
- `User-Agent: probe/<version> (<os>; <arch>)`
- `session_id: <probe session id>` when the runtime has a session id

If subscription auth is missing, unusable, or rate-limited and
`PROBE_OPENAI_API_KEY` is present, Probe now rewrites the target to the public
Responses API at:

- `https://api.openai.com/v1/responses`

In that fallback mode Probe keeps the same model and Responses body shape, but:

- auth comes from `PROBE_OPENAI_API_KEY`
- `ChatGPT-Account-Id` is omitted
- the fallback can also be used when no viable saved subscription account is
  present

Current hosted-body contract:

- Probe sends the hosted request as a Responses-style payload, not a
  chat-completions payload.
- Probe lifts system prompt text into the top-level `instructions` field.
- Probe serializes replayed user or assistant text into `input` items using
  `input_text` and `output_text`.
- Probe serializes tool replay as `function_call` and `function_call_output`
  items so Probe-owned tool loops can resume cleanly.
- The hosted ChatGPT Codex endpoint currently rejects `max_output_tokens`, so
  Probe omits that field on the subscription lane even though it remains valid
  for the public OpenAI Responses API.
- The hosted streaming path can omit `Content-Type`, so Probe accepts
  headerless SSE on this lane instead of hard-failing on the missing header.

## Model Gate

Probe now rejects obviously unsupported subscription models before sending the
request.

Allowed set:

- any model id containing `codex`
- `gpt-5.1-codex`
- `gpt-5.1-codex-max`
- `gpt-5.1-codex-mini`
- `gpt-5.2`
- `gpt-5.4`
- `gpt-5.4-mini`

The explicit allowlist is implemented in
`crates/probe-provider-openai/src/lib.rs` and mirrors the retained
`opencode`-audit reference set plus the broader `contains("codex")` guard.

## Reproduction

Prerequisite:

```bash
cargo run -p probe-cli -- codex login --method headless
```

Use `--method browser` for a local interactive machine. Use `--method headless`
for worker machines and SSH-only hosts.

The first checked-in worker deploy lane uses that headless flow directly
through `scripts/deploy/forge-worker/03-run-headless-codex-login.sh`.

One-shot Codex turn:

```bash
cargo run -p probe-cli -- exec \
  --profile openai-codex-subscription \
  "Reply with the exact text: codex backend ready"
```

Interactive Codex session:

```bash
cargo run -p probe-cli -- chat --profile openai-codex-subscription
```

Inspect the stored auth record:

```bash
cargo run -p probe-cli -- codex status
```

Inspect the saved backend snapshot:

```bash
cat ~/.probe/server/openai-codex-subscription.json
```

Expected behavior:

- Probe resolves the canonical Codex profile.
- Probe uses `https://chatgpt.com/backend-api/codex/responses`.
- Probe refreshes expired auth state before sending the request.
- Probe refreshes cached usage snapshots and picks the account with the most
  remaining headroom.
- Probe rotates to the next saved account if the chosen account still returns
  `429`.
- Probe includes the subscription headers listed above.
- Probe sends `instructions` plus `input` instead of `messages`.
- Probe refuses unsupported non-Codex models before the HTTP call is made.
- If every saved subscription account is rate-limited, missing, or otherwise
  unusable and `PROBE_OPENAI_API_KEY` is exported, Probe falls back to
  `https://api.openai.com/v1/responses`.

## Contrast With The OpenAI-compatible Psionic Lane

The Codex subscription lane still does not require `PROBE_OPENAI_API_KEY` for
normal attached subscription usage.

The built-in OpenAI-compatible Psionic-backed profiles still require that env
var directly. Probe now also treats it as an optional fallback for the Codex
subscription lane when the saved subscription accounts are not usable. The CLI
autoloads that key from `.secrets/probe-openai.env` when Probe starts inside a
workspace tree containing that file.

## Tests

The main retained coverage for this backend lives in:

- `crates/probe-provider-openai/src/lib.rs`
- `crates/probe-provider-openai/tests/provider_suite.rs`
- `crates/probe-core/src/provider.rs`
- `crates/probe-core/src/backend_profiles.rs`
- `crates/probe-core/src/server_control.rs`

Validation commands:

```bash
cargo test -p probe-provider-openai
cargo test -p probe-core --lib
cargo test -p probe-cli --test cli_regressions
cargo test -p probe-tui
bash scripts/deploy/forge-worker/99-local-smoke.sh
```
