# Probe Optimizer System

This document is the canonical system map for Probe's current offline
optimizer stack.

It explains the subsystem that turns retained Probe runtime artifacts into
bounded optimize-anything jobs, promotion decisions, and operator-managed
adoption state.

Related narrower docs:

- `docs/10-acceptance-harness.md`
- `docs/15-replay-and-decision-dataset-export.md`
- `docs/16-decision-modules.md`
- `docs/17-offline-optimizer.md`
- `docs/22-acceptance-report-schema.md`

## What This System Is

Probe now has a three-layer optimizer stack:

1. Probe runtime and harnesses produce retained local artifacts.
2. Probe converts those artifacts into typed candidate families and eval cases.
3. Psionic's generic optimizer substrate runs the search loop, while Probe
   retains promotion and adoption authority.

This is intentionally not a learned runtime.

Probe still owns:

- sessions
- transcripts
- tool execution
- approvals
- harness prompts
- local acceptance receipts
- decision-case exports
- promotion decisions
- operator adoption state

Psionic owns the reusable optimization substrate:

- candidate and case manifests
- run specs
- minibatch evaluation receipts
- frontier state
- evaluation cache
- run receipts

The current landing point is offline and bounded. Probe optimizes narrow
surfaces above the runtime rather than trying to "learn" the runtime itself.

## Current Scope

The current optimizer system supports three target kinds in
`crates/probe-optimizer/src/lib.rs`:

- `decision_module`
- `harness_profile`
- `skill_pack`

Those map to three concrete Probe families:

- decision-module manifests in `crates/probe-decisions`
- harness candidate manifests in `crates/probe-core`
- composite skill-pack manifests in `crates/probe-optimizer`

Important current boundary:

- harness manifests are live runtime truth because Probe resolves them when it
  launches tool-enabled turns
- decision-module manifests are currently offline-eval artifacts, not live turn
  loop policy
- skill-pack manifests are optimizer artifacts, not a live runtime selection
  layer
- the promotion ledger is currently consumed by the skill-pack lane and by the
  operator `adopt-candidate` flow, not by automatic runtime default switching

## End-To-End Flow

The system is easiest to understand as one dataflow:

1. Probe runtime creates local sessions and append-only transcripts.
2. `probe accept` creates retained acceptance reports over coding cases.
3. `probe export --dataset decision-cases` derives per-decision local case
   bundles from real transcripts.
4. Probe candidate families define what is tunable:
   - decision-module manifests
   - harness manifests
   - skill-pack manifests
5. `probe-cli` converts those Probe artifacts into Psionic candidate and case
   manifests.
6. Psionic's `OptimizationEngine` runs a bounded search loop and emits run
   receipts plus search state.
7. Probe turns the Psionic result into a Probe-side bundle with scorecards,
   promotion report, and retained artifact refs.
8. Probe writes or updates `probe_promotion_ledger.json`.
9. An operator can move an admitted candidate through:
   - `not_adopted`
   - `shadow`
   - `promoted`
10. The skill-pack lane can compose an admitted aggregate candidate from the
    ledger and score it over retained tasks.

## Code Ownership Map

### `crates/probe-core`

This crate owns the retained local source artifacts that feed optimization.

Main files:

- `crates/probe-core/src/dataset_export.rs`
- `crates/probe-core/src/harness.rs`

Key responsibilities:

- export replay, decision-summary, and decision-case datasets
- derive stable decision-case ids and train or validation split membership
- define harness candidate manifests
- resolve live harness profiles for runtime and TUI use

### `crates/probe-decisions`

This crate owns the offline-evaluable decision-module layer above the runtime.

Main file:

- `crates/probe-decisions/src/lib.rs`

Key responsibilities:

- decision-module family enums and eval specs
- manifest-backed candidate definitions
- baseline and candidate rule tables
- Rust-native evaluators over exported decision cases

### `crates/probe-optimizer`

This crate is the Probe-side optimizer bridge.

Main file:

- `crates/probe-optimizer/src/lib.rs`

Key responsibilities:

- Probe scorecards and promotion rules
- bundle schemas for module, harness, and skill-pack runs
- promotion ledger and adoption state transitions
- Probe-to-Psionic candidate and case conversion
- Probe evaluators that score Psionic candidates against retained Probe data

### `crates/probe-cli`

This crate exposes the operator workflows.

Main file:

- `crates/probe-cli/src/main.rs`

Key responsibilities:

- export commands
- offline evaluation commands
- optimization launch and bundle replay commands
- ledger updates and adoption state changes
- acceptance-report adaptation into optimizer input

## Source Artifacts

The optimizer does not fabricate its own truth. It starts from retained Probe
artifacts.

### Sessions And Transcripts

Sessions and transcripts are the raw runtime record.

They contribute:

