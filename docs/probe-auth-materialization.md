# Probe Per-Run Auth Materialization

Date: 2026-06-07

Status: implemented contract slice for Probe issue #159.

## Contract

Probe materializes ChatGPT/Codex auth only after resolving an Omega grant and
only inside the current run context. Long-lived provider account material stays
behind Omega secret refs.

The materializer accepts:

- an `OmegaResolvedAuthGrant`
- a brokered secret payload whose `providerSecretRef` matches the grant
- a run-scoped home directory

It supports:

- env materialization through `PROBE_CHATGPT_AUTH_CONTENT`
- file materialization through a relative path inside the run home
- redacted materialized/scrubbed receipts
- scrub-on-closeout
- Effect bracket cleanup for failing runs

## Safety Rules

- Materialization paths must be relative.
- Materialization paths may not escape the run home.
- Receipts include refs, target kind, timestamps, and `contentRedacted: true`.
- Receipts never include raw auth content.
- Failure paths still scrub file materialization.

## Tests

`packages/runtime/tests/materializer.test.ts` covers:

- fake ChatGPT auth materialized into a per-run file
- env materialization
- no-provider smoke checks
- scrub after closeout
- secret-ref mismatch rejection
- path escape rejection
- cleanup after simulated run failure
