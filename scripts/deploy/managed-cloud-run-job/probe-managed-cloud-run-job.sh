#!/usr/bin/env bash
set -euo pipefail

log() {
  printf '[probe-managed-cloud-run-job] %s\n' "$*" >&2
}

die() {
  printf '[probe-managed-cloud-run-job] ERROR: %s\n' "$*" >&2
  exit 1
}

PROBE_BINARY="${PROBE_BINARY:-/usr/local/bin/probe}"
PROBE_HOME="${PROBE_HOME:-/var/lib/probe}"
PROBE_MANAGED_PROFILE="${PROBE_MANAGED_PROFILE:-openai-codex-subscription}"
PROBE_MANAGED_CWD="${PROBE_MANAGED_CWD:-/workspace}"
PROBE_MANAGED_JOB_ARTIFACT_DIR="${PROBE_MANAGED_JOB_ARTIFACT_DIR:-${PROBE_HOME}/managed/cloud-run-job/artifacts}"
PROBE_MANAGED_ASSIGNMENT_TOKEN_ENV="${PROBE_MANAGED_ASSIGNMENT_TOKEN_ENV:-PROBE_MANAGED_ASSIGNMENT_TOKEN}"
PROBE_MANAGED_ASSIGNMENT_SIGNING_SECRET_ENV="${PROBE_MANAGED_ASSIGNMENT_SIGNING_SECRET_ENV:-PROBE_MANAGED_ASSIGNMENT_SIGNING_SECRET}"

[[ -x "$PROBE_BINARY" ]] || die "missing probe binary: ${PROBE_BINARY}"
mkdir -p "$PROBE_HOME" "$PROBE_MANAGED_CWD" "$PROBE_MANAGED_JOB_ARTIFACT_DIR"

if [[ -z "${PROBE_MANAGED_ASSIGNMENT_TOKEN:-}" && -z "${PROBE_MANAGED_ASSIGNMENT_TOKEN_FILE:-}" ]]; then
  die "missing assignment token; set PROBE_MANAGED_ASSIGNMENT_TOKEN or PROBE_MANAGED_ASSIGNMENT_TOKEN_FILE"
fi
if [[ -z "${PROBE_MANAGED_ASSIGNMENT_SIGNING_SECRET:-}" && -z "${PROBE_MANAGED_ASSIGNMENT_SIGNING_SECRET_FILE:-}" ]]; then
  die "missing signing secret; set PROBE_MANAGED_ASSIGNMENT_SIGNING_SECRET or PROBE_MANAGED_ASSIGNMENT_SIGNING_SECRET_FILE"
fi

args=(
  managed
  cloud-run-job
  run-once
  --probe-home "$PROBE_HOME"
  --assignment-token-env "$PROBE_MANAGED_ASSIGNMENT_TOKEN_ENV"
  --signing-secret-env "$PROBE_MANAGED_ASSIGNMENT_SIGNING_SECRET_ENV"
  --callback-bearer-env "${PROBE_MANAGED_CALLBACK_BEARER_ENV:-PROBE_MANAGED_CALLBACK_BEARER_TOKEN}"
  --profile "$PROBE_MANAGED_PROFILE"
  --cwd "$PROBE_MANAGED_CWD"
  --artifact-dir "$PROBE_MANAGED_JOB_ARTIFACT_DIR"
)

if [[ -n "${PROBE_MANAGED_ASSIGNMENT_TOKEN_FILE:-}" ]]; then
  args+=(--assignment-token-file "$PROBE_MANAGED_ASSIGNMENT_TOKEN_FILE")
fi
if [[ -n "${PROBE_MANAGED_ASSIGNMENT_SIGNING_SECRET_FILE:-}" ]]; then
  args+=(--signing-secret-file "$PROBE_MANAGED_ASSIGNMENT_SIGNING_SECRET_FILE")
fi
if [[ -n "${PROBE_MANAGED_SYSTEM_PROMPT:-}" ]]; then
  args+=(--system "$PROBE_MANAGED_SYSTEM_PROMPT")
fi
if [[ -n "${CLOUD_RUN_JOB:-}" ]]; then
  args+=(--cloud-run-job "$CLOUD_RUN_JOB")
fi
if [[ -n "${CLOUD_RUN_EXECUTION:-}" ]]; then
  args+=(--cloud-run-execution "$CLOUD_RUN_EXECUTION")
fi
if [[ -n "${CLOUD_RUN_TASK_INDEX:-}" ]]; then
  args+=(--cloud-run-task-index "$CLOUD_RUN_TASK_INDEX")
fi
if [[ -n "${CLOUD_RUN_TASK_ATTEMPT:-}" ]]; then
  args+=(--cloud-run-task-attempt "$CLOUD_RUN_TASK_ATTEMPT")
fi
if [[ -n "${PROBE_MANAGED_CLOUD_RUN_LOGS_URL:-}" ]]; then
  args+=(--logs-url "$PROBE_MANAGED_CLOUD_RUN_LOGS_URL")
fi
if [[ "${PROBE_MANAGED_DRY_RUN:-false}" == "true" ]]; then
  args+=(--dry-run)
fi
if [[ "${PROBE_MANAGED_PRETTY:-false}" == "true" ]]; then
  args+=(--pretty)
fi

log "starting managed Cloud Run Job assignment"
"$PROBE_BINARY" "${args[@]}"

if [[ -n "${PROBE_MANAGED_ARTIFACT_GCS_PREFIX:-}" ]]; then
  if ! command -v gcloud >/dev/null 2>&1; then
    die "PROBE_MANAGED_ARTIFACT_GCS_PREFIX is set but gcloud is not installed in the image"
  fi
  log "uploading artifacts to ${PROBE_MANAGED_ARTIFACT_GCS_PREFIX}"
  gcloud storage cp --recursive "$PROBE_MANAGED_JOB_ARTIFACT_DIR" "$PROBE_MANAGED_ARTIFACT_GCS_PREFIX"
fi

log "managed Cloud Run Job assignment finished"
