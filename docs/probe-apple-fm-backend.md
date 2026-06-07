# Probe Apple FM Backend

Date: 2026-06-07

Status: implemented contract, attach/status, plain-text smoke, and
assignment-routing slices for Probe issues #163 through #166.

## Contract

Apple Foundation Models is the first concrete backend family in the new Probe
Bun/Effect runtime. It is modeled as its own backend kind:

- kind: `apple_fm_bridge`
- profile: `apple-fm-local`
- model: `apple-foundation-model`
- default base URL: `http://127.0.0.1:11435`
- attach mode: `attach_existing`
- auth: `none`
- readiness path: `/health`
- stream mode: `snapshot`

The backend profile resolver preserves the old Probe/Psionic override order:

1. explicit assignment/profile override
2. `PROBE_APPLE_FM_BASE_URL`
3. `OPENAGENTS_APPLE_FM_BASE_URL`
4. default loopback URL

## Implemented Probe Surface

`packages/runtime/src/backends/apple-fm/contract.ts` defines Effect v4 schemas
for Apple FM health, unavailable reasons, chat messages, chat completion
requests/responses, usage truth, and snapshot stream events.

`packages/runtime/src/backends/apple-fm/receipts.ts` defines redacted
availability, failure, and transcript receipt helpers. Token usage is explicitly
`exact`, `estimated`, or `unknown`; Probe must not label approximate Apple FM
usage as exact.

`packages/runtime/src/backends/registry.ts` registers the first Apple FM local
profile and resolves base URL overrides.

`packages/runtime/src/backends/apple-fm/client.ts` implements the attach-only
readiness path. It checks `GET /health`, decodes typed availability, and returns
redacted availability receipts for ready, unavailable, unsupported, malformed,
and unreachable bridge states.

The same client implements the first inference path:

- `completePlainText(messages)`
- `smoke(prompt)`

Plain-text completion posts to `/v1/chat/completions`, normalizes the bridge
response into Probe's Apple FM contract, and emits redacted transcript receipts.
Usage truth is preserved as `exact`, `estimated`, or `unknown`; OpenAI-shaped
token counts without explicit truth are treated as `estimated`, not exact.

`packages/runtime/src/cli.ts` exposes:

- `probe apple-fm status [--base-url URL] [--profile apple-fm-local]`
- `probe apple-fm smoke [--base-url URL] [--profile apple-fm-local]
  [--prompt TEXT]`

The status command performs no inference. It exits with `0` only when live
health is ready, and exits nonzero with typed status output when the bridge is
unavailable, unsupported, unreachable, or malformed.

The smoke command runs readiness first. It sends one plain-text prompt only
after `requireReady()` succeeds, prints assistant text, reports usage truth, and
prints a redacted backend transcript or failure receipt.

## Assignment Routing

Probe assignments can now select Apple FM without provider account refs:

```json
{
  "backend": {
    "kind": "apple_fm_bridge",
    "profile": "apple-fm-local"
  }
}
```

`packages/runtime/src/runtime/backend-assignment.ts` implements the no-auth
assignment path. It requires runner capability
`probe.backend.apple_fm_bridge`, checks live Apple FM health, runs the same
plain-text client used by the CLI, and emits redacted backend
start/finish/failure events.

Apple FM assignments do not require ChatGPT accounts, OpenAI API keys, Omega
provider auth grants, or local auth materialization.

## Tests

`packages/runtime/src/backends/apple-fm/fake-server.test.ts` covers:

- Apple FM local profile resolution and env precedence
- CI-safe fake bridge health and completion response decoding
- redacted availability/transcript receipt behavior

`packages/runtime/tests/apple-fm-cli.test.ts` covers ready, unsupported, and
unreachable status output, smoke readiness gating, estimated usage
normalization, and typed completion failures without admitted Apple hardware.

`packages/runtime/tests/backend-assignment.test.ts` covers Apple FM assignment
routing, missing backend capability rejection, and non-ready live health
rejection with an availability receipt.

The fake bridge tests do not require admitted Apple hardware.
