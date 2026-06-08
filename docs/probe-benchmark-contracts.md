# Probe Benchmark Contracts

Date: 2026-06-08

Probe now has the first runtime-local contract slice for public Benchmark Cloud
and Pylon-distributed GEPA rollout work. The contracts live in
`packages/runtime/src/contracts/benchmark.ts` and are exported from the runtime
package entry point.

## Implemented Schemas

- `probe.benchmark_assignment.v1`
- `probe.benchmark_run.v1`
- `probe.benchmark_closeout.v1`
- `probe.benchmark_decision_trace.v1`
- `probe.prompt_candidate.v1`
- `probe.blueprint_candidate.v1`
- `probe.tool_menu_candidate.v1`
- `probe.loop_policy_candidate.v1`
- `probe.benchmark_promotion_decision.v1`

The assignment schema carries the Benchmark Cloud run and task refs, dataset
and split refs, public-safe task checksum or ref, Probe commit, backend and
runtime profile, optional account/grant refs, selected Blueprint signatures,
tool-menu ref, candidate hash, timeout and budget policy refs, required
artifact refs, required proof-bundle refs, and callback/proof sink refs.

The closeout schema carries the assignment and run refs, candidate hash,
selected signatures, tool menu, backend route, verifier/scorer refs, artifact
manifest refs, proof bundle refs, resource/cost refs, policy findings, failure
classification, retained-failure refs, redaction state, run status, split, and
promotion status.

## Safety Boundary

The contract validators reject public benchmark records containing raw provider
credentials, raw benchmark secrets, hidden verifier content, wallet or payment
material, private repository refs, unbounded raw logs, public-claim upgrade
authority, or runtime-promotion authority. The sanitizer can scrub or drop those
fields before public-safe artifact emission, but decoders reject unsafe input.

Promotion decisions are evidence-only. They can record that retained,
validation, holdout, or live evidence exists, but they cannot promote runtime
behavior or upgrade a public benchmark claim. External Omega/OpenAgents release
gates remain the authority for publication and promotion.

## Test Coverage

`packages/runtime/tests/benchmark-contracts.test.ts` covers valid assignment,
run, decision-trace, candidate, and promotion-decision schema refs; invalid
closeouts missing artifact or proof refs; unsafe projection rejection and
scrubbing; failed and timed-out retained closeouts; and separate retained,
validation, holdout, and live evidence representations.
