# Probe Benchmark Docs

Date: 2026-06-08

This folder tracks how Probe should participate in public benchmark execution,
continual improvement, and optimizer-driven promotion. The docs here are about
architecture and execution plans. They are not public benchmark claims.

## Current Reading Order

1. `2026-06-08-workspace-benchmark-systems-audit.md`
   Inventory of the benchmark systems across Probe, private Cloud source
   material, public OpenAgents Benchmark Cloud target architecture, Psionic,
   Pylon, Omega, and historical repos. Start here when deciding which repo owns
   which part of the benchmark apparatus.

2. `2026-06-08-probe-continual-benchmark-learning-apparatus.md`
   End-state plan for Probe's continual improvement loop. It defines how Probe,
   public OpenAgents Benchmark Cloud, Psionic, Pylon, and Omega should turn
   benchmark failures into prompt, Blueprint, tool-menu, loop-policy, and LoRA
   candidates with explicit promotion gates.

3. `2026-06-08-pylon-gepa-coding-agent-benchmark-run.md`
   First executable optimizer plan. It narrows the initial benchmark-climbing
   work to a GEPA-only text-candidate campaign, using Pylon as the parallel
   rollout engine across retained Terminal-Bench failures, validation splits,
   and frozen holdout tasks.

## Current Decisions

- The benchmark apparatus should be public OpenAgents infrastructure.
- The private `cloud` repo is source material and backfill, not the desired
  long-term benchmark authority.
- Public Benchmark Cloud should be rebuilt or moved into `openagents`, with
  public docs, contracts, scripts, fixtures, and eventually stable protocol
  surfaces where needed.
- Probe should be the coding-agent runtime and evidence emitter. It should not
  become the benchmark product, scorer, public-claim authority, or promotion
  authority.
- Terminal-Bench 2 through Harbor on the SHC box is the first live coding
  benchmark lane.
- The first optimizer run should be GEPA-only over text artifacts: Probe
  prompts, Blueprint usage, Program Signature playbooks, tool-menu policy,
  failure-family playbooks, and closeout instructions.
- LoRA, DPO, GRPO, and Qwen3.6 adapter work should come after the GEPA lane
  creates clean traces, candidate diffs, verifier outcomes, and split-aware
  evidence.
- Pylon should provide distributed rollout and training capacity with explicit
  worker capability envelopes and signed receipts.
- Omega should remain the release gate and projection surface for public and
  private benchmark evidence.

## Public Claim Boundaries

Do not call retained fixture improvements public benchmark scores. Do not call
validation-split GEPA improvements frozen holdout performance. Do not publish
"Probe beats Terminal-Bench" from retained, validation, local smoke, or
optimizer-accepted evidence.

Every claim should name:

- dataset and version;
- split;
- task selector;
- agent slug;
- model/backend;
- Probe commit;
- candidate hash;
- retry and timeout policy;
- verifier or scorer result;
- cost and duration;
- artifact availability;
- redaction state;
- whether the evidence is retained, validation, frozen holdout, or live public
  claim evidence.

## Maintenance Rule

When adding a benchmark doc to this folder, update this README with its purpose,
status, and reading-order position. If the new doc changes ownership,
promotion gates, public claim boundaries, or the immediate implementation
sequence, update the older docs instead of leaving contradictory plans in the
folder.
