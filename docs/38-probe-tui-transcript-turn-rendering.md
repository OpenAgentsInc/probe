# Probe TUI Transcript Turn Rendering

## Summary

Issue #38 turns the Probe TUI transcript into the main shell surface.

The shell now renders:

- committed user turns
- committed tool/runtime turns
- committed assistant turns
- one explicit in-flight active turn cell

Composer submission now produces a visible user turn immediately, then drives a
worker-owned active turn before committing the assistant response.

Issue `#42` later replaced the original demo worker behind this shell shape
with a real persisted Probe runtime session loop.

## What Changed

### Real transcript turns

The retained transcript model from issue #35 now carries actual shell turns
instead of mostly setup copy.

`TranscriptEntry` is used for committed turns:

- `user`
- `tool`
- `assistant`
- setup/status events where needed

`ActiveTurn` remains the single live mutable cell at the bottom of the
transcript.

### Composer -> transcript path

Submitting from the bottom composer now:

1. commits a user turn to the transcript immediately
2. queues a background runtime task
3. commits a runtime/tool entry
4. renders a live assistant active turn
5. commits the final assistant reply

That gave Probe its first transcript-first shell loop before the real runtime
worker landed in issue `#42`.

### Layout shift

The `Chat` tab is now more transcript-dominant:

- transcript widened to the primary visual column
- the right rail compressed into narrow shell/setup context panels
- the UI reads as a coding/chat shell first, not a dashboard first

## Worker And Message Seam

`probe-tui` now uses generic transcript app messages in addition to the Apple
FM setup messages:

- `TranscriptEntryCommitted`
- `TranscriptActiveTurnSet`

That gives the TUI a reusable path for future real controller-turn rendering
without coupling turn presentation to the Apple FM setup prove-out.

## Tests

Coverage now proves:

- composer submission produces a visible user turn
- the worker can drive a live active assistant turn and then commit it
- snapshots cover empty transcript, running transcript turn, and committed
  transcript turn states

Validation commands:

```bash
cargo test -p probe-tui -- --nocapture
cargo test -p probe-cli --test cli_regressions -- --nocapture
```
