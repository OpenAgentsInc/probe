# Experimental Terminal Inline Overlay

This document captures the second Probe proof of concept for WGPUI visuals:
render the overlay scene offscreen, then send the resulting animated image back
into the terminal through a terminal graphics protocol instead of opening a
separate window.

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
iTerm2 also supports animated GIF playback, which the current Probe path now
uses to avoid destructive per-frame terminal swaps.

## Architecture

The implementation deliberately does not pretend that `ratatui` gained native
pixel widgets.

The flow is:

1. build the same WGPUI demo scene used by the sidecar proof
2. immediately clear the current terminal surface and paint a minimal loading state
3. render a short sequence of WGPUI frames offscreen through `wgpui::capture_scene`
4. encode those frames into one looping animated GIF payload
5. emit that single animated asset back to the terminal with an inline-image escape sequence
6. clear the terminal surface and force a fresh TUI redraw

So the terminal proof stays honest:

- WGPUI still renders real pixels
- the TUI still owns the retained text UI
- the terminal hosts a transient image overlay, not a retained GPU widget tree

## TUI Handoff

When launched from `probe tui`, Probe now:

- blocks the event loop
- runs the overlay subcommand with a hidden TUI-handoff flag
- keeps the existing alternate screen and raw-mode terminal session alive
- renders the inline image directly into that same TUI-owned terminal surface
- clears the terminal state before the next redraw

That keeps `Ctrl+G` inside the active Probe terminal session instead of visibly
dropping back to the underlying shell before the overlay appears.

## Non-Goals

This work still does not claim:

- portable terminal graphics support across all terminals
- multiplexed tmux or zellij support
- retained WGPUI interactivity inside the terminal grid
- replacement of the typed Probe overlay stack

This is a terminal graphics proof, not a new universal rendering substrate for
Probe TUI.
