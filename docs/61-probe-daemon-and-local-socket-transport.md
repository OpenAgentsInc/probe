# Probe Daemon And Local Socket Transport

`probe-daemon` is the Phase 2 transport foundation for detached local Probe
work.

It does not introduce a second runtime protocol. It reuses the shipped
`probe-server` request, response, and event contract over a long-lived local
Unix socket.

## What Landed

- new `probe-daemon` binary
- shared `ProbeServerCore` plus per-connection `ProbeServerConnection`
- Unix-socket JSONL transport on top of the same typed runtime protocol
- `probe-client` transport selection between:
  - spawned stdio child process
  - local daemon socket
- explicit daemon `run` and `stop` entrypoints

The daemon socket defaults to:

- `PROBE_HOME/daemon/probe-daemon.sock`

The current protocol version is now `5` so transport-capability changes fail
honestly during initialize instead of silently drifting.

## Startup And Shutdown Semantics

Daemon startup is explicit:

```bash
cargo run -p probe-daemon -- run --probe-home ~/.probe
```

Daemon shutdown is also explicit:

```bash
cargo run -p probe-daemon -- stop --probe-home ~/.probe
```

The daemon only accepts shutdown when it has no unfinished turns. That keeps
the shutdown path honest instead of silently killing active runtime work.

## Stale Socket Handling

On startup the daemon now checks the socket path before binding:

- if another daemon is actively listening there, startup fails with
  `addr_in_use`
- if the socket path exists but nothing is listening, Probe removes the stale
  socket and starts cleanly
- if the path exists but is not a socket, startup fails instead of deleting an
  unrelated file

That is the minimum boring operator behavior needed for local daemon restarts.

## Client Boundary

`probe-client` now owns both local transport shapes:

- `SpawnStdio`
- `LocalDaemon`

The important behavior split is:

- spawned stdio children still shut down on client drop
- daemon connections do not shut down the daemon on client drop

That keeps detached local runtime ownership possible without forcing every
foreground client to stay alive forever.

## Current Limits

This landing is intentionally narrow.

It gives Probe:

- one long-lived local daemon
- multiple sequential client attaches
- the same typed initialize or request or event or response contract over the
  daemon socket
- the daemon-owned detached-session registry documented in
  `62-daemon-owned-detached-session-registry.md`
- the detached watch or log lane documented in
  `63-detached-session-watch-and-log-subscriptions.md`
- the operator CLI documented in `64-daemon-operator-cli.md`
- the watchdog policy documented in `65-detached-watchdog-and-timeout-policy.md`

It does not yet give Probe:

- first-party chat or TUI defaulting to daemon attach

Those are the next Phase 2 layers on top of this transport foundation.
