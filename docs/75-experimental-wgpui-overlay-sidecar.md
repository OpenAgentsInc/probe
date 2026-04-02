# Experimental WGPUI Overlay Sidecar

This document captures the first honest Probe proof of concept for mixing a
WGPUI visual lane into the existing terminal-first runtime without pretending
that the `ratatui` shell itself became a pixel renderer.

The later in-terminal follow-up now lives in
`76-experimental-terminal-inline-overlay.md`. The sidecar path documented here
remains available as the explicit fallback and `--target sidecar` path.

## Decision

Probe now ships an experimental overlay lane with two entrypoints:

- `Ctrl+G` from `probe tui`
- `probe overlay demo`

The implementation launches a separate desktop WGPUI window that renders a
synthetic telemetry view with status panels, a history chart, and an event
feed. The TUI keeps terminal ownership and keeps running while the sidecar
window owns desktop focus and pointer input.

## Why A Sidecar

The current Probe TUI is a normal terminal UI built on `ratatui` and
`crossterm`. That stack renders character cells, not portable GPU-backed pixel
regions. So the first honest POC is not an in-terminal compositor and not a
cross-terminal overlay protocol.

The sidecar approach gives Probe a real WGPUI proof lane now while keeping the
architecture boundaries explicit:

- Probe TUI still owns transcript, tools, approvals, and terminal input
- WGPUI owns only the experimental visual window
- unsupported hosts fail fast instead of silently degrading the text UI

## Current Scope

The first demo window intentionally stays narrow:

- hotkey: `Ctrl+G`
- manual launch: `probe overlay demo`
- content: synthetic status badges, token-cadence chart, and event feed
- dismiss: `Esc` or window close

The TUI spawn path waits briefly to detect immediate startup failure so the
bottom status line can report missing desktop display support or similar early
errors.

## Non-Goals

This work does not claim any of the following:

- true pixel compositing inside the terminal grid
- portable support across arbitrary terminals or headless SSH sessions
- shared focus/input routing between `ratatui` widgets and WGPUI widgets
- replacement of the typed TUI overlay stack documented in
  `39-probe-tui-typed-overlay-stack-and-focus-routing.md`

If Probe later needs a richer visual lane, the honest next step is a supported
graphics mode or a fuller GUI shell, not pretending that ordinary terminal
cells can host native WGPUI widgets.
