# Hosted Session Participants And Controller Lease

Issue `#103` adds explicit participant attribution and controller ownership to
hosted Probe sessions.

## What Landed

Hosted Probe sessions now persist:

- a typed participant roster on `SessionMetadata`
- the same participant roster on detached summaries
- an active controller lease with the current participant id and acquisition
  timestamp
- participant ids on turn authors and child-session initiators

Probe also adds explicit protocol calls for:

- attaching a participant to a session
- claiming, releasing, handing off, or taking over the controller lease

`probe-client` now carries optional `display_name` and `participant_id` fields
so first-party consumers can identify the human or shell instance driving a
hosted session.

## Controller Rules

Hosted controller behavior is now explicit:

- the first identified participant can claim control directly or by submitting
  a hosted turn
- once a hosted session has participant state, control actions require a
  participant id
- if a hosted session already has a controller lease, Probe rejects turn
  submission and follow-up control actions from a different participant until
  the lease is handed off or taken over
- handoff requires the target participant to already be attached

This keeps shared hosted sessions from degrading into "last writer wins"
control.

## Hosted Receipt History

Hosted receipt history now records controller ownership changes as retained
runtime facts:

- `claim`
- `release`
- `handoff`
- `takeover`

Each retained event includes:

- actor participant id
- optional target participant id
- session-owner and execution-host identity
- a human-readable summary

That gives Forge consumers a runtime-owned trail for who controlled the hosted
session over time.

## Validation

The new hosted collaboration test proves:

- participant roster projection through detached inspection
- controller handoff from one participant to another
- hosted turn rejection when a non-controller participant tries to queue the
  next turn
- retained takeover history after the original participant reclaims control

## Current Limits

This does not yet solve:

- a team-reachable hosted attach surface
- hosted session discovery for other operators
- typed restart, orphan-cleanup, or cleanup-closure history

Those are follow-on hosted internal-MVP issues above the controller lease
seam.
