# Slash Command Command Surface Plan

## Purpose

This document turns the desired "Claude Code style slash command" experience
into a tracked Probe TUI implementation plan.

The goal is not to add a novelty command parser. The goal is to make the core
Probe actions discoverable, fast, and keyboard-native from the composer.

The follow-on operator-command work for `/mcp`, `/plan`, `/clear`, `/compact`,
and `/usage` is tracked separately in
`83-operator-commands-and-context-controls-plan.md`.

## UX Goal

Typing `/` in Probe should feel like opening a lightweight command surface:

- obvious list of available actions
- arrow-key navigation without mode confusion
- one clear primary action when a command is selected
- local app actions should execute locally instead of becoming fake transcript
  turns
- argument-taking commands should guide the user instead of forcing them to
  memorize hidden flags

## Why This Matters

Today Probe already detects slash-like text in the composer, but it does not
yet provide a first-class command UX.

That leaves key actions harder to discover than they should be:

- switching backend behavior
- changing model or reasoning
- opening backend details
- reviewing approvals
- starting fresh or resuming
- understanding the current workspace

## Non-Negotiable Principles

### 1. Slash Commands Must Be Discoverable

Typing `/` should immediately reveal what the operator can do next.

### 2. The Composer Must Stay The Home Surface

Do not turn slash commands into a maze of heavy modal screens. Prefer a compact
inline command list or a lightweight overlay anchored to the composer.

### 3. Local App Commands Must Not Pretend To Be Agent Turns

Commands like `/help`, `/backend`, `/approvals`, `/model`, and `/reasoning`
should not create fake user transcript entries when they are only changing TUI
or session state.

### 4. Commands Should Be State-Aware

If the current lane does not support a command, Probe should either hide it or
explain why it is unavailable.

### 5. Command UX Should Reduce Memorization

The operator should not need to remember `Ctrl+S`, `Shift+Tab`, or launch-time
flags just to find common actions.

### 6. Multi-Step Commands Must Show The Next Move

If a slash command opens a picker, overlay, or second-step flow, Probe should
make it obvious what the user must do next.

That means clear post-selection feedback such as:

- where focus moved
- what `Up` and `Down` now control
- what `Enter` will do
- how `Esc` exits

## MVP Command Set

These commands are the highest-value first wave:

- `/help`
- `/backend`
- `/model`
- `/reasoning`
- `/cwd`
- `/approvals`
- `/new`
- `/resume`

## Phases

## Phase 1: Palette Foundations

Status: `[~]`

### Outcome

Typing `/` opens a real command list in the composer, with arrow-key
navigation and Enter-to-complete behavior.

### Scope

- [x] detect slash command entry as a real UI state, not just metadata
- [x] render a compact command list under the composer metadata line
- [x] use `Up` and `Down` to move through slash commands while the list is open
- [x] use `Enter` to complete the selected slash command into the composer
- [x] keep ordinary history navigation when the slash list is not open
- [ ] add `Esc` to dismiss the slash list without clearing the draft
- [ ] visually distinguish selected versus unavailable commands

### Notes

This phase deliberately stops at command completion. It gives Probe a real
slash-command UX foundation without yet binding every command to local app
actions.

## Phase 2: Local Action Commands

Status: `[~]`

### Outcome

Core app commands execute directly from the slash surface instead of becoming
plain user prompts.

### Scope

- [x] `/help` opens the help screen
- [x] `/backend` opens backend details
- [x] `/approvals` opens approval review
- [x] `/reasoning` updates Codex reasoning without requiring `Shift+Tab`
- [x] `/new` starts a fresh task or fresh session on the active lane
- [x] `/resume` opens resumable session affordances
- [ ] `/backend` grows a backend-switching picker from the same surface

## Phase 3: Backend Settings Commands

Status: `[~]`

### Outcome

Model and workspace changes become first-class in-app flows.

### Scope

- [x] `/model` opens a backend-specific model picker
- [x] persist model changes through the saved backend snapshot
- [x] `/cwd` surfaces current workspace and supports switching it
- [x] unavailable settings show honest copy instead of silent failure

## Phase 4: Polish And Ranking

Status: `[ ]`

### Outcome

The slash surface feels like a best-in-class command launcher instead of a
thin parser.

### Scope

- [ ] command descriptions stay concise and action-oriented
- [ ] rank likely commands by context
- [ ] hide irrelevant commands for the active backend
- [ ] show recent commands or most-used commands
- [ ] align help text and placeholder copy with `/` as the main discovery path

## Acceptance Checks

- [ ] typing `/` feels immediately useful to a new operator
- [ ] arrow navigation never conflicts with history in a surprising way
- [ ] command completion is visually obvious before submit
- [ ] local slash commands do not create noisy fake transcript turns
- [ ] `/model` becomes easier than editing backend config files by hand
- [ ] multi-step commands make the next required action obvious

## Implementation Seams

- `crates/probe-tui/src/bottom_pane.rs`
- `crates/probe-tui/src/event.rs`
- `crates/probe-tui/src/app.rs`
- `crates/probe-tui/src/screens.rs`

## Validation

- `cargo test -p probe-tui`
- targeted composer tests for slash command navigation and completion
- manual TUI pass:
  - type `/`
  - arrow through commands
  - press `Enter`
  - confirm the command completes into the draft instead of submitting
  - confirm ordinary history behavior still works when the slash list is closed
