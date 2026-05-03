# Managed Cloud Run Worker Pool

Probe now has a pull-only Cloud Run Worker Pool runner for managed agents.
This is the continuous complement to the one-shot Cloud Run Job runner.

Canonical entrypoint:

```bash
probe managed cloud-run-worker-pool run
```

Canonical Rust types:

- `probe_core::managed_cloud_run_worker_pool`
- `ManagedCloudRunWorkerPoolRunner`
- `ManagedCloudRunWorkerPoolAssignment`
- `ManagedCloudRunWorkerPoolRunReport`

## Why Worker Pools

Cloud Run Worker Pools are the first continuous GCP execution lane because
they keep the worker private and pull work from Laravel. The container does not
need public inbound access.

Relevant Google Cloud contracts:

- deploy with `gcloud run worker-pools deploy WORKER_POOL --image IMAGE_URL`
  as documented by Google Cloud:
  <https://docs.cloud.google.com/run/docs/deploy-worker-pools>
- Cloud Run sets `CLOUD_RUN_WORKER_POOL` and
  `CLOUD_RUN_WORKER_POOL_REVISION` for worker pools:
  <https://docs.cloud.google.com/run/docs/container-contract#environment_variables>
- worker pools are manually scaled, default to one instance, can be disabled
  with zero instances, and receive `SIGTERM` before shutdown:
  <https://docs.cloud.google.com/run/docs/container-contract#worker-pools>
  and <https://docs.cloud.google.com/run/docs/managing/workerpools>

## Laravel Worker API Contract

The worker uses a Sanctum bearer token and calls the Laravel admin managed-agent
API. These are Probe-side paths that Laravel must expose:

| Method | Path | Purpose |
| --- | --- | --- |
| `POST` | `/api/admin/managed-agents/v1/runtime/workers/attach` | register worker identity, Cloud Run revision, and capabilities |
| `POST` | `/api/admin/managed-agents/v1/runtime/workers/{worker}/heartbeat` | publish health, current allocation, deploy revision, and drain state |
| `POST` | `/api/admin/managed-agents/v1/runtime/workers/{worker}/assignments/claim-next` | atomically claim the next leased assignment |
| `POST` | `/api/admin/managed-agents/v1/runtime/workers/{worker}/assignments/{assignment}/events` | report assignment terminal events and artifacts |

All requests include:

- `Authorization: Bearer <Sanctum token>`
- `Accept: application/json`
- `X-Probe-Worker-Id: <worker id>`
- `schemaVersion: probe.managed_cloud_run_worker_pool.v1`

The attach and heartbeat payloads include:

- `worker.workerId`
- `worker.workerPool`
- `worker.workerPoolRevision`
- `worker.instanceId`
- `worker.region`
- `worker.logsUrl`
- `deployRevision`
- `capabilities` as `probe.managed_environment.v1`

Laravel should persist these fields so the admin API/UI can show pool health,
worker identity, current allocation, last heartbeat, deploy revision, logs URL,
and environment compatibility.

## Assignment Shape

`claim-next` returns either:

```json
{"assignment": null, "pollIntervalMs": 1000}
```

or:

```json
{
  "assignment": {
    "schemaVersion": "probe.managed_cloud_run_worker_pool.assignment.v1",
    "assignmentId": "assignment-001",
    "leaseId": "lease-001",
    "idempotencyKey": "assignment-001:lease-001",
    "managedSessionId": "managed-session-001",
    "managedRunId": "managed-run-001",
    "goalPrompt": "Resolve the next approved issue.",
    "environment": {
      "managedEnvironmentId": "environment-001",
      "provider": "google_cloud",
      "hostClass": "cloud_run_worker_pool",
      "environmentClass": "gcp-coding-standard"
    },
    "artifactGcsPrefix": "gs://bucket/managed-runs/managed-run-001"
  }
}
```

Probe validates that the assignment targets `google_cloud` and
`cloud_run_worker_pool`, and that optional assignment constraints do not reject
that provider or host class.

## Idempotency

Probe stores assignment state under:

```text
<artifact-dir>/state/<sha256-idempotency-prefix>.json
```

If a worker sees the same idempotency key again, it reports
`assignment.duplicate_skipped` without starting a second Probe runtime attempt.
Laravel should still make `claim-next` lease-safe and avoid assigning the same
lease to two workers.

## Shutdown And Cancellation

The deployment wrapper writes a shutdown file when the container receives
`SIGTERM` or `SIGINT`. The Rust worker checks that file between assignments and
after the current assignment completes. That means a Cloud Run shutdown drains
the active assignment and then stops claiming more work.

Heartbeat responses can set:

```json
{
  "shutdownRequested": true,
  "cancelCurrentAssignment": true
}
```

`shutdownRequested` stops future claims. `cancelCurrentAssignment` is honored
before runtime start for a freshly claimed assignment. Mid-turn interruption
still belongs to the lower-level Probe session/turn control APIs; this worker
pool lane deliberately starts with safe drain semantics rather than killing a
running model/tool loop from a signal handler.

## Deploy

Build and push:

```bash
gcloud builds submit \
  --config scripts/deploy/managed-cloud-run-worker-pool/cloudbuild.yaml \
  --substitutions _IMAGE=us-central1-docker.pkg.dev/PROJECT/REPOSITORY/probe-managed-worker-pool:latest \
  .
```

Deploy one manually scaled instance first:

```bash
gcloud run worker-pools deploy probe-managed-workers \
  --image us-central1-docker.pkg.dev/PROJECT/REPOSITORY/probe-managed-worker-pool:latest \
  --region us-central1 \
  --instances 1 \
  --service-account probe-managed-workers@PROJECT.iam.gserviceaccount.com \
  --env-vars-file scripts/deploy/managed-cloud-run-worker-pool/probe-managed-cloud-run-worker-pool.env.example
```

Rollback / pause:

```bash
gcloud run worker-pools update probe-managed-workers --region us-central1 --instances 0
```

Resume:

```bash
gcloud run worker-pools update probe-managed-workers --region us-central1 --instances 1
```

## Required Service Account Access

The worker service account needs only outbound access:

- read worker bearer token / OpenAI fallback secrets from Secret Manager if
  those are injected at runtime
- read/write the configured GCS artifact prefix if artifact upload is enabled
- reach `openagents.com` or the private Laravel endpoint
- pull the Artifact Registry image

Do not set `GOOGLE_APPLICATION_CREDENTIALS` in Cloud Run. Use the worker pool
service identity.

## Verification

Focused coverage:

```bash
cargo test -p probe-core managed_cloud_run_worker_pool -- --nocapture
cargo test -p probe-cli managed_cloud_run_worker_pool_command_parses -- --nocapture
bash -n scripts/deploy/managed-cloud-run-worker-pool/probe-managed-cloud-run-worker-pool.sh
```
