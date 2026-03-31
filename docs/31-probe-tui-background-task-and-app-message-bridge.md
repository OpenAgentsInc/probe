# Probe TUI Background Task And App-Message Bridge

GitHub issue:
`https://github.com/OpenAgentsInc/probe/issues/31`

## Summary

Probe's first TUI shell now has a real retained app-message and worker seam
instead of only local key-driven state changes.

The TUI still starts as a deliberately small Rust screen shell, but it can now
queue a bounded background task, keep repainting while that task runs, and
fold typed worker messages back into visible screen state.

## Why

The pre-issue TUI in `probe-tui` only handled synchronous local UI events:

- `AppShell` owned terminal lifecycle and immediate event dispatch
- `HelloScreen` owned only local tab or body-copy state
- there was no background worker boundary
- there was no typed app-message lane for model or runtime work

That was enough for the bootstrap shell, but it was the wrong shape for the
next step. Apple FM provider calls are blocking today, so bolting them
directly into the existing render or key-handler path would have frozen the
alt-screen UI.

## What changed

The TUI now has:

- a dedicated background worker thread in `probe-tui`
- a typed `AppMessage` enum for worker-originated task updates
- a typed `BackgroundTaskRequest` enum for narrow screen-to-worker commands
- queued, running, completed, and failed task state in the base hello screen
- a separate worker-event log alongside the existing UI-event log
- retained snapshot coverage for initial, loading, completed, and help-modal
  states

The retained hello shell now proves these additional architectural seams:

- screens can request bounded background work without owning thread logic
- the app shell can poll worker messages on tick and fold them into screen state
- the UI can keep repainting while background work is in flight
- worker success and failure paths are visible and tested

## Scope boundary

This issue intentionally does not build the real Apple FM screen yet.

It only establishes the app-message and worker seam needed so the next issue
can use the existing Apple FM backend lane honestly without freezing the UI.

## Validation

Retained coverage now includes:

- app-shell unit tests for worker message ingestion
- a successful background-task path that keeps repainting while the task runs
- a failing background-task path that surfaces explicit error detail
- narrow snapshots for initial, loading, completed, and help-modal states

## Follow-on

Issue `#32` uses this seam to replace the retained fake task with a real
Apple Foundation Models setup/demo screen that checks availability up front and
then renders a short series of live inference calls.
