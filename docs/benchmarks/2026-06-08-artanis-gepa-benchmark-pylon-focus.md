# Artanis GEPA Benchmark And Pylon Focus

Date: 2026-06-08
Status: audit and refocus plan for Artanis in the Probe benchmark loop

## Thesis

Artanis already points at the right mission shape for Probe benchmark learning:
public-safe oversight of training-program work, Pylon routing, Model Lab
evidence, Benchmark Cloud evaluation, Forum/public reporting, and promotion
gates. The current implementation and proof trail are mostly aimed at Pylon
release supervision, SHC bootstrap workrooms, public claim discipline, and
payment-backed dispatch gates.

The update is to narrow Artanis' next mission from broad Pylon launch overseer
to public overseer for Probe GEPA coding-agent benchmark campaigns through
Pylons. Artanis should not become the benchmark runner, scorer, optimizer, or
runtime authority. Artanis should coordinate the campaign state, select and
gate work through typed Program/Blueprint signatures, ingest public-safe
evidence from Probe, Benchmark Cloud, Psionic, and Pylon, and project only
evidence-bound status to `/artanis`, Forum, and operator surfaces.

## Source Material Reviewed

Workspace and historical Artanis source:

- `agents/training-program-maintenance-agent.md`
- `docs/2026-05-22-artanis-fake-projection-to-live-agent-gap-audit.md`
- `docs/2026-05-23-autopilot-semantic-memory-blueprint-extension-audit.md`
- `docs/omni/vortex-public-proof-open-positioning-synthesis.md`
- `docs/omni/vortex-domain-agent-subsystem-builder-synthesis.md`
- `docs/omni/vortex-to-omni-product-gap-analysis-roadmap.md`
- `vortex/docs/public-agents-artanis.md`
- `autopilot4-deprecated/src/artanis.rs`
- `autopilot4-deprecated/src/programs.rs`
- `autopilot4-deprecated/src/work_orders.rs`

Active Omega source and docs:

- `autopilot-omega/docs/2026-06-03-team-project-rooms.md`
- `autopilot-omega/docs/artanis/2026-06-06-artanis-implementation-audit.md`
- `autopilot-omega/docs/artanis/2026-06-06-artanis-full-deployment-readiness-audit.md`
- `autopilot-omega/docs/artanis/2026-06-07-artanis-deploy-readiness-full-audit.md`
- `autopilot-omega/docs/pylon/2026-06-06-r10-artanis-pylon-campaign-ledger.md`
- `autopilot-omega/docs/nexus/2026-06-07-artanis-payment-backed-dispatch-gates.md`
- `autopilot-omega/workers/api/src/artanis-runtime.ts`
- `autopilot-omega/workers/api/src/artanis-loop.ts`
- `autopilot-omega/workers/api/src/artanis-public-report.ts`
- `autopilot-omega/workers/api/src/artanis-work-routing.ts`
- `autopilot-omega/workers/api/src/artanis-continual-learning-templates.ts`
- `autopilot-omega/workers/api/src/artanis-nexus-pylon-adapters.ts`
- `autopilot-omega/apps/web/src/product-policy.ts`

OpenAgents, Pylon, and Cloud proof material:

- `openagents/docs/pylon/PYLON_VERIFICATION_MATRIX.md`
- `openagents/docs/reports/nexus/2026-06-07-pylon-v02-production-blockers.md`
- `openagents/docs/reports/nexus/2026-06-07-pylon-v02-artanis-bootstrap-evidence.md`
- `openagents/docs/reports/nexus/2026-06-07-pylon-v02-live-artanis-shc-bootstrap-proof.md`
- `openagents/docs/reports/nexus/2026-06-07-artanis-pylon-v022-integrated-paid-work-proof.md`
- `openagents/docs/reports/nexus/2026-06-07-artanis-mdk-settlement-bridge-smoke.md`
- `cloud/docs/bootstrap/CND-055-artanis-pylon-bootstrap.md`
- `cloud/docs/contracts/openagents.artanis_bootstrap_assignment.v1.md`

Probe benchmark docs:

- `docs/benchmarks/README.md`
- `docs/benchmarks/2026-06-08-workspace-benchmark-systems-audit.md`
- `docs/benchmarks/2026-06-08-omni-continual-learning-training-loop.md`
- `docs/benchmarks/2026-06-08-probe-continual-benchmark-learning-apparatus.md`
- `docs/benchmarks/2026-06-08-pylon-gepa-coding-agent-benchmark-run.md`

