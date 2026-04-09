# Best-In-Class Coding Agent Gap Plan

## Purpose

This document is the next-level product plan for turning Probe into a
best-in-class coding agent that can compete directly with Claude Code, Codex,
Cursor, Windsurf, Aider, Gemini CLI, Goose, and similar agentic coding tools.

The top priority is not adding random features.

The top priority is an incredible UX:

- intuitive enough that a new user can succeed without reading docs first
- trustworthy enough that an expert will hand it real code changes
- responsive enough that the app never feels stalled or mysterious
- reversible enough that users can take risks without anxiety
- powerful enough that it grows from "smart TUI" into "daily driver coding shell"

## Research Basis

This plan is based on two inputs:

1. Probe's current shipped surface as of April 7, 2026, audited from:
   - `README.md`
   - `docs/69-coding-agent-mvp-plan.md`
   - `docs/81-coding-agent-phased-execution-plan.md`
   - `docs/82-slash-command-command-surface-plan.md`
   - `docs/83-operator-commands-and-context-controls-plan.md`
   - `docs/84-mcp-provider-command-onboarding-plan.md`

2. Current market UX patterns from official docs and primary sources:
   - Claude Code slash commands, memory, hooks, MCP, and subagents:
     - https://docs.claude.com/en/docs/claude-code/slash-commands
     - https://docs.claude.com/en/docs/claude-code/memory
     - https://docs.claude.com/en/docs/claude-code/hooks
     - https://docs.claude.com/en/docs/claude-code/mcp
     - https://docs.claude.com/en/docs/claude-code/subagents
   - Cursor rules, memories, MCP, background agents, and Bugbot:
     - https://docs.cursor.com/en/context/rules-for-ai
     - https://docs.cursor.com/en/context/memories
     - https://docs.cursor.com/en/context/mcp
     - https://docs.cursor.com/cli/mcp
     - https://docs.cursor.com/en/background-agents
     - https://docs.cursor.com/en/bugbot
   - Aider chat modes and slash commands:
     - https://aider.chat/docs/usage/modes.html
     - https://aider.chat/docs/usage/commands.html
   - Windsurf Cascade checkpoints and real-time awareness:
     - https://docs.windsurf.com/windsurf/cascade/cascade
   - Gemini CLI checkpoints, MCP, custom context, and automation surface:
     - https://github.com/google-gemini/gemini-cli
   - Goose extensions, session management, and shareable recipes:
     - https://block.github.io/goose/docs/getting-started/using-extensions/
     - https://block.github.io/goose/docs/guides/managing-goose-sessions
     - https://block.github.io/goose/docs/guides/recipes/session-recipes
   - OpenAI Codex MCP configuration and broader Codex agent surface:
     - https://platform.openai.com/docs/docs-mcp
     - https://docs.github.com/en/copilot/concepts/agents/openai-codex

## Current Probe Strengths

Probe already has a stronger foundation than many early coding shells in the
areas that are hardest to retrofit later:

- explicit backend lanes and typed backend metadata
- append-only transcript and session persistence
- real approval model and pending-approval resume path
- task receipts and verification summaries
- runtime activity model
- detached daemon-backed sessions and resume flows
- TUI slash-command surface
- early MCP management UX
- local-first tool loop and clear ownership of tool execution

This means Probe does not need to start from zero.

It already has credible runtime truth.

The remaining work is mostly about:

- making the right actions easier to discover
- making the safest actions easier than the dangerous ones
- making the results more legible and reversible
- making configuration, memory, context, and integrations feel native

## Market Read: What Best-In-Class Tools Actually Win On

Across Claude Code, Cursor, Windsurf, Aider, Gemini CLI, Goose, and Codex,
the strongest UX patterns repeat:

### 1. Command surfaces are deep, not shallow

The best tools do not stop at `/help` and `/model`.

They expose a real operator shell for:

- mode switching
- memory or rules management
- approvals and permissions
- context compaction or clearing
- integrations
- cost and usage
- review or git actions
- recovery and diagnostics

### 2. Users can steer the agent without re-prompting it

Top tools let the operator change the environment through commands and menus,
not through fragile natural-language rituals.

Examples:

- switch model
- add or remove context files
- add another directory
- compact context
- enable or disable tools
- inspect usage and limits
- re-open recent sessions
- adjust MCPs and permissions

### 3. Great tools make work reversible

The biggest UX trust accelerator is reversibility.

Strong tools offer some combination of:

- checkpoints
- rewind
- revert
- undo
- diff review
- branch isolation
- safe approvals

This is one of the clearest current gaps between Probe and the strongest
operators.

### 4. Great tools have real memory and rules

The leaders do not rely only on conversation history.

They expose persistent instruction layers like:

- project memory
- personal memory
- repo rules
- folder-scoped rules
- reusable commands or skills
- hooks or automation points

