# Operator Commands And Context Controls Plan

## Purpose

This document turns the next "Claude Code / Codex style operator controls"
work into a tracked implementation plan for Probe.

The goal is not only to add more slash commands. The goal is to make Probe
feel like it has a real operator command layer for:

- integrations
- mode switching
- context management
- usage visibility

inside the existing TUI constraints.

## Requested Command Set

The initial standard-operator commands covered by this plan are:

- `/mcp`
- `/plan`
- `/clear`
- `/compact`
- `/usage`

These should feel like first-class local controls, not fake chat prompts.

## UX Goal

A strong coding-shell operator should be able to type `/` and immediately find:

- how to change the operating mode
- how to manage context safely
- how to inspect integrations
- how to check token usage

without needing repo-specific knowledge, config-file edits, or hidden keybind
memorization.

The best version of this feels closest to Claude Code or Codex:

- lightweight command discovery
- obvious next action after selection
- minimal surprise about whether a command changes local UI state or model
  context
- honest copy when Probe does not yet support a requested capability

## Non-Negotiable Principles

### 1. Commands Must Be Honest About Scope

Probe must distinguish between:

- local UI state
- future-turn runtime state
- current-session model context
- persisted backend or integration config

Do not use the same command shape for all four without telling the operator
what will actually change.

### 2. Context Commands Must Never Be Destructive By Accident

`/clear` and `/compact` must explain whether they:

- preserve transcript scrollback
- start a fresh runtime session
- carry forward a summary
- discard any in-flight turn state

The operator should not lose trusted context silently.

### 3. Plan Mode Must Be Visibly Different From Normal Coding Mode

If Probe is in plan mode, the shell should make that obvious before submit.

The operator should be able to answer:

- am I asking for a plan or code?
- can this turn edit files?
- what will happen if I press Enter?

### 4. Usage Must Be Actionable, Not Debug Noise

Token usage should help the operator understand:

- how large the conversation is getting
- whether a compact or clear action is warranted
- whether a backend is returning exact or estimated usage

Do not dump raw accounting fields without guidance.

### 5. MCP Must Be Shipped Honestly

Probe docs already note that it does not yet have a real MCP boundary.

That means `/mcp` cannot pretend a full external-tool ecosystem exists if the
runtime and protocol do not support it yet. The command should either:

- open a truthful integrations manager for shipped MCP-backed capabilities, or
- explain what is and is not available yet and guide the operator toward the
  next valid action.

### 6. Every Command Must Leave A Visible Trace

After a slash command runs, Probe should always do at least one of these:

- visibly change shell state
- open the next required picker or overlay
- show a compact success status
- show a compact blocked-state reason

The operator should never have to guess whether the command was accepted.

If a command needs another choice, Probe must make the next action obvious with
clear copy such as:

- what changed focus
- what keys now work
- what will happen on `Enter`
- how to cancel with `Esc`

## Command Semantics

## `/plan`

### Operator Meaning

Switch the active lane into plan-first mode for future turns.

### Recommended MVP Behavior

- local toggle, not a transcript turn
- persistent per lane for the current TUI session
- clearly visible in the shell rail and composer metadata
- defaults to no file edits without explicit follow-up confirmation
- changes the active prompt contract or harness guidance to bias toward
  planning, sequencing, risk review, and implementation breakdown

### Why This Matters

Users often want to think with the agent before writing code. That should be a
clean mode, not a prompt ritual.

### MVP Exit Criteria

- the current mode is always visible
- the first turn after enabling plan mode does not unexpectedly edit files
- switching back to normal coding mode is one command away

## `/clear`

### Operator Meaning

Start a fresh runtime context on the active lane without carrying forward the
 previous conversation turns.

### Recommended MVP Behavior

- local action, not a transcript turn
- creates a fresh Probe runtime session on the active lane
- preserves backend, model, cwd, approvals, and lane identity
- preserves visible local scrollback as archived history only if clearly
  labeled, or clears the visible transcript entirely if that is simpler and
  less confusing
- blocked while a turn is running or approvals are pending

### Why This Matters

Operators need a quick "start over but stay here" tool when the context window
has become noisy or irrelevant.

### MVP Exit Criteria

- the next submitted prompt does not replay prior conversation turns
- the command makes it obvious that only model context was cleared, not repo
  changes
- pending or running work cannot be silently orphaned

## `/compact`

### Operator Meaning

Compress the current conversation into a Probe-owned carry-forward summary and
continue from a fresh session seeded with that summary.

### Recommended MVP Behavior

- local action with a confirmation step
- use a bounded summarization step to build a compact carry-forward artifact
- fresh runtime session starts with that compact summary instead of the full
  transcript replay
- summary is visible to the operator before or after compaction
- summary should include task goal, files changed, verification state,
  unresolved questions, and important constraints

