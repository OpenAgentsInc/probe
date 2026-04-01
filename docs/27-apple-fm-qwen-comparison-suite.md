# Apple FM Qwen Comparison Suite

Issue `#28` adds the first Probe-owned comparison lane for the overlapping
Apple FM and Psionic Qwen coding cases.

## Goal

Probe should be able to answer a narrow backend question with one local
artifact:

"How did Apple FM behave on the retained overlapping Probe workloads relative
to the current Psionic Qwen lane?"

The comparison lane is not trying to force both backends into a fake identical
model. It is trying to make overlap explicit and honest.

## Commands

Single-backend accepted lanes:

```bash
cargo run -p probe-cli -- accept
cargo run -p probe-cli -- accept --profile psionic-apple-fm-bridge
```

Comparison lane:

```bash
cargo run -p probe-cli -- accept-compare
```

Optional comparison overrides:

- `--qwen-profile`
- `--apple-profile`
- `--qwen-base-url`
- `--qwen-model`
- `--apple-base-url`
- `--apple-model`
- `--probe-home`
- `--report-path`

## Current Case Set

The retained overlapping comparison set is currently the same six
`coding_bootstrap` cases used by the main acceptance harness:

- `read_file_answer`
- `list_then_read`
- `search_then_read`
- `shell_then_summarize`
- `patch_then_verify`
- `approval_pause_or_refusal`

That is intentional.

Probe already has real Apple FM coverage for plain-text turns, tool-backed
coding turns, refusal, pause, and transcript-backed resume. The comparison
lane should start from the cases both backends can actually run today.

## Artifact Shape

`accept-compare` produces:

- one comparison report
- one backend-specific Qwen acceptance report
- one backend-specific Apple FM acceptance report

The comparison report records:

- comparison run identity and schema version
- Qwen backend identity
- Apple FM backend identity
- shared harness metadata
- aggregate comparable-pass, comparable-fail, and unsupported counts
- per-case comparison status
- per-backend case status:
  - `passed`
  - `failed`
  - `unsupported`
- per-backend embedded case receipts when the backend actually ran the case

Because the embedded backend case reports are retained, operators can inspect:

- failure categories
- tool-policy counts
- observability summaries
- backend-receipt summaries
- transcript paths

without having to stitch multiple files together by hand.

## Unsupported Posture

The comparison model has an explicit unsupported state even though the current
retained overlapping set is fully supported by both backends.

That matters for future growth:

- if a future retained case only makes sense on one backend, Probe should say
  `unsupported`
- it should not silently skip the case
- it should not relabel unsupported as a generic backend failure

## Operating Assumption

This is an admitted-Mac intentional lane.

For now `accept-compare` expects both endpoints to already be reachable:

- the Psionic Qwen OpenAI-compatible endpoint
- the Apple FM bridge endpoint

It is not a merge-safe default path. It is an explicit local admitted-Mac lane
run through `./probe-dev accept-compare` when an operator actually has the
required hardware and backend reachability.

## What It Proves

The comparison suite can support claims like:

- both backends passed or failed the same retained Probe-owned case
- one backend showed different observability or receipt posture on the same
  case
- a result was unsupported rather than comparable

## What It Does Not Prove

The comparison suite does not claim:

- global capability parity
- throughput parity outside the retained case set
- that unsupported cases are equivalent to failures
- that Apple FM and Qwen expose identical backend semantics

The point is a compact honest comparison receipt, not a benchmark marketing
sheet.
