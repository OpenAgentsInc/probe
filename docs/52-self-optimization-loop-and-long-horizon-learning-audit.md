# Self-Optimization Loop And Long-Horizon Learning Audit

This document audits what Probe still needs in order to optimize itself in a
real loop.

The target is not "let the agent roam forever and hope."

The target is a bounded, operator-auditable system where Probe can:

- study its own codebase over long horizons
- gather evidence with real tools
- propose and test candidate improvements
- score those candidates against retained cases
- adopt better behavior without silently corrupting runtime truth

Related docs:

- `docs/10-acceptance-harness.md`
- `docs/15-replay-and-decision-dataset-export.md`
- `docs/16-decision-modules.md`
- `docs/17-offline-optimizer.md`
- `docs/18-oracle-consultation.md`
- `docs/19-long-context-repo-analysis.md`
- `docs/51-probe-optimizer-system.md`

## Honest Current State

Probe already has the first offline optimizer stack.

That stack is real and useful:

- retained sessions and append-only transcripts
- retained acceptance receipts
- replay and decision-case export
- offline decision-module evaluation
- offline harness evaluation
- optimizer bundles and promotion ledgers
- explicit adoption states such as `not_adopted`, `shadow`, and `promoted`

But this is still an offline comparison system.

Probe does not yet have a true self-optimization loop where the agent can:

- nominate its own weaknesses from real failures
- launch a long-running study campaign over its own codebase
- convert that study into candidate behavior changes
- run those candidates against retained evals automatically
- shadow those candidates in live operation
- promote them only after retained wins

That missing controller loop is the main gap.

## What Probe Already Has That We Should Reuse

The good news is that the repo already contains the correct early building
blocks.

### Retained Runtime Truth

`probe-core` already retains:

- transcripts
- tool calls and tool results
- approval outcomes
- observability receipts
- backend receipts
- harness profile provenance

That means Probe already has real evidence to learn from.

### Typed Decision Surfaces

`probe-decisions` already exposes narrow tunable surfaces instead of pretending
the whole runtime is learnable mush.

Current families such as:

- `tool_route`
- `patch_readiness`
- `long_context_escalation`

are exactly the right pattern.

### Offline Search And Promotion

`probe-optimizer` plus Psionic already give Probe:

- candidate manifests
- search receipts
- scorecards
- promotion reports
- adoption ledgers

That is the right substrate for bounded improvement.

### Bounded Auxiliary Intelligence

Probe already has:

- `consult_oracle`
- `analyze_repository`

Those are important because self-improvement needs auxiliary reasoning, but
that auxiliary reasoning must stay subordinate to the main controller loop.

## What Is Missing

To become a real self-optimizing system, Probe needs six missing layers.

### 1. A First-Class Optimize Campaign Primitive

Right now Probe has sessions and offline optimizer bundles, but not a runtime
object for a long-horizon self-improvement campaign.

It needs a new retained unit such as an `optimize_campaign` or
`self_improvement_task` with explicit fields like:

- campaign id
- target repo and cwd
- target surface
  - `study_only`
  - `decision_module`
  - `harness_profile`
  - `skill_pack`
  - later `workflow_recipe`
- goal statement
- operator-provided scope
- allowed tools
- wallclock, token, and tool-call budgets
- required evals
- approval posture
- current phase
- candidate refs produced so far
- final disposition

Without this, Probe has no durable object that connects:

- the observed failure
- the long study session
- the produced candidate
- the eval result
- the promotion decision

### 2. A Long-Horizon Study Lane

Probe can already do bounded repo analysis, but that is not yet the same thing
as a self-study loop.

The missing lane is a persistent study workflow that can spend many tool calls
learning the codebase before trying to edit anything.

This should not be implemented as one opaque mega-tool call.

That would destroy:

- transcript visibility
- resumability
- budget control
- approval boundaries
- replay quality

The right model is:

- one retained optimize campaign
- many normal Probe turns inside it
- explicit checkpoints after exploration, synthesis, proposal, edit, and
  verification

