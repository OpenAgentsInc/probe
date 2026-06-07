# Probe

Probe is being reset.

This repository used to contain a Rust implementation of Probe. That tree is now
deprecated and removed from `main`. The old implementation remains available in
Git history as source material for protocol ideas, command ergonomics, tests,
runtime failure cases, and deployment lessons, but it is not the new runtime
contract and should not be revived as a compatibility layer by default.

The intended product surface is still `probe`: the first-party OpenAgents coding
agent runtime. It should be one coherent surface for coding agents that can run
with API-key model providers, local inference, swarm inference, Codex-style
hosted execution, and future OpenAgents inference routes without exposing those
implementation choices as product names. Probe should be opinionated around the
OpenAgents stack: Omega dispatch, OpenAgents Sync events, SHC boxes, Pylons,
forum-thread workrooms, artifact receipts, and explicit product acceptance.

OpenCode is an important inspiration point, especially for the shape of a
modern coding-agent runtime: durable sessions, a clean tool registry, permission
flow, local control plane, and a focused CLI. Probe will not wrap, fork, or
vendor OpenCode as its base. The goal is a first-party runtime that learns from
OpenCode while fitting OpenAgents infrastructure more tightly than a generic
coding agent can.

Pylons should eventually be able to spawn Probes as local or remote work
executors. A Pylon-hosted Probe may combine local machine capability, swarm
compute, and API-backed model calls behind one assignment contract. The user or
OpenAgents product surface should ask for a coding outcome, not choose between a
pile of separate agent products.

The same runtime should also be deployable into sandboxes from
`openagents.com`. Forum threads, workroom requests, and operator actions should
be able to create bounded Probe assignments that run in SHC boxes or other
approved sandboxes, emit redacted events back to Omega, produce artifacts, and
wait for explicit acceptance before any product or writeback authority is
treated as complete.

## Current Branch State

`main` is intentionally clean while the refactor begins.

Tracked files should stay minimal until the new architecture lands:

- `README.md` explains the reset and target direction.
- `package.json` defines a Bun workspace/catalog layout modeled after the
  OpenCode workspace style while pinning the Effect line used by Omega.
- `packages/runtime/src/contracts/provider-account.ts` defines the first
  Probe/Omega ChatGPT/Codex account contract.
- `packages/runtime/src/contracts/assignment.ts` defines the Probe run
  assignment contract for Omega refs and grants.
- `packages/runtime/src/omega/grant-client.ts` resolves Omega auth grants into
  Probe materialization plans.
- `packages/runtime/src/auth/materializer.ts` materializes brokered auth only
  inside per-run env/file targets and scrubs it on closeout.
- `packages/runtime/src/runner/identity.ts` gates Omega grant use on linked
  Probe runner identity before resolving and materializing auth.
- `packages/runtime/src/runtime/backend-assignment.ts` routes no-auth Apple FM
  assignments through live backend health and emits backend run events.
- `packages/runtime/src/cli.ts` exposes the first Probe CLI commands for Omega
  linking and account management.
- `packages/runtime/src/fleet/telemetry.ts` reports auth/account health signals
  and requests Omega failover without locally iterating raw account tokens.
- `packages/runtime/src/backends/registry.ts` registers the first Apple FM
  backend profile.
- `packages/runtime/src/backends/apple-fm/contract.ts` defines the Effect v4
  Apple FM backend contract.
- `packages/runtime/src/backends/apple-fm/client.ts` attaches to Apple FM
  bridge health and preserves typed availability.
- `packages/runtime/src/backends/apple-fm/receipts.ts` defines redacted Apple
  FM availability, failure, and transcript receipts.
- `docs/probe-omega-auth-contract.md` records the implemented account-contract
  slice.
- `docs/probe-omega-run-assignment.md` records the implemented assignment and
  grant-resolution slice.
- `docs/probe-auth-materialization.md` records the implemented per-run auth
  materialization slice.
- `docs/probe-runner-identity.md` records the SHC/Pylon/sandbox runner
  identity slice.
- `docs/probe-cli-omega-auth.md` records the CLI account-management slice.
- `docs/probe-fleet-telemetry.md` records the fleet telemetry and failover
  slice.
- `docs/2026-06-07-apple-fm-first-backend-audit.md` audits the previous Apple
  FM implementations and records the plan for making Apple FM the first
  supported backend in the new Probe runtime.
- `docs/probe-apple-fm-backend.md` records the implemented Apple FM backend
  contract slice.
- `.gitignore` keeps local build/cache noise out of the repo.
- Add a license file before publishing or distributing a new runtime artifact.

Do not add compatibility scaffolding only because the old repo had it. Search
for real callers first. If a deprecated command, protocol, adapter, or runtime
mode has no current caller and no clear final-product role, harvest its useful
idea into tests or notes and leave the surface deleted.

## Direction

The next Probe should start from the final shape:

- `probe` as the only runtime/product name.
- A typed assignment and event protocol shared deliberately with Omega.
- A durable session and event log that can resume, replay, and close out runs.
- A single tool and approval model across local, swarm, API-key, and Codex-style
  inference routes.
- SHC-first deployment, with later Pylon and other sandbox hosts using the same
  assignment contract.
- Redacted artifacts and events suitable for OpenAgents Sync and
  `openagents.com` workrooms.
- Explicit separation between runtime completion and product acceptance.

The refactor should optimize for the code that should exist, not the smallest
diff from the deprecated implementation.
