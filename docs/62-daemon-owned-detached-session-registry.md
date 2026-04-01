# Daemon-Owned Detached Session Registry

Phase 2 now has a real daemon-owned detached-session index on top of the
socket transport from `61-probe-daemon-and-local-socket-transport.md`.

The goal of this layer is narrow:

- make daemon-owned sessions first-class data
- persist enough control summary to survive client disconnect
- reconcile restart behavior honestly instead of inferring it from ad-hoc
  process state

## What Landed

`probe-daemon` now persists detached-session summaries under the Probe daemon
state directory and exposes them through the runtime protocol.

The shipped protocol now includes:

- `list_detached_sessions`
- `inspect_detached_session`

The detached summary carries:

- `session_id`
- session title and cwd
- detached status
  - `idle`
  - `running`
  - `queued`
  - `approval_paused`
  - `completed`
  - `failed`
  - `cancelled`
- active-turn id when present
- queued-turn count
- pending-approval count
- most recent terminal turn id and status
- registry timestamps
- restart-recovery state and note

This is the first Probe-native answer to "what detached work does the daemon
currently own?"

## Ownership Semantics

The daemon is now the runtime owner for detached local sessions.

That means:

- a session started through the daemon is registered immediately
- later daemon requests against that session reuse the same daemon-owned
  runtime state
- reconnecting does not create a second competing runtime
- queued background work can keep moving after the initiating client exits

The append-only transcript and pending-approval records stay the truth for user
history. The detached registry is a persisted summary and control index above
that truth, not a replacement for it.

## Restart Semantics

Daemon restart behavior is now explicit.

Two cases matter:

- approval-paused active turns remain resumable
  - detached summary reports `approval_paused`
  - recovery state reports `approval_paused_resumable`
- running turns that were in flight when the daemon died become terminal
  - turn control marks them failed on restart
  - detached summary reports `failed`
  - recovery state reports `running_turn_failed_on_restart`

That keeps restart behavior honest:

- Probe does not pretend an interrupted running model call can resume
- Probe also does not throw away a real pending-approval pause that the
  operator can still act on

## Current Limit

This layer is no longer summary-only.

Detached watch and log subscriptions now exist above it in
`63-detached-session-watch-and-log-subscriptions.md`.

The remaining gap is now above the registry and watch substrate:

- operator `probe ps|attach|logs|stop` commands
- timeout or watchdog policy
- first-party chat or TUI default attach flows

Those remain the next Phase 2 items above the detached-session registry.