That gives Probe long-horizon behavior without hiding the trajectory.

### 3. Better Export Shapes For Long Trajectories

Current `decision-cases` are useful, but they are too shallow for real
self-optimization.

Probe needs additional retained exports for:

- exploration episodes
  - what files were listed, searched, and read before action
- verification episodes
  - what checks were run, what failed, and what those failures caught
- patch episodes
  - what changed, how large the diff was, and whether the change survived
    verification
- approval episodes
  - what risky actions were requested and why
- failure clusters
  - repeated backend, tool, or policy failure families
- codebase coverage maps
  - which files, directories, or subsystems the agent actually touched while
    solving a class of tasks
- candidate genealogy
  - which candidate was derived from which failure set, harness profile, and
    prior candidate lineage

Right now Probe can export decisions.

It cannot yet export a full long-horizon learning trace in a way that later
optimizer jobs can score cleanly.

### 4. More Tunable Surfaces Above The Runtime

Current decision families are a good start, but they are not enough for a
self-improvement loop.

Probe needs additional optimizer-visible surfaces such as:

- `exploration_plan`
  - when to list, search, read, or escalate
- `evidence_sufficiency`
  - whether the agent has enough proof to answer, patch, or escalate
- `verification_plan`
  - which checks to run after a change and in what order
- `diff_scope`
  - how large a patch is acceptable before requiring another evidence pass
- `campaign_route`
  - whether a failure should trigger:
    - no action
    - read-only study
    - candidate generation
    - harness retuning
    - skill-pack retuning
- `promotion_risk`
  - whether a candidate should stay `shadow` longer before promotion

These should remain manifest-backed and Probe-owned.

The runtime should still own tool execution, persistence, and approvals.

## The Biggest Safety Gap

The biggest safety gap is not model quality.

It is missing isolation.

If Probe is going to optimize itself, it cannot do that work directly in the
operator's live checkout with the same authority as normal chat.

It needs explicit isolation boundaries.

### Required Isolation Model

Every self-improvement campaign should run in one of three modes:

- `study_only`
  - read-only tools, export, synthesis, and candidate proposal only
- `candidate_patch`
  - edits allowed, but only inside an ephemeral worktree or sandbox
- `candidate_validation`
  - run verification and acceptance against the isolated candidate

That implies new Probe-owned machinery for:

- ephemeral Git worktrees
- retained candidate branch names or refs
- diff capture and diff size limits
- workspace cleanup on success, failure, or cancellation
- candidate artifact refs back into the optimizer bundle

Until this exists, "self-improvement" is too close to "let the agent modify
its own runtime in place."

That is not acceptable.

## The Control Loop We Actually Want

The correct loop is:

1. Observe real failures and weak spots from transcripts, acceptance reports,
   and decision datasets.
2. Nominate one bounded improvement target.
3. Launch a read-only study campaign in an isolated scope.
4. Produce candidate manifests or candidate code changes.
5. Run retained offline evals plus targeted acceptance.
6. If admitted, move the candidate to `shadow`.
7. Compare shadow behavior against the retained baseline.
8. Promote only when the candidate beats baseline without trust or correctness
   regressions.

This is much closer to:

- "continuous evidence-backed retuning"

than:

- "autonomous recursive self-rewrite"

That distinction matters.

## Long Tool Calls: What To Build And What To Avoid

The user goal here is long-horizon learning with real codebase navigation.

Probe should support that, but it should not express that as a single hidden
tool call that disappears into the substrate for minutes.

### Do Build

- long-running optimize campaigns with retained checkpoints
- bounded study budgets measured in turns, wallclock, tokens, and files
- explicit evidence ledgers
- resumable campaign state
- operator-visible progress
- transcript-visible summaries after each phase

### Do Not Build

- one giant hidden "self_optimize" tool that crawls forever
- direct mutation of runtime defaults without eval and promotion
- optimizer-side tool execution that bypasses Probe transcripts
- silent auto-promotion after one good run
- hidden recursive subagents with no retained provenance

