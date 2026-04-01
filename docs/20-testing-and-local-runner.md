# Testing And Local Runner

Probe now has a real shared test-support boundary and a canonical local runner
contract.

## What Exists Now

The workspace now includes `crates/probe-test-support`.

That crate owns reusable helpers for:

- fake OpenAI-compatible backend servers
- fake Apple FM bridge responders
- temp Probe home and workspace setup
- seeded coding-workspace fixtures
- CLI binary launch helpers
- shared fake attach-config writers
- stderr, transcript, and acceptance-report normalization for snapshots
- explicit `INSTA_WORKSPACE_ROOT` setup for stable snapshots

The immediate goal is not to centralize every test helper in one patch. The
goal is to stop re-implementing the same fake-backend and temp-workspace logic
inside unrelated crate test modules.

The first adoption points now are:

- `probe-provider-openai` tests
- `probe-core` runtime tests
- `probe-cli` regression tests
- `probe-tui` snapshot tests

Probe now also has explicit top-level suite files for the runtime-facing test
domains that were previously mostly trapped inside large inline modules:

- `crates/probe-core/tests/runtime_suite.rs`
- `crates/probe-core/tests/tool_suite.rs`
- `crates/probe-core/tests/policy_suite.rs`
- `crates/probe-core/tests/server_suite.rs`
- `crates/probe-provider-openai/tests/provider_suite.rs`
- `crates/probe-provider-apple-fm/tests/provider_suite.rs`
- `crates/probe-tui/tests/runtime_suite.rs`

Those suites are intentionally coarse-grained and ownership-oriented. The goal
is to give future patches a stable landing zone for runtime, provider, tool,
policy, server-control, and TUI coverage before we decide whether more of the
remaining inline tests should migrate out.

Those are the highest-value places because they drive most of the backend,
session-loop, binary, and UI behavior that later acceptance, self-test, and
matrix suites depend on.

Probe does not have a real MCP boundary yet, so the shared support crate stops
at the repo's actual current seams instead of inventing fake MCP fixtures
prematurely.

## Local Runner Contract

Probe now ships a top-level `probe-dev` script so the repo has one canonical
local validation surface without depending on extra task-runner installs.

Primary commands:

```bash
./probe-dev fmt
./probe-dev fmt-check
./probe-dev check
./probe-dev test
./probe-dev integration
./probe-dev accept-live
./probe-dev self-test
./probe-dev accept-compare
./probe-dev matrix-eval
./probe-dev optimizer-eval decision-export
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
- `./probe-dev integration`
  - deterministic subprocess integration lane for built-binary chat and TUI
    coverage
- `./probe-dev accept-live`
  - heavier live-backend acceptance lane
- `./probe-dev self-test`
  - heavier retained first-person Probe behavior lane
- `./probe-dev accept-compare`
  - heavier admitted-Mac Apple FM versus Qwen comparison lane
- `./probe-dev matrix-eval`
  - heavier scenario-matrix lane with worst-of-N retention per cell
- `./probe-dev optimizer-eval <lane>`
  - umbrella entrypoint for offline export, eval, and optimization lanes that
    stay out of the fast path
- `./probe-dev decision-export`, `./probe-dev module-eval`,
  `./probe-dev optimize-modules`, `./probe-dev optimize-harness`, and
  `./probe-dev optimize-skill-packs`
  - retained direct aliases for the same offline eval and optimization work

The binary lane now covers more than `probe exec`.

It includes:

- real subprocess `probe chat` session creation and resume coverage
- a hidden headless `probe tui` smoke path used only by tests so the binary can
  submit a real background turn, wait for the reply, and retain a structured
  report artifact without requiring PTY orchestration in the test harness

## Local Execution Policy

Probe keeps these tiers local and explicit rather than mirroring them in
GitHub workflows.

- `./probe-dev pr-fast`
  - default precommit and pre-push lane
- `./probe-dev integration`
  - targeted subprocess regression lane
- `./probe-dev accept-live` and `./probe-dev self-test`
  - operator-triggered live backend lanes
- `./probe-dev matrix-eval`, `./probe-dev accept-compare`, and
  `./probe-dev optimizer-eval <lane>`
  - heavier local-only lanes for admitted hardware, repeated runs, or offline
    optimization work

That is an intentional repo policy, not an omission. Probe does not keep a
GitHub CI copy of these commands.

## Why This Matters

This patch is the testing substrate for the next steps:

- process-level CLI regression tests
- stable snapshot receipts
- richer acceptance-report metadata
- clearer local test-tier orchestration
- domain-partitioned suite ownership across runtime, providers, policy, and UI

Without a shared support crate, each of those would keep growing ad hoc test
scaffolding inside unrelated crates.
