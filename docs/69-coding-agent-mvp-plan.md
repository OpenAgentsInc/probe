# Probe Coding Agent MVP Plan

## Purpose

This document captures a minimal, concrete plan for making Probe easier to use
as an active coding agent while keeping the current runtime boundary explicit.

It is intentionally narrower than the broader background-agent roadmap in
`docs/53-probe-background-agent-roadmap.md`.

For the tracked execution program and phase gates above this MVP direction, see
`docs/81-coding-agent-phased-execution-plan.md`.

## Goals

The MVP should let an operator:

- understand what the agent can do
- understand which backend, workspace, and approval posture are active
- understand what the agent is doing right now
- see when work is in progress
- understand when the agent is blocked on approval or input
- review what changed and what was verified
- understand what happened when a task failed, stopped, or only partially finished

## MVP Scope

The first coding-agent foundation should center on five surfaces:

1. **Agent contract**
   - clear operating rules
   - bounded tool access
   - explicit approval expectations
2. **Task execution loop**
   - inspect
   - plan
   - edit
   - verify
   - summarize
3. **User-visible progress state**
   - idle
   - thinking
   - reading
   - editing
   - running validation
   - waiting for approval
   - done
   - failed
4. **Verification contract**
   - what was changed
   - what commands were run
   - what passed or failed
   - what remains uncertain
5. **First-party documentation**
   - where the rules live
   - how operators should expect the agent to behave

## Recommended Near-Term Deliverables

### 1. Shared system prompt / runtime contract

Add or standardize a Probe-owned coding-agent prompt contract that defines:

- repo-bounded file operations
- preference for read/search before mutation
- deterministic edits when possible
- verification before claiming success
- concise status updates during multi-step work
- explicit uncertainty when validation was not run
- what to do with an already-dirty worktree
- what to do if the user changes files during a running task
- how to report unsupported or non-text file edits honestly

The contract should also stay aligned with the existing `coding_bootstrap`
runtime boundary from `docs/09-tool-loop-and-local-tools.md` and the approval
policy shape from `docs/14-approval-classes-and-structured-tool-results.md`.

### 2. Agent execution lifecycle

Keep the loop explicit in product surfaces:

1. inspect the task and repository context
2. state a short plan
3. perform bounded reads/searches
4. apply edits
5. run verification where appropriate
6. report results, risks, and next steps

### 3. Visible in-progress states

Probe should expose agent activity in a way the operator can immediately see.
At minimum, the UI should show:

- current phase
- active file or command when relevant
- whether the agent is waiting on the model, tools, or the user
- whether verification is still running
- whether edits have already landed in the repo
- whether the current turn is still using the expected backend and workspace

Recommended first implementation:

- add a compact status row or banner in the TUI
- mirror the same lifecycle in CLI/chat text output
- make approval waits visually distinct from normal thinking

Example status labels:

- `Planning`
- `Reading files`
- `Editing code`
- `Running tests`
- `Waiting for approval`
- `Waiting for backend`
- `Stopped`
- `Failed`
- `Complete`

### 4. Backend, workspace, and policy clarity

Probe currently supports multiple backend lanes and attach targets. For coding
work, the operator should not have to infer whether the active session is:

- Codex subscription
- local Qwen attach
- Tailnet attach
- Apple FM attach

The active shell should make the following obvious before the first submit:

- backend kind
- target base URL or attach target
- workspace cwd
- whether write tools are auto-approved, paused, or refused
- whether network or destructive shell actions require approval

This matters because a "backend request failed" message is not actionable if
the operator does not know which backend Probe actually tried to use.

### 5. Approval and blocking clarity

The operator should never have to guess why progress stopped.

When blocked, Probe should explicitly identify one of:

- waiting for user instruction
- waiting for approval
- waiting for tool result
- waiting for backend response
- waiting for long-running shell validation
- validation failed and needs operator choice
- task stopped by operator
- task interrupted because the session resumed on another client

### 6. Workspace change visibility and trust

Probe should separate three different truths that are easy to conflate during
interactive use:

1. files that were already dirty before the task started
2. files the agent changed during the task
3. files changed later by the operator while the task was running

The MVP should add a task-scoped change summary that can answer:

- did anything in the repo actually change?
- which files changed because of this task?
- were there partial edits before failure?
- does the current diff include pre-existing unrelated work?

Without that separation, an operator cannot trust the end-of-task summary or
tell whether the app really coded anything.

### 7. End-of-task summary

Each task should end with a compact, trustworthy summary:

- files changed
- whether those files were already dirty at task start
- high-level change summary
- verification performed
- outstanding risks or follow-ups
- explicit note when no repo changes were made

