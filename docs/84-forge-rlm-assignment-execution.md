# Forge RLM Assignment Execution

Probe executes Forge-owned RLM work. It does not own the canonical policy or
promotion model.

## Boundary

- Forge owns:
  - strategy-family names
  - policy bundle refs
  - runtime-assignment truth
  - issue-thread evaluation rules and scoring
- Probe owns:
  - execution of the assigned work
  - corpus materialization
  - chunk planning for large inputs
  - runtime event emission
  - replayable artifact writing

The first checked-in lane consumes these Forge crates directly:

- `forge-runtime-protocol`
- `forge-policy`
- `forge-signatures`
- `forge-rlm-core`
- `forge-eval`

## Typed Execution Envelope

Probe's execution envelope lives in `probe-core::forge_rlm` as
`ForgeRlmExecutionPlan`.

That plan wraps the canonical Forge `RuntimeAssignment` with Probe-local
execution details:

- optional `workspace_ref`
- optional `publication_label`
- explicit `required_artifacts`

The wrapped `RuntimeAssignment` carries:

- strategy family
- policy bundle ref
- corpus locator
- execution budget
- output schema

## Large-Corpus Handling

Probe does not collapse an issue thread into one opaque prompt blob before
execution.

The current lane:

- materializes the corpus locally from:
  - a live GitHub issue thread, or
  - a local serialized corpus path
- validates the materialized item count
- builds an `IssueThreadChunkManifestEntry` plan so large corpora are tracked
  as bounded chunks
- fails honestly when the chunk plan exceeds the assignment's
  `max_loaded_chunks` budget

For the current issue-thread schema, the final grounded analysis comes from the
Forge-owned `forge-eval` / `forge-rlm-core` path, while Probe owns the
execution envelope and artifact publication around it.

## Operator Commands

Execute an arbitrary saved plan:

```bash
cargo run -p probe-cli -- forge rlm execute \
  --plan /path/to/forge-rlm-plan.json \
  --output-dir var/forge-rlm
```

Run the live proof for `OpenAgentsInc/openagents#4368`:

```bash
cargo run -p probe-cli -- forge rlm proof-openagents-4368 \
  --output-dir var/forge-rlm
```

The live proof plan is generated from Forge-owned defaults in
`ForgeRlmExecutionPlan::openagents_4368_issue_thread_proof()`.
For live GitHub issue corpora, Probe uses `GITHUB_TOKEN` or `GH_TOKEN` when
set, and otherwise asks the existing `gh` CLI login for a token without
printing it.

## Artifacts

Each execution writes a timestamped directory under `var/forge-rlm/` with:

- `assignment.json`
- `corpus.json`
- `corpus.md`
- `chunk_manifest.json`
- `report.json`
- `trace.json`
- `events.json`
- `runtime_result.json`
- `brief.md`

`runtime_result.json` carries the typed Forge execution result with:

- final status
- structured output for `issue_thread_analysis_v1`
- artifact refs
- summary text

## Validation

Current checked-in coverage:

- `cargo test -p probe-core forge_rlm`
- `cargo test -p probe-cli forge_rlm`
- ignored live unit test:
  - `cargo test -p probe-core live_openagents_4368_plan_executes_full_thread -- --ignored`

The live proof must stay honest about changing GitHub state. The issue body is
stable, but the comment count can change, so the GitHub materialization path
validates live metadata instead of pinning an old comment-count constant in the
executor.
