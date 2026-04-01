# Detached Session Watch And Log Subscriptions

Phase 2 now ships the first real detached-session watch surface above the local
daemon transport and detached-session registry.

This layer stays inside the existing Probe runtime protocol. It does not add a
second daemon-only API.

## What Landed

The shared protocol and client now include:

- `read_detached_session_log`
- `watch_detached_session`
- `DetachedSessionStream` server events
- `supports_detached_watch_subscriptions` in runtime capabilities

The daemon now persists append-only detached event logs under:

- `PROBE_HOME/daemon/events/<session_id>.jsonl`

Each record carries:

- a monotonic per-session cursor
- the detached session id
- a timestamp
- an explicit truth label
- a typed payload

The current payload set is:

- `summary_updated`
- `runtime_progress`
- `pending_approvals_updated`
- `note`

## Truth Labels

Detached watch output stays explicit about what is authoritative.

- `authoritative`
  - detached summary updates
  - pending approval updates
  - runtime events that were already `lossless`
- `best_effort`
  - detached copies of streamed assistant deltas, snapshots, and tool-call
    deltas

That keeps detached watch and logs aligned with the existing runtime event
discipline instead of inventing a separate daemon-specific truth model.

## Replay Semantics

`read_detached_session_log` and `watch_detached_session` now both use cursor
semantics.

- when `after_cursor` is set, Probe returns records strictly after that cursor
- when `after_cursor` is absent, Probe returns the recent tail up to `limit`
- every read response also returns `newest_cursor`

`watch_detached_session` uses the same rule for bounded replay first and then
stays attached for live events.

The server now subscribes before it reads replay data and then de-duplicates by
cursor. That closes the obvious race where an event could have landed between a
plain replay read and the live subscription attach.

## Client Boundary

`probe-client` now exposes both detached supervision helpers:

- `read_detached_session_log`
- `watch_detached_session`

That keeps `probe logs`, later TUI watch panes, and downstream Autopilot attach
work on the same shared client seam rather than growing a second control path.

## Current Limits

This landing is still deliberately narrow.

It gives Probe:

- bounded replay for recent detached session history
- push-style local watch streams without polling `inspect_session_turns`
- visible approval-paused lifecycle and runtime progress in detached logs

It does not yet give Probe:

- operator CLI commands above this surface
- watchdog or timeout actions
- remote or multi-tenant subscriptions
- a browser-facing transport

The current local daemon is also still single-connection in practice, so this
watch surface is the detached-session event substrate rather than a fully
concurrent multi-view operator bus.
