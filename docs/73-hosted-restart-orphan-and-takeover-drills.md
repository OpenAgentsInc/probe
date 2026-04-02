# Hosted Restart, Orphan Cleanup, And Takeover Drills

Issue `#97` adds the first owned hosted-drill proof for detached Probe
sessions.

## What This Covers

The hosted TCP lane now has explicit drill coverage for:

- running-turn failure when the hosted server dies mid-turn
- approval-paused takeover after hosted restart
- orphaned detached-registry cleanup on hosted startup

That is the hosted equivalent of the earlier daemon restart drills.

## Drill Cases

### 1. Hosted Running Turn Dies Mid-Restart

`hosted_restart_marks_running_turns_failed_when_process_dies`

This drill:

- starts a hosted detached session
- queues a turn that is still running
- kills the hosted server process
- restarts the hosted server against the same Probe home
- confirms the detached summary reports:
  - `failed` status
  - `running_turn_failed_on_restart` recovery state
  - explicit recovery note and turn failure message

### 2. Hosted Approval Pause Survives Restart

`hosted_restart_keeps_approval_paused_sessions_resumable`

This drill confirms that an approval-paused hosted turn stays reattachable
after the hosted server restarts.

Probe keeps:

- `approval_paused` detached status
- resumable recovery state
- pending approval count
- active turn identity

That is the first honest takeover story for a hosted human operator reattaching
to a paused detached session.

### 3. Hosted Startup Reaps Orphaned Registry Entries

`hosted_startup_reaps_orphaned_detached_registry_entries_without_session_metadata`

This drill deletes the session metadata after a hosted session has been
registered, then restarts the hosted server.

Startup reconciliation removes the stale detached registry entry instead of
pretending the detached session is still valid.

## Why This Matters

Forge cannot claim a hosted background-agent lane if restart and reattach only
work for the local daemon.

These drills prove that the same detached-session truth is carried by the
hosted TCP lane:

- sessions do not become mystery state after a crash
- approval-paused work can still be taken over
- stale registry entries are reaped on startup

## Current Limits

This still does not claim:

- a real HA control plane
- automatic multi-worker failover
- hosted watchdog tuning beyond the existing detached-runtime rules
- real GCP fleet incident automation

The important change is that hosted detached-session restart and cleanup
semantics are now tested and documented instead of assumed.
