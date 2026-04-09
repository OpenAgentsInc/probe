# Phase 1 Spec: Reversibility And Review-First Editing

## Purpose

This document turns Phase 1 from `86-best-in-class-coding-agent-delivery-roadmap.md`
into an implementation-ready spec.

The goal is to make Probe feel safe enough for real coding work by shipping a
complete trust loop:

1. ask for a change
2. watch Probe work
3. inspect the proposed diff
4. apply or reject
5. undo or revert if needed

This phase is about user trust more than raw capability.

## Product Promise

After this phase, a user should be able to say:

"Probe can edit my repo, but I never feel trapped by what it did."

That means Probe must support:

- visible pre-edit safety
- review before risky writes
- a clean diff story
- a reversible last-task flow
- honest failure receipts when full revert is not safe

## Scope

This phase includes six linked capabilities:

1. task checkpoints
2. review-first apply mode
3. diff inspection
4. revert last task
5. conversation vs trace transcript separation
6. clearer final handoffs and failure copy

These are intentionally bundled. Shipping only one or two of them would create
more UX confusion than trust.

## Non-Negotiable UX Principles

### 1. Probe must never hide whether a change is already applied

The operator must always be able to answer:

- is this still a proposal?
- did the edit already land?
- can I still reject it safely?

### 2. The safest path must be obvious

If a task is medium or high risk, the primary path should be:

- inspect diff
- approve apply

not "hope it was correct and clean up later."

### 3. Undo language must be precise

Probe should distinguish:

- reject proposed edit
- revert applied task
- restore checkpoint
- cannot safely revert automatically

Do not overload all of these as "undo."

### 4. Conversation must stay readable

The main transcript should tell the human story.

Tool detail, patch detail, and diff detail should be available on demand, but
must not dominate the default reading experience.

### 5. Partial failure must stay honest

If Probe changed files and then failed, the app must never imply "nothing
happened."

## User Flows

## Flow 1: Small Safe Edit, Auto-Apply

### Example

"Rename this variable in one file."

### Expected UX

1. User submits prompt.
2. Probe shows immediate activity.
3. Session rail shows:
   - `checkpoint: ready`
   - `edit policy: auto-apply safe edits`
4. Probe applies the change.
5. Transcript ends with:
   - edited file path(s)
   - plain-English confirmation
6. Right rail shows:
   - `last task: applied`
   - `revert available`

### Notes

This is the fast path. It keeps Probe feeling responsive on low-risk work.

## Flow 2: Risky Edit, Review Before Apply

### Example

"Refactor the auth middleware and update its tests."

### Expected UX

1. User submits prompt.
2. Probe creates a task checkpoint before the first write.
3. Probe gathers or synthesizes candidate edits without applying them to the
   repo yet.
4. UI enters a `Review Changes` state.
5. User sees:
   - concise summary of what would change
   - changed file list
   - diff preview
   - primary actions:
     - `A` apply
     - `R` reject
     - `O` open full diff
6. If applied, Probe verifies and then hands work back conversationally.
7. If rejected, Probe ends with:
   - `No repo changes were applied`
   - proposal retained only in session trace

## Flow 3: Revert Last Task

### Example

"That edit was wrong, revert it."

### Expected UX

1. User types `/revert` or chooses `Revert last task`.
2. Probe opens a confirmation overlay showing:
   - last task summary
   - files affected
   - whether revert is exact or best-effort
3. User confirms.
4. Probe restores the checkpoint or applies the inverse task patch.
5. Transcript ends with:
   - reverted file path(s)
   - plain-English result
6. If revert cannot be completed safely, Probe says why and offers next steps.

## Flow 4: Partial Failure After Edit

### Example

Probe edits files, then validation crashes or backend dies.

### Expected UX

1. Receipt explicitly says edits landed.
2. Session rail shows:
   - `last task: partial`
   - `revert available` or `revert limited`
3. Transcript says:
   - what changed before failure
   - what failed
   - whether revert is available

## Flow 5: Conversation vs Trace

### Expected UX

Default transcript mode is `Conversation`.

That view shows:

- user messages
- assistant handoffs
- approval prompts
- compact edited file path lines
- compact failure notes

Optional `Trace` view shows:

- tool calls
- tool results
- patch plumbing
- validation commands
- raw step-by-step execution story

The operator should be able to toggle between them without losing place.

## Slash Commands And Actions

This phase adds or finalizes these commands:

