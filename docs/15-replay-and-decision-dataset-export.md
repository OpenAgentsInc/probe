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

and:

```bash
cargo run -p probe-cli -- export \
  --dataset signature-cases \
  --output ~/.probe/reports/probe_signature_cases
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

## Signature Case Bundle

`signature-cases` emits one validation-only case for each selected signature on
a session that carried `SessionSignatureContext`.

Each case records:

- selected signature id, version, adoption state, source ref, rank, score, and
  reason code
- selected signature ids and runner-up signatures from the selection decision
- recommended tool set, recommended tool choice, actual tool choice when
  observed, forbidden tools, and tool-policy counts
- result status, typed failure type, verifier outcome, and a hash of final
  assistant text when present
- source session id, transcript path, and transcript item refs
- an explicit `outcome_label`, currently `unknown` until an evaluator or human
  marks the signature as helped, hurt, or irrelevant

The export path writes:

- `signature_cases_all.jsonl`
- `signature_cases_val.jsonl`
- `signature_case_manifest.json`

The manifest sets `train_cases` to `0`. These cases are intentionally
validation-only until a separate signature-promotion job admits them into a
training set. This keeps raw selection scores and adoption state separate from
promotion authority.

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
- signature-case bundles for clustering failures by selected signature and
  typed failure reason before writing new failure-derived signatures

That means later DSPy/GEPA work can consume stable exported data instead of
scraping ad hoc logs or trying to infer policy behavior from free-form text.
