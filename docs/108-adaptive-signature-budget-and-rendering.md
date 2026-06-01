# Adaptive Signature Budget And Rendering

Status: implementation contract

Date: 2026-06-01

Related:

- `docs/103-signature-context-contract.md`
- `docs/105-signature-selector.md`
- `docs/106-codex-signature-attachment.md`
- `../crates/probe-core/src/signature_registry.rs`
- `../crates/probe-core/src/harness.rs`

## Purpose

Probe now treats signature injection as a measured budget decision, not a
"stuff every signature into the prompt" shortcut.

The selector supports:

- `no_signature` baseline;
- `fixed_top_k` baseline;
- `capped_selector` baseline;
- `adaptive_threshold` default mode;
- `full_injection` controlled-eval mode.

Full injection is blocked by default. It can be enabled only by setting
`allow_full_injection=true` in `SignatureSelectorConfig`, which keeps the
baseline available for fixture evaluation without making it a runtime default.

## Selection Trace

`SignatureSelectionDecision` now records:

- selected signature budget;
- budget mode;
- selected signatures;
- runner-up signatures;
- rejected high-score signatures excluded by cap or budget;
- rendered task-local context.

The rendered context stays in the internal selection trace. Website-safe
projection exposes the budget mode and selected budget, but not the rendered
context body.

## Set-Aware Rendering

The signature addendum now renders each selected signature with:

- `Use for`
- `Do not use for`
- `Required evidence`
- `Neighbor boundaries`

Neighbor boundaries are explicit when multiple signatures are co-selected. They
tell Codex/Probe where one signature stops and another begins, which reduces
collision between similar skills without mutating canonical signature bodies.

## Retained Fixture Evaluation

`build_signature_ablation_report` produces the four baseline rows required for
retained fixture evaluation:

```text
no_signature
fixed_top_k
capped_selector
full_injection
```

The report records selected ids, selected budget, rejected high-score ids, and
fallback reason. The full-injection row is marked `blocked_by_default=true`.

## Threshold Calibration

`calibrate_signature_threshold` evaluates retained utility labels using:

- pass/fail;
- verifier outcome;
- optional cost;
- optional message count;
- tool failures;
- typed failure class.

The current implementation is intentionally simple and deterministic. It picks
the threshold with the best retained utility. A learned selector can replace it
later after retained ablations prove the need.
