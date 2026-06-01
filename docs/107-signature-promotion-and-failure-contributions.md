# Signature Promotion And Failure Contributions

Status: implementation contract

Date: 2026-06-01

Related:

- `docs/104-seed-signature-packs.md`
- `docs/105-signature-selector.md`
- `docs/106-codex-signature-attachment.md`
- `../crates/probe-core/src/signature_promotion.rs`
- `../crates/probe-core/src/dataset_export.rs`

## Purpose

Probe can now turn failed signature-selection cases into reviewable candidate
signature proposals. This is the failure-learning loop:

```text
failed run -> signature case -> failure cluster -> candidate proposal -> fixture run
-> retained replay -> human review -> shadow -> promoted
```

The important boundary is that proposal generation is not promotion. Optimizer,
agent, or benchmark output can propose a signature. Probe still requires fixture
evidence, retained-run evidence, and reviewer acceptance before the proposal can
enter shadow or promoted routing.

## Proposal Shape

`probe.signature_contribution_report.v1` contains:

- source and failed case counts;
- one proposal per signature/failure cluster;
- source signature id and version;
- proposed candidate id and version;
- failed case ids and source session ids;
- retained run evidence refs;
- required fixture refs;
- proposed signature content;
- promotion gate statuses;
- required fixture run count;
- a Vortex review-card payload.

The Vortex payload is intentionally a review card, not a runtime authority grant.
Allowed actions are review actions such as accepting the candidate, requesting a
fixture, moving to shadow, or deprecating.

## CLI

Create proposals from a signature-case dataset:

```bash
probe signatures propose \
  --signature-cases ~/.probe/reports/signature_cases/signature_cases_all.jsonl \
  --output ~/.probe/reports/signature_contributions.json \
  --owner probe \
  --reviewer autopilot \
  --required-fixture-runs 1
```

Directories are accepted too; Probe will read `signature_cases_all.jsonl` from
the directory.

Use `--json` to print the report to stdout:

```bash
probe signatures propose --signature-cases ./signature_cases --json
```

## Promotion Gates

Promotion gates are:

- failure cluster exists;
- reviewer assigned;
- fixture evidence exists;
- retained-run evidence exists;
- runtime authority boundary preserved;
- reviewer accepted.

Candidate proposals cannot jump directly to `promoted`; they must enter
`shadow` first. Deprecated proposals keep their failed case ids and retained-run
refs so old decisions remain traceable.

## Vortex Boundary

Probe emits the review-card payload, but Autopilot/Vortex remains the product
review and acceptance surface. Vortex should project these proposal records into
signature contribution review cards and record the human approval, fixture run,
and retained-run replay refs back into the promotion evidence.