- `/diff`
- `/revert`
- `/checkpoint`
- `/review_mode`
- `/trace`
- `/conversation`

### `/diff`

#### Meaning

Show the last task diff or current proposed diff.

#### Behavior

- if task is in review-first state: open proposed diff
- if last task already applied: open applied diff
- if no diff exists: explain that clearly

### `/revert`

#### Meaning

Revert the last applied task on the active lane.

#### Behavior

- blocked if no reversible task exists
- opens confirmation overlay
- shows exact vs limited revert truth

### `/checkpoint`

#### Meaning

Inspect available task checkpoints for the active lane.

#### Behavior

- shows latest checkpoint plus restore affordance
- eventually can grow into checkpoint history
- for phase 1, latest-task checkpoint is sufficient

### `/review_mode`

#### Meaning

Switch edit policy for future tasks on this lane.

#### Modes

- `auto_safe`
- `review_risky`
- `review_all`

Default recommendation:

- `auto_safe` on small edits
- `review_risky` as the overall product default for coding lanes

### `/trace`

Switch transcript to trace-first view.

### `/conversation`

Switch transcript back to conversation-first view.

## UX Surfaces

## 1. Session Rail

Add these fields:

- `edit policy: auto-safe | review-risky | review-all`
- `checkpoint: none | ready | restoring | unavailable`
- `last task: proposed | applied | rejected | reverted | partial`
- `revert: available | limited | unavailable`

## 2. Review Changes Overlay

New overlay purpose:

- inspect proposed edit package before apply

### Required Sections

- title
- natural-language summary
- changed file list
- diff preview snippet
- validation plan preview if available
- actions

### Required Keys

- `Up/Down`: move files or diff hunks
- `Tab`: switch summary/files/diff pane
- `A`: apply
- `R`: reject
- `Enter`: open focused section
- `Esc`: close back to transcript

## 3. Revert Confirmation Overlay

### Required Copy

- what task will be reverted
- which files are expected to change
- whether revert is exact checkpoint restore or best-effort patch
- whether user edits since the task may limit safety

### Required Actions

- confirm revert
- cancel
- inspect diff first

## 4. Diff Overlay

### Required Features

- file list
- per-file diff body
- added/removed line styling
- binary or non-text warning
- too-large diff truncation with honest copy

### Phase 1 Constraint

Do not attempt a fully featured git pager clone.

Ship a clean, legible, scrollable diff reader.

## 5. Transcript Mode Toggle

### Conversation Mode

Show:

- user request
- edited file path lines
- assistant handoff
- approval or failure banners

### Trace Mode

Show:

- all tool calls
- all tool results
- validation steps
- execution detail

The chosen mode should persist per lane for the TUI session.

## Runtime And Data Model Changes

## A. New Task Reversibility Record

Add a task-owned reversibility structure to `probe-protocol` session types.

### Proposed Shape

- `TaskCheckpointStatus`
  - `none`
  - `created`
  - `restore_available`
  - `restore_limited`
  - `restored`
  - `restore_failed`

- `TaskRevertibility`
  - `task_id`
  - `checkpoint_id`
  - `changed_files`
  - `created_before_write_at_ms`
  - `restore_status`
  - `restore_reason`
  - `user_dirty_overlap_files`
  - `revert_strategy`

- `TaskDiffSummary`
  - `files_changed`
  - `added_lines`
  - `removed_lines`
  - `binary_paths`
  - `truncated`

## B. Task Edit Policy

Add lane-scoped edit policy to runtime turn config and TUI lane config.

### Proposed enum

- `AutoApplySafe`
- `ReviewRisky`
- `ReviewAll`

## C. Proposed Edit Package

For review-first tasks, Probe needs a temporary package before final apply.

### Proposed shape

- `ProposedTaskEdit`
  - `task_id`
  - `summary`
  - `changed_files`
  - `diff_ref` or inline preview
  - `validation_plan`
  - `risk_level`

This should live in runtime/server state, not only in the UI.

## D. Transcript Preference

Add lane-scoped transcript mode:

- `Conversation`
- `Trace`

## E. Restore Strategy

Phase 1 should support two restore strategies:

1. exact checkpoint restore when safe
2. best-effort task patch revert when exact restore is not possible

Exact checkpoint restore is preferred.

## Current Seam Recommendations

### `probe-core`

