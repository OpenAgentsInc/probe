# Apple FM Setup Demo Screen

GitHub issue:
`https://github.com/OpenAgentsInc/probe/issues/32`

## Summary

`probe tui` now boots into a real Apple Foundation Models prove-out screen
instead of a static hello-world demo.

On launch, Probe queues the canonical Apple FM setup request in the TUI worker
lane. The screen checks availability first and only issues inference calls if
the Apple FM bridge reports the configured model as ready.

## Why

Probe already had the real backend pieces:

- a canonical Apple FM profile in `probe-core`
- typed availability checking through the Apple FM provider
- plain-text Apple FM inference through the existing provider/core seam

What was missing was the visible retained UI path above that runtime truth.

The first TUI shell proved app/screen/widget structure and the later worker
issue proved non-blocking message flow, but the default `cargo probe` path was
still only a demo lane.

## What changed

The TUI now ships a dedicated Apple FM setup prove-out flow:

- `AppShell::new()` auto-queues the Apple FM setup task
- the worker checks `system_model_availability()` before any inference request
- unavailable or non-admitted machines stay in a typed unavailable state with
  explicit reason and message detail
- reachable and admitted machines run three short plain-text Apple FM setup
  calls
- the overview tab renders:
  - current or last response detail
  - setup status
  - backend facts
  - availability detail
- the events tab renders:
  - UI-local events
  - Apple FM setup timeline

## Scope boundary

This issue intentionally remains narrow:

- plain-text Apple FM only
- no tool-backed Apple FM coding in the TUI
- no managed Apple FM launch
- no full chat/composer surface yet

The point is to prove a live runtime-to-TUI seam on startup, not to ship the
final long-term Probe interaction model.

## Testing

Retained TUI coverage now includes:

- unavailable Apple FM state through `FakeAppleFmServer`
- typed provider failure surfaced into the screen model
- successful three-call Apple FM setup flow through `FakeAppleFmServer`
- snapshot coverage for initial, unavailable, running, completed, and help
  states

## Operator behavior

From the repo root:

```bash
cargo probe
```

Expected posture:

- if Apple FM is ready, the screen shows the three-call setup prove-out
- if Apple FM is unavailable, still downloading, or not admitted, the screen
  shows the typed failure or availability truth and does not fake a successful
  run

## Relationship to issue 31

Issue `#31` established the worker thread and typed app-message seam. This
issue uses that seam for the first honest runtime-backed TUI path.
