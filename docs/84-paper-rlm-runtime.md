# Paper RLM Runtime

This document describes the first paper-complete recursive language model
runtime in `probe-core`.

The implementation lives in:

- `crates/probe-core/src/paper_rlm.rs`

## Purpose

`probe#122` added the `RlmLite` issue-thread harness that materializes a full
corpus and runs Forge's deterministic analyzer.

This new runtime is different:

- it targets Forge `strategy_family = rlm`
- it keeps the long corpus outside the controller prompt
- it gives the controller a bounded REPL
- it allows recursive sub-LM calls through `llm_query(...)`
- it finalizes only through `FINAL(...)` or `FINAL_VAR(...)`

That makes it the first honest paper-shaped RLM lane in Probe rather than a
corpus-eval wrapper labeled as RLM.

## Current Shape

The runtime currently:

- consumes a Forge `RuntimeAssignment`
- validates `strategy_family = rlm`
- requires a typed `repl_policy`
- externalizes the corpus into a `PaperRlmCorpus`
- builds controller history from:
  - system instructions
  - query + corpus metadata
  - prior controller code
  - bounded observation summaries
- executes controller code in a sandboxed Rhai REPL
- exposes helper surfaces from the REPL:
  - `context_metadata()`
  - `context_preview(...)`
  - `context_search(...)`
  - `context_load(...)`
  - `context_chunk(...)`
  - `llm_query(...)`
  - `FINAL(...)`
  - `FINAL_VAR(...)`

## Artifact Contract

Each run writes first-class retained artifacts:

- `assignment.json`
- `corpus_manifest.json`
- `controller_history.json`
- `trajectory.json`
- `subcall_receipts.json`
- `final_output.txt` or `final_output.json`
- `runtime_result.json`

The controller history intentionally keeps only metadata and bounded
observations. It does not inline the full corpus back into the root-turn
history.

## Budget Enforcement

The runtime enforces Forge-provided budgets for:

- controller iterations
- loaded chunks
- loaded bytes
- sub-LM calls
- stdout bytes
- observation bytes

Budget failures are retained as failed runtime results with artifacts instead
of silently compacting or flattening the corpus.

## Current Boundary

This issue introduces the runtime only.

It does not yet decide when Probe should route into RLM or how the TUI/CLI
should expose it. That wiring belongs to the follow-on routing/integration
issue.
