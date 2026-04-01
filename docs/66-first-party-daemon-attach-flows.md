# First-Party Daemon Attach Flows

Phase 2 now ships the first honest detached-local attach flow for Probe's own
interactive clients.

The daemon and detached registry were already present. What was missing was
moving the default first-party surfaces onto that same seam so local detached
sessions stopped being a side path.

## Shipped

- `probe chat` now connects to the local daemon transport instead of spawning a
  private stdio runtime per process
- `probe chat` auto-starts the local daemon when the socket is missing
- `probe chat --resume <session-id>` now reattaches to the same daemon-owned
  session instead of creating a second runtime
- the TUI worker now also talks to the local daemon through `probe-client`
- `probe tui --resume <session-id>` attaches to an existing detached session on
  startup, hydrates the transcript, and restores pending approvals before a new
  prompt runs
- the shared client now owns the local daemon autostart helper

## Attach Semantics

The local operator semantics are intentionally simple:

- `probe chat` without `--resume` starts a fresh daemon-owned session
- `probe chat --resume <session-id>` continues an existing daemon-owned session
- `probe tui` without `--resume` opens a fresh UI against the selected backend
  lane
- `probe tui --resume <session-id>` attaches to the stored detached session
  first

For TUI resume, `--profile` and `--cwd` overrides are rejected. The attach path
uses the stored detached session settings instead of pretending a resumed
session is a fresh launch.

## Daemon Startup Ownership

`probe-client` now owns the shared daemon autostart path.

The lookup order is:

- sibling `probe-daemon`
- `PROBE_DAEMON_BIN`
- fallback to `probe-cli __internal-probe-daemon`

That keeps `probe chat`, the TUI worker, and the operator commands on one
boring local-daemon startup path.

## Verification

Process-level coverage now proves:

- `probe chat` can create a daemon-owned session, exit, resume it later, and
  still inspect that same detached session through the daemon
- `probe tui` can create a detached daemon-owned session, exit, relaunch with
  `--resume`, and render the existing transcript without issuing a duplicate
  runtime turn
