# Decision Modules

## Purpose

Probe now has a narrow Rust-native decision-module layer in
`crates/probe-decisions`.

The point is to add DSPy-style structure above the runtime without rebuilding
Probe around a second orchestration system.

## Boundary

The decision-module crate is explicitly subordinate to:

- the runtime turn loop
- the transcript model
- the tool runtime
- the approval and policy layer

It does not execute tools.

It does not replace the session runtime.

It consumes derived session summaries and produces typed decisions or
scorecards.

## First Module Families

The first shipped module families are:

- `ToolRoute`
  - ranks or selects the next tool family to use
- `PatchReadiness`
  - decides whether the session has enough evidence to justify editing
- `LongContextEscalation`
  - decides whether a bounded repo-analysis escalation is justified or whether
    the session should stay on the normal coding lane

The current baseline and candidate variants now live as serializable manifest
artifacts rather than only unit structs in Rust.

Rust still stays authoritative for evaluation and decision execution. The
manifests make the candidate family optimizer-visible without moving runtime
truth out of Probe.

## Runtime-Only Signatures

Not every DSPy-style signature needs to become an offline-eval family on day
one.

Probe now also carries a typed GitHub issue-selection signature in
`crates/probe-decisions`:

- input:
  - priority text
  - discovered repo contexts
  - open GitHub issue candidates
- output:
  - selected repo owner
  - selected repo name
  - selected issue number
  - selected issue title
  - typed no-match when nothing is relevant

The current TUI uses that signature online, not through the decision-case
export path yet.

That is intentional. The signature is real and typed now, while the offline
dataset and optimizer lane for issue selection can come later after we have
enough grounded traces.

Probe also now carries a typed runtime-only `rlm_trigger` signature for
issue-thread routing:

- input:
  - operator override (`auto`, `direct`, `rlm`)
  - explicit issue-reference signal
  - selected GitHub issue-thread handle
  - coarse corpus-size / long-context pressure hints
- output:
  - selected strategy (`direct`, `compact`, `rlm`)
  - concrete execution strategy id
  - trigger reason
  - optional RLM budget envelope

That signature is the current Probe-side integration point between GitHub
issue selection and the paper RLM runtime. It stays typed and replayable
without pretending the offline decision-case export already has grounded
issue-thread route labels.

## Offline Evaluation Path

Probe now supports an offline evaluation loop:

1. export a decision dataset:

```bash
cargo run -p probe-cli -- export \
  --dataset decision-cases \
  --output ~/.probe/reports/probe_decision_cases
```

2. evaluate the current heuristic modules against that dataset:

```bash
cargo run -p probe-cli -- module-eval \
  --dataset ~/.probe/reports/probe_decision_cases
```

The current CLI can still read the old per-session summary JSONL, but the
canonical path is now the decision-case bundle with train or validation
membership and transcript provenance.

The current built-in candidate manifests cover:

- `heuristic_tool_route_v1`
- `aggressive_tool_route_v2`
- `heuristic_patch_readiness_v1`
- `strict_patch_readiness_v2`
- `heuristic_long_context_escalation_v1`

Each manifest carries:

- a stable candidate id
- a family id
- typed rule schemas
- a stable manifest digest
- a Rust evaluator that scores real exported decision cases

## Why This Matters

This is the first honest place to attach later DSPy/GEPA-style optimization.

Probe should optimize:

- tool routing
- patch readiness
- verification planning
- later oracle escalation
- later long-context escalation thresholds and evidence gating

Probe should not optimize by turning:

- `read_file`
- `apply_patch`
- session storage
- approval policy ownership

into pseudo-DSPy artifacts.

The runtime stays the authority.

The decision modules sit above it and become the tunable comparison surface.
