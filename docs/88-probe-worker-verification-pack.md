# Probe Worker Verification Pack

Issue `#128` adds a Forge-safe verification pack for hosted Probe workers.

## Goal

Forge needs a machine-readable proof that a Probe worker can be trusted for
health-agent and coding-agent assignments before the worker is allowed to
produce delivery evidence.

The verification pack is intentionally local and redacted. It does not require
a live Forge assignment, does not call a model provider, and does not print raw
model keys or Forge worker session material.

## Command

Run:

```shell
cargo run -p probe-cli -- forge verification-pack --pretty
```

The installed CLI form is:

```shell
probe forge verification-pack --pretty
```

The command emits a JSON report with:

- `artifact_kind=probe.forge_worker.verification_pack_report`
- required and advisory checks with explicit pass/fail status
- Forge worker auth-store and assignment-run-loop proof
- Codex route status with only fallback availability and a sanitized source
- hosted environment redaction proof
- prepared-environment sync-gate proof
- child-session read-only status proof
- transcript/runtime/worker-status/summary artifact references

Use `--scratch-root <path>` only when you want the synthetic auth-store proof
to write under a specific temporary directory. The command never needs the real
worker's `PROBE_HOME`.

## Required Checks

The current required checks are:

- `probe.forge_worker.auth_store`
- `probe.forge_worker.assignment_run_loop_contract`
- `probe.codex.route_status_redacted`
- `probe.hosted_environment.redaction`
- `probe.workspace.sync_gate`
- `probe.child_session.status_tools`
- `probe.evidence.artifacts`
- `probe.evidence.redaction`

All required checks must pass before Forge should attach the report as trusted
Evidence.

## Redaction Contract

Hosted workers may receive `PROBE_OPENAI_API_KEY` from their execution
environment and should set `PROBE_OPENAI_API_KEY_SOURCE` to a non-secret
locator such as a Secret Manager path.

The verification report may say whether an API-key fallback is available and
may include the sanitized source label. It must not include:

- the raw model-provider key
- bearer tokens
- raw Forge worker session material
- values from secret-bearing environment variables

The command includes explicit redaction checks so Forge can reject reports that
violate this contract.

## Verification

Use these local checks before closing issue `#128`:

```shell
cargo test -p probe-core forge_worker_verification -- --nocapture
cargo test -p probe-cli --test forge_cli forge_verification_pack -- --nocapture
cargo test -p probe-cli --test forge_cli forge_run_once_executes_an_assigned_run -- --nocapture
cargo test -p probe-core workspace_sync_gate -- --nocapture
cargo run -p probe-cli -- forge verification-pack --pretty
cargo run -p probe-cli -- codex status
```

The `codex status` smoke is advisory. Its purpose is to confirm the current
host route state is observable without printing raw credentials.