## Current Artanis Mission

Artanis began as the public identity for the training-program maintenance
agent. The root maintenance-agent instruction frames Artanis as the public
wrapper around a loop that keeps benchmark/product evidence, Blueprint
promotion authority, Psionic training/eval, Pylon work, Nexus validation,
closeout, stats, payout accounting, served candidate models, retained
benchmark runs, and promotion or rollback decisions moving.

The historical Artanis projection docs define the core safety contract:

```text
private workroom state
-> redaction
-> public projection rows
-> instruction digest
-> session summary
-> health snapshot
-> dispatch gates
-> redacted event timeline
-> public-safe artifacts
```

That public route must not read private prompts, raw tool logs, private
workrooms, provider payloads, private repository contents, wallet material, or
secrets. It is a projection, not a live public control plane.

The deprecated Autopilot4 source material already had useful Artanis Program
signatures:

- `artanis.objective_status`
- `artanis.context_selector`
- `artanis.work_selector`
- `artanis.capability_matcher`
- `artanis.dispatch_risk_classifier`
- `artanis.public_summary`
- `artanis.promotion_readiness`
- `artanis.next_action`

It also had training-maintenance Work Order templates for support, evaluation,
integrity, adapter training, benchmark replay, promotion requests, rollback
requests, failed trajectory export, preference-pair construction, and
tool-use discipline trace extraction. The execution routes were
`hosted_coder_runtime`, `local_pylon`, and `psionic_training_runtime`.

That mission remains directionally right, but the active implementation home
has moved. New product behavior now belongs in Omega. The deprecated
Autopilot4 and Vortex material are source material, not implementation homes.

## Active Omega Shape

Omega now has the concrete Artanis surface:

- public routes at `/artanis` and `/agents/artanis`;
- authenticated project identity `project_artanis` under the OpenAgents Core
  Team;
- a compact project-agent projection with runtime `Autopilot`, backend `SHC`,
  repo `autopilot-omega`, and focus `Pylon`;
- public current-goal and public Pylon stats loading;
- Artanis runtime, loop, health, public report, Forum, approval-gate,
  work-routing, continual-learning-template, Nexus/Pylon adapter, and
  production-launch-gate contracts;
- public claim-state caveats that block overclaiming.

The Artanis runtime and loop contracts are intentionally read-only by default.
The authority records set no deployment, no provider mutation, no runtime
promotion, no training launch, no wallet spend, no payment spend, no
settlement mutation, and no public claim upgrade unless an operator-approved
path grants a narrower authority.

This is the right boundary for benchmark campaigns. Artanis can describe and
supervise a campaign, but the authority to launch metric calls, spend money,
train adapters, promote a runtime candidate, or publish a stronger claim must
remain with the relevant operator gate, Benchmark Cloud, Psionic, Pylon,
Omega, or payment authority.

## Current Proof Trail

The current proof trail is Pylon-launch heavy.

What is proven:

- Artanis can be represented as a public Omega project and public agent
  projection.
- Omega has a public Artanis report shape that aggregates runtime, loop,
  health, Model Lab, Pylon, Forum, claim, receipt, and release-gate state.
- Omega has work-routing capabilities that already include
  `benchmark_cloud`, `probe`, `psionic`, `pylon`, `runner`,
  `coding_runtime_probe`, `benchmark_evaluation`, `gepa_dspy_optimization`,
  `lora_finetuning`, and `pylon_training`.
- Omega has continual-learning template kinds for benchmark reruns, GEPA/DSPy
  optimization, dataset curation, adapter validation, LoRA fine-tuning, and
  regression analysis.
- The private Cloud Artanis bootstrap contract can launch a bounded
  account-backed SHC Codex workroom with `wallet_authority=false`.
- A live account-backed Artanis SHC bootstrap completed for the Pylon release
  path and captured the required launch artifacts.
- The Pylon v0.2.2 integrated paid-work proof ties together Artanis launch
  supervision, public-path Pylon install, accepted/rewarded work, and real MDK
  payment movement.
- The later Artanis to MDK settlement bridge smoke proves the id-chain shape
  and real payment movement for a generated settlement assignment id.

What is not proven:

- Artanis is not proven as a continuously autonomous production
  administrator.
- The public report is not enough by itself to prove a live scheduled Artanis
  loop retaining its own production rows.
