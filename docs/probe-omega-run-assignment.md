# Probe/Omega Run Assignment And Grant Resolution

Date: 2026-06-07

Status: implemented contract slice for Probe issue #158.

## Assignment Shape

Probe runtime assignments may carry Omega provider auth references:

- `provider: "chatgpt_codex"`
- `providerAccountRef`
- `authGrantRef`
- `runnerSessionId`
- `assignmentId`
- optional `leaseRef`
- repo, goal, callback, and sandbox metadata

Assignments must carry refs and grants only. They must not include raw
ChatGPT/OAuth token material.

Assignments may also select a no-auth backend instead of a provider account.
The first implemented backend selection is:

```json
{
  "backend": {
    "kind": "apple_fm_bridge",
    "profile": "apple-fm-local"
  }
}
```

That Apple FM path uses local attach configuration and live health. It does not
use `providerAccountRef`, `authGrantRef`, ChatGPT account linking, or Omega
grant resolution.

## Grant Resolution

`packages/runtime/src/omega/grant-client.ts` implements an Effect-based Omega
grant resolver. It posts assignment refs to:

`/api/provider-accounts/chatgpt-codex/grants/resolve`

The resolved grant must match the assignment's provider account ref, grant ref,
and runner session. The grant must be unexpired and must include a Probe-shaped
materialization plan, not an OpenCode-shaped env hint.

Current Probe materialization plans use:

- `kind: "probe_chatgpt_auth"`
- `target.name: "PROBE_CHATGPT_AUTH_CONTENT"` for env materialization
- `homeIsolation: "per_run"`
- `scrubAfterCloseout: true`

Omega may return `status: "used"` after a successful one-time resolve. Probe
accepts that resolved response only when the payload includes the
Probe-compatible materialization plan.

## Tests

`packages/runtime/tests/grant-client.test.ts` covers:

- assignment decoding
- fake grant resolution
- provider-account mismatches
- expired grants
- already-used grant records without materialization
- OpenCode env name rejection
- unavailable Omega responses
