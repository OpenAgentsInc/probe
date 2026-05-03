# Managed Daytona Provider

Probe now has a Daytona adapter behind the provider-neutral managed environment
contract. Daytona remains supplemental: Google Cloud stays the default hosted
managed-agent path while the credit balance is material and the GCP runners are
healthy.

Canonical entrypoints:

```bash
probe managed daytona advertise
probe managed daytona run-once
```

Canonical Rust types:

- `probe_core::managed_daytona`
- `ManagedDaytonaProviderAdapter`
- `ManagedDaytonaRunner`
- `ManagedDaytonaSnapshotTemplate`
- `ManagedDaytonaAssignmentClaims`

## Current Daytona Contract

This implementation follows the current Daytona documentation:

- create sandboxes with `POST https://app.daytona.io/api/sandbox`
  as shown in <https://www.daytona.io/docs/>
- execute commands through
  `POST https://proxy.app.daytona.io/toolbox/{sandboxId}/process/execute`
  as shown in <https://www.daytona.io/docs/en/process-code-execution/>
- use snapshots as sandbox templates, documented at
  <https://www.daytona.io/docs/en/snapshots/>
- authenticate with an API key from `DAYTONA_API_KEY`, documented at
  <https://www.daytona.io/docs/en/api-keys/>

The legacy `DaytonaSandboxGateway` in `backroom` was used only as historical
guidance. The active implementation keeps Daytona below Probe's managed
environment/provider boundary instead of reviving the old product control
plane.

## Capability Advertisement

Use `advertise` to produce a safe `probe.managed_environment.v1` capability
document for Laravel:

```bash
probe managed daytona advertise \
  --worker-id daytona-worker-1 \
  --managed-environment-id env_daytona_coding \
  --environment-class daytona-coding \
  --snapshot probe-managed-agent \
  --target us \
  --cpu-millicores 4000 \
  --memory-mib 8192 \
  --disk-mib 51200 \
  --backend-profile openai-codex-subscription \
  --label supplemental \
  --pretty
```

The output advertises:

- provider `daytona`
- host class `daytona_workspace`
- the snapshot as a safe `daytona://snapshots/...` runtime ref
- public resource limits and backend profile names
- safe public metadata only

Do not put Daytona API keys, bearer tokens, repository tokens, model provider
keys, or secret-manager payloads into labels, metadata, assignment claims, or
callback payloads.

## Assignment Shape

`run-once` accepts a signed `probe.managed_daytona.assignment.v1` token. The
claims mirror the Cloud Run assignment contract but require:

```json
{
  "environment": {
    "provider": "daytona",
    "hostClass": "daytona_workspace"
  }
}
```

Optional Daytona fields:

- `snapshot`: snapshot name to use for a new sandbox
- `target`: Daytona target/region
- `sandboxId`: attach to an existing sandbox instead of creating a new one
- `sandboxName`: name for a new sandbox
- `bootstrapCommand`: command to execute inside the sandbox

If `bootstrapCommand` is absent, Probe builds a default command:

```bash
probe exec --profile <profile> --cwd <cwd> --title <title> -- <goal-prompt> 2>&1
```

That means the Daytona snapshot must include a working `probe` binary and any
required runtime secrets if the run is not `--dry-run`.

## Runtime And Evidence

`probe managed daytona run-once`:

- verifies the signed assignment token and idempotency key
- creates or attaches a Daytona sandbox
- maps the sandbox id, snapshot, target, resources, and safe metadata into a
  managed runtime allocation
- runs the bootstrap command through Daytona's process execution API
- records `SessionStarted` and terminal managed-runtime events
- writes `daytona-evidence.json`
- posts Laravel callbacks for `daytona.started`, `daytona.completed`,
  `daytona.failed`, or `daytona.duplicate_skipped`

Daytona provider failures are normalized with stable codes:

- `not_configured`
- `unauthorized`
- `forbidden`
- `sandbox_not_found`
- `timeout`
- `api_error`
- `invalid_response`
- `network`

The state and evidence files intentionally omit Daytona credentials.

## Laravel Boundary

Laravel should remain the durable source of truth for:

- environment records and provider policy
- Daytona credential binding and Secret Manager references
- managed sessions, runs, outcomes, callbacks, and artifacts
- billing/governance decisions
- admin API/UI visibility

Probe owns:

- signed assignment verification
- sandbox allocation calls
- provider error normalization
- runtime event/evidence generation
- safe capability advertisement

Daytona is selectable by policy and capability matching. Do not hardcode it as
the first provider while the Google Cloud path is healthy.

## Verification

Focused coverage:

```bash
cargo test -p probe-core managed_daytona -- --nocapture
cargo test -p probe-cli managed_daytona -- --nocapture
```
