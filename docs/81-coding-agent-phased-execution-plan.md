# Coding Agent Phased Execution Plan

## Purpose

This document turns `69-coding-agent-mvp-plan.md` into a tracked execution
program for making Probe feel like a strong day-to-day local coding agent.

The goal is not only to add more runtime capabilities. The goal is to make the
existing Probe runtime feel obvious, trustworthy, and easy to use inside a TUI.

## UX Bar

Probe should feel closest to the best properties operators expect from
Claude Code and Codex-style coding tools, while staying honest about Probe's
current local-first runtime boundary.

The TUI should optimize for:

- obvious current backend, workspace, and approval posture before submit
- one clear golden path for "ask for a change, watch work happen, review what changed"
- transcript-first interaction with compact status and detail, not modal overload
- progressive disclosure where detail is available on demand without cluttering the main shell
- honest failure and partial-progress reporting instead of vague "backend failed" dead ends
- keyboard-first operation with predictable commands and minimal memorization
- defaults that work for the most common coding flow without requiring setup archaeology

## Non-Negotiable UX Principles

### 1. Always show "where this prompt will run"

Before the operator submits, Probe should make it obvious:

- which backend lane is active
- which cwd is active
- whether tools can write automatically, pause, or refuse
- whether the session is attached locally, via Tailnet, or to hosted Codex

The operator should not need to open a secondary overlay just to avoid the
wrong backend.

### 2. Make progress visible within one second

After submit, the user should see immediate evidence that Probe is doing work.

That means:

- phase/status changes quickly
- active file or command when useful
- visible distinction between thinking, reading, editing, validating, and waiting
- no silent dead air that looks like the app froze

### 3. Keep the transcript as the source of operator trust

The main transcript should answer:

- what the user asked
- what Probe did
- where it got blocked
- what changed
- what was verified

The right answer is not "open another pane and guess." The transcript should
carry the main story, with overlays used only for richer inspection or action.

### 4. Make failure states actionable

Failure copy should tell the operator:

- what failed
- which backend or command failed
- whether any edits already landed
- what they can do next

### 5. End every task with a trustworthy receipt

A coding turn should conclude with:

- changed files
- verification performed
- outstanding uncertainty
- explicit note when nothing changed

## Success Criteria

Probe has reached the intended UX bar when an operator can reliably do the
following without prior repo-specific coaching:

1. launch the TUI and confidently know which backend and workspace will handle
   the next prompt
2. submit a coding request and immediately see that the agent is working
3. tell whether Probe is reading, editing, waiting, validating, or blocked
4. review which files changed because of the task
5. understand whether the work is verified, partially complete, or failed
6. recover cleanly from approval pauses, backend failures, or resumed sessions

## Tracking Model

Each phase below has:

- a user-facing outcome
- concrete implementation scope
- clear exit criteria
- validation guidance

Use the checkbox state to track status:

- `[ ]` not started
- `[~]` in progress
- `[x]` done

## Phase 1: Operator Truth And Golden Path

Status: `[x]`

### Outcome

The operator can always tell where the next prompt will run and what authority
Probe currently has.

### Scope

- [x] surface backend kind directly in the main TUI shell
- [x] surface cwd directly in the main TUI shell
- [x] surface approval posture directly in the main TUI shell
- [x] improve the empty-state and startup copy so the golden path is obvious
- [x] change wrong-backend or missing-backend errors to name the active lane and target

### Primary Seams

- `crates/probe-tui/src/app.rs`
- `crates/probe-tui/src/screens.rs`
- `crates/probe-tui/src/bottom_pane.rs`
- `crates/probe-tui/src/worker.rs`
- `crates/probe-core/src/runtime.rs`

### Exit Criteria

- [x] the main screen shows backend, cwd, and approval posture without opening an overlay
- [x] a user can distinguish Codex from local Qwen/Tailnet at a glance
- [x] backend failure copy names the attempted backend and target
- [x] the default startup path explains how to submit a prompt and where it will run

### Validation

- [x] `cargo test -p probe-tui`
- [ ] `cargo test -p probe-cli --test cli_regressions`
- [x] snapshot coverage for each backend lane startup state

## Phase 2: Runtime Activity Model And Visible Progress

Status: `[x]`

### Outcome

A running turn visibly moves through explicit phases instead of feeling like a
single black box.

### Scope

- [x] add a first-class activity/state model in `probe-core`
- [x] include waiting-for-backend, reading, editing, validating, paused,
      stopped, and failed states
- [x] propagate that state through runtime events
- [x] render a compact activity banner in the TUI
- [x] keep the transcript and status bar consistent with the same activity model

### Primary Seams

