# 89 Session Rail UI Refactor Plan

This doc defines the next major UI cleanup for the Probe TUI session rail.

The goal is to turn the current right-side rail from a verbose operator dump
into a calm, high-signal summary that feels closer to Claude Code, Codex, and
other best-in-class coding-agent shells.

## Problem

The current rail is honest but too dense.

It mixes together:

- current task state
- long-lived configuration
- default values
- debugging details
- keyboard hints

That makes the panel visually expensive while still failing to answer the two
questions users care about most:

1. what is happening right now?
2. what should I do next?

## Design Principles

The refactor should follow these rules:

- default to calm, not exhaustive
- promote exceptions, not defaults
- keep the top of the rail action-oriented
- collapse static configuration into `/status` and `/doctor`
- show only the fields that change user behavior
- use compact grouped sections instead of a flat list of key-value rows

## Always Visible

These fields are important enough to stay visible in the default rail:

- `activity`
- `next`
- `workspace`
  - short repo or cwd label
- `lane`
  - but only if lane switching matters in normal use
- `safety`
  - one compact line covering review/approval posture
- `task`
  - but only when there is active, proposed, failed, reverted, or landed work

## Remove From Default Rail

These should not be in the always-visible default rail:

- `target`
- `transport`
- `mode: coding`
- `launch: foreground`
- `view: conversation`
- `memory: ...`
- `tools: on`
- raw multi-line approval posture
- `context: fresh session`
- keyboard hint rows

These remain useful, but belong in:

- `/status`
- `/doctor`
- `/git`
- `/memory`
- `/mcp`

## Default Rail Structure

The default rail should be grouped into four compact sections.

### 1. State

- title: `Ready`, `Working`, `Review Changes`, `Action Needed`, `Needs Review`
- `next: ...`
- optional `applied: ...`

### 2. Workspace

- `workspace: OpenAgents-Probe`
- optional `lane: Codex`

### 3. Safety

- `safety: auto-safe`
- examples:
  - `safety: review-risky`
  - `safety: review-all`
  - `safety: approval required`

### 4. Task

Only visible when there is relevant task state.

Examples:

- `task: proposed -> README.md`
- `task: updating -> crates/probe-tui/src/screens.rs`
- `task: landed -> README.md`
- `task: reverted`
- `task: failed after edits`

## Conditional Fields

The rail should expand only when something non-default matters.

### Show only when non-default

- `launch`
  - show only for `background` or `delegate`
- `memory`
  - show only when non-empty and behaviorally relevant
- `view`
  - show only when the user is in `trace`
- `git`
  - show only when branch state needs attention
- `mcp`
  - show only when MCP is connected, failed, or required for current work
- `recovery`
  - show only after interruption, attach, or restart

### Hide when default

- `coding`
- `foreground`
- `conversation`
- `tools on`
- `fresh session`
- happy-path approval defaults when they are already represented by `safety`

## Better Organization Rules

### Replace raw config lines with compact summaries

Bad:

- `mode: coding`
- `launch: foreground`
- `review: auto-safe`

Better:

- `safety: auto-safe`

### Replace verbose cwd with a workspace label

Bad:

- full truncated path row by default

Better:

- repo or folder name in the rail
- full path available in `/status`

### Remove keyboard hints from the rail

Keyboard guidance belongs in:

- footer
- `/help`
- slash palette command descriptions

The rail should not spend permanent space on shortcut reminders.

## Proposed Example

### Idle

```text
Ready
next: Describe the change you want

workspace: OpenAgents-Probe
lane: Codex

safety: auto-safe
```

### Working

```text
Working
next: Probe is inspecting the workspace

workspace: OpenAgents-Probe
lane: Codex

safety: auto-safe
task: updating -> crates/probe-tui/src/screens.rs
```

### Review Mode

```text
Review Changes
next: /diff previews README.md · A applies · R rejects

workspace: OpenAgents-Probe
lane: Codex

safety: review-risky
task: proposed -> README.md
```

### Failure

```text
Needs Review
next: Start the local backend, or switch lanes with Tab

workspace: OpenAgents-Probe
lane: Qwen

safety: auto-safe
task: failed
```

## Delivery Plan

### Slice 1: Calm Default Rail

- remove default-only rows from the full rail
- add grouped sections
- replace `cwd` with a short workspace label
- remove keyboard-hint rows from the rail

### Slice 2: Conditional Expansion

- only show `launch`, `view`, `memory`, `git`, `mcp`, and recovery lines when
  they are non-default or action-relevant
- collapse approval posture into a single `safety` line

### Slice 3: Task-Centric Summaries

- unify `last task`, `checkpoint`, `revert`, `edits`, and receipt state under
  a calmer `task` section
- keep `/status` as the place for the full detailed truth

### Slice 4: Small-Terminal Polish

- keep the compact top summary in sync with the same grouped model
- make narrow layouts match the same information hierarchy as the full rail

## Acceptance Criteria

The refactor is successful when:

- the default rail fits on screen without feeling like a config dump
- a new user can answer “what’s happening?” in under a second
- a new user can answer “what should I do next?” in under a second
- default rail content no longer includes keyboard hints or purely default
  config
- `/status` and `/doctor` still preserve full operator truth
- review, approval, failure, background, delegation, and MCP states still
  remain obvious when active

## Test Coverage

Add or update tests for:

- idle rail hides default-only rows
- working rail promotes `task: updating`
- review rail promotes `task: proposed`
- failure rail keeps the correct `next` guidance
- trace mode shows `view: trace` while conversation default hides it
- background and delegate modes surface `launch` only when active
- memory and MCP lines appear only when behaviorally relevant
- compact top summary matches the same grouped information hierarchy
