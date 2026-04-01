# Probe Background Agent Roadmap

## Purpose

This document translates Ramp Builders' January 12, 2026 article
["Why We Built Our Own Background Agent"](https://builders.ramp.com/post/why-we-built-our-background-agent)
into a Probe-specific plan.

It also builds on the existing Probe direction in:

- `../README.md`
- `02-runtime-ownership-and-boundaries.md`
- `04-session-turn-item-and-transcript-model.md`
- `11-server-attach-and-launch.md`
- `43-probe-runtime-event-stream-and-live-tui-lifecycle.md`
- workspace docs:
  - `/Users/christopherdavid/work/docs/probe/01-repo-recommendation-and-roadmap.md`
  - `/Users/christopherdavid/work/docs/probe/02-opencode-architecture-and-lessons.md`
  - `/Users/christopherdavid/work/docs/probe/03-autopilot-product-surface-and-probe-integration.md`
  - `/Users/christopherdavid/work/docs/probe/04-openai-codex-architecture-and-lessons.md`

This is not a plan to copy Ramp's full product stack.

It is a narrower plan for how Probe should become an honest background coding
agent using Probe's own runtime boundary, with:

- `probe` CLI and TUI as first-party operator clients
- Autopilot in `openagents` as the rich product host
- no hosted VS Code requirement
- no browser extension requirement
- no fake "Probe is a SaaS console" detour

## Bottom Line

Probe already has several ingredients a background agent needs:

- durable sessions
- append-only transcripts
- resume
- tool execution
- approval persistence
- runtime event streaming
- a real TUI
- remote model attach for Psionic inference

But current Probe is still a local-first controller.

Today the main background shape is:

- local Probe owns the session, tools, approvals, and UI
- remote Psionic can serve inference

That is useful, but it is not yet a real background agent.

Probe becomes a background agent only when the full coding session can run away
from the user's laptop in a prepared isolated workspace, while the operator
attaches later from:

- `probe chat`
- `probe tui`
- Autopilot

The key architectural shift is:

- move from "remote inference"
- to "remote or detached Probe session workers"

## What Probe Should Borrow

The Ramp article is directionally right about the core properties.

Probe should borrow these ideas directly:

### 1. Server-first runtime

The runtime should be the product center.

That matches the existing Probe direction from the `opencode` and Codex audits:

- one runtime
- one typed control plane
- multiple clients on top

For Probe, that means shipping `probe-server` as the canonical runtime
supervision seam instead of letting the CLI, TUI, and Autopilot each grow
their own runtime logic.

### 2. Prepared per-session execution environments

A background agent needs its own working environment, not just a remote model.

Each Probe session should run in:

- an isolated workspace checkout or worktree
- a prepared development environment
- a git branch tied to that session

The important Ramp lesson is not "use Modal specifically."

The important lesson is:

- pay setup costs before the first token when possible
- keep session startup fast
- let users start many concurrent sessions without tying up their laptop

### 3. Fast resume and session handoff

Probe sessions should feel durable and resumable across clients.

The same session should be attachable from:

- CLI
- TUI
- Autopilot

This is already directionally consistent with Probe's append-only transcript
model and its local resume flow. Background mode widens that into a detached
session model instead of a terminal-bound process model.

### 4. Queueing and interruption

Ramp explicitly calls out two operator behaviors that matter:

- queue follow-up prompts while work is still running
- stop a session mid-flight

Probe needs both.

Detached coding sessions are not usable if follow-ups only work when the
current turn is idle, or if the only way to stop a runaway task is killing the
whole worker process.

### 5. Session-spawn tools

Ramp is right that a strong background agent should be able to spawn another
session for bounded research or decomposition work.

Probe should eventually expose a typed session-spawn tool with a matching
status-read tool so one session can:

- spin up a sibling research session
- fan out across repos or subsystems
- poll status
- merge the findings back into the parent task

### 6. Verification, not just code generation

The strongest part of the article is the closure loop:

- run tests
- inspect app behavior
- produce screenshots or previews where relevant
- finish with enough evidence to justify the patch

Probe should copy that principle.

It should not copy Ramp's exact tool list into core.

The right Probe version is:

- Probe core owns the runtime, tools, approvals, and transcript truth
- host environments inject extra verification tools
- Autopilot can provide richer product-specific tools when available

## What Probe Should Not Copy

Probe should stay disciplined and avoid product sprawl.

Do not copy these parts of Ramp's implementation as first-class Probe goals:

- hosted VS Code editing
- browser-extension workflows
- Slack-first product behavior
- generic hosted console or billing surfaces
- Probe-core ownership of product-specific telemetry or business tools

Those would blur repo boundaries and slow down the real runtime work.

The clean split remains:

- Probe owns coding-runtime truth
- `openagents` owns Autopilot product truth
- `psionic` owns inference or execution substrate truth

## Current Gap

Probe's current docs and codebase already define the right runtime boundary,
but several background-agent requirements are still missing.

## 1. No shipped `probe-server` control plane yet

The workspace docs already recommend a first-class `probe-server`.

That server does not yet exist as the canonical path for:

- session creation
- turn submission
- attach or detach
- status inspection
- approval resolution
- interrupt or cancel
- task spawn

Without that server seam, Probe is still mostly a foreground local app with
durable files, not a true background runtime.

## 2. Remote inference is not remote execution

The Tailnet Qwen docs are clear about the current first remote boundary:

- local Probe still owns session lifecycle
- local Probe still owns tools and approvals
- only inference crosses the network

That is the right current boundary, but it is not the end state.

For a real background agent, the detached Probe worker must own:

- the workspace checkout
- shell commands
- file edits
- test runs
- git operations
- runtime lifecycle

Otherwise the user's machine is still the actual execution host.

## 3. No prepared workspace pool or snapshot story yet

The current Probe server-attach docs cover backend readiness, not coding
workspace readiness.

Probe still needs a real prepared-environment story for:

- cloning or updating repos
- dependency installation
- cache warming
- reusing prepared baselines across sessions
- restoring or resuming detached workspaces

## 4. No detached session queue model yet

Current Probe sessions persist well, but the docs still center mostly on:

- one foreground interactive session
- one TUI worker
- one local command execution flow

Background mode needs a broader session-state machine with explicit:

- queued prompts
- running prompts
- pending approvals
- paused or blocked state
- completed or failed tasks
- cancellation and timeout state

## 5. No branch or PR closure lane yet

A background agent should not stop at "files changed."

Probe needs a typed branch-and-delivery lane that can:

- create or reuse a session branch
- attribute commits to the user
- push changes
- create a PR or merge request through an injected forge adapter
- ingest CI and PR status back into the session

## Required Probe Shape

Probe should become a server-first runtime with detached session workers.

## 1. `probe-server` becomes the canonical runtime surface

`probe-server` should own:

- session start, resume, list, inspect
- turn submit, steer, queue, interrupt, cancel
- live event streaming
- approval queries and resolution
- task and sub-session lifecycle
- artifact and transcript references
- git and delivery state

The first transport can stay simple:

- stdio for local child-process supervision from Autopilot
- a local socket or HTTP plus SSE lane once detached local daemon mode lands
- remote attach only after the local server contract is stable

Probe should also ship a shared client crate so:

- CLI
- TUI
- Autopilot

all speak the same typed contract.

## 2. Detached session workers

Each background Probe session should map to one detached worker with:

- a session id
- a workspace or repo identity
- a checked-out branch
- a prepared execution root
- a runtime state machine
- append-only transcript and event truth

The session model should stay compatible with the existing append-only storage
direction:

- transcript and event log stay append-only
- metadata and indexes remain queryable snapshots

Probe can add SQLite indexes or session-local databases for performance, but it
should not abandon append-only runtime truth.

## 3. Prepared execution environments

Probe needs a real workspace-prep layer.

The first version does not need full VM orchestration.

A pragmatic first implementation is enough:

- dedicated worker machines reachable over Tailnet or local IPC
- one clean checkout or worktree per session
- repo-specific prepared baselines
- dependency and build caches warmed ahead of time
- snapshot or clone reuse for fast startup

Only after that is working should Probe decide whether it actually needs:

- VM snapshots
- container snapshots
- hosted sandbox providers

The important product property is fast isolated session startup, not a specific
vendor.

## 4. Workspace sync and write gating

One of the best tactical ideas in the Ramp article is:

- let the agent start reading before sync is complete
- block writes until the workspace is fully synced

Probe should implement the same idea through its own tool policy layer.

That means:

- a workspace prep state is visible to the runtime
- read-only tools can run during final sync
- edit and write tools are refused or delayed until the checkout is ready

This fits Probe's existing approval and structured tool-result posture better
than hiding sync state inside shell scripts.

## 5. Background-session control behavior

Detached sessions need a real operator contract.

Probe should support:

- follow-up prompts queued while a task is running
- interrupt and cancel
- status inspection without attach
- reattach from another client later
- approval handling while detached
- explicit failure and timeout surfaces

The operator should never have to guess whether a session is:

- still running
- blocked on approval
- waiting on setup
- finished
- dead

## 6. Verification-first tool surface

Probe core should definitely ship:

- file read and search
- patch and write
- shell
- test and lint wrappers
- git status and diff

Probe should also support injected verification tools through:

- MCP
- typed connectors
- host-provided dynamic tools

That lets Autopilot or other hosts provide richer tools such as:

- preview launch
- screenshot capture
- telemetry queries
- feature-flag checks

without forcing Probe core to absorb product-specific integrations.

## 7. Delivery loop and forge integration

A background Probe session should be able to finish with a deliverable branch.

That means Probe needs session-visible delivery objects such as:

- current branch
- commit set
- push status
- PR or MR id
- CI state
- merge state

The actual forge adapter can stay narrow and injected.

The important Probe-owned truth is:

- the session knows what branch it owns
- the transcript records what was delivered
- clients can monitor delivery without scraping git output manually

## 8. Minimal multi-client truth

Ramp emphasizes multiplayer. Probe does not need full multiplayer as the first
goal.

But it does need a minimal version of shared session truth:

- a session can be attached from more than one client over time
- prompts and approvals carry author identity
- clients see the same transcript and runtime state

That is enough for the first honest Probe version spanning:

- CLI
- TUI
- Autopilot

## Client Strategy

Probe should keep the first client set intentionally small.

Primary clients:

- `probe chat`
- `probe tui`
- Autopilot in `openagents`

That is enough.

Do not block the background-agent roadmap on:

- a browser extension
- a hosted web IDE
- a general-purpose web dashboard
- a Slack bot

Those can exist later if they become necessary. They are not required to prove
the runtime.

## Ownership Split

## Probe owns

- detached session runtime
- task queue and sub-session orchestration
- tool execution and approval policy
- transcript, event, and artifact truth
- workspace prep state
- git branch and delivery state
- first-party CLI and TUI runtime clients
- the server protocol used by Autopilot

## `openagents` owns

- the Autopilot shell
- operator presentation
- pane and thread UX
- product-specific tools and product truth
- richer screenshot or preview surfaces when those belong to the product shell

## `psionic` owns

- inference serving
- local or remote model execution substrate
- backend runtime performance and serving behavior

## Phased Roadmap

## Phase 1: Serverize the current runtime

Ship:

- `probe-server`
- a shared Probe client crate
- typed session and turn APIs
- typed live event streaming
- attach, list, inspect, interrupt, and approval APIs

Success condition:

- CLI, TUI, and Autopilot can all talk to the same Probe runtime contract.

## Phase 2: Detached local daemon mode

Ship:

- a long-lived local Probe daemon
- detached session execution away from the foreground terminal
- `probe ps`, `probe attach`, `probe logs`, `probe stop`
- queued follow-up prompts

Success condition:

- a user can start a Probe session, close the client, and return later without
  losing runtime state.

## Phase 3: Remote worker mode

Ship:

- Probe workers on dedicated machines
- remote session attach over an explicit Probe server protocol
- per-session isolated checkouts
- prepared workspace baselines
- workspace sync state and write gating

Success condition:

- the user's laptop is no longer the execution host for background coding
  sessions.

## Phase 4: Delivery and verification closure

Ship:

- branch ownership per session
- commit and push flows
- forge adapter for PR creation
- delivery state in the session model
- richer verification artifacts such as test receipts and screenshots where
  available

Success condition:

- a detached Probe session can end in a reviewable branch or PR with evidence.

## Phase 5: Session fan-out and deeper handoff

Ship:

- session-spawn tools
- child-session status tools
- authorship-aware prompt records
- minimal multi-client collaboration semantics

Success condition:

- Probe can decompose work across sessions and survive real handoff between
  operator surfaces.

## Practical First Implementation

Probe should resist overengineering the first background lane.

The first honest version can be much simpler than Ramp's production stack:

- one `probe-server`
- one worker process per detached session
- local disk persistence
- Tailnet-reachable worker machines
- prepared repo baselines refreshed periodically
- CLI, TUI, and Autopilot as the only supported clients

That is enough to prove the real transition:

- from local coding helper
- to durable detached coding runtime

## Success Metrics

Probe should measure the background-agent lane with product-real metrics:

- session startup latency
- time to first token
- time to first tool execution
- resumed-session success rate
- branch or PR creation rate
- PR merge rate
- percent of sessions closed by verification evidence rather than raw diff only

The important metric is not "messages sent."

The important metric is:

- how often a Probe session produces a merged or adopted outcome

## Final Recommendation

Probe should become its own version of a background agent by deepening the
runtime it already has, not by copying Ramp's outer product shell.

The correct Probe move is:

- build `probe-server`
- make detached session workers first-class
- run those workers in prepared isolated workspaces
- keep Probe CLI, Probe TUI, and Autopilot attached to the same session truth
- let host environments inject richer verification tools where appropriate

That gives Probe the useful part of the background-agent model while keeping
the repo boundary honest and the product scope under control.
