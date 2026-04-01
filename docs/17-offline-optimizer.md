# Offline Optimizer

## Purpose

Probe now has a first offline optimizer lane in `crates/probe-optimizer`.

This is the first honest GEPA-style landing zone in the repo:

- offline
- bounded
- scorecard-driven
- baseline-versus-candidate
- promotion-gated

## Current CLI Paths

Module candidates over decision datasets:

```bash
cargo run -p probe-cli -- optimize-modules \
  --dataset ~/.probe/reports/probe_decision_cases \
  --output ~/.probe/reports/probe_module_optimization_bundle.json
```

Harness candidates over retained acceptance receipts:

```bash
cargo run -p probe-cli -- optimize-harness \
  --baseline-report ~/.probe/reports/probe_acceptance_baseline.json \
  --candidate-report ~/.probe/reports/probe_acceptance_candidate.json \
  --output ~/.probe/reports/probe_harness_optimization_bundle.json
```

Skill-pack candidates over retained coding tasks:

```bash
cargo run -p probe-cli -- optimize-skill-packs \
  --decision-dataset ~/.probe/reports/probe_decision_cases \
  --baseline-report ~/.probe/reports/probe_acceptance_baseline.json \
  --candidate-report ~/.probe/reports/probe_acceptance_verify_first.json \
  --ledger ~/.probe/reports/probe_promotion_ledger.json \
  --output ~/.probe/reports/probe_skill_pack_optimization_bundle.json
```

## What The Optimizer Owns

The optimizer crate currently owns:

- generic scorecard types
- a shared promotion rule
- Psionic artifact bundles for offline module jobs
- retained coding skill-pack manifests and task bundles
- promotion ledgers and adoption state transitions
- baseline-versus-candidate comparison receipts above those Psionic runs

The CLI currently uses that shared rule in two places:

- decision-module candidates launched through the Psionic optimizer substrate
  from exported `decision-cases` bundles
- harness candidates launched through the same Psionic substrate from retained
  acceptance reports adapted into per-attempt cases
- skill-pack candidates composed from retained module plus harness artifacts and
  evaluated against mixed retained coding tasks

`optimize-modules` now writes one bundle that includes, per family:

- the Probe-side candidate manifests
- the Psionic run spec and run receipt
- the retained frontier snapshot when one exists
- train and validation case manifests handed to Psionic
- the retained winner plus the final Probe promotion report

The same command can also ingest an existing bundle with:

```bash
cargo run -p probe-cli -- optimize-modules \
  --artifact-bundle ~/.probe/reports/probe_module_optimization_bundle.json \
  --output ~/.probe/reports/probe_module_optimization_bundle.json
```

Both `optimize-modules` and `optimize-harness` now also write a durable Probe
promotion ledger next to the bundle by default:

- `probe_promotion_ledger.json`

Each entry stores:

- baseline ref and candidate ref
- Psionic run id and run-receipt ref
- whether the candidate was the retained search winner
- whether Probe promotion admitted or rejected it
- the runtime adoption state: `not_adopted`, `shadow`, or `promoted`
- refusal reason when promotion rejected the candidate

Probe now keeps search, promotion, and runtime adoption separate. A search
winner is not implicitly live runtime truth.

To move an admitted candidate into shadow or promoted state:

```bash
cargo run -p probe-cli -- adopt-candidate \
  --ledger ~/.probe/reports/probe_promotion_ledger.json \
  --target decision_module \
  --candidate aggressive_tool_route_v2 \
  --state shadow
```

## Promotion Rule

The current default rule is intentionally strict:

- candidate correctness must be at least as good as baseline
- candidate latency must stay within the allowed regression budget when both
  sides have wallclock receipts
- candidate operator-trust penalty must not increase
- candidate must beat the baseline on at least one promotion dimension

This is the key line that keeps Probe from turning optimization into churn.

Defaults do not change just because a candidate exists.

Defaults change only when a candidate beats the retained baseline without
unacceptable regressions.

## What This Is Not

The optimizer lane does not optimize:

- raw tool implementations
- approval-policy ownership
- transcript schema
- backend transport mechanics

Those remain runtime concerns.

The optimizer lane only compares bounded surfaces above the runtime.

## Why This Matters

This gives Probe the minimal honest stack for later GEPA work:

- retained acceptance set
- replay and decision exports
- explicit manifest-backed decision modules
- Psionic search receipts plus Probe promotion reports
- enforced promotion rules

That is enough to start bounded offline optimization without pretending the
runtime itself is a learned system.
