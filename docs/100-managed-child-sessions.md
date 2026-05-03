# Managed Child Sessions

Issue `#141` productizes Probe child sessions as the first one-level delegation
surface for managed agents.

## Contract

The coordinator session remains the parent. A child session is a normal Probe
session with:

- an explicit `parent_link`
- inherited workspace state and mounted refs
- same-repo workspace guardrails
- bounded direct child count
- bounded delegation depth
- an initiator and purpose for auditability

The first supported shape is one level of child delegation. A child can run its
own turns, use the normal tool approval policy, and produce its own transcript,
artifacts, branch state, delivery state, and pending approvals.

## Runtime API

`spawn_child_session` creates the child and links it to the parent.

`inspect_child_session` is the parent-scoped drilldown API. It verifies that the
requested child belongs to the parent, then returns:

- the parent id
- the current child summary
- the full child `SessionSnapshot`
- recent detached child events when the daemon event log is available

This gives Laravel and other admin clients a compact parent-visible summary
while keeping detailed child traces available without scraping transcript prose
or guessing session ids.

Parent `inspect_session` still includes `child_sessions`, so a coordinator can
list delegated work before drilling into a child.

## Cancellation

When an approval-paused parent turn is interrupted, Probe now also cancels
linked child work that is queued or approval-paused:

- queued child turns are marked `cancelled`
- pending child approvals are rejected
- approval-paused child turns are marked `cancelled`
- a note is appended to the child transcript
- detached child summaries are synced back to the parent event stream

Probe still does not claim cooperative cancellation for an actively executing
child turn that is not paused on approval. That remains a runtime-control
limitation rather than hidden best-effort behavior.

## Current Boundaries

This feature is intentionally a managed-session feature, not a scheduled-agent
special case. The scheduled-agent layer can use it, but the durable truth lives
in Probe sessions, turn-control records, child summaries, transcripts, and
detached events.

The first version does not create deep child trees. Deeper delegation should
only be added after Laravel's managed-agent UI and API can make one-level child
state understandable and controllable.