### 8. Failure, interruption, and resume behavior

The MVP should define honest behavior for the cases where a coding session does
not end cleanly:

- backend request failed before any tool calls ran
- backend failed after edits were already applied
- shell command timed out or produced truncated output
- approval paused and the user left the session
- the operator stopped the task mid-flight
- the TUI or client detached and later resumed

In those cases Probe should preserve:

- last known phase
- last active file or command
- whether any edits landed
- which verification steps completed
- what action the operator can take next

## Suggested Implementation Phases

### Phase 1: Docs and contract alignment

- add this MVP plan
- link it from `README.md`
- align prompt/agent docs with the expected execution loop
- define the task-scoped change-summary contract
- define the operator-visible backend and approval contract

### Phase 2: Runtime activity model and operator truth

- add a first-class runtime activity enum or equivalent shared state
- propagate it through runtime events
- render it in TUI and CLI/chat surfaces
- include backend kind, cwd, and approval posture in the surfaced session state
- persist enough task metadata to reconstruct the last known phase after resume

### Phase 3: Repo change visibility and summaries

- capture a task-start workspace baseline
- compute task-scoped changed files separately from pre-existing dirty files
- show whether edits landed before a failure or stop
- expose a compact "files changed by this task" summary in TUI and CLI/chat

### Phase 4: Stronger verification UX

- show validation start/finish clearly
- summarize command outcomes in a standard format
- distinguish unverified edits from verified edits
- surface truncated or timed-out validation honestly

### Phase 5: Blocking, cancellation, and resume

- improve resumable blocked states
- preserve user-visible task phase across resume/reattach
- add a first-class stopped/interrupted state
- preserve partial-completion truth after backend or tool failure
- align with the broader background-agent roadmap

## Edge Cases The MVP Must Cover

- starting a task from an already-dirty git worktree
- running in a non-git directory where diff-based summaries are limited
- editing a file that changed on disk while the agent was still working
- partial success where one edit landed but later validation failed
- no-op tasks where the agent only read files and made no code changes
- backend misconfiguration or wrong-lane submission
- binary or generated-file edits that do not fit the normal text-patch path
- command timeout or truncated shell output that hides the real failure
- approval pause followed by detach, resume, approve, and continue
- operator stop/cancel after edits landed but before summary generation
- sessions resumed from another client or surface

## Concrete MVP Backlog

1. Add a shared runtime activity/state enum in `probe-core` that includes
   backend wait, tool wait, validation, stopped, and failed states.
2. Add a task-scoped workspace snapshot at turn start so Probe can tell
   pre-existing dirty files apart from agent-written files.
3. Extend runtime events and transcript-facing summaries with backend kind,
   cwd, approval posture, active file/command, and task change counts.
4. Render a compact operator banner in `probe-tui` that shows backend, cwd,
   phase, and whether repo edits have landed yet.
5. Add CLI/chat end-of-task summaries that report changed files, verification,
   and remaining uncertainty using the same shared summary builder.
6. Add explicit failed/stopped/interrupted summary paths so partial progress is
   still visible after an error or cancel.
7. Add focused tests for wrong-backend selection, dirty-worktree baselines,
   partial-edit failures, approval resume, and resumed-session status replay.

## UX Note: In-Progress Visibility

A key product requirement surfaced during interactive use is that the user needs
an obvious visual indication when the agent is actively working.

That means Probe should avoid a silent gap between user request and final
answer. Even a minimal signal is better than none, for example:

- spinner plus current phase
- timestamped lifecycle events in transcript
- a persistent status line in the TUI footer or header

The important outcome is trust: the operator should be able to tell the
difference between:

- the agent actively working
- the agent waiting on a tool or model
- the agent waiting on the operator
- the agent using the wrong backend or workspace for the intended task
- the app appearing stuck

## Relationship to Existing Docs

This MVP plan complements, but does not replace:

- `AGENTS.md`
- `docs/09-tool-loop-and-local-tools.md`
- `docs/14-approval-classes-and-structured-tool-results.md`
- `docs/43-probe-runtime-event-stream-and-live-tui-lifecycle.md`
- `docs/45-probe-tui-resumable-approval-broker.md`
- `docs/53-probe-background-agent-roadmap.md`

## Definition of Done for This MVP

Probe has a usable coding-agent foundation when:

- the agent contract is documented
- the execution loop is consistent across surfaces
- the active backend, cwd, and approval posture are obvious before submit
- the user can visually tell when work is in progress
- blocked states are explicit
- repo changes attributable to the task are visible
- failures, stops, and resume cases preserve honest partial-progress truth
- final summaries are trustworthy and verification-aware
