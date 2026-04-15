#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

require_cmd gcloud
ensure_gcloud_context
instance_exists "$PROBE_FORGE_VM" || die "VM does not exist: ${PROBE_FORGE_VM}. Run 01-provision-baseline.sh first."

remote_ssh "sudo systemctl restart '${PROBE_FORGE_SERVICE_NAME}' && \
  sudo systemctl --no-pager --full status '${PROBE_FORGE_SERVICE_NAME}' | sed -n '1,80p' && \
  sudo -u '${PROBE_FORGE_REMOTE_USER}' env HOME='${PROBE_FORGE_PROBE_HOME}' PROBE_HOME='${PROBE_FORGE_PROBE_HOME}' '${PROBE_FORGE_REMOTE_BINARY}' forge status --probe-home '${PROBE_FORGE_PROBE_HOME}'"

