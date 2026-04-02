# Hosted Receipt History

Issue `#98` extends hosted Probe receipts from a latest-snapshot bundle into a
retained hosted-history surface.

## What Landed

Hosted `SessionHostedReceipts` now keep a `history` vector in:

- `SessionMetadata`
- `SessionSnapshot.session`
- `DetachedSessionSummary`

The retained history covers:

- `cleanup_state_changed`
  - records the previous cleanup status when one existed
  - keeps the cleanup strategy and workspace root explicit
  - includes the execution-host id when Probe knows it
- `approval_paused_takeover_available`
  - records the active turn id
  - keeps session-owner and execution-host identity explicit
  - records the pending approval count
- `running_turn_failed_on_restart`
  - records the failed turn id
  - keeps session-owner and execution-host identity explicit
  - carries the restart failure summary

## Why

The first hosted Forge audit already had the latest hosted receipts:

- auth
- checkout
- worker
- cost
- cleanup

That was not enough for closeout or recovery review because the operator still
had to infer restart, takeover, and cleanup transitions from raw transcript or
manual notes.

This issue keeps those transitions inside the runtime-owned receipt seam.

## Receipt Rules

The history is intentionally narrow:

- it only records transitions Probe can state authoritatively from runtime and
  detached-session truth
- it does not invent cloud-provider events or distributed-trace spans
- it deduplicates repeated reconciliation so the same restart or takeover state
  does not get appended on every summary refresh

## Current Limits

This still does not add:

- provider-specific incident timelines
- fleet-wide worker event logs
- a server-wide orphan-registry tombstone feed

The important change is that hosted session snapshots can now report retained
restart, takeover, and cleanup history without pushing that burden into the
OpenAgents shell.
