# Probe Docs

This folder holds technical planning docs for the Probe runtime.

## Table Of Contents

- `01-psionic-qwen-hermes-deep-dive-and-probe-cli-roadmap.md`
  - deep dive on the prior Psionic Hermes/Qwen work and the concrete roadmap
    for consuming that backend from the first Rust Probe CLI
- `02-runtime-ownership-and-boundaries.md`
  - ownership line for what Probe should own itself and what it should consume
    from the backend substrate
- `03-workspace-map.md`
  - initial crate map for the Probe Rust workspace
- `04-session-turn-item-and-transcript-model.md`
  - the first durable local truth model for sessions, turns, items, and
    append-only transcript storage
- `05-openai-compatible-provider-client.md`
  - the first typed backend client seam for local OpenAI-compatible endpoints
- `06-psionic-qwen35-backend-profile.md`
  - the first explicit built-in backend profile for a local Psionic-served
    Qwen3.5 model
- `07-probe-exec.md`
  - the first non-interactive end-to-end Probe controller lane with local
    transcript persistence
- `08-interactive-cli-and-resume.md`
  - the first interactive session loop and transcript-backed resume flow
- `09-tool-loop-and-local-tools.md`
  - the first bounded local tool runtime, batch execution path, and replay
    contract
- `10-acceptance-harness.md`
  - the retained local acceptance runner for plain and tool-backed controller
    cases
- `11-server-attach-and-launch.md`
  - local server config, attach mode, and supervised launch behavior for
    `psionic-openai-server`
- `12-observability-and-cache-signals.md`
  - per-turn wallclock, token usage, throughput, and cache-signal behavior for
    the first local controller lane
- `13-harness-profiles.md`
  - Probe-owned versioned harness profiles for the coding tool lane, including
    the first `coding_bootstrap_default@v1` profile and its relationship to
    `--system`
- `14-approval-classes-and-structured-tool-results.md`
  - explicit risk classes, local approval policy, CLI approval controls, and
    structured tool-result records for the coding tool lane
- `15-replay-and-decision-dataset-export.md`
  - local-first JSONL export for replay and derived decision datasets from
    real Probe sessions
- `16-decision-modules.md`
  - narrow Rust-native decision-module boundary above the runtime, plus the
    first offline-evaluable `ToolRoute` and `PatchReadiness` modules
- `17-offline-optimizer.md`
  - GEPA-style offline optimization receipts, shared promotion rules, and the
    baseline-versus-candidate comparison flow for modules and harness reports
- `18-oracle-consultation.md`
  - typed bounded oracle consultation as an auxiliary tool and backend role
- `19-long-context-repo-analysis.md`
  - opt-in bounded repo-analysis escalation with explicit evidence pointers,
    budgets, and transcript-visible provenance
- `20-testing-and-local-runner.md`
  - shared test-support helpers, canonical local validation commands, and the
    `nextest`-first runner contract
- `21-cli-regression-and-snapshots.md`
  - process-level binary tests, narrow snapshot coverage, and the normalized
    receipt boundary for the CLI surface
- `22-acceptance-report-schema.md`
  - run identity, backend and harness metadata, failure categories, counts,
    and transcript references for the richer `probe accept` report
- `23-local-test-tiers.md`
  - explicit local fast-test, binary-regression, live-acceptance, and
    offline-eval lanes in `probe-dev`
- `24-apple-fm-backend-lane.md`
  - the first real Apple FM backend lane for plain-text turns, server attach,
    and bounded oracle use
- `25-apple-fm-tool-lane.md`
  - session-backed Apple FM coding turns through Probe-owned tool callbacks,
    Probe transcript replay, and the existing approval or refusal policy
