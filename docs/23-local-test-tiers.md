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
./probe-dev integration
```

This lane isolates the deterministic subprocess integration surface for the
built Probe binary. It is useful when iterating on command output, transcript
receipts, acceptance report shape, or other operator-visible behavior without
rerunning every workspace test.

The narrower CLI-only snapshot lane remains available as:

```bash
./probe-dev cli-regressions
```

That binary lane now includes:

- `probe exec`
- subprocess `probe chat` create and resume flows
- subprocess `probe tui` smoke coverage through the hidden headless driver used
  by tests

The adjacent crate-level integration suites now cover:

- runtime and session-loop behavior
- tool and tool-policy behavior
- server-control behavior
- provider adapters
- retained TUI runtime rendering

Those suites still run under the normal workspace `test` lane, but the docs now
name them explicitly so contributors stop treating the whole repo as one
undifferentiated cargo-test bucket.

## Tier 3: Live Acceptance Path

Use:

```bash
./probe-dev accept-live --help
./probe-dev accept-live
./probe-dev self-test --help
```

This lane is intentionally separate from `pr-fast` because it depends on a
reachable backend and produces heavier live receipts. It is the local coding
acceptance lane, not the fast merge-safe lane.

The retained self-test lane lives beside it:

- `cargo run -p probe-cli -- self-test`
- `./probe-dev self-test`

That lane keeps the same runtime and tool loop, but adds first-person cases for
shell failure, approval resume, multi-turn continuation, and backend-failure
honesty.

The heavier matrix lane lives above both:

- `cargo run -p probe-cli -- matrix --profile psionic-qwen35-2b-q8-registry`
- `./probe-dev matrix-eval --profile psionic-qwen35-2b-q8-registry`

That lane is for backend/model/harness/scenario combinations with repeated
runs and worst-of-N retention, not for the default retained coding receipt.

The Apple FM admitted-Mac paths live here too:

- `cargo run -p probe-cli -- accept --profile psionic-apple-fm-bridge`
- `cargo run -p probe-cli -- accept-compare`
- `./probe-dev accept-compare`

Those commands are explicit operator lanes, not merge-safe defaults.

Acceptance, self-test, and matrix remain operator-triggered because they still
depend on explicit backend reachability rather than guaranteed CI-local
inference.

## Tier 4: Offline Eval And Optimization Paths

Use the wrappers for the existing Probe eval commands:

- `./probe-dev optimizer-eval decision-export`
- `./probe-dev optimizer-eval module-eval`
- `./probe-dev optimizer-eval optimize-modules`
- `./probe-dev optimizer-eval optimize-harness`
- `./probe-dev optimizer-eval optimize-skill-packs`

Direct aliases still exist when a shorter local command is useful:

- `./probe-dev decision-export`
- `./probe-dev module-eval`
- `./probe-dev optimize-modules`
- `./probe-dev optimize-harness`
- `./probe-dev optimize-skill-packs`

These remain local, explicit, and opt-in. They are part of the evaluation and
optimization workflow, not the default fast regression pass.

Probe intentionally does not keep a GitHub CI mirror of these lanes.
