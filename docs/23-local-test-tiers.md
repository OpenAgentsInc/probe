# Local Test Tiers

Probe now has an explicit local tiering contract in `probe-dev`.

## Tier 1: Fast Local Merge-Safe Path

Use:

```bash
./probe-dev pr-fast
```

This lane is intentionally limited to:

- `fmt-check`
- `check`
- `test`

It is the command an operator should reach for before pushing or merging normal
runtime changes.

## Tier 2: Targeted Binary Regression Path

Use:

```bash
./probe-dev cli-regressions
```

This lane isolates the built-binary regression and snapshot surface for the
CLI. It is useful when iterating on command output, transcript receipts,
acceptance report shape, or other operator-visible behavior without rerunning
everything else.

## Tier 3: Live Acceptance Path

Use:

```bash
./probe-dev accept-live --help
./probe-dev accept-live
```

This lane is intentionally separate from `pr-fast` because it depends on a
reachable backend and produces heavier live receipts. It is the local coding
acceptance lane, not the fast merge-safe lane.

## Tier 4: Offline Eval And Optimization Paths

Use the wrappers for the existing Probe eval commands:

- `./probe-dev decision-export`
- `./probe-dev module-eval`
- `./probe-dev optimize-modules`
- `./probe-dev optimize-harness`

These remain local, explicit, and opt-in. They are part of the evaluation and
optimization workflow, not the default fast regression pass.
