# Managed Cloud Run Job Runner

Probe ships a first hosted managed-session runner for Google Cloud Run Jobs.
This is a bounded task-to-completion lane, not a long-running control plane.

Canonical surfaces:

- `probe managed cloud-run-job run-once`
- `probe_core::managed_cloud_run_job`
- `scripts/deploy/managed-cloud-run-job/Dockerfile`
- `scripts/deploy/managed-cloud-run-job/probe-managed-cloud-run-job.sh`

## Contract

Laravel creates a signed assignment token with
`probe.managed_cloud_run_job.assignment.v1` claims:

- assignment id and idempotency key
- managed session id and managed run id
- callback URL
- goal prompt
- managed environment id
- provider allocation: `google_cloud` / `cloud_run_job`
- environment class
- optional Probe profile, cwd, title, GCS artifact prefix, and expiration

Probe verifies the HMAC assignment token, rejects non-Google-Cloud or
non-Cloud-Run-Job allocations, writes an idempotency state file, executes the
managed prompt once, records managed runtime terminal events, writes evidence,
and posts `job.started`, `job.completed`, or `job.failed` callback events.

Retries with the same idempotency key skip execution and return the retained
state so duplicate Cloud Run executions do not duplicate commits or artifacts.

## Required Cloud Run Configuration

Use a Cloud Run Job, not a Cloud Run service, for this lane.

Required image contents:

- `/usr/local/bin/probe`
- `/usr/local/bin/probe-managed-cloud-run-job`
- git and SSH client for repository work
- Google Cloud CLI if the runner should upload artifact directories to GCS

Required service account permissions:

- read Secret Manager entries used for assignment/callback/model secrets
- read/write the configured GCS artifact bucket
- read source repositories or mounted workspace resources
- write Cloud Logging

Required environment variables or mounted secret files:

- `PROBE_MANAGED_ASSIGNMENT_TOKEN` or `PROBE_MANAGED_ASSIGNMENT_TOKEN_FILE`
- `PROBE_MANAGED_ASSIGNMENT_SIGNING_SECRET` or
  `PROBE_MANAGED_ASSIGNMENT_SIGNING_SECRET_FILE`
- `PROBE_MANAGED_CALLBACK_BEARER_TOKEN`
- `PROBE_OPENAI_API_KEY` when the selected profile needs an API-key fallback
- `PROBE_MANAGED_ARTIFACT_GCS_PREFIX` for artifact upload

Optional environment variables:

- `PROBE_HOME`
- `PROBE_MANAGED_CWD`
- `PROBE_MANAGED_PROFILE`
- `PROBE_MANAGED_JOB_ARTIFACT_DIR`
- `PROBE_MANAGED_SYSTEM_PROMPT`
- `PROBE_MANAGED_CLOUD_RUN_LOGS_URL`
- `PROBE_MANAGED_DRY_RUN`
- `PROBE_MANAGED_PRETTY`

Cloud Run provides `CLOUD_RUN_JOB`, `CLOUD_RUN_EXECUTION`,
`CLOUD_RUN_TASK_INDEX`, and `CLOUD_RUN_TASK_ATTEMPT`; Probe includes those in
state, callbacks, and evidence.

## Build

```bash
gcloud builds submit \
  --project "$GCP_PROJECT" \
  --tag "$GCP_REGION-docker.pkg.dev/$GCP_PROJECT/probe/probe-managed-cloud-run-job:latest" \
  --file scripts/deploy/managed-cloud-run-job/Dockerfile .
```

## Create Or Update Job

```bash
gcloud run jobs deploy probe-managed-agent-runner \
  --project "$GCP_PROJECT" \
  --region "$GCP_REGION" \
  --image "$GCP_REGION-docker.pkg.dev/$GCP_PROJECT/probe/probe-managed-cloud-run-job:latest" \
  --service-account "probe-managed-runner@$GCP_PROJECT.iam.gserviceaccount.com" \
  --task-timeout 24h \
  --max-retries 1 \
  --set-env-vars PROBE_MANAGED_PROFILE=openai-codex-subscription \
  --set-secrets PROBE_MANAGED_ASSIGNMENT_SIGNING_SECRET=probe-managed-assignment-signing-secret:latest \
  --set-secrets PROBE_MANAGED_CALLBACK_BEARER_TOKEN=probe-managed-callback-token:latest \
  --set-secrets PROBE_OPENAI_API_KEY=probe-managed-openai-api-key:latest
```

Laravel should pass a fresh `PROBE_MANAGED_ASSIGNMENT_TOKEN` per execution.
For production, prefer `--update-secrets` or task-specific job overrides rather
than putting assignment tokens in shell history.

## Laravel Callback Payloads

Callbacks include:

- schema version
- event type
- assignment id
- managed session id
- managed run id
- idempotency key
- Cloud Run job/execution/task identity
- Probe session id
- terminal managed event sequence
- artifact refs
- error summary

Laravel should persist those fields on the managed run and expose the logs URL,
runtime status, terminal event, and artifact refs in the admin UI/API.

## Verification

Focused local coverage:

```bash
cargo test -p probe-core managed_cloud_run_job -- --nocapture
cargo test -p probe-cli managed_cloud_run_job_command_parses -- --nocapture
```