- `crates/probe-core/src/runtime.rs`
- `crates/probe-protocol/src/runtime.rs`
- `crates/probe-tui/src/worker.rs`
- `crates/probe-tui/src/app.rs`
- `crates/probe-tui/src/screens.rs`

### Exit Criteria

- [x] the operator sees a visible state change within one second of submit
- [x] the UI differentiates backend wait from tool execution and validation
- [x] approval pauses and backend failures are visually distinct from ordinary thinking
- [x] resumed sessions recover the last meaningful activity state instead of resetting to vague idle text

### Validation

- [x] `cargo test -p probe-core --lib`
- [x] `cargo test -p probe-tui`
- [x] targeted runtime-event ordering tests for new activity states

## Phase 3: Task-Scoped Repo Change Visibility

Status: `[x]`

### Outcome

Probe can tell the operator what changed because of this task, not just what is
currently dirty in the repo.

### Scope

- [x] capture a task-start workspace baseline
- [x] distinguish pre-existing dirty files from agent-made changes
- [x] distinguish agent-made changes from later user edits when possible
- [x] expose "did edits land yet?" during the turn
- [x] add task-scoped changed-file summaries for success and failure paths

### Primary Seams

- `crates/probe-core/src/runtime.rs`
- `crates/probe-core/src/session_store.rs`
- `crates/probe-core/src/session_summary_artifacts.rs`
- `crates/probe-protocol/src/session.rs`
- `crates/probe-tui/src/screens.rs`

### Exit Criteria

- [x] Probe can explicitly report "no repo changes were made"
- [x] Probe can explicitly report "partial edits landed before failure"
- [x] the final summary distinguishes task changes from pre-existing dirty files
- [x] non-git or unsupported cases degrade honestly instead of fabricating diff truth

### Validation

- [x] `cargo test -p probe-core --lib`
- [x] targeted tests for dirty-worktree baselines and partial-edit failures
- [x] snapshot coverage for no-op, success, and partial-failure summaries

## Phase 4: Verification And Final Receipts

Status: `[x]`

### Outcome

Probe ends coding turns with a trustworthy receipt similar to what operators
expect from strong coding agents: what changed, what ran, what passed, and what
remains uncertain.

### Scope

- [x] standardize final summary construction across TUI and CLI/chat
- [x] include changed files, verification commands, and outcome summaries
- [x] surface timed-out or truncated validation clearly
- [x] mark unverified edits explicitly
- [x] improve transcript rows so tool/validation output stays compact but informative

### Primary Seams

- `crates/probe-core/src/session_summary_artifacts.rs`
- `crates/probe-core/src/tools.rs`
- `crates/probe-tui/src/screens.rs`
- `crates/probe-tui/src/worker.rs`
- `crates/probe-cli/src/main.rs`

### Exit Criteria

- [x] successful tasks end with a compact receipt
- [x] failed tasks still end with a partial-progress receipt
- [x] validation commands and outcomes are summarized consistently across surfaces
- [x] the operator can tell the difference between "passed", "not run", and "timed out"

### Validation

- [x] `cargo test -p probe-core --lib -- --test-threads=1`
- [x] `cargo test -p probe-tui`
- [x] `cargo test -p probe-cli --test cli_regressions`

## Phase 5: Blocking, Approval, Cancel, And Resume Polish

Status: `[x]`

### Outcome

Probe behaves predictably when a turn pauses, gets cancelled, fails mid-flight,
or is resumed later from another surface.

### Scope

- [x] add first-class stopped/interrupted states
- [x] preserve last known phase, active command, and change status on failure
- [x] make approval waits read as "action needed", not generic busy state
- [x] improve resume messaging so the operator knows whether work is continuing or waiting
- [x] ensure cancellation leaves behind an honest receipt instead of an abrupt transcript gap

### Primary Seams

- `crates/probe-core/src/runtime.rs`
- `crates/probe-core/src/session_store.rs`
- `crates/probe-core/src/session_summary_artifacts.rs`
- `crates/probe-tui/src/app.rs`
- `crates/probe-tui/src/screens.rs`

### Exit Criteria

- [x] cancelling a task leaves a visible stopped state and partial receipt
- [x] approval-paused sessions remain obvious after detach and resume
- [x] backend failures after edits do not hide the fact that edits landed
- [x] resumed sessions restore the operator story without forcing transcript archaeology

### Validation

- [x] `cargo test -p probe-core --lib`
- [x] `cargo test -p probe-tui`
- [x] targeted tests for pause -> detach -> resume -> approve and stop-after-edit cases

## Phase 6: Best-In-Class TUI Ergonomics

Status: `[x]`

### Outcome

