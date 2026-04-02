# Experimental Terminal Inline Overlay

This document captures the second Probe proof of concept for WGPUI visuals:
render the overlay scene offscreen, then send the resulting PNG back into the
terminal through a terminal graphics protocol instead of opening a separate
window.

## Decision

Probe now supports a terminal-first experimental graphics path:

- `Ctrl+G` from `probe tui`
- `probe overlay demo`
- `probe overlay demo --target terminal`

The current implementation prefers an inline-terminal image overlay when Probe
detects a supported interactive terminal session. If that capability is not
available, `auto` falls back to the existing sidecar window, and
`--target sidecar` still forces that older path.

## Current Protocol Scope

The first shipped terminal mode targets:

- interactive iTerm2 sessions
- direct stdin/stdout attached to the terminal
- no tmux or zellij passthrough in this first cut

Probe currently uses iTerm2's OSC 1337 inline-image protocol for this lane.

## Architecture

The implementation deliberately does not pretend that `ratatui` gained native
pixel widgets.

The flow is:

1. build the same WGPUI demo scene used by the sidecar proof
2. render that scene offscreen to a PNG through `wgpui::capture_scene`
3. emit the PNG back to the terminal with an inline-image escape sequence
4. wait for dismissal
5. restore the Probe alternate screen and force a fresh TUI redraw

So the terminal proof stays honest:

- WGPUI still renders real pixels
- the TUI still owns the retained text UI
- the terminal hosts a transient image overlay, not a retained GPU widget tree

## TUI Handoff

When launched from `probe tui`, Probe now:

- blocks the event loop
- leaves the alternate screen
- disables raw mode
- runs the overlay subcommand with a hidden TUI-handoff flag
- restores raw mode and the alternate screen after the overlay exits
- clears the terminal state before the next redraw

That gives the inline image lane a clean terminal surface without leaving the
main TUI in a broken post-handoff state.

## Non-Goals

This work still does not claim:

- portable terminal graphics support across all terminals
- multiplexed tmux or zellij support
- retained WGPUI interactivity inside the terminal grid
- replacement of the typed Probe overlay stack

This is a terminal graphics proof, not a new universal rendering substrate for
Probe TUI.
