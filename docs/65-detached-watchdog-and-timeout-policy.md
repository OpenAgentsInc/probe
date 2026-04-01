# Detached Watchdog And Timeout Policy

Phase 2 now ships the first daemon-owned watchdog policy for detached Probe
turns.

This is the minimum honest answer to stuck detached work:

- do not leave a detached turn in `running` forever
- do not mislabel approval-paused work as stuck compute
- do not silently drop why the daemon changed session state

## What Landed

The detached daemon now tracks enough per-turn metadata to supervise running
turns:

- `last_progress_at_ms`
- `execution_timeout_at_ms`
- explicit terminal turn status `timed_out`
- explicit detached summary status `timed_out`

The daemon now runs a background watchdog loop with three knobs:

- `watchdog_poll_ms`
- `watchdog_stall_ms`
- `watchdog_timeout_ms`

The shipped defaults stay boring:

- poll every `500ms`
- mark a turn as stalled after `30000ms` without runtime progress
- mark a turn as timed out after `300000ms` of detached execution

Both `probe-daemon run` and `probe daemon run` now accept:

- `--watchdog-poll-ms`
- `--watchdog-stall-ms`
- `--watchdog-timeout-ms`

## Policy Rules

The watchdog only evaluates active turns that are:

- `running`
- not already terminal
- not waiting on operator approval

Two triggers exist:

- progress stall
  - no runtime progress event for longer than `watchdog_stall_ms`
- total execution timeout
  - detached execution exceeded `watchdog_timeout_ms`

Approval-paused turns are explicitly exempt.

When a turn pauses for approval:

- the daemon clears the execution deadline
- the watchdog skips that turn while approval is pending

When the operator resumes the turn:

- Probe records fresh progress time
- Probe sets a fresh execution deadline

That keeps operator wait time from being mislabeled as stuck compute time.

## Timeout Effects

When the watchdog fires, the daemon now:

- marks the active turn `timed_out`
- preserves a concrete failure message on that turn
- cancels queued follow-up turns that have not started yet
- appends transcript notes explaining both the timeout and any cancelled
  follow-up work
- appends a detached event-log `note` record with the watchdog reason
- updates detached summary and inspect surfaces to `timed_out`

Operators can now see the result through:

- `probe ps`
- `probe attach`
- `probe logs`
- `inspect_detached_session`
- `read_detached_session_log`
- `watch_detached_session`

## Honest Limit

This is detached control-plane truth, not hard compute preemption.

The current local daemon still runs turns in-process, so Probe does not claim
that it can instantly kill arbitrary in-flight runtime work inside the worker
thread.

The current honest behavior is:

- the detached session moves to `timed_out`
- queued follow-up work is cancelled instead of silently hanging
- new work stays blocked while the timed-out worker is still draining
- once that late worker exits, the control plane remains `timed_out`

That is enough for an honest local Phase 2 daemon. Remote-worker preemption
and stronger isolation still belong to later phases.