Once the runtime truth is solid, Probe gets the ergonomic polish needed to feel
fast, obvious, and pleasant in everyday coding work.

### Scope

- [x] simplify the main-shell copy so the most important state dominates
- [x] reduce keyboard-command discoverability burden with context-sensitive hints
- [x] keep overlays focused and lightweight instead of dumping dense operator prose
- [x] make the transcript easier to scan during long coding turns
- [x] tighten spacing and labels so reading/editing/validation states feel crisp

### UX Guardrails

- [x] prefer terse labels over verbose debug text
- [x] prefer one obvious primary action per screen state
- [x] prefer stable layout regions over shifting UI chrome
- [x] prefer operator-language copy over internal-runtime jargon

### Exit Criteria

- [x] a new operator can complete the golden path without external explanation
- [x] the main screen remains readable during long tool-heavy sessions
- [x] important state is available without burying the user in overlays
- [x] the TUI feels coding-shell-native rather than like a generic admin console

### Validation

- [x] refresh TUI snapshots for all primary shell states
- [x] do a manual golden-path pass on Codex and Qwen lanes
- [x] capture UX regressions as follow-up issues instead of letting them hide in polish debt

### Validation Notes

- April 3, 2026: a clean daemon-backed Codex chat pass on a temporary
  `PROBE_HOME` completed with the coding tool set enabled, executed six tool
  calls, and returned a normal task receipt without the earlier empty-path tool
  failures.
- April 3, 2026: the matching Qwen pass failed fast and honestly with
  `server at http://127.0.0.1:8080/v1 did not become ready within 15s`, which
  is the expected operator truth when the local Psionic Qwen server is not
  running.
- Regression fixed during validation: `code_search` and `list_files` could
  receive `path: ""` from Codex, burn controller turns on `tool paths must not
  be empty`, and push the model into unnecessary shell fallback. Probe now
  treats blank navigation paths as the session workspace root and has explicit
  regression coverage for that behavior.

## Cross-Cutting Edge Cases

These cases should be treated as mandatory acceptance coverage across phases:

- [x] wrong backend lane selected for the task
- [x] backend unreachable before any tool call
- [x] backend fails after edits already landed
- [x] dirty worktree before task start
- [x] no-op task that only reads files
- [x] validation timeout or truncated shell output
- [x] approval pause followed by detach and resume
- [~] user edits the same file while Probe is working
- [x] binary or generated-file change that does not fit the text patch path
- [x] non-git working directory

### Remaining Concurrent-Edit Limitation

Probe now surfaces additional dirty files that appear during a task outside its
tracked tool results, which covers many generated-file and concurrent-workspace
change cases honestly.

The remaining hard case is exact attribution when both Probe and the operator
edit the same path during the same task window. That should stay on the local
UX shakedown list until Probe has per-file snapshot or watcher support.

## Local UX Shakedown

Before opening a PR, run this operator pass locally:

1. `cargo test -p probe-server`
2. `cargo test -p probe-tui`
3. `cargo test -p probe-cli --test cli_regressions`
4. Launch `cargo probe` on the Codex lane and verify the main shell makes the
   active backend, cwd, approval posture, and current activity obvious before
   and during a turn.
5. Submit a read-only request and confirm the receipt ends with "no repo
   changes" rather than vague success language.
6. Submit a small patch request with a validation command and confirm the rail
   shows edits, verification, and any remaining uncertainty without opening
   extra overlays.
7. Trigger an approval pause, detach or restart the client, resume, approve,
   and confirm the task story stays intact.
8. Exercise the Qwen lane with the local server both down and up, and confirm
   the failure copy is honest when the backend is unavailable and the active
   target is obvious when it is available.
9. Repeat one task from a non-git directory and one from a dirty git worktree
   so the receipts are easy to compare.

## Recommended Implementation Order

If implementation starts immediately, follow this order:

1. Phase 1
2. Phase 2
3. Phase 3
4. Phase 4
5. Phase 5
6. Phase 6

This keeps the work aligned to the highest UX leverage:

- first fix "what backend/workspace am I on?"
- then fix "is the agent working or stuck?"
- then fix "did it actually change my repo?"
- then fix "what was verified?"
- then harden the blocked/resume cases
- then spend polish effort once the operator truth is already strong

## Relationship To Other Docs

This plan is the execution layer above:

- `69-coding-agent-mvp-plan.md`
- `09-tool-loop-and-local-tools.md`
- `14-approval-classes-and-structured-tool-results.md`
- `43-probe-runtime-event-stream-and-live-tui-lifecycle.md`
- `45-probe-tui-resumable-approval-broker.md`
- `57-codex-third-inference-mode.md`
