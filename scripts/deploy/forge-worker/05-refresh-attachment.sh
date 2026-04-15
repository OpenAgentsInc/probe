#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

require_cmd gcloud
ensure_gcloud_context
instance_exists "$PROBE_FORGE_VM" || die "VM does not exist: ${PROBE_FORGE_VM}. Run 01-provision-baseline.sh first."
[[ -n "${PROBE_FORGE_BOOTSTRAP_TOKEN}" ]] || die "Set PROBE_FORGE_BOOTSTRAP_TOKEN to a fresh Forge bootstrap credential before running this script."

TMP_ENV_FILE="$(mktemp -t probe-forge-worker-refresh-env.XXXXXX)"
trap 'rm -f "$TMP_ENV_FILE"' EXIT
write_worker_env_file "$TMP_ENV_FILE"
remote_scp_to "$TMP_ENV_FILE" "/tmp/probe-forge-worker.env"

remote_ssh "sudo install -m 0600 /tmp/probe-forge-worker.env '${PROBE_FORGE_REMOTE_ENV_FILE}' && \
  sudo rm -f '${PROBE_FORGE_PROBE_HOME}/auth/forge-worker.json' && \
  sudo systemctl restart '${PROBE_FORGE_SERVICE_NAME}' && \
  sudo systemctl --no-pager --full status '${PROBE_FORGE_SERVICE_NAME}' | sed -n '1,80p'"

