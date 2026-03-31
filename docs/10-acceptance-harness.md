# Acceptance Harness

## Purpose

`probe accept` is the retained local acceptance runner for the Probe coding
controller lane.

It is a Probe-owned harness above the runtime, not a backend-side test.

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
```

Optional overrides:

- `--base-url <url>`
- `--model <model_id>`
- `--probe-home <path>`
- `--report-path <path>`

## Output

The runner writes a JSON report that records:

- base URL
- model id
- overall pass/fail
- repeat-run count per case
- per-case pass/fail
- per-case median wallclock when available
- per-attempt session id when available
- per-attempt assistant text when available
- per-attempt executed tool-call count
- per-attempt tool names
- per-attempt auto-allowed, approved, refused, and paused tool counts
- per-attempt final-turn observability receipts when available
- error text when a case fails

When reading the report, the main things to watch are:

- correctness:
  - did the case pass across every retained repeat run
- tool behavior:
  - did the expected coding tools actually appear
  - did the policy counts match the intended lane
- performance:
  - did wallclock or token receipts regress materially between runs

## Current Operating Assumption

The runner can now use the same attach-or-launch server preparation path as the
other CLI commands.

In practice that means:

- default attach mode checks an already-running local server
- launch mode can supervise `psionic-openai-server` for the lifetime of the
  acceptance command

## Validation Boundary

The repo now validates the coding-lane harness logic against a mocked local
HTTP server in Rust tests.

A real live run still depends on the operator having:

- a reachable local `psionic-openai-server`
- the retained Qwen3.5 model lane available on the host

Those artifacts are not bundled into this repo.
