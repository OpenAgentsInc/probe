# Replay And Decision Dataset Export

## Purpose

Probe now has a first local-first dataset export path for real sessions.

The goal is to stop treating transcripts as human-only debugging artifacts and
start treating them as optimizer input.

## CLI Surface

Probe now accepts:

```bash
cargo run -p probe-cli -- export \
  --dataset replay \
  --output ~/.probe/reports/probe_replay.jsonl
```

and:

```bash
cargo run -p probe-cli -- export \
  --dataset decision \
  --output ~/.probe/reports/probe_decision.jsonl
```

and:

```bash
cargo run -p probe-cli -- export \
  --dataset decision-cases \
  --output ~/.probe/reports/probe_decision_cases
```

Optional scope controls:

- `--session <id>`
  - export one specific session
- `--all-sessions`
  - widen beyond the default coding-session filter

Without `--all-sessions`, Probe exports coding-lane sessions by default.

## Replay Dataset

The replay dataset is the closest thing to raw runtime truth.

Each JSONL record currently includes:

- session id
- title
- cwd
- backend profile and model when known
- harness profile when known
- turn count
- full serialized transcript

This is the format to use when later work needs to reconstruct or re-score
actual controller traces.

## Decision Dataset

The decision dataset is the first derived summary layer above the transcript.

Each JSONL record currently includes fields such as:

- `first_tool_name`
- `tool_names`
- `files_listed`
- `files_searched`
- `files_read`
- `patch_attempts`
- `successful_patch_attempts`
- `failed_patch_attempts`
- `verification_step_count`
- `verification_caught_problem`
- `too_many_turns`
- auto-allowed, approved, refused, and paused tool-call counts
- `oracle_calls`
- `long_context_calls`
- `repo_analysis_files`
- likely-warm turn count
- cache-reuse latency and throughput improvement booleans
- final assistant text when present

This is the format to use when later decision modules, harness tuning, or GEPA
jobs need compact per-session receipts instead of full transcript replay.

## Decision Case Bundle

`decision-cases` widens the export surface from one row per session to one row
per observed decision point.

Probe now derives turn-level cases for:

- `tool_route`
- `patch_readiness`
- `long_context_escalation`

Each case records:

- stable `case_id` plus a content digest
- deterministic train or validation split membership
- pre-decision typed context
- observed label or outcome
- source session id, turn index, and transcript path
- transcript item refs for later inspection

The export path writes a bundle directory containing:

- `decision_cases_all.jsonl`
- `decision_cases_train.jsonl`
- `decision_cases_val.jsonl`
- `decision_case_split_manifest.json`

That split manifest is the canonical retained-case inventory for later Probe to
Psionic optimizer jobs.

## Privacy And Scope Boundary

The export path is intentionally local-first.

It writes JSONL files to an operator-chosen local path and does not send
session data anywhere by itself.

Operators should still treat replay exports as sensitive because they can
contain:

- user prompts
- assistant responses
- tool arguments
- tool outputs
- file contents read through `read_file`

The first implementation makes this boundary explicit rather than pretending
exports are automatically safe to share.

## Relation To Later DSPy And GEPA Work

This export path is the bridge between the runtime issues and the optimizer
issues.

Probe can now produce:

- replay records for offline trace inspection and reranking
- decision records for studying tool order, read/search patterns, patching,
  verification, approval behavior, and cache effects
- decision-case bundles with stable train or validation membership and
  transcript provenance for module-family evaluation

That means later DSPy/GEPA work can consume stable exported data instead of
scraping ad hoc logs or trying to infer policy behavior from free-form text.
