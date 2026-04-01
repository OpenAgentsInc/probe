# CLI Regression And Snapshots

Probe now has a process-level CLI regression layer in addition to crate-local
unit tests.

## What The Binary Tests Cover

The new `probe-cli` integration tests exercise the built binary rather than
only library functions.

Current coverage includes:

- successful `probe exec` against an ephemeral fake backend wired through the
  normal attach-mode server config path
- stderr summary regression checks for session, harness-profile, and
  observability output
- selected persisted transcript-event snapshots after a real CLI run
- validation failures for incompatible tool and harness combinations
- resume-override rejection paths for `probe chat`
- successful `probe accept` runs with a stable normalized JSON snapshot of the
  emitted report
- real daemon-backed operator flows for `probe ps`, `probe attach`,
  `probe logs`, `probe stop`, and `probe daemon stop`

## Snapshot Boundary

Probe keeps the snapshot surface intentionally narrow.

What is snapshot-tested:

- normalized `probe exec` stderr summaries
- one selected persisted transcript event from a real CLI session
- a normalized acceptance report JSON shape
- one normalized detached operator-control transcript covering `ps`, `attach`,
  `logs`, `stop`, and daemon shutdown

What is not snapshot-tested:

- full transcripts
- raw absolute paths
- raw wallclock metrics
- session ids and other machine-local identifiers

The test helpers replace machine-specific paths with stable placeholders such
as `$PROBE_WORKSPACE_ROOT` and `$TEST_ROOT` before snapshotting.
