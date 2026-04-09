# Best-In-Class Coding Agent Delivery Roadmap

This document turns `85-best-in-class-coding-agent-gap-plan.md` into a
delivery-oriented roadmap.

The main principle is simple:

- do not ship isolated features
- ship complete, confidence-building user journeys

## Delivery Order

1. Reversibility and review-first editing
2. Memory and rules
3. Operator shell completion
4. MCP runtime
5. Git and review workflow
6. Background work and delegation
7. Final polish, performance, and team features

## Phase 1: Reversibility And Review-First Editing

### User Promise

"Probe can change code safely because I can inspect and undo what it did."

### Must Ship Together

- task checkpoint creation
- task revert
- diff viewer
- apply-after-review path
- clear receipts for partial failure
- conversation vs trace separation

### Do Not Ship Halfway

Do not ship checkpoint creation without a way to discover, inspect, and restore
those checkpoints from the main shell.

## Phase 2: Memory And Rules

### User Promise

"Probe remembers how this project and I like to work."

### Must Ship Together

- user memory
- repo memory
- folder-scoped memory
- `/memory`
- active memory visibility in the shell
- malformed-memory recovery copy

## Phase 3: Operator Shell Completion

### User Promise

"I can operate Probe without memorizing hidden keys or editing config files."

### Must Ship Together

- complete slash command set for daily use
- doctor and status surfaces
- workspace picker
- file context picker
- temporary mode overrides
- clearer status and usage flows

## Phase 4: MCP Runtime

### User Promise

"If Probe says an MCP is enabled, it actually works."

### Must Ship Together

- runtime execution
- live connected state
- tool inventory
- auth state
- per-session toggles
- failure receipts

## Phase 5: Git And Review Workflow

### User Promise

"Probe can help me finish the whole coding loop, not just edit files."

### Must Ship Together

- diff
- stage
- commit
- branch
- revert-task-to-branch-state
- review mode
- PR comment intake

## Phase 6: Background Work And Delegation

### User Promise

"Probe can work on longer tasks without monopolizing the current session."

### Must Ship Together

- background task launch
- task list
- reopen or resume
- delegated worker or subagent foundation
- handoff receipts

## Phase 7: Polish And Team Differentiation

### User Promise

"Probe feels premium, fast, and clearly more usable than ordinary coding TUI
tools."

### Must Ship Together

- latency and redraw polish
- smaller-terminal UX pass
- improved accessibility and keyboard ergonomics
- recipes and reusable workflows
- hosted or team collaboration polish
