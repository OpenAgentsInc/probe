#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

export GCP_PROJECT="${GCP_PROJECT:-openagentsgemini}"
export GCP_REGION="${GCP_REGION:-us-central1}"
export GCP_ZONE="${GCP_ZONE:-us-central1-a}"

export OA_VPC="${OA_VPC:-oa-lightning}"
export OA_SUBNET="${OA_SUBNET:-oa-lightning-us-central1}"

export PROBE_FORGE_VM="${PROBE_FORGE_VM:-probe-forge-worker-1}"
export PROBE_FORGE_MACHINE_TYPE="${PROBE_FORGE_MACHINE_TYPE:-e2-standard-4}"
export PROBE_FORGE_SERVICE_ACCOUNT_NAME="${PROBE_FORGE_SERVICE_ACCOUNT_NAME:-probe-forge-worker}"
export PROBE_FORGE_SERVICE_ACCOUNT_EMAIL="${PROBE_FORGE_SERVICE_ACCOUNT_EMAIL:-${PROBE_FORGE_SERVICE_ACCOUNT_NAME}@${GCP_PROJECT}.iam.gserviceaccount.com}"
export PROBE_FORGE_TAG="${PROBE_FORGE_TAG:-probe-forge-worker}"

export PROBE_FORGE_DATA_DISK="${PROBE_FORGE_DATA_DISK:-probe-forge-worker-data}"
export PROBE_FORGE_DATA_DISK_DEVICE_NAME="${PROBE_FORGE_DATA_DISK_DEVICE_NAME:-probe-forge-worker-data}"
export PROBE_FORGE_DATA_DISK_SIZE_GB="${PROBE_FORGE_DATA_DISK_SIZE_GB:-200}"
export PROBE_FORGE_DATA_DISK_TYPE="${PROBE_FORGE_DATA_DISK_TYPE:-pd-ssd}"

export PROBE_FORGE_REMOTE_USER="${PROBE_FORGE_REMOTE_USER:-probe-worker}"
export PROBE_FORGE_PROBE_HOME="${PROBE_FORGE_PROBE_HOME:-/var/lib/probe-worker}"
export PROBE_FORGE_REMOTE_SOURCE_DIR="${PROBE_FORGE_REMOTE_SOURCE_DIR:-/opt/probe-src}"
export PROBE_FORGE_REMOTE_BINARY="${PROBE_FORGE_REMOTE_BINARY:-/usr/local/bin/probe}"
export PROBE_FORGE_REMOTE_LAUNCHER="${PROBE_FORGE_REMOTE_LAUNCHER:-/usr/local/bin/probe-forge-worker}"
export PROBE_FORGE_REMOTE_ENV_FILE="${PROBE_FORGE_REMOTE_ENV_FILE:-/etc/probe-forge-worker.env}"
export PROBE_FORGE_REMOTE_SERVICE_UNIT="${PROBE_FORGE_REMOTE_SERVICE_UNIT:-/etc/systemd/system/probe-forge-worker.service}"
export PROBE_FORGE_SERVICE_NAME="${PROBE_FORGE_SERVICE_NAME:-probe-forge-worker.service}"

export PROBE_FORGE_LOCAL_AUTH_FILE="${PROBE_FORGE_LOCAL_AUTH_FILE:-${HOME}/.probe/auth/openai-codex.json}"
export PROBE_FORGE_LOCAL_SERVER_CONFIG_FILE="${PROBE_FORGE_LOCAL_SERVER_CONFIG_FILE:-${HOME}/.probe/server/openai-codex-subscription.json}"

export PROBE_FORGE_BASE_URL="${PROBE_FORGE_BASE_URL:-}"
export PROBE_FORGE_WORKER_ID="${PROBE_FORGE_WORKER_ID:-}"
export PROBE_FORGE_BOOTSTRAP_TOKEN="${PROBE_FORGE_BOOTSTRAP_TOKEN:-}"
export PROBE_FORGE_PROFILE="${PROBE_FORGE_PROFILE:-openai-codex-subscription}"
export PROBE_FORGE_WORKSPACE_ROOT="${PROBE_FORGE_WORKSPACE_ROOT:-${PROBE_FORGE_PROBE_HOME}/workspaces/default}"
export PROBE_FORGE_POLL_INTERVAL_MS="${PROBE_FORGE_POLL_INTERVAL_MS:-1000}"
export PROBE_FORGE_EXIT_ON_IDLE="${PROBE_FORGE_EXIT_ON_IDLE:-false}"
export PROBE_FORGE_MAX_ITERATIONS="${PROBE_FORGE_MAX_ITERATIONS:-}"
export PROBE_FORGE_SERVER_MODE="${PROBE_FORGE_SERVER_MODE:-attach}"
export PROBE_FORGE_SERVER_HOST="${PROBE_FORGE_SERVER_HOST:-}"
export PROBE_FORGE_SERVER_PORT="${PROBE_FORGE_SERVER_PORT:-}"
export PROBE_FORGE_SERVER_MODEL_ID="${PROBE_FORGE_SERVER_MODEL_ID:-}"
export PROBE_FORGE_SERVER_MODEL_PATH="${PROBE_FORGE_SERVER_MODEL_PATH:-}"
export PROBE_FORGE_HOSTNAME_OVERRIDE="${PROBE_FORGE_HOSTNAME_OVERRIDE:-}"
export PROBE_FORGE_ATTACHMENT_METADATA_JSON="${PROBE_FORGE_ATTACHMENT_METADATA_JSON:-}"

