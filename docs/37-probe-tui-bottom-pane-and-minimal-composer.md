# Probe TUI Bottom Pane And Minimal Composer

## Summary

Issue #37 adds the first real input subsystem to `probe-tui`.

Probe now has:

- a dedicated `BottomPane` owner instead of a passive footer widget
- a cursor-bearing multiline composer with submit and newline behavior
- explicit input-state handling for active, busy, and disabled shell states
- modifier-based global shell commands so plain typing no longer collides with
  one-letter app shortcuts

## Why This Shape

The earlier shell had transcript cards plus a footer bar, but no interaction
owner. That made the missing text box a structural problem, not a cosmetic one.

`BottomPane` is the first honest seam for:

- composer state and editing behavior
- bottom-of-screen shell status
- cursor placement
- future draft history, slash commands, mentions, attachments, and overlay
  replacement

## What Landed

### Bottom-pane ownership

`AppShell` now owns a `BottomPane` value directly and renders it below the main
screen area on every frame.

The bottom pane owns:

- shell status text
- composer rendering
- cursor math
- composer editing commands

### Minimal composer behavior

The initial composer supports:

- text insertion
- backspace and delete
- left / right cursor movement
- home / end movement within the current line
- multiline editing
- `Enter` submit
- `Ctrl+J` newline

Submissions are currently captured back into shell state and recorded in the UI
event log. That proves the input seam without pretending the full runtime turn
loop is already wired through the TUI.

### Key routing

Global shell commands moved to modifier-driven bindings:

- `Tab` and `Shift+Tab` switch tabs
- `Ctrl+R` reruns Apple FM setup
- `Ctrl+T` toggles operator notes
- `F1` opens help
- `Ctrl+C` quits

That leaves plain characters plus cursor/edit keys available to the composer.

### Busy and disabled states

The bottom pane now renders explicit state:

- `Active`: normal composer
- `Busy`: setup is running, but the composer stays usable
- `Disabled`: help or a non-chat tab owns focus

This is the first step toward the stronger focus and overlay routing planned in
issue #39.

## Tests

Retained coverage now includes:

- composer editing unit tests inside `bottom_pane.rs`
- an app-level test proving a composer submission is captured by `AppShell`
- refreshed TUI snapshots showing active, busy, and disabled bottom-pane states

Validation commands:

```bash
cargo test -p probe-tui -- --nocapture
cargo test -p probe-cli --test cli_regressions -- --nocapture
```
