# Acceptance Harness

## Purpose

`probe accept` is the retained local acceptance runner for the Probe coding
controller lane.

It is a Probe-owned harness above the runtime, not a backend-side test.

The same harness can now be pointed at Apple FM intentionally, and Probe now
also has a separate comparison command for Apple FM versus Psionic Qwen.

## Cases

The current runner records retained coding-lane truth for six cases:

- `read_file_answer`
- `list_then_read`
- `search_then_read`
- `shell_then_summarize`
- `patch_then_verify`
- `approval_pause_or_refusal`

Each case currently runs twice so the report includes a small repeat-run
receipt instead of only a single pass/fail datapoint.

## CLI Shape

```bash
cargo run -p probe-cli -- accept
cargo run -p probe-cli -- accept --profile psionic-apple-fm-bridge
cargo run -p probe-cli -- accept-compare
```

Optional overrides:

- `--profile <profile_name>` for `probe accept`
- `--base-url <url>`
- `--model <model_id>`
- `--probe-home <path>`
- `--report-path <path>`

`probe accept-compare` currently accepts:

- `--qwen-profile <profile_name>`
- `--apple-profile <profile_name>`
- `--qwen-base-url <url>`
- `--qwen-model <model_id>`
- `--apple-base-url <url>`
- `--apple-model <model_id>`
- `--probe-home <path>`
- `--report-path <path>`

## Output

The runner writes a JSON report that records:

- run identity and schema version
- Probe version plus best-effort git SHA and dirty-state provenance
- backend profile, base URL, and model metadata
- harness tool set, harness profile, and repeat-run count
- aggregate case and attempt counts
- per-case pass/fail plus passed-attempt and failed-attempt counts
- per-case median elapsed time when available
- per-case latest session id and transcript path when available
- per-case latest tool names, policy counts, and observability summary
- per-attempt failure category when an attempt does not verify cleanly
- per-attempt session id, transcript path, assistant text, policy counts, and
  observability summary
- error text when a case emits an error path even if that path is the expected
  passing behavior for the case

`probe accept-compare` writes one separate comparison artifact plus two
backend-specific acceptance reports under the comparison run root.

When reading the report, the main things to watch are:

- correctness:
  - did the case pass across every retained repeat run
- provenance:
  - which Probe revision and local repo state produced the receipt
- tool behavior:
  - did the expected coding tools actually appear
  - did the policy counts match the intended lane
- failure typing:
  - when a case does fail, is the breakage backend-side, tool-side,
    policy-related, or simple verification drift
- performance:
  - did elapsed time or observability receipts regress materially between runs

## Current Operating Assumption

The runner can now use the same attach-or-launch server preparation path as the
other CLI commands.

In practice that means:

- default attach mode checks an already-running local server
- launch mode can supervise `psionic-openai-server` for the lifetime of the
  acceptance command
- the runtime contract below the harness now overlaps with Apple FM through the
  session-backed callback lane
- `probe accept` still treats the Qwen lane as the retained default unless the
  operator explicitly switches profiles
- `probe accept-compare` is an intentional admitted-Mac attach lane, not the
  merge-safe default path

## Validation Boundary

The repo now validates the coding-lane harness logic against a mocked local
HTTP server in Rust tests.

A real live run still depends on the operator having:

- a reachable local `psionic-openai-server`
- the retained Qwen3.5 model lane available on the host

Those artifacts are not bundled into this repo.
