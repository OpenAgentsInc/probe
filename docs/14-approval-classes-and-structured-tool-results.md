# Approval Classes And Structured Tool Results

## Purpose

Probe now has an explicit local approval layer for the `coding_bootstrap`
tool lane.

This closes the gap between a teaching harness and a real runtime by making
two things durable:

- which policy boundary applied to a tool call
- what actually happened when the tool was allowed, refused, or paused

## Risk Classes

Probe currently classifies built-in coding tools into the following runtime
risk classes:

- `read_only`
  - `read_file`
  - `list_files`
  - `code_search`
- `shell_read_only`
  - bounded shell commands that match the conservative read-only shell policy
- `write`
  - `apply_patch`
  - shell commands classified as local writes
- `network`
  - shell commands classified as networked or install-oriented
- `destructive`
  - shell commands classified as destructive

The shell classifier is intentionally conservative. Unknown shell commands do
not get auto-allowed just because they are shell.

## Default Policy

The default `coding_bootstrap` approval policy is:

- auto-allow `read_only`
- auto-allow `shell_read_only`
- refuse `write`
- refuse `network`
- refuse `destructive`

The CLI can widen that policy with:

- `--approve-write-tools`
- `--approve-network-shell`
- `--approve-destructive-shell`

The CLI can also switch denied risky tool calls from refusal into a persisted
pause with:

- `--pause-for-approval`

That gives Probe four explicit policy outcomes:

- `auto_allow`
- `approved`
- `refused`
- `paused`

## Transcript Contract

`tool_result` transcript items now carry a structured `tool_execution` record.

The current record includes:

- `risk_class`
- `policy_decision`
- `approval_state`
- `command`
- `exit_code`
- `timed_out`
- `truncated`
- `bytes_returned`
- `files_touched`
- `reason`

This is separate from the user-visible `text` payload of the tool result. The
text still drives replay into later model turns. The structured record exists
for operator truth, resume clarity, and later replay/eval export.

## Runtime Behavior

When the model emits tool calls, Probe now does the following in order:

1. Persist the `tool_call` turn.
2. Classify each requested tool call.
3. Evaluate policy for each requested tool call.
4. Execute auto-allowed or approved calls.
5. Persist a `tool_result` turn with structured policy and execution records.
6. Replay executed or refused tool results into the next model request.
7. Stop and return a runtime pause error if any tool call was marked `paused`.

This means a paused approval request is no longer implicit CLI behavior. It is
persisted runtime state.

## Why This Matters

Later acceptance, replay export, decision-module work, and GEPA optimization
all need more than "the tool returned some JSON".

They need to know:

- whether the call was safe by default or required approval
- whether the operator widened policy
- whether the tool actually ran
- whether the result was truncated
- how much output came back

This issue establishes that runtime truth without yet turning Probe into a
full interactive approval broker.
