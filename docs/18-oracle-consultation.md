# Oracle Consultation

## Purpose

Probe now has a typed auxiliary oracle tool:

- `consult_oracle`

This is the Probe-native version of the executor-plus-oracle split.

The main controller loop remains authoritative.

The oracle exists as a bounded tool invocation inside that loop.

## Configuration

Oracle usage is enabled with:

- `--oracle-profile <name>`
- `--oracle-max-calls <n>`

Example:

```bash
cargo run -p probe-cli -- exec \
  --tool-set coding_bootstrap \
  --oracle-profile psionic-qwen35-2b-q8-oracle \
  --oracle-max-calls 1 \
  "Ask the oracle for a checking recommendation before editing."
```

Probe now ships a built-in auxiliary profile:

- `psionic-qwen35-2b-q8-oracle`
- `psionic-apple-fm-oracle`

## Boundaries

The oracle tool is intentionally constrained to:

- `planning`
- `checking`
- `research`

It is not intended to:

- execute tools directly
- patch files directly
- become a second controller loop
- silently replace the primary executor

## Budget And Visibility

Oracle usage is bounded by `--oracle-max-calls`.

When the budget is exhausted, later `consult_oracle` requests are refused and
persist that refusal in the normal tool-result trail.

Operator visibility is explicit:

- the CLI prints the active oracle profile and budget when configured
- transcripts record oracle usage as `consult_oracle` tool results
- replay and export data retain those oracle calls distinctly

## Why This Matters

This gives Probe the right executor-plus-oracle split without collapsing
runtime clarity.

The coding lane still owns execution truth.

The oracle just becomes one more bounded, typed decision aid inside that lane.

## Apple FM Oracle Boundary

Probe's Apple FM oracle support uses the real Apple FM backend kind rather than
pretending the existing Qwen profile is interchangeable.

Current honest claim:

- `consult_oracle` can target `psionic-apple-fm-oracle`
- Probe preserves typed Apple FM error detail if the bridge refuses or reports
  a Foundation Models failure
- this does not yet imply full Apple FM coding-tool parity