### Why This Matters

This is the "keep the thread alive without dragging the whole transcript"
control that advanced coding shells rely on.

### MVP Exit Criteria

- the operator can tell what was preserved
- the compacted session stays materially shorter than the original replay path
- compaction failure degrades honestly instead of silently dropping context

## `/usage`

### Operator Meaning

Inspect token usage and conversation size for the active lane.

### Recommended MVP Behavior

- local action, likely a lightweight overlay
- show latest turn usage and session aggregate usage
- show exact versus estimated truth where available
- show whether usage is unavailable on the active backend
- show a simple recommendation when usage suggests `/compact` or `/clear`

### Why This Matters

Probe already carries usage truth in the runtime and CLI, but the TUI does not
yet expose it as a first-class operator control.

### MVP Exit Criteria

- a user can answer "how big is this conversation getting?"
- the command does not require reading raw transcript receipts
- no backend is shown as precise when its usage is only estimated

## `/mcp`

### Operator Meaning

Inspect and manage Probe's external integration boundary.

### Recommended MVP Behavior

Because Probe does not yet have a generalized MCP runtime boundary, ship this
in layers:

1. an integrations surface that truthfully lists what Probe can currently use
2. typed status for configured or shipped integrations
3. only later, generic MCP server management once the runtime supports it

### Why This Matters

Operators expect one place to understand what external tools are available.

### MVP Exit Criteria

- `/mcp` never overpromises capability that Probe does not have
- the command provides clear status and next steps
- the eventual transition to real MCP server management does not require a UX
  redesign

## Phases

## Phase 1: Command Layer Semantics And UI Contracts

Status: `[ ]`

### Outcome

Probe has one coherent model for local operator commands versus transcript
turns.

### Scope

- [ ] extend the slash-command plan with operator-command semantics
- [ ] define command types:
  - local immediate action
  - local overlay action
  - local toggle
  - context-reset action
  - summarizing transition
- [ ] define common confirmation, blocked-state, and success-copy rules
- [ ] add visible command affordances for "changes next turn" versus "changes
      now"
- [ ] define one shared feedback contract for:
  - command accepted
  - command blocked
  - command requires another choice
  - command completed successfully

### Primary Seams

- `crates/probe-tui/src/bottom_pane.rs`
- `crates/probe-tui/src/app.rs`
- `crates/probe-tui/src/screens.rs`
- `docs/82-slash-command-command-surface-plan.md`

### Exit Criteria

- [ ] every command in this document has explicit semantics before code lands
- [ ] the slash surface can route local commands without fake transcript turns
- [ ] blocked states read consistently across commands
- [ ] every command produces visible post-action feedback without transcript
      archaeology

## Phase 2: Plan Mode And Usage

Status: `[ ]`

### Outcome

Probe gains the safest and highest-value operator controls first:

- `/plan`
- `/usage`

### Scope

- [x] add a first-class lane mode model with at least `coding` and `plan`
- [x] surface current mode in the shell rail and composer metadata
- [x] add `/plan` as a local toggle or two-step picker
- [x] add `/usage` overlay with latest-turn and session aggregate usage
- [x] reuse existing exact-versus-estimated usage truth already present in
      runtime receipts
- [ ] add heuristics for "consider compacting" when session usage is high
- [x] make the mode or usage action visibly acknowledge success in the shell

### Primary Seams

- `crates/probe-tui/src/app.rs`
- `crates/probe-tui/src/screens.rs`
- `crates/probe-tui/src/message.rs`
- `crates/probe-tui/src/worker.rs`
- `crates/probe-core/src/harness.rs`
- `crates/probe-core/src/runtime.rs`

### Exit Criteria

- [x] plan mode is visible before submit
- [x] plan mode does not silently behave like normal coding mode
- [x] usage overlay can show latest turn and aggregate session totals
- [x] exact versus estimated truth is preserved in the TUI
- [x] the operator can tell immediately whether `/plan` toggled or `/usage`
      opened

## Phase 3: Context Reset And Compaction

Status: `[~]`

### Outcome

Probe gives operators explicit, trustworthy controls for managing context size:

- `/clear`
- `/compact`

### Scope

- [x] add "fresh runtime session on same lane" support for `/clear`
- [x] block clear while a turn is active or an approval is pending
- [x] define whether transcript scrollback is fully cleared or archived locally
- [x] add a compaction flow that creates a carry-forward summary
- [x] show the compacted summary to the operator
- [x] start the next runtime session from compact summary context instead of
      full replay
- [ ] persist compact summaries as Probe-owned artifacts for inspection and
      resume
- [x] provide explicit confirmation and next-step copy before context-changing
      actions finalize

### Primary Seams

