# Hosted Prepared Environments And Sync Gates

Issue `#127` adds the first explicit prepared-environment and default-branch
sync contract for hosted Probe workers.

## Goal

Hosted Probe should be able to start from an engineer-ready checkout without
giving the agent unsafe write authority before that checkout is known to match
the intended default-branch state.

The target behavior is:

- prepared environments are named and attached to session metadata
- default-branch sync state is explicit in `workspace_state`
- read-only research can start while sync is still in progress
- edits, patches, shell writes, network calls, and destructive tools are
  blocked until sync is complete
- follow-up runs can resume from snapshot metadata instead of pretending every
  session is a full fresh clone

## Prepared Environment Metadata

`SessionWorkspaceState` now has an optional `prepared_environment` ref:

- `environment_id`
- `repo_slug`
- optional `image_ref`
- optional `cache_ref`
- optional `dependency_cache_key`
- optional `prepared_at_ms`
- `warm_commands`

The first checked-in catalog lives at:

```text
scripts/deploy/forge-hosted/prepared-environments.catalog.json
```

It defines the initial hosted Probe repo set:

- `OpenAgentsInc/openagents`
- `OpenAgentsInc/openagents.com`
- `OpenAgentsInc/forge`
- `OpenAgentsInc/probe`
- `AtlantisPleb/workspace`

The catalog is intentionally provider-neutral. A GCP VM, image builder,
Cloud Build cache, or future snapshot system can all publish the same typed
session metadata.

## Sync Metadata

`SessionWorkspaceState` now also has an optional `sync` state:

- `status`
  - `unknown`
  - `syncing`
  - `complete`
  - `failed`
- optional `default_branch`
- optional `requested_ref`
- optional `synced_ref`
- optional `started_at_ms`
- optional `completed_at_ms`
- optional `message`

Hosted baseline manifests may include `prepared_environment` and `sync`.
When `probe-server` resolves a requested prepared baseline, it copies those
manifest fields into the stored session workspace state unless the caller
already provided more specific values.

## Write Gate

Probe enforces the sync contract at tool execution time.

When a session has `workspace_state.sync.status != complete`, Probe installs a
tool write gate. The gate allows:

- read-only tools
- read-only shell commands

The gate refuses:

- file edits and `apply_patch`
- write-class shell commands
- network-class shell commands
- destructive shell commands
- any other non-read-only tool risk class

The refusal is visible in the normal tool result as:

```text
workspace sync gate blocked non-read-only tool execution: ...
```

This means the hosted agent can inspect files and form a plan immediately, but
cannot mutate the workspace until the default-branch sync state is trustworthy.

## GCP Forge Dogfood Lane

The current GCP dogfood script:

```text
scripts/deploy/forge-hosted/03-prepare-openagents-workspace.sh
```

refreshes the managed `openagents` checkout and writes a hosted baseline
manifest that includes:

- prepared environment id
- repo slug
- cache root
- warm command list
- completed sync state
- resolved commit SHA

That is still a narrow single-worker lane. It does not claim Modal-style
snapshot orchestration or a fleet image builder. The important contract is that
any future hosted image or cache publisher must emit the same metadata and must
not mark sync `complete` until writes are safe.

## Snapshot And Follow-Up Contract

Probe already has typed `SessionWorkspaceSnapshotRef` support in
`workspace_state.snapshot`.

The intended follow-up flow is:

1. start from a prepared environment or prior snapshot
2. attach sync metadata to the session before agent work begins
3. keep writes blocked until sync is complete
4. after useful work, publish a snapshot manifest that references the source
   baseline or prior snapshot
5. resume follow-up prompts from that snapshot rather than forcing a new clone

The snapshot storage backend remains provider-owned. Probe's responsibility is
the typed protocol state and the local write-safety gate.

## Verification

The retained local tests cover:

- hosted baseline manifests projecting prepared-environment metadata
- hosted baseline manifests projecting sync metadata
- read-only tools running while the sync gate is active
- write tools being refused while the sync gate is active, even when normal
  write approval would allow them

The recommended local verification lane for this issue is:

```shell
cargo test -p probe-core workspace_sync_gate -- --nocapture
cargo test -p probe-core forge_worker_verification -- --nocapture
cargo run -p probe-cli -- forge verification-pack --pretty
cargo test -p probe-client client_can_connect_to_hosted_tcp_transport_and_inspect_runtime_owner -- --nocapture
cargo check --workspace
```
