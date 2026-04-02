# Hosted Repo Checkout And Cleanup Proof

Issue `#95` closes the loop on the first hosted receipt bundle by proving the
boring operational facts through owned tests and a written proof bundle.

## What This Proves

Probe now has explicit proof for three hosted-runtime facts that Forge needs:

- repo access and checkout truth
- execution-host and worker attribution
- cleanup outcome visibility

The proof is local and deterministic.

It does not claim that we have already run a production GCP fleet. It does
claim that the shipped hosted TCP lane exposes the exact runtime facts the
future GCP dogfood phase will need.

## Proof Cases

### 1. Hosted Git Checkout Proof

`hosted_turn_receipts_capture_git_checkout_and_cost_observability`

This test:

- creates a real temporary git repo
- adds an `origin`
- runs a hosted Probe turn over the TCP transport
- asserts that hosted receipts include:
  - git checkout kind
  - repo identity
  - head ref
  - resolved head commit
  - cost-estimation observability

That gives Probe-owned proof for "which repo or ref was actually checked out"
without forcing Forge to scrape git output itself.

### 2. Detached Summary Proof

`hosted_detached_summary_projects_checkout_worker_and_cleanup_receipts`

This test proves that detached hosted sessions do not lose those receipts when
the operator inspects the detached registry.

The detached summary keeps:

- checkout receipt
- worker receipt
- cleanup receipt

So Forge can inspect hosted session truth from the detached path, not only from
foreground session snapshots.

### 3. Managed Cleanup Completion Proof

`managed_hosted_workspace_cleanup_receipt_marks_completed_once_path_is_gone`

This test creates a managed hosted workspace under:

- `~/.probe/hosted/workspaces/...`

It then removes that workspace path and confirms that the detached summary now
reports cleanup as `completed`.

That gives Probe an explicit cleanup outcome instead of forcing operators to
guess whether a hosted workspace was still present.

## Cleanup Semantics

The first cleanup contract is intentionally explicit:

- operator-supplied workspace paths
  - cleanup status: `not_required`
  - Probe records that it is attached to an existing path and will not delete
    it
- Probe-managed hosted workspace paths under `hosted/workspaces`
  - cleanup status: `pending` while the path still exists
  - cleanup status: `completed` once the path is gone

That is honest enough for the first hosted dogfood phase.

## Current Limits

This proof still does not claim:

- a live GCP fleet run
- automatic workspace allocation and teardown hooks
- remote repo cloning or scheduling policy
- cloud-provider auth or billing integration

The important change is that Probe now has stable, typed proof for hosted repo
identity, execution host, and cleanup posture before OpenAgents starts writing
hosted closeout audits above it.