Without this, the user must keep re-explaining how they like to work.

### 5. Great tools separate thinking, editing, and reviewing cleanly

The best agent experiences distinguish:

- ask or explain
- plan
- code
- review
- background execution

Some do this through chat modes, some through subagents, some through review
bots, some through checkpoints and diff review. But they all reduce ambiguity
about whether the next prompt will discuss, plan, or mutate code.

### 6. Great tools make integrations feel like products, not config blobs

Strong MCP and extension UX has:

- add, list, inspect, enable, disable, remove
- connection status
- authentication state
- tool inventory
- clear error handling
- same config across CLI and GUI when possible

Probe has started this journey, but it is not yet at the level of Cursor,
Claude Code, Goose, or Codex.

### 7. Great tools invest heavily in background work

Cursor background agents, Claude subagents, Windsurf simultaneous cascades, and
Goose recipes all point to the same operator need:

- do more than one thing at once
- keep the main thread clean
- preserve context without overwhelming it

Probe is still primarily a single-thread interactive shell.

### 8. Great tools reduce "agent anxiety"

The strongest products reduce five kinds of uncertainty:

- What workspace am I in?
- What model/backend is active?
- What changed?
- Can I undo it?
- What do I do next?

Every part of the Probe roadmap should be judged against those five questions.

## Priority-Ordered Gap List

This is the recommended product backlog in strict priority order.

Items near the top are the highest leverage for user trust and daily usability.

### P0: Must-Have To Compete Seriously

1. Reversible editing with checkpoints, rewind, and undo
   - Probe needs a first-class way to create a pre-edit checkpoint, preview
     changes, and revert the last task or selected files.
   - This is table stakes for trust against Claude Code, Windsurf, and modern
     agentic editors.

2. Native project memory and rules
   - Probe needs a Probe-owned equivalent of `CLAUDE.md`, `GEMINI.md`,
     `.cursor/rules`, and AGENTS-style project instructions.
   - Minimum shape:
     - user-level memory
     - repo-level memory
     - folder-scoped memory
     - in-app editing surface

3. Conversation-first transcript plus on-demand trace view
   - The main story must be human-readable.
   - Tool detail should be collapsible, inspectable, and recoverable, but not
     the default visual weight.
   - Probe should have explicit `Conversation` and `Trace` views or a similarly
     strong toggle.

4. Fully functional MCP runtime
   - The current MCP manager is a strong UX start, but Probe still lacks:
     - executable external MCP sessions
     - live connected/disconnected state
     - tool inventory
     - per-session MCP enablement
     - MCP tool receipts in turns

5. Safe review-before-apply workflow
   - Probe should support a "show me the diff first" or "apply after review"
     workflow for medium and high-risk edits.
   - This includes chunked diff views and selective apply where feasible.

6. Better failure and recovery UX
   - 429s, auth failures, dead backends, broken MCPs, shell tool failures, and
     partial edits need strong next steps, retry guidance, and visible recovery
     actions from the shell.

7. First-class workspace management
   - Users should be able to:
     - open a workspace
     - switch workspace
     - add a secondary workspace
     - see active roots clearly
   - This should not require relaunching or remembering `--cwd`.

8. Git-native trust loop
   - Probe needs better built-in git affordances:
     - show diff
     - stage
     - commit
     - create branch
     - revert task
     - understand merge conflicts
     - review PR comments

### P1: High-Value UX Differentiators

9. Stronger mode model
   - Probe currently has plan and coding concepts, but it still needs a fully
     legible operating-mode system:
     - ask
     - plan
     - code
     - review
     - maybe background
   - Temporary one-message mode overrides should also exist.

10. Rich context control
   - Users need more than `/clear` and `/compact`.
   - They need:
     - add file
     - drop file
     - pin file
     - inspect active context
     - copy/export context
     - compact with focus instructions

11. Rules, hooks, and workflow automation
   - Probe should let teams define pre/post-tool or pre/post-task automation
     and validation hooks, inspired by Claude Code hooks and Goose recipes.

12. Real review mode
   - A dedicated review mode should optimize for:
     - bug finding
     - regression spotting
     - security risks
     - PR comment generation
   - This should feel different from general coding.

13. Background or delegated work
   - Probe needs a way to launch long-running work without holding the main
     foreground shell hostage.
   - This could be:
     - detached background tasks
     - task queue
     - subagents
     - remote hosted workers

14. Better usage and quota UX
   - Usage should answer:
     - what this turn cost
     - how big this conversation is
     - whether the backend is rate-limited
     - whether compacting would help

15. Better onboarding and diagnostics
   - New users should be able to self-serve:
     - auth status
     - backend health
     - sandbox or approval posture
     - MCP health
     - common fixes

### P2: Powerful Expansion Areas

