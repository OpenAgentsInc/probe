#!/usr/bin/env bash
set -euo pipefail

shutdown_file="${PROBE_MANAGED_WORKER_SHUTDOWN_FILE:-/tmp/probe-managed-worker-pool.shutdown}"
artifact_dir="${PROBE_MANAGED_WORKER_ARTIFACT_DIR:-${PROBE_HOME:-/var/lib/probe}/managed/cloud-run-worker-pool/artifacts}"

rm -f "$shutdown_file"
mkdir -p "$(dirname "$shutdown_file")" "$artifact_dir"

if [[ -z "${PROBE_MANAGED_CONTROLLER_BASE_URL:-}" ]]; then
  echo "PROBE_MANAGED_CONTROLLER_BASE_URL is required" >&2
  exit 2
fi

if [[ -z "${PROBE_MANAGED_WORKER_BEARER_TOKEN:-}" && -z "${PROBE_MANAGED_WORKER_BEARER_TOKEN_FILE:-}" ]]; then
  echo "PROBE_MANAGED_WORKER_BEARER_TOKEN or PROBE_MANAGED_WORKER_BEARER_TOKEN_FILE is required" >&2
  exit 2
fi

request_shutdown() {
  touch "$shutdown_file"
  echo "managed Cloud Run Worker Pool shutdown requested; draining after current assignment" >&2
}

trap request_shutdown TERM INT

args=(
  managed cloud-run-worker-pool run
  --probe-home "${PROBE_HOME:-/var/lib/probe}"
  --controller-base-url "${PROBE_MANAGED_CONTROLLER_BASE_URL}"
  --artifact-dir "$artifact_dir"
  --cwd "${PROBE_MANAGED_CWD:-/workspace}"
  --shutdown-file "$shutdown_file"
  --poll-interval-ms "${PROBE_MANAGED_WORKER_POLL_INTERVAL_MS:-1000}"
  --profile "${PROBE_MANAGED_WORKER_PROFILE:-openai-codex-subscription}"
)

if [[ -n "${PROBE_MANAGED_WORKER_BEARER_TOKEN_FILE:-}" ]]; then
  args+=(--bearer-token-file "$PROBE_MANAGED_WORKER_BEARER_TOKEN_FILE")
fi

if [[ -n "${PROBE_MANAGED_WORKER_ID:-}" ]]; then
  args+=(--worker-id "$PROBE_MANAGED_WORKER_ID")
fi

if [[ -n "${PROBE_MANAGED_ENVIRONMENT_CLASS:-}" ]]; then
  args+=(--environment-class "$PROBE_MANAGED_ENVIRONMENT_CLASS")
fi

if [[ -n "${PROBE_MANAGED_WORKER_IMAGE_REF:-}" ]]; then
  args+=(--image-ref "$PROBE_MANAGED_WORKER_IMAGE_REF")
fi

if [[ -n "${PROBE_MANAGED_WORKER_CPU_MILLICORES:-}" ]]; then
  args+=(--cpu-millicores "$PROBE_MANAGED_WORKER_CPU_MILLICORES")
fi

if [[ -n "${PROBE_MANAGED_WORKER_MEMORY_MIB:-}" ]]; then
  args+=(--memory-mib "$PROBE_MANAGED_WORKER_MEMORY_MIB")
fi

if [[ -n "${PROBE_MANAGED_WORKER_DISK_MIB:-}" ]]; then
  args+=(--disk-mib "$PROBE_MANAGED_WORKER_DISK_MIB")
fi

if [[ -n "${PROBE_MANAGED_GCP_REGION:-}" ]]; then
  args+=(--region "$PROBE_MANAGED_GCP_REGION")
fi

if [[ "${PROBE_MANAGED_WORKER_EXIT_ON_IDLE:-}" == "1" ]]; then
  args+=(--exit-on-idle)
fi

probe "${args[@]}" "$@" &
child_pid=$!
status=0
while true; do
  if wait "$child_pid"; then
    status=0
    break
  fi
  status=$?
  if kill -0 "$child_pid" 2>/dev/null; then
    continue
  fi
  break
done

if [[ -n "${PROBE_MANAGED_ARTIFACT_GCS_PREFIX:-}" ]]; then
  gcloud storage cp --recursive "$artifact_dir" "$PROBE_MANAGED_ARTIFACT_GCS_PREFIX" >&2
fi

exit "$status"
