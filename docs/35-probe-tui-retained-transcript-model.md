# Probe TUI Retained Transcript Model

## Summary

Issue `#35` chooses the initial Probe transcript rendering model explicitly:

- use a retained in-memory transcript widget inside the ratatui screen
- keep one explicit active-turn cell for in-flight work
- do not adopt Codex-style terminal scrollback insertion yet

## Why This Model

Probe's current TUI is still early and ratatui-native.

The retained widget model is the smallest honest choice because it:

- matches the current full-screen ratatui architecture
- keeps transcript state explicit in `probe-tui`
- supports deterministic snapshot testing
- gives Probe a clear path to render committed turns plus one live mutable turn
- avoids prematurely coupling the TUI to a custom scrollback manager

This is intentionally not a claim that Codex's scrollback-plus-live-tail model
is wrong.

It is the narrower claim that Probe should choose a simpler first shell model
before it has a real chat screen, bottom pane, and composer.

## Chosen Seam

The chosen seam is:

- committed entries live in a retained `RetainedTranscript`
- an in-flight task renders as one `ActiveTurn`
- the transcript renders inside the main TUI layout as a normal ratatui widget

Current source:

- `crates/probe-tui/src/transcript.rs`

Core types:

- `TranscriptRole`
- `TranscriptEntry`
- `ActiveTurn`
- `RetainedTranscript`

## What This Enables Next

This model is the foundation for the next TUI issues:

- `#36` can make a transcript-first `ChatScreen` the primary home screen
- `#37` can add a real bottom pane and composer below the transcript
- `#38` can commit user / assistant / tool turns into the transcript and keep a
  live active-turn cell visible while work is running
- `#39` can layer overlays above the shell without changing transcript storage
- `#40` can extend the composer without changing transcript ownership

## Non-Goals

This issue does not yet provide:

- shell-native transcript scrollback
- full transcript paging
- transcript search
- a final chat UX
- a final overlay model

Those can be added later if Probe outgrows the retained widget model.

## Current Decision

For the next implementation phase, Probe should treat:

- the retained transcript widget as the source of TUI transcript truth
- the active-turn cell as the source of in-flight render truth
- the future bottom pane as a sibling interaction surface, not the owner of
  transcript history