The right abstraction is a campaign made of ordinary visible steps, not a
magic black-box tool.

## What Probe Needs In Each Crate

### `crates/probe-core`

Needs:

- retained optimize-campaign records
- campaign phase transitions
- isolated worktree management
- campaign budgets and kill switches
- richer export shapes for long trajectories
- failure-family clustering over retained sessions
- evidence ledgers and codebase coverage summaries

### `crates/probe-decisions`

Needs:

- new decision families for exploration, evidence sufficiency, verification,
  and campaign routing
- evaluators over long-horizon exported cases
- manifest families that stay narrow and comprehensible

### `crates/probe-optimizer`

Needs:

- new target kinds above the current trio
- candidate genealogy tracking
- longitudinal scorecards over multiple campaigns
- shadow-versus-baseline comparison artifacts
- promotion rules that account for trust, regressions, and campaign cost

### `crates/probe-cli`

Needs operator commands such as:

- `probe optimize-self nominate`
- `probe optimize-self study`
- `probe optimize-self eval`
- `probe optimize-self shadow`
- `probe optimize-self promote`
- `probe optimize-self cancel`

These do not need to be the final command names, but Probe needs an explicit
operator surface for the lifecycle.

### `crates/probe-tui`

Needs:

- a campaign inspector
- long-running progress and checkpoint display
- shadow-candidate comparison views
- candidate diff and verification review
- explicit adopt or reject controls

This should be treated as an operator shell for self-improvement, not just a
chat transcript.

## Eval And Acceptance Work Still Missing

If Probe is going to learn over long horizons, the acceptance set must get
deeper too.

The current six coding cases are not enough.

Probe needs retained evals for:

- repo exploration quality
- multi-file architecture synthesis
- edit planning before patching
- verification discipline
- recovery after failed edits
- refusal discipline when evidence is weak
- shadow-candidate regressions
- long-context escalation correctness
- oracle usage quality

This matters because self-improvement without stronger retained evals just
teaches the system to overfit its tiny initial harness.

## Anti-Fuckup Requirements

If Probe is going to optimize itself, these controls are mandatory.

### Immutable Baselines

Every optimization campaign needs a pinned baseline:

- runtime revision
- harness profile digest
- decision-module manifest digests
- skill-pack manifest digest
- backend profile and model

No campaign should compare against a moving target.

### Read-Only First

The default campaign mode should be `study_only`.

Candidate patching should require an explicit transition into an isolated
candidate workspace.

### Promotion Is Never Automatic

Probe may auto-search.
Probe may auto-score.
Probe may auto-shadow.

It should not auto-promote by default.

### Provenance Everywhere

Every candidate needs retained refs to:

- the failure set that motivated it
- the study trajectory that produced it
- the diffs it created
- the evals it passed
- the promotion rule that admitted it

### Fast Abort Paths

Campaigns need immediate stop conditions for:

- exploding tool counts
- runaway token usage
- repeated failed verification
- repeated backend parse failures
- repeated no-progress loops
- unexpectedly broad diffs

## Recommended Build Order

The shortest honest path is:

1. Add retained optimize-campaign records and CLI lifecycle commands.
2. Add read-only study campaigns with evidence ledgers and coverage summaries.
3. Widen dataset export to include exploration, patch, verification, and
   failure-cluster artifacts.
4. Add new decision families for exploration, evidence sufficiency, and
   verification planning.
5. Add isolated candidate worktrees and candidate-validation campaigns.
6. Add shadow comparison and longitudinal scorecards.
7. Only then consider broader automatic nomination and scheduling.

That order keeps Probe grounded in retained evidence at every step.

## The Real Near-Term Goal

The near-term goal should not be:

- "Probe rewrites itself continuously"

The near-term goal should be:

- "Probe can repeatedly study its own failures, propose bounded improvements,
  evaluate them against retained data, and safely adopt them through explicit
  promotion stages"

That is the first self-optimization loop worth trusting.
