# Testing And Local Runner

Probe now has a real shared test-support boundary and a canonical local runner
contract.

## What Exists Now

The workspace now includes `crates/probe-test-support`.

That crate owns reusable helpers for:

- fake OpenAI-compatible backend servers
- temp Probe home and workspace setup
- seeded coding-workspace fixtures
- workspace-root path normalization for later snapshot tests

The immediate goal is not to centralize every test helper in one patch. The
goal is to stop re-implementing the same fake-backend and temp-workspace logic
inside unrelated crate test modules.

The first adoption points are:

- `probe-provider-openai` tests
- `probe-core` runtime tests

Those are the two highest-value places because they drive most of the local
backend and session-loop behavior that later CLI and acceptance tests depend
on.

## Local Runner Contract

Probe now ships a top-level `probe-dev` script so the repo has one canonical
local validation surface without depending on extra task-runner installs.

Primary commands:

```bash
./probe-dev fmt
./probe-dev fmt-check
./probe-dev check
./probe-dev test
./probe-dev accept-live
./probe-dev accept-compare
```

`./probe-dev test` is `nextest`-first. If `cargo nextest` is installed, Probe uses
`cargo nextest run --no-fail-fast`. If it is not installed, Probe falls back to
`cargo test --workspace`.

This keeps the default path fast where `nextest` is available without making
the repo unusable on a fresh machine.

The runner now also exposes explicit local tiers:

- `./probe-dev pr-fast`
  - merge-safe local lane: `fmt-check`, `check`, and `test`
- `./probe-dev cli-regressions`
  - targeted binary-level Probe CLI regression and snapshot lane
- `./probe-dev accept-live`
  - heavier live-backend acceptance lane
- `./probe-dev accept-compare`
  - heavier admitted-Mac Apple FM versus Qwen comparison lane
- `./probe-dev decision-export`, `./probe-dev module-eval`,
  `./probe-dev optimize-modules`, and `./probe-dev optimize-harness`
  - explicit local eval and optimization lanes that stay out of the fast path

## Why This Matters

This patch is the testing substrate for the next steps:

- process-level CLI regression tests
- stable snapshot receipts
- richer acceptance-report metadata
- clearer local test-tier orchestration

Without a shared support crate, each of those would keep growing ad hoc test
scaffolding inside unrelated crates.
