# Shared Test Support And Stable Snapshot Root

Issue `#59` closes the remaining gap between "Probe has a test-support crate"
and "Probe tests actually rely on one shared support layer."

## What Landed

`crates/probe-test-support` now owns the shared helpers that were still
duplicated in binary and snapshot tests:

- fake OpenAI-compatible and Apple FM HTTP backends
- temp Probe home and coding workspace setup
- shared CLI binary launch helper
- shared attach-config writers for fake OpenAI and Apple FM servers
- explicit `INSTA_WORKSPACE_ROOT` setup for stable snapshots
- shared stderr, transcript, and acceptance-report normalization helpers

`probe-cli` regression tests now consume those helpers directly instead of
re-implementing local config-writing and normalization logic.

`probe-tui` snapshot tests now opt into the same explicit snapshot-root setup.

## Boundary

The support crate is intentionally scoped to Probe's real seams today:

- HTTP-style fake backends
- temp homes and workspaces
- CLI and snapshot helpers
- transcript and report normalization

It does not pretend Probe already has a real MCP or stdio-tool boundary. When
Probe grows one, the same crate should absorb the matching fake server support
instead of creating a separate ad hoc fixture layer.

## Why This Matters

This is the foundation for the follow-on work in the testing expansion stack:

- domain-partitioned suites
- real binary temp-workspace E2E
- richer acceptance and eval receipts
- first-person self-tests
- matrix eval execution
- explicit local lane codification

Without this patch, those later suites would keep re-copying binary launch,
server attach config, and snapshot normalization logic into unrelated test
files.
