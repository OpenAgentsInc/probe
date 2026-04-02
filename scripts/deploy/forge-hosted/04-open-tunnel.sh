#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

require_cmd gcloud

ensure_gcloud_context

log "Opening local tunnel on 127.0.0.1:${PROBE_FORGE_LOCAL_TUNNEL_PORT} -> ${PROBE_FORGE_VM}:${PROBE_FORGE_LISTEN_ADDR}"
exec gcloud compute ssh "$PROBE_FORGE_VM" \
  --tunnel-through-iap \
  --project "$GCP_PROJECT" \
  --zone "$GCP_ZONE" \
  -- -N -L "${PROBE_FORGE_LOCAL_TUNNEL_PORT}:${PROBE_FORGE_LISTEN_ADDR}"