16. Shareable task recipes and reusable workflows
   - Save a successful agent setup as a reusable recipe:
     - goal
     - enabled MCPs
     - mode
     - model
     - workspace assumptions
     - validation steps

17. Multimodal implementation flows
   - Screenshot, image, and mockup input should be first-class for coding and
     UI work.

18. PR review automation and issue triage
   - Probe should be able to review diffs, summarize PRs, and help with issue
     triage in a first-class way.

19. Remote and hosted execution story
   - Probe already has hosted substrate work. Productizing that into a clear
     operator story could become a major differentiator.

20. Team collaboration primitives
   - Shared session handoff, participant awareness, and session ownership are a
     long-term team differentiator if paired with excellent UX.

### P3: Nice-To-Have After Core UX Is Elite

21. Voice and spoken command entry
22. Built-in extension marketplace or discovery directory
23. One-click app deployment workflows
24. Desktop companion or IDE bridge parity
25. Higher-level analytics dashboards for team workflows

## Recommended Implementation Phases

This phase order is optimized for UX impact, not for architectural neatness.

## Phase A: Trust, Reversibility, And Human-Legible Output

### Goal

Make users feel safe letting Probe touch real code.

### Scope

- checkpoint before risky tasks
- rewind or revert last task
- diff review overlay
- conversation vs trace toggle
- better final assistant handoff language
- clearer partial-edit and failure recovery copy

### Why This Is First

Without reversibility and readable output, every other power feature feels more
dangerous, not more helpful.

### Edge Cases

- task fails after partial edit
- user edited the same file after Probe changed it
- repo is already dirty before checkpoint
- non-git repo
- binary file touched
- revert target no longer applies cleanly

### Exit Criteria

- user can ask for an edit, inspect it, and undo it confidently
- transcript defaults to a conversation-first story
- recovery UI is explicit when Probe cannot safely revert

## Phase B: Memory, Rules, And Persistent Working Style

### Goal

Teach Probe how the user and repo want work done without repeated prompting.

### Scope

- `PROBE.md` or equivalent memory file hierarchy
- user-level memory
- repo-level memory
- directory-scoped memory
- `/memory` management command
- UI editor or picker for active memory layers
- surfaced active rules in shell status

### Competitive Parity Target

- Claude Code memory
- Cursor rules and memories
- Gemini `GEMINI.md`
- AGENTS-style repo guidance

### Edge Cases

- conflicting user-level and repo-level rules
- memory file missing or malformed
- multiple nested memory files
- switching workspaces with different memory stacks

### Exit Criteria

- users can inspect and edit active instructions without leaving Probe
- Probe clearly shows what memory layers are active for the next turn

## Phase C: Real Operator Shell

### Goal

Make Probe feel like an actual daily-driver agent shell.

### Scope

- complete slash-command parity for common operator actions
- status, doctor, permissions, model, backend, cwd, usage, memory, review,
  checkpoint, revert, diff
- richer keyboard hints and state-aware ranking
- workspace picker
- file context picker
- mode switcher with temporary override mode

### Edge Cases

- command unavailable on active backend
- command blocked during running task
- command changes future turns but not current turn
- ambiguous slash prefix with many results

### Exit Criteria

- a new user can discover most important actions just by typing `/`
- no critical workflow requires hidden key knowledge

## Phase D: MCP Done Properly

### Goal

Turn MCP from promising UX scaffolding into a real runtime capability.

### Scope

- execute enabled MCP servers during turns
- connection lifecycle and retries
- tool inventory browser
- auth status
- per-session enable or disable
- tool receipts and runtime activity
- import adapters for major providers
- clear local vs remote transport semantics

### Edge Cases

- server installed but not runnable
- OAuth required
- duplicate tool names across servers
- server hangs on startup
- server tool fails midway through task
- session resumes with changed MCP state

### Exit Criteria

- `/mcp` can manage live, usable integrations end to end
- users can tell what tools are actually available before prompting

## Phase E: Git, Review, And Delivery Workflow

### Goal

Make Probe excellent at the actual coding loop, not just code mutation.

### Scope

- diff browser
- selective apply
- stage and commit flows
- branch creation
- revert task to branch state
- review mode
- PR comment consumption
- post-change checklist and validation workflow

### Competitive Parity Target

- Aider git loop
- Cursor Bugbot review model
- Claude Code `/review`, `/pr_comments`, `/memory`, `/permissions`

### Edge Cases

- merge conflict in current branch
- staged and unstaged changes mixed
- unrelated dirty files
- user on detached HEAD
- no git repo

### Exit Criteria

- user can go from “make a change” to “review, validate, and prepare delivery”
  without leaving Probe

## Phase F: Background Work, Delegation, And Session Orchestration

### Goal

Let Probe do more than one thing without overwhelming the main conversation.