log() {
  printf '[probe-forge-worker] %s\n' "$*" >&2
}

die() {
  printf '[probe-forge-worker] ERROR: %s\n' "$*" >&2
  exit 1
}

require_cmd() {
  local cmd="$1"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    die "Missing required command: ${cmd}"
  fi
}

ensure_gcloud_context() {
  require_cmd gcloud

  local active_project active_account
  active_project="$(gcloud config get-value project 2>/dev/null || true)"
  active_account="$(gcloud config get-value account 2>/dev/null || true)"

  [[ -n "$active_account" ]] || die "No active gcloud account. Run: gcloud auth login"

  if [[ "$active_project" != "$GCP_PROJECT" ]]; then
    log "Switching gcloud project from '${active_project}' to '${GCP_PROJECT}'"
    gcloud config set project "$GCP_PROJECT" >/dev/null
  fi

  gcloud config set compute/region "$GCP_REGION" >/dev/null
  gcloud config set compute/zone "$GCP_ZONE" >/dev/null
}

ensure_services() {
  gcloud services enable \
    compute.googleapis.com \
    iap.googleapis.com \
    logging.googleapis.com \
    monitoring.googleapis.com \
    --project "$GCP_PROJECT" >/dev/null
}

instance_exists() {
  gcloud compute instances describe "$1" --project "$GCP_PROJECT" --zone "$GCP_ZONE" >/dev/null 2>&1
}

disk_exists() {
  gcloud compute disks describe "$1" --project "$GCP_PROJECT" --zone "$GCP_ZONE" >/dev/null 2>&1
}

firewall_rule_exists() {
  gcloud compute firewall-rules describe "$1" --project "$GCP_PROJECT" >/dev/null 2>&1
}

remote_ssh() {
  gcloud compute ssh "$PROBE_FORGE_VM" \
    --tunnel-through-iap \
    --project "$GCP_PROJECT" \
    --zone "$GCP_ZONE" \
    --command "$1"
}

remote_scp_to() {
  local source="$1"
  local dest="$2"
  gcloud compute scp --tunnel-through-iap \
    --project "$GCP_PROJECT" \
    --zone "$GCP_ZONE" \
    "$source" "${PROBE_FORGE_VM}:${dest}"
}

write_env_line() {
  local file="$1"
  local key="$2"
  local value="$3"
  printf '%s=%q\n' "$key" "$value" >>"$file"
}

write_worker_env_file() {
  local file="$1"
  : >"$file"
  {
    printf '# Probe Forge worker environment\n'
    printf '# Codex subscription auth lives at: %s/auth/openai-codex.json\n' "$PROBE_FORGE_PROBE_HOME"
    printf '# Leave PROBE_OPENAI_API_KEY empty for the Codex subscription lane.\n'
    printf '# Set PROBE_FORGE_BOOTSTRAP_TOKEN only for the initial attach or a later refresh.\n'
  } >>"$file"
  write_env_line "$file" "HOME" "$PROBE_FORGE_PROBE_HOME"
  write_env_line "$file" "PROBE_HOME" "$PROBE_FORGE_PROBE_HOME"
  write_env_line "$file" "PROBE_BINARY" "$PROBE_FORGE_REMOTE_BINARY"
  write_env_line "$file" "PROBE_FORGE_BASE_URL" "$PROBE_FORGE_BASE_URL"
  write_env_line "$file" "PROBE_FORGE_WORKER_ID" "$PROBE_FORGE_WORKER_ID"
  write_env_line "$file" "PROBE_FORGE_BOOTSTRAP_TOKEN" "$PROBE_FORGE_BOOTSTRAP_TOKEN"
  write_env_line "$file" "PROBE_FORGE_PROFILE" "$PROBE_FORGE_PROFILE"
  write_env_line "$file" "PROBE_FORGE_WORKSPACE_ROOT" "$PROBE_FORGE_WORKSPACE_ROOT"
  write_env_line "$file" "PROBE_FORGE_POLL_INTERVAL_MS" "$PROBE_FORGE_POLL_INTERVAL_MS"
  write_env_line "$file" "PROBE_FORGE_EXIT_ON_IDLE" "$PROBE_FORGE_EXIT_ON_IDLE"
  write_env_line "$file" "PROBE_FORGE_MAX_ITERATIONS" "$PROBE_FORGE_MAX_ITERATIONS"
  write_env_line "$file" "PROBE_FORGE_SERVER_MODE" "$PROBE_FORGE_SERVER_MODE"
  write_env_line "$file" "PROBE_FORGE_SERVER_HOST" "$PROBE_FORGE_SERVER_HOST"
  write_env_line "$file" "PROBE_FORGE_SERVER_PORT" "$PROBE_FORGE_SERVER_PORT"
  write_env_line "$file" "PROBE_FORGE_SERVER_MODEL_ID" "$PROBE_FORGE_SERVER_MODEL_ID"
  write_env_line "$file" "PROBE_FORGE_SERVER_MODEL_PATH" "$PROBE_FORGE_SERVER_MODEL_PATH"
  write_env_line "$file" "PROBE_FORGE_HOSTNAME_OVERRIDE" "$PROBE_FORGE_HOSTNAME_OVERRIDE"
  write_env_line "$file" "PROBE_FORGE_ATTACHMENT_METADATA_JSON" "$PROBE_FORGE_ATTACHMENT_METADATA_JSON"
  write_env_line "$file" "PROBE_OPENAI_API_KEY" "${PROBE_OPENAI_API_KEY:-}"
}

