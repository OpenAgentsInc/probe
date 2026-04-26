# Probe Health Diagnosis Forge Worker Lane

Issue `#129` adds a health-diagnosis path to the Forge worker loop.

## Goal

Probe must be able to claim a Forge health diagnosis Work Order, consume the
health snapshot/events/evidence refs provided by Forge, create a durable Probe
transcript, and attach structured diagnosis artifacts back to Forge.

Probe is still not the production recovery authority. It diagnoses incidents,
prepares patch plans and issue-comment drafts, and attaches evidence. Any
actual recovery action must be routed through the deterministic health worker
policy and a Forge lease.

## Assignment Detection

`probe forge run-once` and `probe forge run-loop` now recognize health
diagnosis assignments when the Forge assignment has one of:

- `requested_outputs.kind=probe_health_diagnosis`
- `requested_outputs.artifact_kind=probe.forge_worker.health_diagnosis_report`
- a `health_diagnosis` marker in `requested_outputs` or `verification_policy`
- a work-order title containing `health diagnosis`

The normal software-work path remains model/tool driven. Health diagnosis
assignments use the deterministic health lane so they can produce safe,
structured evidence even when the incident is about model/provider health.

## Output Contract

The ready-for-verification event includes:

- `probe_health_diagnosis`
- `probe_worker_verification_pack`
- `probe_session`
- `probe_artifacts.transcript`
- `probe_artifacts.summary_artifacts`
- `probe_artifacts.health_diagnosis_artifacts`
- `recovery_policy.direct_recovery_actions_executed=false`
- `recovery_policy.route=forge_health_worker_policy_lease`

The structured diagnosis report has artifact kind:

```text
probe.forge_worker.health_diagnosis_report
```

It includes root-cause classification, supporting signals, recommended action,
patch plan, verification-pack status, and safety fields.

## Safety Rules

- Probe never runs production recovery actions directly from this lane.
- Probe never writes raw model keys, bearer tokens, worker session material, or
  raw environment values into the report.
- Recovery recommendations must point back to Forge health-worker policy and
  leased recovery actions.
- Verification-pack results are attached to every health diagnosis run so Forge
  can reject unsafe worker evidence.

## Verification

Use:

```shell
cargo test -p probe-core forge_health_diagnosis -- --nocapture
cargo test -p probe-core forge_health_diagnosis_run_reports_structured_evidence_without_recovery_actions -- --nocapture
cargo test -p probe-core forge_worker_verification -- --nocapture
cargo test -p probe-cli --test forge_cli forge_run_once_executes_an_assigned_run -- --nocapture
cargo check --workspace
```

When a live key-backed route is available and safe to use, run a tiny one-shot
smoke:

```shell
cargo run -p probe-cli -- exec --profile openai-codex-subscription "Return exactly: probe health diagnosis smoke"
```
