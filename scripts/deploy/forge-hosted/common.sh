#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

export GCP_PROJECT="${GCP_PROJECT:-openagentsgemini}"
export GCP_REGION="${GCP_REGION:-us-central1}"
export GCP_ZONE="${GCP_ZONE:-us-central1-a}"

export OA_VPC="${OA_VPC:-oa-lightning}"
export OA_SUBNET="${OA_SUBNET:-oa-lightning-us-central1}"

export PROBE_FORGE_VM="${PROBE_FORGE_VM:-probe-hosted-forge-1}"
export PROBE_FORGE_MACHINE_TYPE="${PROBE_FORGE_MACHINE_TYPE:-e2-standard-8}"
export PROBE_FORGE_SERVICE_ACCOUNT_NAME="${PROBE_FORGE_SERVICE_ACCOUNT_NAME:-probe-hosted-forge}"
export PROBE_FORGE_SERVICE_ACCOUNT_EMAIL="${PROBE_FORGE_SERVICE_ACCOUNT_EMAIL:-${PROBE_FORGE_SERVICE_ACCOUNT_NAME}@${GCP_PROJECT}.iam.gserviceaccount.com}"
export PROBE_FORGE_TAG="${PROBE_FORGE_TAG:-probe-hosted-forge}"

export PROBE_FORGE_DATA_DISK="${PROBE_FORGE_DATA_DISK:-probe-hosted-forge-data}"
export PROBE_FORGE_DATA_DISK_DEVICE_NAME="${PROBE_FORGE_DATA_DISK_DEVICE_NAME:-probe-hosted-data}"
export PROBE_FORGE_DATA_DISK_SIZE_GB="${PROBE_FORGE_DATA_DISK_SIZE_GB:-200}"
export PROBE_FORGE_DATA_DISK_TYPE="${PROBE_FORGE_DATA_DISK_TYPE:-pd-ssd}"

export PROBE_FORGE_PROBE_HOME="${PROBE_FORGE_PROBE_HOME:-/var/lib/probe-hosted}"
export PROBE_FORGE_REMOTE_SOURCE_DIR="${PROBE_FORGE_REMOTE_SOURCE_DIR:-/opt/probe-src}"
export PROBE_FORGE_REMOTE_BINARY="${PROBE_FORGE_REMOTE_BINARY:-/usr/local/bin/probe-server}"
export PROBE_FORGE_LISTEN_ADDR="${PROBE_FORGE_LISTEN_ADDR:-127.0.0.1:7777}"
export PROBE_FORGE_LOCAL_TUNNEL_PORT="${PROBE_FORGE_LOCAL_TUNNEL_PORT:-17777}"
export PROBE_FORGE_BASELINE_ID="${PROBE_FORGE_BASELINE_ID:-forge-openagents-main}"
export PROBE_FORGE_OWNER_ID="${PROBE_FORGE_OWNER_ID:-probe-hosted-gcp-forge}"
export PROBE_FORGE_DISPLAY_NAME="${PROBE_FORGE_DISPLAY_NAME:-Probe Hosted GCP Forge}"
export PROBE_FORGE_AUTH_AUTHORITY="${PROBE_FORGE_AUTH_AUTHORITY:-gcp://${GCP_PROJECT}/${GCP_ZONE}/${PROBE_FORGE_VM}}"
export PROBE_FORGE_AUTH_SUBJECT="${PROBE_FORGE_AUTH_SUBJECT:-forge-dogfood}"
export PROBE_FORGE_AUTH_KIND="${PROBE_FORGE_AUTH_KIND:-operator_token}"
export PROBE_FORGE_AUTH_SCOPE="${PROBE_FORGE_AUTH_SCOPE:-probe.hosted.session}"
export PROBE_FORGE_WATCHDOG_POLL_MS="${PROBE_FORGE_WATCHDOG_POLL_MS:-500}"
export PROBE_FORGE_WATCHDOG_STALL_MS="${PROBE_FORGE_WATCHDOG_STALL_MS:-180000}"
export PROBE_FORGE_WATCHDOG_TIMEOUT_MS="${PROBE_FORGE_WATCHDOG_TIMEOUT_MS:-300000}"

export PROBE_FORGE_LOCAL_AUTH_FILE="${PROBE_FORGE_LOCAL_AUTH_FILE:-${HOME}/.probe/auth/openai-codex.json}"
export PROBE_FORGE_LOCAL_SERVER_CONFIG_FILE="${PROBE_FORGE_LOCAL_SERVER_CONFIG_FILE:-${HOME}/.probe/server/openai-codex-subscription.json}"

export OPENAGENTS_REPO_URL="${OPENAGENTS_REPO_URL:-https://github.com/OpenAgentsInc/openagents.git}"
export OPENAGENTS_REF="${OPENAGENTS_REF:-origin/main}"
export PROBE_FORGE_REMOTE_WORKSPACE_ROOT="${PROBE_FORGE_REMOTE_WORKSPACE_ROOT:-${PROBE_FORGE_PROBE_HOME}/hosted/workspaces/${PROBE_FORGE_BASELINE_ID}}"
export PROBE_FORGE_REMOTE_OPENAGENTS_DIR="${PROBE_FORGE_REMOTE_OPENAGENTS_DIR:-${PROBE_FORGE_REMOTE_WORKSPACE_ROOT}/openagents}"

log() {
  printf '[probe-forge-hosted] %s\n' "$*" >&2
}

die() {
  printf '[probe-forge-hosted] ERROR: %s\n' "$*" >&2
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

normalize_github_clone_url() {
  local url="$1"
  if [[ "$url" =~ ^git@github.com:(.+)$ ]]; then
    printf 'https://github.com/%s\n' "${BASH_REMATCH[1]}"
    return 0
  fi
  if [[ "$url" =~ ^ssh://git@github.com/(.+)$ ]]; then
    printf 'https://github.com/%s\n' "${BASH_REMATCH[1]}"
    return 0
  fi
  printf '%s\n' "$url"
}

resolve_openagents_checkout_defaults() {
  local sibling_dir="${ROOT_DIR}/../openagents"
  if [[ -d "${sibling_dir}/.git" ]]; then
    OPENAGENTS_REPO_URL="$(normalize_github_clone_url "$(git -C "$sibling_dir" remote get-url origin)")"
    OPENAGENTS_REF="$(git -C "$sibling_dir" rev-parse HEAD)"
  fi
  export OPENAGENTS_REPO_URL
  export OPENAGENTS_REF
}
