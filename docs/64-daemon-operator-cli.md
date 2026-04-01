# Daemon Operator CLI

Phase 2 now ships the first human-facing operator commands above the local
`probe-daemon` transport, detached-session registry, and detached watch or log
surfaces.

This is the minimum boring local control plane needed before Autopilot or a
richer TUI attach flow sits on top.

## What Landed

`probe-cli` now exposes:

- `probe daemon run`
- `probe daemon stop`
- `probe ps`
- `probe attach <session-id>`
- `probe logs <session-id>`
- `probe stop <session-id>`

These commands all use the shared `probe-client` daemon transport instead of
reaching into daemon files directly.

`probe daemon run` also now accepts:

- `--watchdog-poll-ms`
- `--watchdog-stall-ms`
- `--watchdog-timeout-ms`

That keeps the detached watchdog policy configurable without introducing a
second daemon-only configuration path.

## Autostart Rule

`probe ps`, `probe attach`, `probe logs`, and `probe stop` now try the local
daemon first and auto-start it when nothing is listening yet.

The auto-start path is still intentionally boring:

- prefer a sibling `probe-daemon` binary when one exists
- otherwise fall back to the hidden internal daemon entrypoint inside the
  current `probe` executable
- wait briefly for the Unix socket to accept connections before the command
  continues

That keeps detached session inspection usable in normal local development and
binary tests without forcing the operator to start the daemon manually every
time.

## Command Shape

`probe ps` prints one detached-session summary per line from the daemon-owned
registry.

`probe attach` prints:

- the detached summary
- backend profile and transcript counts
- active, queued, and recent turn-control records
- pending approval records
- a bounded transcript tail

`probe logs` replays detached event-log records through
`read_detached_session_log`. With `--follow`, it attaches the live watch stream
through `watch_detached_session`.

`probe stop` uses the existing typed control APIs rather than killing the
worker process blindly:

- interrupt the active turn when one exists
- cancel queued turns that have not started yet
- print the resulting detached status summary

For an approval-paused turn, this now produces an honest preserved transcript
note rather than dropping the interruption on the floor.

## Regression Coverage

`probe-cli` now has a real binary integration test that:

- starts a real `probe-daemon`
- creates a detached session through the shared client
- drives the session into `approval_paused`
- exercises `probe ps`
- exercises `probe attach`
- exercises `probe logs`
- exercises `probe stop`
- exercises `probe daemon stop`

The snapshot boundary stays narrow:

- normalized detached summary text
- normalized approval and transcript rows
- normalized detached log lines
- normalized daemon-stop output

## Current Limits

This is an operator CLI, not a stable automation API.

Current limits remain:

- the daemon is still effectively single-connection in practice
- `probe stop` is only as cooperative as the existing runtime interrupt path
- the operator CLI now shares the same daemon seam as `probe chat` and
  `probe tui --resume`, but remote worker attach still does not exist