- session ids
- transcript item order
- tool calls and tool results
- approval outcomes
- file-touch evidence
- backend profile and harness metadata

Probe only exports coding-lane sessions by default. A session is included when
it has a `coding_bootstrap*` harness profile or when its transcript shows
coding tools such as `read_file`, `list_files`, `code_search`, `shell`, or
`apply_patch`.

### Acceptance Reports

Acceptance reports are the retained harness-level eval receipts described in
`docs/10-acceptance-harness.md` and `docs/22-acceptance-report-schema.md`.

They contribute:

- pass or fail status per retained attempt
- wallclock observations
- executed tool count
- tool names
- refusal and pause counts
- backend failure families and reasons
- transcript refs back into the runtime record

These reports are the input to `optimize-harness` and part of the input to
`optimize-skill-packs`.

### Dataset Exports

Probe exports three related dataset shapes:

- `replay`
  - near-raw transcript replay records
- `decision`
  - one per-session summary row
- `decision-cases`
  - one per observed decision point

The optimizer system is built around `decision-cases`.

## Decision-Case Bundle

`probe export --dataset decision-cases` writes a bundle directory containing:

- `decision_cases_all.jsonl`
- `decision_cases_train.jsonl`
- `decision_cases_val.jsonl`
- `decision_case_split_manifest.json`

Each `DecisionCaseRecord` contains:

- `case_id`
- `stable_digest`
- `family`
- `split`
- session and transcript provenance
- typed pre-decision context
- retained observed label
- transcript item refs

Current case families:

- `tool_route`
- `patch_readiness`
- `long_context_escalation`

Case generation behavior is important:

- Probe derives cases from the first named tool call in each turn
- each observed tool call currently yields one case for all three families
- split membership is deterministic from the stable digest
- the split threshold is digest-suffix based and produces an approximate 80/20
  train or validation partition

This means the decision-case bundle is stable enough for repeated offline
evaluation while still preserving direct provenance back to real transcripts.

## Decision Modules

Decision modules are Probe's first narrow optimize-anything family above the
runtime.

They live in `crates/probe-decisions/src/lib.rs` as manifest-backed artifacts
with Rust-native evaluators.

Current families:

- `ToolRoute`
- `PatchReadiness`
- `LongContextEscalation`

Current built-in candidate ids:

- `heuristic_tool_route_v1`
- `aggressive_tool_route_v2`
- `heuristic_patch_readiness_v1`
- `strict_patch_readiness_v2`
- `heuristic_long_context_escalation_v1`

Each `DecisionModuleCandidateManifest` carries:

- schema version
- candidate id
- family id
- human description
- typed family-specific rule spec
- stable manifest digest

The current rule shapes are explicit and hand-authored:

- tool-route rules choose a `selected_tool`, ranking, and reason
- patch-readiness rules choose `ready`, confidence, and next steps
- long-context rules choose whether to escalate plus the requested task type

Current implementation detail that matters:

- Probe is not yet synthesizing new decision-module candidates automatically
- it serializes a bounded set of known manifests into Psionic candidates
- the Psionic search loop then evaluates and orders those candidates

So the current search is "optimizer substrate over retained manifest families",
not reflective generation of novel policies.

## Harness Profiles

Harness profiles are both runtime truth and optimizer candidates.

They live in `crates/probe-core/src/harness.rs`.

Each `HarnessCandidateManifest` carries:

- schema version
- candidate id
- tool set
- profile name
- profile version
- description
- system prompt template
- stable manifest digest

Current built-ins:

- `coding_bootstrap_default@v1`
- `coding_bootstrap_patch_guard@v1`
- `coding_bootstrap_verify_first@v1`

The runtime uses these manifests directly:

- `resolve_harness_profile` selects the manifest
- Probe renders the prompt template with `cwd`, shell, and operating system
- TUI and CLI default to `coding_bootstrap_default`

The optimizer reuses the same manifests as the candidate family for
`harness_profile` optimization.

## Skill Packs

Skill packs are the current aggregate optimize-anything family.

They live in `crates/probe-optimizer/src/lib.rs` as `SkillPackManifest`.

Each skill pack references:

- one tool-route candidate id
- one patch-readiness candidate id
- one long-context candidate id
- one harness candidate id

Current skill-pack construction is intentionally narrow:

- `probe_skill_pack_baseline_v1`
  - fixed to retained baseline module and harness artifacts
- `probe_skill_pack_admitted_v1`
  - assembled from the best admitted candidates in the promotion ledger

`preferred_ledger_candidate_id` currently prefers admitted candidates in this
order:

- `promoted`
- `shadow`
- `not_adopted`

If no admitted candidate exists for a family, the baseline candidate id is used
instead.

Important current limitation:

- the skill-pack lane does not yet search a large combinatorial family
- it currently compares the baseline pack against a single ledger-assembled
  admitted pack when those digests differ

