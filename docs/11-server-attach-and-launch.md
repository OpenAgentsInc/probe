# Server Attach And Launch

## Purpose

Probe now owns the local operator seam for server preparation.

That means:

- loading a local server config
- attaching to an already-running local server
- optionally launching `psionic-openai-server` as a supervised child process

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
4. waits for `GET <base_url>/models` to succeed
5. runs the requested Probe command against that server

## Launch Mode

In `launch` mode, Probe:

1. requires a server binary path
2. requires a model path
3. spawns `psionic-openai-server` with the configured host, port, backend, and
   optional reasoning budget
4. waits for the server to become ready
5. keeps the child process alive for the lifetime of the Probe command
6. terminates the child on drop

## Current Boundary

The launcher is intentionally narrow:

- one child-process supervision path
- one health-check rule
- no embedded serving logic
- no attempt to replace Psionic startup semantics

This is the correct boundary for the current Probe stage.