- `crates/probe-tui/src/app.rs`
- `crates/probe-tui/src/screens.rs`
- `crates/probe-tui/src/worker.rs`
- `crates/probe-core/src/runtime.rs`
- `crates/probe-core/src/session_summary_artifacts.rs`
- `crates/probe-core/src/session_store.rs`
- `crates/probe-protocol/src/session.rs`

### Exit Criteria

- [x] `/clear` starts a fresh context without changing cwd or backend
- [x] `/compact` preserves the important thread state in a visible summary
- [x] the operator can tell whether they are in a cleared or compacted session
- [x] failure to compact never silently drops context
- [x] the shell clearly explains what happened after clear or compact

## Phase 4: Honest MCP And Integrations Surface

Status: `[~]`

### Outcome

Probe has a real operator-facing integrations view with a safe path toward
future MCP expansion.

### Scope

- [x] define the operator-facing "integrations" model Probe can honestly ship
      today
- [x] add `/mcp` overlay with capability status, availability, and next-step
      guidance
- [x] list shipped integrations and runtime-backed statuses
- [x] add persistent configured-server management for `/mcp`, including add,
      list, and enable or disable state
- [ ] define the protocol and runtime extension required for real MCP server
      management
- [ ] add live tool inventory, connection status, and auth state once the
      runtime boundary exists

### Primary Seams

- `crates/probe-tui/src/app.rs`
- `crates/probe-tui/src/screens.rs`
- `crates/probe-core/src/runtime.rs`
- `crates/probe-core/src/tools.rs`
- `crates/probe-protocol/src/runtime.rs`
- future MCP-specific crates or modules once introduced

### Exit Criteria

- [x] `/mcp` ships honest status on day one
- [x] configured MCP servers can be added, listed, and enabled or disabled
      from the TUI without pretending they are runtime-mounted yet
- [x] the future path to real MCP management is explicit in code and docs
- [x] Probe never suggests integrations are available when they are not
- [x] the operator always knows what to do next after opening `/mcp`

## Phase 5: Command Polish And Cross-Command Trust

Status: `[ ]`

### Outcome

These operator commands feel like one system instead of five isolated features.

### Scope

- [ ] add command descriptions that explain the consequence of each action
- [ ] add confirmation where context could be lost or transformed
- [ ] add consistent success-copy and blocked-copy across commands
- [ ] add consistent "next step" helper copy for multi-step commands and
      pickers
- [ ] add one-step keyboard flow for the common commands:
  - `/plan`
  - `/usage`
  - `/clear`
  - `/compact`
  - `/mcp`
- [ ] add manual shakedown checklist against Claude Code/Codex expectations

### Exit Criteria

- [ ] a new operator can discover these commands from `/` alone
- [ ] command results are visible without reading debug text
- [ ] the shell makes context and mode state obvious after each command
- [ ] every multi-step command makes the next keystroke obvious

## Edge Cases That Must Be Covered

- [ ] running turn when `/clear` is invoked
- [ ] pending approval when `/clear` or `/compact` is invoked
- [ ] compaction summary fails to generate
- [ ] no token usage available from backend
- [ ] estimated-only usage on Apple FM or streamed lanes
- [ ] switching into and out of plan mode mid-session
- [ ] plan mode with approval-paused or resumed sessions
- [ ] model changes after compaction
- [ ] `/mcp` before any real MCP runtime support exists
- [ ] detached-session resume after clear or compact

## Recommended Build Order

1. `/plan`
2. `/usage`
3. `/clear`
4. `/compact`
5. `/mcp`

This order gives the fastest operator value while keeping the biggest runtime
and protocol work, MCP and compaction, after the command framework is stable.

## Manual Shakedown Checklist

- [ ] use `/plan`, submit a request, and confirm the mode is obvious before and
      during the turn
- [ ] use `/usage` after several turns and confirm the output is readable and
      actionable
- [ ] use `/clear` and verify the next turn starts without prior context replay
- [ ] use `/compact` and verify the carry-forward summary is visible and sane
- [ ] compare `/clear` versus `/compact` and make sure the difference is obvious
- [ ] open `/mcp` and confirm it is honest about current Probe capability
- [ ] restart the TUI and verify any intended persisted state behaves correctly

## Validation

- `cargo test -p probe-tui`
- `cargo test -p probe-core --lib`
- targeted snapshot coverage for new overlays and mode chips
- targeted runtime tests for clear versus compact session transitions
- targeted regression tests for usage aggregation and truth labels

## Relationship To Existing Plans

This plan extends:

- `81-coding-agent-phased-execution-plan.md`
- `82-slash-command-command-surface-plan.md`

The coding-agent plan defined the overall UX bar.

The slash-command plan defined how command discovery and local actions should
feel.

This document defines the next operator-control layer above both of them.