## How Probe Converts Artifacts Into Psionic Runs

The Probe-to-Psionic bridge is the core of the system.

Probe serializes its candidates into Psionic manifests with one main component
per target kind:

- `decision_module_manifest_json`
- `harness_candidate_manifest_json`
- `skill_pack_manifest_json`

Probe also adds provenance refs such as:

- `probe_decision_manifest_digest:<digest>`
- `probe_harness_manifest_digest:<digest>`
- `probe_skill_pack_manifest_digest:<digest>`

Case conversion is similarly explicit.

### Decision-Module Cases

Probe converts each `DecisionCaseRecord` into a `PsionicCaseManifest` with:

- label derived from the retained observed label
- serialized context and observed label in metadata
- transcript path and session refs in evidence refs

### Harness Cases

Probe converts each retained acceptance attempt into a `HarnessEvaluationCase`
and then into a `PsionicCaseManifest` with:

- pass or fail label
- case name and attempt index in metadata
- failure category and tool-name list in metadata
- transcript-path evidence when available

Harness split membership is deterministic but separate from decision-case
splits. Probe currently hashes `case_name` plus `attempt_index` and assigns
validation when `checksum % 5 == 0`.

### Skill-Pack Tasks

Probe converts retained decision cases and retained harness attempts into
`SkillPackTask`s.

Current task inventory:

- every retained decision case becomes a task
- the first harness input contributes the retained harness task ids

That means the skill-pack lane evaluates mixed retained tasks, but the task
inventory is still anchored to one retained harness case set.

## Psionic Search Mechanics

Every current Probe optimization run uses the same broad Psionic pattern:

- a seeded baseline candidate
- typed train and validation cases
- `OptimizationFrontierMode::Scalar`
- bounded iteration and candidate budgets
- `OptimizationSequentialMinibatchSampler`
- `OptimizationEvaluationCache`

Current run ids are Probe-generated and target-specific, for example:

- `probe-optimize-tool_route-<count>`
- `probe-optimize-harness-<profile>`
- `probe-optimize-skill-pack-<count>`

The current proposer is deliberately simple:

- `OrderedPsionicCandidateProposer`

That proposer:

- walks a precomputed queue of Probe candidates
- proposes the next unseen candidate
- records a diff over the one serialized Probe component
- does not perform reflective mutation, merge, or gating

So the current system already gets Psionic's run specs, cached evaluation,
minibatching, lineage, and receipts, but the search policy itself is still a
Probe-authored ordered sweep over a bounded candidate set.

## Probe Evaluators

Probe keeps evaluation semantics local even when Psionic runs the search loop.

Current evaluators:

- `DecisionModulePsionicEvaluator`
- `HarnessPsionicEvaluator`
- `SkillPackPsionicEvaluator`

Each evaluator:

- deserializes the Probe artifact from the Psionic candidate component
- looks up the retained Probe case by id
- computes a scalar correctness score of `1` or `0`
- emits shared feedback plus component-specific feedback
- caches case receipts for repeat evaluation

Evaluation semantics by target:

- decision modules
  - candidate matches the retained observed label or it does not
- harness profiles
  - retained acceptance attempt passed or failed
- skill packs
  - selected component candidate matched the retained task outcome or it did not

The current evaluator layer is intentionally honest and narrow. It does not
pretend to infer richer metrics that Probe did not actually retain.

## Bundles And Retained Artifacts

Probe wraps every Psionic run in a Probe-side bundle.

Current bundle types:

- `DecisionModuleOptimizationBundle`
- `HarnessOptimizationBundle`
- `SkillPackOptimizationBundle`

Each bundle retains:

- a schema version and report id
- source dataset or task refs
- optional issue ref
- baseline and retained candidate ids
- baseline and retained manifest digests
- baseline and retained scorecards
- Probe promotion report
- the full Psionic run spec
- the full Psionic run receipt
- optional frontier snapshot
- serialized Psionic candidate manifests
- the train and validation cases handed to Psionic
- a retained search-state digest

This is important because Probe is not just storing "winner" state. It stores
enough information to audit how the run was constructed.

## Scorecards And Promotion

Probe promotion is stricter than simple search winner selection.

The promotion layer uses:

- `OptimizationScorecard`
- `PromotionRule`
- `compare_candidate`

Current scorecard fields:

- correctness numerator
- correctness denominator
- median wallclock ms
- operator trust penalty

Current default rule is `PromotionRule::gepa_default()`:

- `max_latency_regression_bps = 11000`
- `require_strict_improvement = true`

Translated into behavior:

- candidate correctness must be at least as good as baseline
- candidate latency must stay within the allowed regression budget when both
  sides have wallclock data
- candidate trust penalty must not increase
- candidate must beat baseline on at least one promotion dimension

Important current nuance:

- harness optimization populates correctness, median wallclock, and operator
  trust penalty from acceptance reports
- decision-module and skill-pack scorecards currently populate correctness only
  because the Psionic batch receipts do not yet carry those richer Probe-side
  signals for those lanes

That means the module and skill-pack lanes already use the shared promotion
rule, but in practice promotion is correctness-driven today.

## Promotion Ledger And Adoption State

Every optimization lane writes or updates a durable promotion ledger:

- `probe_promotion_ledger.json`

If the operator does not pass `--ledger`, module and harness optimization write
the ledger next to the output bundle.

Each `PromotionLedgerEntry` stores:

- target kind
- family key
- baseline and candidate ids
- baseline and candidate refs with digests
- Psionic run id and receipt ref
- artifact-bundle ref
- whether the candidate was the retained search winner
- promotion disposition
- adoption state
- refusal reason when promotion failed

Promotion and adoption are separate:

- search winner
  - best retained candidate from the optimization run
- promotion disposition
  - whether Probe admits it under the promotion rule
- adoption state
  - whether an admitted candidate is live, shadowed, or still parked

Allowed adoption transitions:

- `not_adopted -> shadow`
- `shadow -> promoted`
- `promoted -> not_adopted` is allowed by setting the state back explicitly

Blocked transition:

- `not_adopted -> promoted` is rejected

Only admitted candidates can change adoption state.

## CLI Workflows

### Dataset Export

```bash
cargo run -p probe-cli -- export \
  --dataset decision-cases \
  --output ~/.probe/reports/probe_decision_cases
```

### Offline Module Evaluation

```bash
cargo run -p probe-cli -- module-eval \
  --dataset ~/.probe/reports/probe_decision_cases
```

### Decision-Module Optimization

```bash
cargo run -p probe-cli -- optimize-modules \
  --dataset ~/.probe/reports/probe_decision_cases \
  --output ~/.probe/reports/probe_module_optimization_bundle.json
```

Replay an existing bundle:

```bash
cargo run -p probe-cli -- optimize-modules \
  --artifact-bundle ~/.probe/reports/probe_module_optimization_bundle.json \
  --output ~/.probe/reports/probe_module_optimization_bundle.json
```

### Harness Optimization

```bash
cargo run -p probe-cli -- optimize-harness \
  --baseline-report ~/.probe/reports/probe_acceptance_baseline.json \
  --candidate-report ~/.probe/reports/probe_acceptance_verify_first.json \
  --output ~/.probe/reports/probe_harness_optimization_bundle.json
```

### Skill-Pack Optimization

```bash
cargo run -p probe-cli -- optimize-skill-packs \
  --decision-dataset ~/.probe/reports/probe_decision_cases \
  --baseline-report ~/.probe/reports/probe_acceptance_baseline.json \
  --candidate-report ~/.probe/reports/probe_acceptance_verify_first.json \
  --ledger ~/.probe/reports/probe_promotion_ledger.json \
  --output ~/.probe/reports/probe_skill_pack_optimization_bundle.json
```

### Adoption State Changes

```bash
cargo run -p probe-cli -- adopt-candidate \
  --ledger ~/.probe/reports/probe_promotion_ledger.json \
  --target decision_module \
  --candidate aggressive_tool_route_v2 \
  --state shadow
```

## What The Runtime Uses Today

Current live runtime truth is still mostly separate from the optimizer lane.

Today:

- runtime and TUI use the resolved built-in harness manifests directly
- decision-module manifests are evaluated offline
- skill-pack manifests are evaluated offline
- the ledger influences skill-pack composition and operator bookkeeping
- the runtime does not yet auto-load a promoted decision-module or skill-pack
  candidate from the ledger

That is deliberate. Probe currently requires explicit operator action before
offline optimization results can influence live behavior.

## Current Limitations

This is a real system, but it is still intentionally early and bounded.

Main limits:

- search is ordered candidate enumeration, not reflective candidate synthesis
- decision-module candidates are hand-authored rule manifests
- skill-pack search is currently baseline versus one admitted aggregate
- module and skill-pack promotion currently lack latency and trust measurements
- runtime defaults are not yet auto-driven from the promotion ledger
- acceptance-derived harness tasks currently come from one retained task set
- all retained artifacts are local-first and can contain sensitive transcript
  data

Those are not accidental gaps. They are the guardrails that keep the system
honest while Probe proves the substrate.

## Why This Matters

Probe now has a concrete optimize-anything stack with explicit seams:

- runtime truth stays in Probe
- optimization substrate lives in Psionic
- all candidates are manifest-backed and auditable
- all evaluations run over retained local artifacts
- promotion is explicit and strict
- adoption is operator-controlled rather than implicit

That is enough to iterate on module families, harness prompts, and composed
coding skill packs without pretending the runtime itself has become a black-box
learner.
