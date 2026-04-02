# Server Attach And Launch

## Purpose

Probe now owns the local operator seam for server preparation.

That means:

- loading a local server config
- attaching to an already-running local server
- discovering routed inventory from a Psionic mesh control plane when the
  selected backend profile targets that lane
- optionally launching `psionic-openai-server` as a supervised child process
- distinguishing the current Psionic OpenAI-compatible lane from the Apple FM bridge attach lane

It does not mean Probe owns serving semantics.

The server binary, model loading, and backend execution still belong to
Psionic.

## Local Config Path

By default, Probe stores the local server config at:

```text
<probe_home>/server/psionic-local.json
```

The config records:

- mode
- api kind
- optional control-plane kind
- host
- port
- backend
- optional binary path
- optional model path
- optional model id
- optional reasoning budget

## CLI Flags

`probe exec`, `probe chat`, and `probe accept` now accept:

- `--server-mode <attach|launch>`
- `--server-config <path>`
- `--server-binary <path>`
- `--server-model-path <path>`
- `--server-model-id <id>`
- `--server-host <host>`
- `--server-port <port>`
- `--server-backend <cpu|cuda>`
- `--server-reasoning-budget <n>`

## Attach Mode

`attach` is the default mode.

In this mode, Probe:

1. loads the local server config
2. applies any CLI overrides
3. writes the effective config back to disk
4. waits for the configured backend kind to become ready
5. runs the requested Probe command against that server

Current readiness rules:

- `open_ai_chat_completions`
  - direct attach waits for `GET <base_url>/models`
  - the `psionic-inference-mesh` profile instead reads
    `GET <management_base_url>/psionic/management/status`
  - resolves the effective model from the live routed inventory
  - preserves targetable models, local mesh role, local posture, and proxied
    fallback truth in typed session metadata
- `apple_fm_bridge`
  - checks `GET <base_url>/health`
  - refuses early if the bridge reports the model unavailable and preserves the
    typed unavailability reason in the operator error

Current mesh attach output includes:

- `mesh_control_plane`
  - management base URL, topology digest, and default routed model
- `mesh_posture`
  - local worker identity, served-mesh role, posture, reasons, execution mode,
    and fallback posture
- `mesh_model`
  - one line per currently targetable warm model with endpoint and capability
    truth

## Launch Mode

In `launch` mode, Probe:

1. requires a server binary path
2. requires a model path
3. spawns `psionic-openai-server` with the configured host, port, backend, and
   optional reasoning budget
4. waits for the server to become ready
5. keeps the child process alive for the lifetime of the Probe command
6. terminates the child on drop

Current boundary:

- managed launch is only implemented for the OpenAI-compatible Psionic lane
- the mesh-backed OpenAI profile is attach-only
- Apple FM is attach-only for now
- that is intentional because the Apple FM bridge does not share the same
  `psionic-openai-server` launch contract
- that is also intentional because Probe must not claim ownership of Psionic
  mesh bootstrap, join, warmup, or rebalance semantics

## Current Boundary

The launcher is intentionally narrow:

- one child-process supervision path
- one direct attach readiness rule plus one mesh-management discovery rule
- no embedded serving logic
- no attempt to replace Psionic startup semantics

This is the correct boundary for the current Probe stage.
