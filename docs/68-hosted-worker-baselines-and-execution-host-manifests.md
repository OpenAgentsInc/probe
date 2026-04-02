# Hosted Worker Baselines And Execution-Host Manifests

Issue `#92` adds the first typed workspace-provenance surface for hosted Probe
sessions.

## What Landed

Hosted Probe sessions can now persist a `workspace_state` object in:

- `SessionMetadata`
- `DetachedSessionSummary`
- detached `workspace_state_updated` events

That state carries:

- `boot_mode`
  - `fresh`
  - `prepared_baseline`
  - `snapshot_restore`
- optional prepared baseline ref
  - baseline id
  - repo identity
  - base ref
  - status
    - `ready`
    - `missing`
    - `stale`
- optional snapshot ref
  - snapshot id
  - restore manifest id
  - source baseline id
- optional execution-host metadata
  - host kind
  - host id
  - display name
  - location
- optional provenance note

## Hosted Manifest Lookup

The first implementation keeps manifest lookup boring and local to the Probe
home:

- prepared baselines
  - `~/.probe/hosted/baselines/<manifest-id>.json`
- snapshots
  - `~/.probe/hosted/snapshots/<manifest-id>.json`

The file names are derived from the requested id with a simple safe-file-name
normalization.

Probe uses those files as provider-agnostic manifest hints. The typed session
state is the public contract; the on-disk layout is only the first local
implementation.

## Fallback Semantics

Prepared baselines are treated as an optimization, not as hidden truth.

If a hosted session asks for `prepared_baseline` boot and Probe cannot resolve
that baseline manifest, the stored session state now says so explicitly:

- the baseline ref stays attached
- the baseline status becomes `missing` or `stale`
- the final boot mode becomes `fresh`
- the provenance note explains that Probe fell back instead of pretending the
  prepared baseline was used

That gives hosted consumers an honest answer without scraping logs.

## Execution-Host Metadata

Hosted Probe does not yet expose provider-specific worker details.

The first contract only records the machine-readable execution-host identity
Probe can honestly defend:

- `local_machine` for local foreground and daemon-owned work
- `hosted_worker` for hosted control-plane-owned work

That is enough for consumers to distinguish "your laptop is still the worker"
from "this ran under a hosted Probe owner" without locking the protocol to a
specific cloud product.

## Current Limits

This issue does not claim:

- image-build pipelines
- actual worker scheduling
- automatic snapshot creation
- remote tenancy or auth
- provider billing objects

The important change is that hosted consumers can now trust typed workspace and
execution provenance instead of inferring it from logs or side channels.
