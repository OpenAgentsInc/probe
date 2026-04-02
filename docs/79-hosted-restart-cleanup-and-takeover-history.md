# Hosted Restart Cleanup And Takeover History

Issue `#104` closes the main operator-truth gap left after hosted receipt
history and controller leases landed.

## What Landed

Hosted Probe sessions now keep typed lifecycle history for:

- control-plane restart reconciliation
- controller takeover and handoff events
- managed-workspace cleanup closure
- orphaned managed-workspace reap events

Hosted cleanup receipts also stop pretending cleanup is permanently pending.
For managed hosted workspaces, Probe now records cleanup as:

- `pending` while the session is still active or controlled
- `completed` once Probe confirms the managed workspace is gone
- `failed` if Probe attempted cleanup and could not remove the workspace

## Restart And Cleanup Behavior

Hosted restart reconciliation now does more than mark failed running turns.
When Probe starts back up, it:

- records a `control_plane_restart_observed` history event for surviving hosted
  sessions
- checks terminal detached sessions that no longer have an active controller
  lease
- reaps managed hosted workspaces when they still exist
- records the cleanup transition as a retained history event
- records an explicit orphaned-workspace reap event when restart
  reconciliation removed the workspace

That means operators no longer have to guess whether cleanup is still pending
because nobody looked yet or because Probe actually failed to close it out.

## Why It Matters For Internal Forge MVP

Internal shared hosted sessions need restart truth that survives the process.
The team MVP does not require a full distributed control plane, but it does
require honest answers to:

- did the hosted control plane restart?
- who took over control after that?
- did Probe reap the managed workspace or leave it behind?
- is cleanup complete, failed, or still legitimately pending?

This issue keeps those answers in Probe-owned runtime state instead of forcing
OpenAgents or human operators to reconstruct them from logs.

## Validation

The hosted regression coverage now proves:

- a terminal managed hosted session survives process death, records restart
  history, and reaps its managed workspace on reconciliation
- running hosted turns still fail honestly across restart instead of silently
  resuming
- cleanup completion is retained in hosted receipt history once the workspace
  path is gone

## Current Limits

This still does not add:

- a fleet-wide worker event stream
- cloud-provider-native cleanup receipts
- an app-owned shared hosted session directory above Probe runtime ids

The raw hosted attach and discovery surface landed later in `#105`; what
remains is the higher-level shared-session layer above this runtime-owned
restart and cleanup truth.