- The private Cloud bootstrap contract is not the desired long-term public
  Benchmark Cloud authority.
- The Pylon release proof is not a Probe benchmark campaign proof.
- The integrated paid-work proof still does not prove the fully deployed
  production chain from Artanis assignment id to Pylon accepted work to MDK
  settlement receipt to public receipt.
- No current proof shows Artanis supervising a GEPA campaign over
  Terminal-Bench 2 or Probe retained coding-agent fixtures through Pylons.

## Similarities To The Probe Benchmark Docs

The Artanis mission and the Probe benchmark docs are aligned on the important
contracts.

Both require public proof to be a projection of retained records. Neither
allows public claims to be upgraded from model prose, private workroom text, or
operator memory. Both require redaction, receipt refs, benchmark refs, artifact
manifests, and claim states.

Both use Pylon as useful distributed work capacity, not decorative uptime.
Pylon workers should advertise capability envelopes, receive bounded
assignments, return artifacts and receipts, and get credit only when work
survives validation.

Both separate execution from authority. Probe should run coding-agent turns and
emit evidence. Benchmark Cloud should own benchmark contracts and score
imports. Psionic should own optimizer/model-training truth. Pylon should own
worker execution and receipts. Omega should own product release gates and
public projection. Artanis should coordinate and narrate the campaign state
inside those boundaries.

Both emphasize retained failures and promotion discipline. A retained fixture
improvement is not a public leaderboard score. A validation split is not frozen
holdout performance. A GEPA candidate is not active production runtime until a
release gate promotes it.

## Differences From The Current Benchmark Plan

The Probe benchmark docs have moved the foreground hillclimb to coding-agent
benchmarks, starting with Terminal-Bench 2 through Harbor on the SHC box and a
GEPA-only first campaign over text artifacts. Artanis' active public mission is
still mostly framed around Pylon release, Pylon stats, Pylon marketplace work,
Forum reports, and payment/settlement gates.

The benchmark docs want public Benchmark Cloud in `openagents`, while the
Artanis bootstrap contract still lives in the private `cloud` repo. That is
acceptable source material, but it is not the desired public authority for the
benchmark apparatus.

The benchmark docs want Probe-specific artifacts: prompts, Blueprint usage,
Program Signature playbooks, tool-menu policy, failure-family playbooks,
closeout instructions, benchmark attempts, verifier outputs, and candidate
hashes. Current Artanis proofs mostly capture launch plans, Pylon setup,
continual-learning plans, signature-mining plans, work-order drafts, and proof
bundles.

The benchmark docs make GEPA the first optimizer lane and push LoRA/Qwen work
later. Artanis' current continual-learning templates already include both GEPA
and LoRA, but the public projection should make the stage explicit: GEPA first,
model training later, no premature training or promotion claims.

## Refocused Artanis Mission

Artanis should become the public overseer for Probe benchmark learning
campaigns.

The first campaign should be:

```text
artanis.probe.gepa.terminal_bench_2.stage_0_1
```

Plain-language mission:

```text
Coordinate the first public-safe Probe GEPA campaign for coding-agent
benchmarks, using Benchmark Cloud contracts, Pylon rollout capacity, Probe
runtime evidence, Psionic optimizer lineage, and Omega public projection gates.
```

Artanis should own these campaign responsibilities:

- maintain the public campaign objective and stage;
- verify that the public Benchmark Cloud split manifest exists;
- select the relevant Probe, Blueprint, Program Signature, benchmark, and
  failure-family context packs;
- propose Pylon rollout batches for GEPA metric calls;
- require capability-matched Pylon workers and signed receipts;
- ingest Probe closeout artifacts and Benchmark Cloud score imports;
- classify candidate readiness with Program/Blueprint signatures;
- identify blockers before public claims upgrade;
- summarize progress through `/artanis`, Forum, and operator surfaces;
- preserve the line between proposed, running, measured, verified, promoted,
  and settled states.

Artanis should not own these authorities:

- raw benchmark execution;
- benchmark scoring authority;
- GEPA optimizer authority;
- Probe runtime promotion;
- Qwen/LoRA training launch;
- wallet spend;
- settlement mutation;
- provider account mutation;
- public claim upgrade without retained evidence.

## Campaign Data Flow

The refocused flow should be:

```text
Omega Artanis goal
-> public Benchmark Cloud split manifest
-> Psionic/GEPA candidate frontier
-> Pylon batch assignment plan
-> Probe benchmark runs
-> Benchmark Cloud score import
-> artifact, receipt, and resource manifests
-> Artanis campaign import
-> Artanis public report and Forum summary
-> Omega release gate
```

