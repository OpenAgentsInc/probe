# Probe Apple FM Backend

Date: 2026-06-07

Status: implemented contract slice for Probe issue #163.

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

## Tests

`packages/runtime/src/backends/apple-fm/fake-server.test.ts` covers:

- Apple FM local profile resolution and env precedence
- CI-safe fake bridge health and completion response decoding
- redacted availability/transcript receipt behavior

The fake bridge tests do not require admitted Apple hardware.

