# Probe TUI Tool Call And Tool Result Rows

## Summary

Issue `#44` hardens Probe TUI transcript rendering around persisted Probe tool
truth.

The TUI already rebuilt transcript rows from the Probe session store, but the
tool rows still read too much like debug dumps. This issue makes tool activity
first-class in the transcript with distinct row kinds for:

- tool call
- tool result
- tool refused
- approval pending

The source of truth remains the persisted Probe transcript and structured
`tool_execution` record.

## What Changed

### First-class transcript entry kinds

`TranscriptEntry` now carries explicit row-kind information instead of relying
only on a generic role/title/body shape.

That keeps the retained transcript model from issue `#35` while giving the UI a
clear way to distinguish:

- `[tool call]`
- `[tool result]`
- `[tool refused]`
- `[approval pending]`

without inventing a separate TUI-only tool-history store.

### Compact tool summaries

The worker-side transcript mapper now renders compact operator-oriented tool
rows:

- tool call rows show a compact command, path, or argument summary
- tool result rows show the smallest useful result shape for the operator:
  compact output text, a short file range, a short error, or a terse approval
  reason

This is much closer to a coding-shell transcript than dumping full pretty JSON
blocks for every row.

### Distinct refusal and pause presentation

Paused or refused tool outcomes are now rendered as distinct transcript row
types instead of looking like ordinary tool success rows.

That makes the persisted approval contract from
`docs/14-approval-classes-and-structured-tool-results.md` visible in the shell
before the resumable approval broker from issue `#45` wires those paused rows
to a real approve or reject flow.

## Tests

Coverage now proves:

- successful runtime-backed tool turns render `[tool call]` and `[tool result]`
  rows
- paused tool turns render `[approval pending]` rows
- snapshots reflect the new tool transcript taxonomy

Validation:

```bash
cargo test -p probe-tui -- --nocapture
cargo test -p probe-cli --test cli_regressions -- --nocapture
cargo check --workspace
```
