# Hosted GCP Forge Dogfood Deploy Lane

Probe now ships a boring first hosted deploy lane for Forge dogfood on our own
GCP footprint.

The goal is not a generalized hosted control plane yet. The goal is to stand up
one honest worker that can run `probe-server --listen-tcp ...` behind IAP,
attach a durable `PROBE_HOME`, prepare a managed `openagents` checkout, and
give `Autopilot` a real hosted Probe session to project.

## Why this lane exists

The local Forge object layer is already real:

- shared sessions
- queued turns
- knowledge pack routing
- evidence bundles
- delivery receipts
- bounties and settlements
- campaigns and promotion ledgers
- hosted session receipts and remote projection

What was still missing was an owned, repeatable way to prove that stack against
real GCP compute instead of more local simulations.

## Deployment posture

Keep it simple:

- one private GCE VM in `us-central1-a`
- one attached persistent disk mounted at `PROBE_HOME`
- one systemd `probe-hosted.service`
- one hosted TCP listener bound to `127.0.0.1:7777`
- one longer hosted stall budget than the local 30s detached default, because
  remote Codex-backed turns can go quiet for longer without being wedged
- one managed `openagents` checkout under
  `PROBE_HOME/hosted/workspaces/<baseline-id>/openagents`
- one internal IAP attach path that each teammate can open independently into
  that listener

This follows the same boring posture already used elsewhere in the workspace:

- private compute first
- IAP SSH for operator access
- explicit persistent storage
- no fake serverless/decentralized story

## Shipped scripts

The new scripts live under `scripts/deploy/forge-hosted/`.

1. `01-provision-baseline.sh`
   - creates the service account, persistent disk, private VM, and IAP SSH
     firewall rule
2. `02-configure-and-start.sh`
   - copies local Probe auth into the remote `PROBE_HOME`
   - uploads the current Probe source tree
   - builds `probe-server --release` on the VM
   - installs and starts `probe-hosted.service`
3. `03-prepare-openagents-workspace.sh`
   - clones or refreshes a managed `openagents` checkout
   - writes the prepared-baseline manifest that Probe uses for honest startup
     projection
4. `04-open-tunnel.sh`
   - opens a manual local IAP tunnel when the operator wants the old explicit
     tunnel path

## Default environment

The scripts default to:

- project: `openagentsgemini`
- region / zone: `us-central1` / `us-central1-a`
- VPC / subnet: `oa-lightning` / `oa-lightning-us-central1`
- VM: `probe-hosted-forge-1`
- data disk: `probe-hosted-forge-data`
- `PROBE_HOME`: `/var/lib/probe-hosted`
- hosted listener: `127.0.0.1:7777`
- local tunnel: `127.0.0.1:17777`
- prepared baseline id: `forge-openagents-main`

Override them with environment variables if a future run needs a different
worker or baseline.

## Expected operator flow

1. `scripts/deploy/forge-hosted/01-provision-baseline.sh`
2. `scripts/deploy/forge-hosted/02-configure-and-start.sh`
3. `scripts/deploy/forge-hosted/03-prepare-openagents-workspace.sh`
4. Either:
   - run `scripts/deploy/forge-hosted/04-open-tunnel.sh` and point clients at
     `127.0.0.1:17777`, or
   - use Probe's internal GCP IAP attach transport so each teammate opens
     their own tunnel directly from `probe-client` or the Probe CLI
5. Run the hosted OpenAgents harness against the chosen attach path

That harness is where the full Forge proof happens:

- preflight
- hosted session start
- mounted knowledge packs
- patch + read-back turn proof
- hosted receipt projection
- evidence and delivery closure
- bounty, settlement, campaign, and promotion bookkeeping
- exported hosted audit bundles

## First Verified Deployment

The first live hosted Forge proof on April 2, 2026 used this lane exactly as
documented.

Deployed footprint:

- project: `openagentsgemini`
- region / zone: `us-central1` / `us-central1-a`
- VPC / subnet: `oa-lightning` / `oa-lightning-us-central1`
- VM: `probe-hosted-forge-1`
- service account: `probe-hosted-forge@openagentsgemini.iam.gserviceaccount.com`
- persistent Probe home: `/var/lib/probe-hosted`
- hosted listener: `127.0.0.1:7777`
- local IAP tunnel: `127.0.0.1:17777`
- prepared baseline workspace:
  - `/var/lib/probe-hosted/hosted/workspaces/forge-openagents-main/openagents`

The final successful hosted session was:

- `sess_1775150159726_14012_184`

What Probe proved in that run:

- hosted session ownership recorded as `hosted_control_plane`
- prepared baseline state recorded as `ready`
- mounted refs recorded for the project-scoped OpenAgents docs and hosted
  runbook packs
- hosted auth, checkout, worker, cost, and cleanup receipts were present in the
  session metadata
- both hosted turns completed under the longer hosted watchdog posture:
  - patch turn
  - read-back turn

The main runtime lesson from proving this lane live was simple:

- hosted Codex-backed turns need a looser detached watchdog posture than the
  local daemon default, so this deploy lane now starts `probe-server` with:
  - `--watchdog-poll-ms 500`
  - `--watchdog-stall-ms 180000`
  - `--watchdog-timeout-ms 300000`

## Honest limitations

This deploy lane is still intentionally narrow:

- one worker, not a fleet
- operator-run provisioning, not an API control plane
- internal IAP attach, not a public hosted endpoint
- prepared baseline manifests are explicit files, not image registries or
  snapshot orchestration
- cleanup and restart drills still depend on direct operator invocation

That is acceptable for the current phase. The point is proving the real hosted
runtime seam end to end before we invent more platform surface.