The first campaign should keep Stage 0 and Stage 1 small enough to prove the
loop:

```text
retained Terminal-Bench/Probe failures
-> GEPA candidate text bundle
-> Pylon metric-call rollouts
-> verifier receipts
-> candidate comparison
-> validation split replay
-> hold, reject, or promote to release-review candidate
```

## Program Signature Updates

The old Artanis Program signatures should be kept in spirit, but narrowed for
the GEPA benchmark campaign.

Recommended signatures:

- `artanis.gepa_campaign_status`
  Evaluate objective, stage, split, evidence freshness, and blockers.
- `artanis.gepa_context_selector`
  Select Probe docs, Blueprint signatures, failure families, benchmark tasks,
  retained traces, and candidate text artifacts.
- `artanis.gepa_pylon_batch_planner`
  Plan metric-call batches by worker capability, cost cap, retry policy,
  timeout policy, and split.
- `artanis.probe_artifact_import`
  Import Probe closeout, tool-use, verifier, cost, and artifact-manifest refs
  without reading private raw logs into public projection.
- `artanis.gepa_candidate_readiness`
  Compare candidate hashes against retained, validation, regression, and
  policy gates before release-review.
- `artanis.benchmark_claim_gate`
  Lower or block public claims when split, scorer, receipt, redaction,
  settlement, or promotion evidence is missing.
- `artanis.public_campaign_summary`
  Generate public-safe Forum and `/artanis` status from refs, not raw traces.
- `artanis.next_benchmark_action`
  Choose the next operator-safe action: run another retained batch, widen to
  validation, hold for artifact gaps, open implementation issues, or request
  human approval.

These signatures should be implemented in the active Omega/Blueprint-shaped
surface, with source/spec synchronization in public `openagents` where
Benchmark Cloud owns public benchmark contracts. Probe should consume the
resulting policy and emit evidence; Probe should not own Artanis policy.

## Work Order Shape

The deprecated Artanis Work Order model maps cleanly to the benchmark refocus.

Use four work classes:

- Support: split manifest repair, task metadata hygiene, fixture packaging,
  source-ref cleanup, public docs.
- Eval: retained runs, validation runs, verifier reruns, regression analysis.
- Integrity: artifact digest checks, redaction checks, receipt coverage,
  public-claim gating, no-cheat checks.
- AdapterTraining: later Qwen/LoRA work after GEPA creates clean trace
  corpora. This class should remain blocked in the first GEPA-only campaign
  unless an explicit operator gate opens it.

Initial Work Order templates:

- `probe_gepa_retained_replay`
- `probe_gepa_candidate_metric_batch`
- `probe_gepa_validation_replay`
- `probe_tool_use_trace_audit`
- `probe_artifact_integrity_check`
- `probe_public_claim_gate_review`
- `pylon_worker_capability_audit`
- `benchmark_cloud_split_manifest_repair`
- `qwen_probe_lora_trace_corpus_review`

Each Work Order should include:

- campaign id;
- benchmark suite and split refs;
- Probe commit;
- candidate hash when applicable;
- Pylon assignment refs when applicable;
- expected artifacts;
- verifier/scorer refs;
- closeout requirements;
- public projection summary shape;
- rollback or rejection path.

## Public Projection Fields

Artanis needs benchmark-campaign fields in the public report. The public
projection should include only safe refs and aggregates:

- `campaignRef`;
- `objectiveRef`;
- `stage`;
- `claimState`;
- `benchmarkSuiteRefs`;
- `splitManifestRefs`;
- `probeCommitRefs`;
- `baselineCandidateRef`;
- `activeCandidateRefs`;
- `candidateHashRefs`;
- `pylonBatchRefs`;
- `plannedMetricCalls`;
- `completedMetricCalls`;
- `validMetricCalls`;
- `invalidMetricCalls`;
- `retainedResultRefs`;
- `validationResultRefs`;
- `holdoutResultRefs`;
- `artifactManifestRefs`;
- `receiptRefs`;
- `costSummaryRefs`;
- `resourceReceiptRefs`;
- `policyFindingRefs`;
- `blockerRefs`;
- `promotionDecisionRefs`;
- `nextActionRefs`.

