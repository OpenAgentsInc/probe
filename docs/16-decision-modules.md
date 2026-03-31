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

Both modules currently ship as simple heuristic baselines rather than learned
artifacts.

That is intentional. The point of this issue is to establish the boundary and
typed surfaces first.

## Offline Evaluation Path

Probe now supports an offline evaluation loop:

1. export a decision dataset:

```bash
cargo run -p probe-cli -- export \
  --dataset decision \
  --output ~/.probe/reports/probe_decision.jsonl
```

2. evaluate the current heuristic modules against that dataset:

```bash
cargo run -p probe-cli -- module-eval \
  --dataset ~/.probe/reports/probe_decision.jsonl
```

The current CLI prints simple scorecards for:

- `heuristic_tool_route_v1`
- `heuristic_patch_readiness_v1`

## Why This Matters

This is the first honest place to attach later DSPy/GEPA-style optimization.

Probe should optimize:

- tool routing
- patch readiness
- verification planning
- later oracle escalation

Probe should not optimize by turning:

- `read_file`
- `apply_patch`
- session storage
- approval policy ownership

into pseudo-DSPy artifacts.

The runtime stays the authority.

The decision modules sit above it and become the tunable comparison surface.