### Scope

- detached background tasks from the TUI
- task list with live status
- optional delegated subagents or workers
- follow-up prompts on running work
- reopen or take over background task
- structured task handoff receipts

### Competitive Parity Target

- Cursor background agents
- Claude Code subagents
- Windsurf simultaneous cascades

### Edge Cases

- background task edits same files as foreground task
- task orphaned on app restart
- backend quota exhausted mid-run
- delegated worker needs approval
- hosted background task reconnect

### Exit Criteria

- Probe can run long work without blocking the main shell
- users can see task status and re-enter context cleanly

## Phase G: Performance, Polish, And Sensory UX

### Goal

Make the shell feel delightful, fast, and mature.

### Scope

- latency budgets for visible progress
- smoother redraw and reduced flicker
- keyboard consistency pass
- adaptive layouts for small terminals
- better empty states and recovery states
- tasteful color hierarchy and stronger information scent
- optional sound or subtle alerting for task completion

### Edge Cases

- very small terminal
- very long transcripts
- reduced-color terminals
- slow backend streaming
- mixed Unicode and wide characters

### Exit Criteria

- Probe feels fast and understandable even during long-running work
- common actions are obvious at a glance

## Phase H: Ecosystem And Team Differentiation

### Goal

Push beyond parity into category-defining workflow value.

### Scope

- shareable recipes
- project bootstrapping kits
- richer hosted or remote team sessions
- persistent team memory layers
- issue and PR automation
- review bots and scheduled maintenance tasks

### Exit Criteria

- Probe is not only a coding shell, but a strong team coding platform

## Critical User Flows That Must Be Designed Explicitly

These flows should all be treated as first-class design targets, not incidental
byproducts.

1. First run
   - install
   - auth
   - choose backend
   - select workspace
   - understand what Enter will do

2. Ask-only question
   - no edits
   - no edit-template response pollution
   - clear answer, compact trace

3. Plan then code
   - switch to plan
   - discuss approach
   - switch to code
   - apply with confidence

4. Edit with review-first
   - request change
   - see live progress
   - inspect diff
   - approve apply
   - verify
   - undo if needed

5. Resume interrupted work
   - reopen session
   - understand what is pending
   - know whether edits already landed
   - continue or abort safely

6. Add an integration from provider docs
   - `/mcp`
   - paste provider command
   - authenticate if needed
   - enable
   - inspect tools

7. Recover from backend failure
   - understand what failed
   - know whether any changes landed
   - retry, switch backend, or stop

8. Work in a dirty repo
   - distinguish my changes from existing changes
   - show diff truth honestly
   - revert only task-owned changes

9. Review a PR or diff
   - load changed files
   - analyze risk
   - produce concise actionable findings

10. Run long work in background
    - launch task
    - leave it running
    - return later
    - inspect and continue

## Edge-Case Matrix That Must Be Covered

No phase should be considered done without addressing these classes of edge
cases:

- wrong backend selected
- wrong workspace selected
- backend quota or rate limit reached
- backend auth expired
- backend reachable but unhealthy
- tool paused for approval
- tool refused by policy
- user interrupts active turn
- app restarts during active turn
- session resumes after approval pause
- repo already dirty
- non-git directory
- merge conflict appears
- same file changed by user and agent
- binary file involved
- generated file outside tracked diff path
- validation hangs or times out
- MCP server unavailable
- MCP auth expired
- MCP inventory changed between sessions
- long transcript exceeds comfortable context size
- compaction fails
- memory files conflict or are malformed
- terminal too small to show required operator truth

## Product Metrics To Track

Probe should not rely only on taste-based UX decisions. Track:

- time-to-first-visible-progress after submit
- successful completion rate for common edit tasks
- percent of edit tasks followed by explicit verification
- percent of failed tasks where users still understand whether edits landed
- percent of users who discover critical commands without docs
- session abandonment after backend failure
- undo or revert usage after edits
- MCP activation success rate
- frequency of workspace or backend misfires
- compact and clear usage rates

## Immediate Recommendation

If the goal is truly to rival Claude Code CLI and Codex, the next major Probe
program should be:

1. Phase A: reversibility plus conversation-first trust
2. Phase B: project memory and rules
3. Phase D: full MCP runtime
4. Phase E: git and review workflow
5. Phase F: background or delegated work

That order gives Probe the best chance of becoming:

- safe enough to trust
- smart enough to remember
- extensible enough to integrate
- complete enough to finish real coding workflows

## Summary

Probe is already beyond "prototype shell" status.

The remaining distance to best-in-class is not mostly about adding more tool
calls. It is about making the agent easier to operate, easier to trust, easier
to steer, easier to recover, and easier to extend.

If Probe executes the priority order in this document with discipline, it can
become a genuinely elite coding agent product rather than only a strong local
runtime.
