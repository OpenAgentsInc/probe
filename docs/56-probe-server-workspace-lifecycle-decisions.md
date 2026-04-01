# Probe Server Workspace Lifecycle Decisions

Issue `#68` extracts the first pure workspace lifecycle state machine for the
serverized Probe path.

## Why

The background-agent roadmap calls for startup, retry, timeout, and
circuit-breaker policy to live in one pure decision layer instead of being
repeated ad hoc across CLI flags and future server worker code.

Probe does not have prepared baseline restore or remote worker orchestration
yet.

It does now have an explicit decision boundary that can answer:

- reuse a ready workspace worker
- wait for an in-flight startup to finish
- start fresh
- prefer a prepared-baseline restore when one exists later
- reopen work after a circuit-breaker cooldown
- deny a new start while the circuit breaker is still open

## Crate Boundary

The first implementation lives in `crates/probe-server/src/workspace_lifecycle.rs`.

That module is intentionally side-effect free.

It owns:

- lifecycle state enums
- boot-mode choice
- restart versus wait decisions
- timeout handling
- repeated-failure accounting
- circuit-breaker opening and retry timing

It does not own:

- process spawning
- server I/O
- file-system checkout preparation
- remote worker orchestration

Those side effects belong in the server manager that consumes the decisions.

## Core Types

The current decision surface is built around:

- `WorkspaceLifecycleState`
  - `Idle`
  - `Starting`
  - `Ready`
  - `Failed`
  - `CircuitOpen`
- `WorkspaceLifecycleRequest`
  - current time
  - prepared-baseline availability
  - start preference
  - timeout and circuit-breaker policy
- `WorkspaceLifecycleDecision`
  - `ReuseReady`
  - `WaitForStarting`
  - `StartWorkspace`
  - `DenyUntilCircuitCloses`
- `WorkspaceLifecyclePlan`
  - one explicit decision plus the next state to persist

## Boot Modes

The state machine already leaves room for the later Phase 3 restore path:

- `Fresh`
- `RestorePreparedBaseline`

Today Probe still starts fresh in practice unless a future manager reports a
prepared baseline as available.

That is deliberate.

The point of this patch is to freeze the policy seam early, not to pretend the
prepared-baseline implementation already exists.

## Registry Helper

`WorkspaceLifecycleRegistry` is a thin in-memory consumer of the pure
decisions.

It exists so the later `probe-server` request handler can ask for a plan,
persist the returned state, and then perform the side effect the plan
requested.

That keeps the future server code honest:

- decisions stay pure and testable
- the server manager becomes an executor of explicit plans
- timeout and repeated-failure policy stays centralized

## Current Limits

The first state machine does not yet model:

- detached daemon workers
- prepared-baseline materialization
- remote worker leases
- delivery-state transitions
- branch or PR finalization

Those stay follow-on work.

The important Phase 1 move is already in place:

- Probe now has a pure tested lifecycle policy layer that a serverized runtime
  can consume directly.
