# Hosted Session Receipts

Issue `#96` adds the first explicit hosted receipt bundle to Probe session
metadata.

## What Landed

Hosted Probe sessions now persist `hosted_receipts` in:

- `SessionMetadata`
- `SessionSnapshot.session`
- `DetachedSessionSummary`

The first receipt bundle is intentionally boring:

- `auth`
  - control-plane asserted authority
  - subject
  - auth kind
  - optional scope
- `worker`
  - owner kind and id
  - attach transport and target
  - execution-host kind and id
- `checkout`
  - workspace root
  - git repo root when present
  - remote identity when Probe can resolve it
  - head ref and commit when the cwd is inside a repo
- `cost`
  - observed turn count
  - aggregate wallclock
  - aggregate prompt, completion, and total tokens when available
  - an explicit note that this is operator cost-estimation data, not billing
- `cleanup`
  - cleanup status
  - workspace root
  - cleanup strategy
  - an explicit note when Probe is attached to an operator-supplied workspace
- `history`
  - retained cleanup-state transitions
  - retained approval-paused takeover availability
  - retained running-turn failure on restart

## Why

Forge hosted closeout bundles need runtime-owned receipts instead of:

- scraping logs to infer repo checkout
- guessing which worker actually ran the session
- inventing billing or cleanup stories in the app shell

Probe already had runtime-owner and execution-host provenance.

This issue turns those into an explicit hosted receipt bundle that OpenAgents
can reference directly.

## Receipt Rules

Probe only emits `hosted_receipts` when the runtime owner is
`hosted_control_plane`.

The first rules are narrow and honest:

- auth is control-plane asserted metadata, not a full cloud IAM model
- checkout truth comes from the live workspace path plus git inspection when
  the cwd is inside a repository
- cost is raw turn observability aggregated across the current transcript
- cleanup reports whether Probe owns the workspace path or whether cleanup is
  explicitly not required because the operator supplied the workspace

## CLI Flags

`probe-server --listen-tcp ...` now accepts optional hosted auth receipt flags:

- `--hosted-auth-authority <authority>`
- `--hosted-auth-subject <subject>`
- `--hosted-auth-kind <control_plane_assertion|operator_token>`
- `--hosted-auth-scope <scope>`

If omitted, Probe defaults to:

- authority: hosted owner id
- subject: `gcp-internal-dogfood`
- auth kind: `control_plane_assertion`
- scope: `probe.hosted.session`

## Current Limits

This does not yet add:

- cloud-provider IAM integration
- actual dollar billing objects
- automatic managed-workspace teardown hooks
- scheduler or autoscaler receipts

The important change is that hosted session inspection now exposes the
operator-facing runtime truth needed for closeout bundles without forcing
OpenAgents to scrape logs.
