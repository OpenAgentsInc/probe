# Acceptance Harness

## Purpose

`probe accept` is the retained local acceptance runner for the first Probe
controller lane.

It is a Probe-owned harness above the runtime, not a backend-side test.

## Cases

The current runner records pass/fail truth for four controller-facing cases:

- `no_tool_plain_answer`
- `required_single_tool_turn`
- `multi_turn_tool_continuation`
- `same_turn_two_tool_batch`

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
- per-case pass/fail
- session id when available
- assistant text when available
- executed tool-call count
- error text when a case fails

## Current Operating Assumption

The runner can now use the same attach-or-launch server preparation path as the
other CLI commands.

In practice that means:

- default attach mode checks an already-running local server
- launch mode can supervise `psionic-openai-server` for the lifetime of the
  acceptance command

## Validation Boundary

The repo now validates the harness logic against a mocked local HTTP server in
Rust tests.

A real live run still depends on the operator having:

- a reachable local `psionic-openai-server`
- the retained Qwen3.5 model lane available on the host

Those artifacts are not bundled into this repo.
