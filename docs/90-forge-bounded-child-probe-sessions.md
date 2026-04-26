# Forge Bounded Child Probe Sessions

Issue `#130` adds a Forge-scoped child-session controller for parent Probe
Runs.

## Goal

Parent Probe Runs need a safe way to delegate bounded research or explicitly
authorized patch attempts to child Probe sessions. The parent must be able to
read child status and artifacts for synthesis, while Forge policy remains the
authority for budget, repository scope, patch authority, and production
recovery authority.

## Core API

The controller lives in:

```text
probe_core::forge_child_sessions
```

The main entrypoints are:

- `ForgeChildSessionController::spawn_child_session`
- `ForgeChildSessionController::read_child_status`
- `ForgeChildSessionController::parent_synthesis_artifact`

The policy type is `ForgeChildSessionPolicy`. It controls:

- maximum child sessions per parent
- maximum prompt bytes
- maximum child timeout seconds
- allowed repository slugs
- whether patch attempts are allowed
- whether production recovery actions are allowed

Production recovery is denied by default and should remain denied for the
health-agent path. Recovery action execution belongs behind Forge leases and
the deterministic health worker policy.

## Default Behavior

Child sessions default to `research` mode. Research children are read-only by
contract and receive a system prompt that explicitly forbids direct production
recovery actions.

Patch attempts require `allow_patch_attempts=true` in the Forge policy.

Production recovery attempts require `allow_production_recovery_actions=true`.
The health-agent program should not set that flag for Probe children; Probe
should diagnose, inspect, patch code/docs when authorized, and return evidence.

## Artifacts

Each spawned child receives:

- a normal Probe session
- an explicit `SessionParentLink`
- an initial user-message transcript item containing the bounded task
- a transcript artifact
- refreshed retained-session and accepted-patch summary artifacts when
  material is present

`parent_synthesis_artifact` returns all linked child status reports and their
artifact refs so the parent Run can attach them to Forge Evidence.

## Verification

Use:

```shell
cargo test -p probe-core forge_child_session -- --nocapture
cargo check --workspace
./probe-dev pr-fast
```

The retained tests cover:

- child spawn/read
- read-only default behavior
- budget and repository enforcement
- patch-attempt permission enforcement
- production recovery rejection
- parent synthesis artifacts