Public projection must not include raw prompts, raw traces, raw benchmark
fixtures, raw private repo paths, provider credentials, account refs, bearer
material, wallet material, payment ids, invoices, preimages, or local
filesystem paths.

## Claim Rules

Allowed claims after evidence exists:

- Artanis is coordinating a Probe benchmark campaign.
- Pylon workers ran bounded metric-call or verification assignments.
- A GEPA candidate improved retained fixtures, naming the retained suite and
  split.
- A candidate passed validation, naming the validation split and scorer.
- A public-safe artifact manifest and receipt set exists for a specific batch.

Blocked claims until further proof:

- Artanis autonomously improved Probe in production.
- Probe beats Terminal-Bench 2.
- A retained-fixture win is public holdout performance.
- Pylon paid work is fully settled from Artanis assignment id unless the
  deployed production bridge proves it.
- A GEPA candidate is active production runtime before Omega release approval.
- A Qwen/LoRA adapter improved Probe before model-training evidence and
  validation exist.
- Private Cloud bootstrap contracts are the public benchmark authority.

## Implementation Roadmap

1. Public Benchmark Cloud source migration
   Move or rebuild the relevant private Cloud benchmark/Artanis contract
   source into public `openagents` Benchmark Cloud contracts. Keep private
   Cloud as source material only.

2. Artanis campaign schema in Omega
   Extend the active Omega Artanis public report, work-routing, and
   continual-learning-template contracts with Probe GEPA campaign fields,
   using the existing Effect schema style and no-direct-authority defaults.

3. Probe closeout export
   Add Probe benchmark closeout exports that include candidate hash, run
   config, tool-use summary, verifier/scorer refs, artifact manifest refs,
   public-safe cost/resource refs, and redaction state.

4. Benchmark Cloud import
   Add public Benchmark Cloud score/import records for Probe runs, with split
   identity, scorer version, no-cheat metadata, and artifact digests.

5. Pylon GEPA assignment bridge
   Add a Pylon assignment path for `gepa_dspy_optimization` and
   `benchmark_evaluation` work that records Artanis campaign id, assignment
   id, candidate hash, metric batch id, split refs, and receipt refs.

6. Artanis importer
   Add an Artanis importer that reads public Benchmark Cloud and Pylon receipt
   refs, updates campaign state, and refuses to project unsafe/private data.

7. Stage 0 smoke
   Run a small retained batch locally or on the SHC box through Pylon with
   Probe as the runtime and GEPA as a text-candidate optimizer. The claim state
   is measured retained smoke only.

8. Stage 1 retained sprint
   Run the planned GEPA retained-failure campaign through Pylon metric-call
   batches. Publish public-safe batch summaries, not public benchmark claims.

9. Validation and release review
   Replay the accepted candidate on validation splits. Only then open an Omega
   release-review path for a Probe/Blueprint text artifact candidate.

10. Later model training
    After clean traces and failure-family labels exist, route Qwen/LoRA
    candidates through Psionic and Pylon trainer lanes. Keep AdapterTraining
    blocked until explicit operator, budget, and model-artifact gates pass.

## Immediate Issues To Open

Equivalent implementation issues should exist outside Probe because Probe is
not the owner of every surface:

- In `openagents`: create public Benchmark Cloud contracts for Probe GEPA
  campaigns and Pylon metric-call receipts.
- In `autopilot-omega`: add Artanis Probe GEPA campaign projection, work
  routing, and public report fields.
- In `probe`: emit benchmark closeout exports suitable for Benchmark Cloud and
  Artanis import.
- In `openagents` or `pylon`: add the Pylon assignment/receipt bridge for GEPA
  metric-call batches with Artanis campaign ids.
- In `psionic`: register the GEPA candidate frontier and later Qwen/LoRA trace
  corpus path, with promotion evidence separated from model-training launch
  authority.

The first Probe-only task is narrower: define and emit the closeout artifact
shape Artanis needs. Anything involving public projection, release gates,
Pylon dispatch, payment/settlement, or Benchmark Cloud authority belongs in
the owning repo.

## End State

Artanis should make the Probe benchmark learning loop legible in public
without weakening any authority boundary:

```text
Artanis says what campaign is running, what evidence exists, what is blocked,
what Pylons did, what Probe produced, what Benchmark Cloud scored, what GEPA
candidate is under review, and what claim state is allowed.
```

That gives OpenAgents the build-in-public proof surface the Omni docs want
while keeping Probe focused on the coding-agent runtime and evidence stream.
