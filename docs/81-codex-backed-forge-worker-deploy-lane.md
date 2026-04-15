# Codex-backed Forge Worker Deploy Lane

Probe now ships a first-class checked-in deploy lane for the first private
Forge worker that runs `probe forge run-loop` against Forge using the
`openai-codex-subscription` profile.

This is intentionally a boring operator lane:

- one private Linux VM
- one persistent `PROBE_HOME`
- one systemd service
- one worker workspace root
- one explicit worker attach contract with Forge

It exists to close the first real internal worker loop, not to invent a fleet
manager.

## What this lane owns

The scripts under `scripts/deploy/forge-worker/` cover:

1. provisioning a private GCE VM and persistent data disk
2. building and installing the `probe` binary on that VM
3. installing a systemd-managed `probe forge run-loop` launcher
4. handling the initial worker attach or later reattach
5. running the preferred Codex subscription auth flow on the worker machine
6. restart and log-inspection drills

## Auth contract

This lane supports two different auth concerns. Keep them separate.

### 1. Probe runtime auth to the Codex/OpenAI lane

Preferred first path:

- `openai-codex-subscription`
- auth record stored at `PROBE_HOME/auth/openai-codex.json`
- obtained with `probe codex login --method headless`

This path does **not** require `PROBE_OPENAI_API_KEY`.

Use `PROBE_OPENAI_API_KEY` only when you intentionally deploy an
OpenAI-compatible env-backed profile instead of the Codex subscription lane.

### 2. Probe worker auth to Forge

Forge worker-session state is stored separately at:

- `PROBE_HOME/auth/forge-worker.json`

That file is created by:

- `probe forge attach ... --bootstrap-token ...`

The bootstrap credential is a Forge concern. The persisted worker session is a
Probe runtime concern.

## Shipped scripts

### `01-provision-baseline.sh`

Creates or validates:

- service account
- persistent data disk
- private VM
- IAP SSH firewall rule

Defaults:

- project: `openagentsgemini`
- region / zone: `us-central1` / `us-central1-a`
- VPC / subnet: `oa-lightning` / `oa-lightning-us-central1`
- VM: `probe-forge-worker-1`
- Probe home: `/var/lib/probe-worker`

### `02-configure-and-start.sh`

Uploads the current Probe source tree, builds `probe`, installs:

- `/usr/local/bin/probe`
- `/usr/local/bin/probe-forge-worker`
- `/etc/probe-forge-worker.env`
- `/etc/systemd/system/probe-forge-worker.service`

It will start the service immediately only when the machine already has enough
state to do so:

- `PROBE_FORGE_BASE_URL`
- `PROBE_FORGE_WORKER_ID`
- either an existing `PROBE_HOME/auth/forge-worker.json` or a fresh
  `PROBE_FORGE_BOOTSTRAP_TOKEN`
- for the Codex lane, `PROBE_HOME/auth/openai-codex.json`

If those are not all present yet, it leaves the service installed but stopped
and prints the remaining gap honestly.

### `03-run-headless-codex-login.sh`

Runs the preferred worker-machine auth flow:

```bash
probe codex login --method headless --probe-home /var/lib/probe-worker
```

Use this when the worker VM needs to be authenticated directly instead of
copying a local auth file.

### `04-open-logs.sh`

Tails the worker service logs:

- `journalctl -u probe-forge-worker.service -f`

### `05-refresh-attachment.sh`

Rewrites the environment file with a fresh Forge bootstrap credential, removes
the stale local worker session, and restarts the service so the launcher
reattaches cleanly.

### `06-restart-service.sh`

Restarts the systemd service and prints both service status and current Probe
worker status.

### `99-local-smoke.sh`

Local retained validation for the lane.

It starts a fake Forge server, seeds a fake Codex auth record, runs the
launcher, verifies attach plus claim traffic, and confirms the worker loop
exits cleanly on idle.

## Launcher contract

The installed launcher is `probe-forge-worker.sh`.

On startup it:

1. verifies the `probe` binary exists
2. ensures `PROBE_HOME`, auth directories, and the workspace root exist
3. checks for Codex auth when `PROBE_FORGE_PROFILE=openai-codex-subscription`
4. checks whether a Forge worker session already exists
5. if not, uses `PROBE_FORGE_BASE_URL`, `PROBE_FORGE_WORKER_ID`, and
   `PROBE_FORGE_BOOTSTRAP_TOKEN` to run `probe forge attach`
6. starts `probe forge run-loop` under the configured profile and workspace
   root

This keeps the runtime boundary explicit:

- Forge decides whether the worker may attach
- Probe persists only the local worker-session state
- systemd provides restart behavior

## Environment file contract

The generated environment file is `/etc/probe-forge-worker.env`.

Important fields:

- `PROBE_HOME`
- `PROBE_FORGE_BASE_URL`
- `PROBE_FORGE_WORKER_ID`
- `PROBE_FORGE_BOOTSTRAP_TOKEN`
- `PROBE_FORGE_PROFILE`
- `PROBE_FORGE_WORKSPACE_ROOT`
- `PROBE_FORGE_POLL_INTERVAL_MS`
- `PROBE_FORGE_EXIT_ON_IDLE`
- `PROBE_FORGE_MAX_ITERATIONS`

Optional fields for non-Codex lanes:

- `PROBE_OPENAI_API_KEY`
- `PROBE_FORGE_SERVER_HOST`
- `PROBE_FORGE_SERVER_PORT`
- `PROBE_FORGE_SERVER_MODEL_ID`
- `PROBE_FORGE_SERVER_MODEL_PATH`

## Expected operator flow

### Initial machine bootstrap

1. Run `scripts/deploy/forge-worker/01-provision-baseline.sh`
2. Export:
   - `PROBE_FORGE_BASE_URL`
   - `PROBE_FORGE_WORKER_ID`
   - `PROBE_FORGE_BOOTSTRAP_TOKEN`
3. Run `scripts/deploy/forge-worker/02-configure-and-start.sh`
4. If Codex auth is not already present, run:
   - `scripts/deploy/forge-worker/03-run-headless-codex-login.sh`
5. Restart or refresh the worker if needed:
   - `scripts/deploy/forge-worker/06-restart-service.sh`

### Later reattachment after bootstrap rotation

1. Mint a fresh Forge bootstrap credential
2. Export `PROBE_FORGE_BOOTSTRAP_TOKEN`
3. Run `scripts/deploy/forge-worker/05-refresh-attachment.sh`

### Normal inspection

- logs: `scripts/deploy/forge-worker/04-open-logs.sh`
- service restart: `scripts/deploy/forge-worker/06-restart-service.sh`

## Workspace posture

The default worker workspace root is:

- `/var/lib/probe-worker/workspaces/default`

That is only the worker-side execution root. Forge remains authoritative for:

- Work Orders
- Runs
- Workspaces
- leases
- evidence
- verification
- delivery

Probe uses the workspace root only as the runtime execution directory for an
assigned run.

## Validation

Retained validation for this lane now includes:

```bash
cargo test -p probe-cli --test forge_cli
bash scripts/deploy/forge-worker/99-local-smoke.sh
```

The second command is the same launcher path the systemd service uses, but
exercised locally against a fake Forge server.
