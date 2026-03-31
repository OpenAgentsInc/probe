# Probe TUI Typed Overlay Stack And Focus Routing

## Summary

Issue #39 promotes Probe's one-off help modal into a typed overlay stack.

Probe now has explicit overlay homes for:

- help
- setup
- approvals

The home shell no longer needs a dedicated `Setup` tab to expose those
surfaces. `Chat` stays primary, `Events` remains secondary, and overlays take
focus only when needed.

## What Changed

### Typed overlay variants

`probe-tui` now carries explicit overlay state types instead of a single
special-case help screen:

- `Help`
- `Setup`
- `Approval`
- `RequestInput`

These overlay types live in the existing screen stack so focus ownership stays
simple and explicit.

### Focus routing

Focus routing is now:

- base shell + composer when no overlay is active
- disabled composer when a modal-style overlay owns focus
- replaced composer when an overlay intentionally takes over the bottom
  interaction lane

In the current implementation:

- help and setup disable the composer while staying above the shell
- approval replaces the composer entirely

### Setup left the tab row

The old `Setup` tab is gone. Setup now lives in a dedicated overlay opened by
`Ctrl+S`, and `Ctrl+R` reruns setup while opening that overlay.

This keeps the chat shell structurally stable while still giving the Apple FM
setup view a real focused home.

### First approval flow

This issue established the overlay shell shape:

- approval overlay with approve/reject selection

Issue `#45` later replaces the placeholder approval behavior with a real
runtime-backed pending-approval flow while keeping the same focus and overlay
structure.

## Control Model

- `Tab` / `Shift+Tab`: switch `Chat` / `Events`
- `Ctrl+S`: open setup overlay
- `Ctrl+A`: open approval overlay
- `F1`: open help overlay
- `Esc`: dismiss the top overlay

## Tests

Coverage now includes:

- focus and dismissal behavior for help and typed overlays
- real approval overlay behavior driven by pending runtime state
- snapshot coverage for help, setup overlay, and approval overlay surfaces

Validation commands:

```bash
cargo test -p probe-tui -- --nocapture
cargo test -p probe-cli --test cli_regressions -- --nocapture
```
