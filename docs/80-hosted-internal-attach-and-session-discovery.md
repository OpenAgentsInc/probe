# Hosted Internal Attach And Session Discovery

Issue `#105` makes the current GCP-hosted Probe lane usable by teammates
without depending on one operator's local tunnel or handwritten session ids.

## What Landed

Probe now supports an internal-only hosted transport in `probe-client`:

- `ProbeClientTransportConfig::HostedGcpIap`

That transport:

- shells out to `gcloud compute start-iap-tunnel`
- opens a local loopback port on the operator machine
- connects the normal hosted TCP JSONL client through that tunnel
- tears the tunnel child down when the client drops

This keeps the hosted lane boring:

- no new public endpoint
- no new hosted control-plane service
- no separate discovery backend

It just lets each teammate open their own direct attach path to the current
private GCP worker.

## Runtime-Owned Discovery Surface

Probe did not need a new session registry for discovery.

The existing detached-session registry already carries the runtime truth that
other operators need:

- session id
- status and recovery state
- runtime owner and attach target
- hosted receipts
- participant roster
- controller lease

Once a teammate can reach the hosted Probe server directly, `list_detached_sessions`
is enough to discover active hosted work without Slack messages or local shell
copy-paste.

## CLI Surface

The operator CLI now accepts hosted target flags on the detached-session
commands:

- `probe ps`
- `probe attach <session-id>`
- `probe logs <session-id>`
- `probe stop <session-id>`

There are now two explicit hosted attach paths:

- `--hosted-address <host:port>`
  - use this when an internal hosted TCP address is already reachable
- `--hosted-gcp-project`, `--hosted-gcp-zone`, `--hosted-gcp-instance`
  - use this when the team should open an internal GCP IAP tunnel directly

Optional flags:

- `--hosted-gcp-remote-port <port>`
- `--hosted-local-host <host>`
- `--hosted-local-port <port>`

`probe ps` now also renders the detached summary with:

- runtime owner kind and owner id
- attach target when Probe knows it
- participant count
- current controller participant id

That makes the runtime-owned discovery output usable for attach decisions.

## Why This Matters For Internal Forge MVP

The internal MVP is not a public hosted platform.
It is a team workflow:

- Teammate A starts or resumes a hosted Probe session.
- Teammate B reaches the same worker directly from their own machine.
- Teammate B lists active hosted sessions from Probe-owned runtime state.
- Teammate B attaches to the right session and can inspect or take over using
  the existing participant and controller-lease rules.

That is now possible on the current GCP lane without routing traffic through
Teammate A's laptop.

## Current Limits

This still does not add:

- a fleet-wide hosted session directory across multiple workers
- app-owned collaboration state or human-friendly shared session naming above
  Probe runtime ids
- public hosted access outside the internal GCP lane

Those remain follow-on work in OpenAgents and the future hosted control-plane
layers.