- `crates/probe-core/src/runtime.rs`
- `crates/probe-core/src/tools.rs`
- `crates/probe-core/src/session_store.rs`
- `crates/probe-core/src/session_summary_artifacts.rs`

### `probe-protocol`

- `crates/probe-protocol/src/session.rs`
- `crates/probe-protocol/src/runtime.rs`

### `probe-server`

- `crates/probe-server/src/server.rs`
- `crates/probe-server/tests/stdio_protocol.rs`

### `probe-tui`

- `crates/probe-tui/src/app.rs`
- `crates/probe-tui/src/screens.rs`
- `crates/probe-tui/src/transcript.rs`
- `crates/probe-tui/src/worker.rs`
- `crates/probe-tui/src/bottom_pane.rs`

## Implementation Slices

Build this phase in five slices.

## Slice 1: Transcript Trust Upgrade

### Ship

- conversation vs trace mode
- calmer conversation transcript
- edited file path summary line
- last-task applied/partial state in rail

### Validation

- transcript snapshots for both modes
- edit-task and read-only-task rendering tests

## Slice 2: Task Checkpoints And Revertability Metadata

### Ship

- checkpoint creation before first write
- stored checkpoint metadata
- task revertability receipt
- revert availability in rail and CLI

### Validation

- exact restore available after successful edit
- restore limited when user edits same file later
- non-git repo degrades honestly

## Slice 3: Diff And Review Overlay

### Ship

- `/diff`
- review changes overlay
- diff preview
- file list navigation

### Validation

- risky edit opens review overlay
- reject exits with no applied repo changes
- apply proceeds and records receipt

## Slice 4: Revert Command

### Ship

- `/revert`
- revert confirmation overlay
- exact restore path
- best-effort fallback path

### Validation

- revert successful task
- revert partial-failure task
- revert blocked when no reversible task exists

## Slice 5: Edit Policy And Review-First Defaults

### Ship

- `/review_mode`
- lane-scoped edit policy
- safe auto-apply vs risky review decisioning
- better assistant handoff copy

### Validation

- small safe task auto-applies
- risky multi-file task enters review overlay
- policy persists for current lane

## Risk Heuristics For Phase 1

Phase 1 does not need perfect risk scoring, but it does need sane defaults.

Treat as `risky` when any of these are true:

- more than 1 file changes
- file is under `src/` and validation is not yet planned
- edit touches lockfiles, CI, migrations, or config roots
- task includes delete, rename, or replace-all behavior
- shell write is involved
- binary file involved
- total changed lines exceed a modest threshold

Treat as `safe` only when:

- 1 file changes
- text-only diff
- no destructive operation
- no shell write
- bounded line count

## Acceptance Test Matrix

These tests should exist before Phase 1 is marked complete.

### Happy Path

- small safe edit auto-applies and exposes revert
- risky edit opens review overlay before apply
- review overlay apply leads to conversational handoff
- review overlay reject leaves no repo changes

### Failure Path

- backend fails before write and no revert is offered
- backend fails after write and revert is offered
- validation fails after apply and receipt remains honest
- revert fails due to overlapping user edits and explains why

### Repo State Path

- dirty repo before task
- non-git repo
- binary file change
- generated file outside tracked edit path

### Recovery Path

- resume session with pending review state
- resume session with revertable last task
- interrupt during review-first workflow

### UX Path

- conversation mode hides noisy tool trace
- trace mode shows full tool activity
- slash commands reveal `/diff`, `/revert`, `/checkpoint`, `/review_mode`
- rail fields visibly update after apply and revert

## Manual UX Checklist

Before marking the phase complete, perform these live checks:

1. Ask for a one-file docs edit.
2. Ask for a multi-file risky refactor.
3. Reject the proposed risky change.
4. Re-run and apply it.
5. Revert it.
6. Ask a non-edit advice question and confirm no edit-trust chrome pollutes the answer.
7. Force a backend failure and confirm the receipt still says whether edits landed.

## Explicit Non-Goals For Phase 1

To stay disciplined, this phase does not include:

- full project memory
- full MCP runtime execution
- PR review mode
- background tasks or subagents
- team collaboration or hosted orchestration work
- selective hunk apply inside the repo working tree

Those belong to later phases.

## Recommendation

Start with Slice 1 and Slice 2 immediately.

That gives Probe the fastest visible trust upgrade:

- calmer transcript
- explicit revertability
- a tangible safety story before the full review-first overlay lands
