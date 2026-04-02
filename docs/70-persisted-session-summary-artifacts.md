# Persisted Session Summary Artifacts

Probe now persists two machine-legible summary artifacts per session when the
source data exists:

- `retained_session_summary_v1.json`
- `accepted_patch_summary_v1.json`

They live under the session's local artifact directory:

- `sessions/<session-id>/artifacts/retained_session_summary_v1.json`
- `sessions/<session-id>/artifacts/accepted_patch_summary_v1.json`

## Why

Forge needs Probe-owned summary artifacts for later knowledge-pack publication.

Those artifacts should come from Probe runtime truth rather than from:

- transcript scraping in Autopilot
- shell heuristics above Probe
- app-owned summary blobs with missing provenance

## Artifact Shapes

`retained_session_summary_v1.json` carries the session-level retained summary:

- stable artifact id and digest
- source session id and transcript digest
- backend or harness identifiers when present
- tool, file-read, patch, and verification counts
- final assistant text when present
- a deterministic `summary_text` string for later pack publication

`accepted_patch_summary_v1.json` is only persisted when Probe has at least one
successful `apply_patch` result in the transcript.

It carries:

- stable artifact id and digest
- source session id and transcript digest
- successful patch turn refs and touched files
- current branch or delivery posture when Probe can resolve it
- final assistant text when present
- a deterministic `summary_text` string for later pack publication

## Projection

Probe now projects these artifacts through the runtime protocol instead of
making clients reopen transcript files:

- `SessionSnapshot.summary_artifacts`
  - full typed retained-session and accepted-patch artifact payloads
- `DetachedSessionSummary.summary_artifact_refs`
  - lightweight artifact refs for daemon-owned detached session lists and
    inspect surfaces

That means CLI, TUI, daemon consumers, and later Autopilot consumers can read
Probe-owned summary payloads directly from the session inspect contract.

## Materialization Rules

Probe refreshes these artifacts when session snapshots or detached summaries
are built.

Important behavior:

- artifacts are persisted as JSON under the session directory
- stable digests ignore volatile local timestamps
- Probe reuses the existing materialization when the semantic payload did not
  change
- stale accepted-patch artifacts are deleted if the session has no successful
  patch results

## Non-Goals

This does not yet add:

- hosted artifact search
- OpenAgents knowledge-pack UX
- cross-session campaign curation
- model-generated longform summaries
