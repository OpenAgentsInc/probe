# Long-Context Repo Analysis

## Purpose

Probe now has a first explicit long-context escalation path for repo-analysis
and synthesis work.

The goal is not to turn long-context reasoning into the default coding lane.

The goal is to keep normal coding turns on `coding_bootstrap` and only widen
into a bounded auxiliary lane when the task clearly looks like:

- architecture analysis
- change-impact analysis
- multi-file repo synthesis
- larger workspace reasoning that already has explicit evidence pointers

## Runtime Shape

The first implementation uses a typed auxiliary tool:

- `analyze_repository`

This tool stays inside the normal Probe controller loop.

It is not a second recursive runtime.
It is not a hidden swarm.
It is not a default fallback for normal coding tasks.

## Configuration

The lane is opt-in.

Both `probe exec` and `probe chat` accept:

- `--long-context-profile <name>`
- `--long-context-max-calls <n>`
- `--long-context-max-evidence-files <n>`
- `--long-context-max-lines-per-file <n>`

Example:

```bash
cargo run -p probe-cli -- exec \
  --tool-set coding_bootstrap \
  --long-context-profile psionic-qwen35-2b-q8-long-context \
  --long-context-max-calls 1 \
  --long-context-max-evidence-files 6 \
  --long-context-max-lines-per-file 160 \
  "If this is really a repo-analysis task, use analyze_repository with explicit evidence paths."
```

The first built-in profile is:

- `psionic-qwen35-2b-q8-long-context`

Like the oracle lane, this is a Probe-owned backend role and configuration
surface, not a separate product.

## Task Boundary

`analyze_repository` only accepts these task kinds:

- `repo_analysis`
- `architecture_summary`
- `change_impact`
- `synthesis`

It also requires explicit `evidence_paths`.

That keeps the first implementation grounded in concrete repo evidence instead
of letting a long-context lane invent its own hidden exploration story.

## Escalation Gate

Probe does not allow long-context escalation just because the tool exists.

The current heuristic gate requires some combination of:

- explicit evidence files
- prior repo exploration through `list_files`, `code_search`, or `read_file`
- multi-file analysis shape
- prompt or session context pressure
- later support from an earlier oracle pass

If those conditions are not met, Probe refuses the `analyze_repository` call
and keeps the session on the normal coding lane.

This is the first concrete `LongContextEscalation` decision surface.

## Provenance

Every successful repo-analysis result records:

- the long-context backend profile and model
- the question asked
- the evidence file list
- per-file line and truncation metadata
- the analysis text returned by the auxiliary lane

The tool execution record also carries the touched evidence paths.

That means the transcript and later replay/export flows retain enough
provenance to audit what the long-context lane actually saw.

## Relation To Decision Modules

Probe now also has a matching heuristic `LongContextEscalation` module in
`crates/probe-decisions`.

The current role split is:

- runtime enforces the gate, budgets, and transcript truth
- decision modules expose the escalation choice as a typed comparison surface
- later optimizer work can tune the escalation policy without replacing the
  runtime boundary

## What This Does Not Do

The first implementation intentionally does not add:

- recursive remote execution
- a second planner loop
- hidden file crawling inside the long-context lane
- default escalation for everyday read-edit-verify tasks

This keeps the Probe roadmap aligned with the earlier guidance:

- ship the normal coding lane first
- add bounded oracle support
- add long-context escalation later and narrowly
