#!/usr/bin/env bash
set -euo pipefail

log() {
  printf '[probe-forge-worker] %s\n' "$*" >&2
}

die() {
  printf '[probe-forge-worker] ERROR: %s\n' "$*" >&2
  exit 1
}

PROBE_BINARY="${PROBE_BINARY:-/usr/local/bin/probe}"
PROBE_HOME="${PROBE_HOME:-${HOME:-/var/lib/probe-worker}}"
PROBE_FORGE_PROFILE="${PROBE_FORGE_PROFILE:-openai-codex-subscription}"
PROBE_FORGE_WORKSPACE_ROOT="${PROBE_FORGE_WORKSPACE_ROOT:-${PROBE_HOME}/workspaces/default}"
PROBE_FORGE_POLL_INTERVAL_MS="${PROBE_FORGE_POLL_INTERVAL_MS:-1000}"
PROBE_FORGE_EXIT_ON_IDLE="${PROBE_FORGE_EXIT_ON_IDLE:-false}"
PROBE_FORGE_MAX_ITERATIONS="${PROBE_FORGE_MAX_ITERATIONS:-}"
PROBE_FORGE_SERVER_MODE="${PROBE_FORGE_SERVER_MODE:-attach}"
PROBE_FORGE_HOSTNAME_OVERRIDE="${PROBE_FORGE_HOSTNAME_OVERRIDE:-$(hostname -f 2>/dev/null || hostname)}"
PROBE_FORGE_ATTACHMENT_METADATA_JSON="${PROBE_FORGE_ATTACHMENT_METADATA_JSON:-}"

[[ -x "$PROBE_BINARY" ]] || die "missing probe binary: ${PROBE_BINARY}"
mkdir -p "$PROBE_HOME/auth" "$PROBE_HOME/server" "$PROBE_FORGE_WORKSPACE_ROOT"

if [[ "$PROBE_FORGE_PROFILE" == "openai-codex-subscription" ]] && [[ ! -f "$PROBE_HOME/auth/openai-codex.json" ]]; then
  die "missing Codex auth at ${PROBE_HOME}/auth/openai-codex.json; run \`${PROBE_BINARY} codex login --method headless --probe-home ${PROBE_HOME}\`"
fi

CONTEXT_LOG="$(mktemp -t probe-forge-context.XXXXXX)"
trap 'rm -f "$CONTEXT_LOG"' EXIT

attached=false
if "$PROBE_BINARY" forge context --probe-home "$PROBE_HOME" >"$CONTEXT_LOG" 2>&1; then
  if grep -q 'attached=true' "$CONTEXT_LOG"; then
    attached=true
  fi
fi

if [[ "$attached" != "true" ]]; then
  if [[ -z "${PROBE_FORGE_BASE_URL:-}" || -z "${PROBE_FORGE_WORKER_ID:-}" || -z "${PROBE_FORGE_BOOTSTRAP_TOKEN:-}" ]]; then
    cat "$CONTEXT_LOG" >&2 || true
    die "Forge worker session is missing or expired and no bootstrap configuration is present"
  fi

  "$PROBE_BINARY" forge detach --probe-home "$PROBE_HOME" >/dev/null 2>&1 || true

  if [[ -z "$PROBE_FORGE_ATTACHMENT_METADATA_JSON" ]]; then
    PROBE_FORGE_ATTACHMENT_METADATA_JSON="$(printf '{"deploy_lane":"forge-worker","profile":"%s","workspace_root":"%s","hostname":"%s"}' "$PROBE_FORGE_PROFILE" "$PROBE_FORGE_WORKSPACE_ROOT" "$PROBE_FORGE_HOSTNAME_OVERRIDE")"
  fi

  log "attaching_worker=true"
  "$PROBE_BINARY" forge attach \
    --probe-home "$PROBE_HOME" \
    --forge-base-url "$PROBE_FORGE_BASE_URL" \
    --worker-id "$PROBE_FORGE_WORKER_ID" \
    --bootstrap-token "$PROBE_FORGE_BOOTSTRAP_TOKEN" \
    --hostname "$PROBE_FORGE_HOSTNAME_OVERRIDE" \
    --attachment-metadata-json "$PROBE_FORGE_ATTACHMENT_METADATA_JSON"
fi

args=(
  forge
  run-loop
  --probe-home "$PROBE_HOME"
  --profile "$PROBE_FORGE_PROFILE"
  --cwd "$PROBE_FORGE_WORKSPACE_ROOT"
  --poll-interval-ms "$PROBE_FORGE_POLL_INTERVAL_MS"
)

if [[ -n "${PROBE_FORGE_MAX_ITERATIONS}" ]]; then
  args+=(--max-iterations "$PROBE_FORGE_MAX_ITERATIONS")
fi
if [[ "$PROBE_FORGE_EXIT_ON_IDLE" == "true" ]]; then
  args+=(--exit-on-idle)
fi
if [[ -n "${PROBE_FORGE_SERVER_HOST:-}" ]]; then
  args+=(--server-host "$PROBE_FORGE_SERVER_HOST")
fi
if [[ -n "${PROBE_FORGE_SERVER_PORT:-}" ]]; then
  args+=(--server-port "$PROBE_FORGE_SERVER_PORT")
fi
if [[ -n "${PROBE_FORGE_SERVER_MODE:-}" ]]; then
  args+=(--server-mode "$PROBE_FORGE_SERVER_MODE")
fi
if [[ -n "${PROBE_FORGE_SERVER_MODEL_ID:-}" ]]; then
  args+=(--server-model-id "$PROBE_FORGE_SERVER_MODEL_ID")
fi
if [[ -n "${PROBE_FORGE_SERVER_MODEL_PATH:-}" ]]; then
  args+=(--server-model-path "$PROBE_FORGE_SERVER_MODEL_PATH")
fi

exec "$PROBE_BINARY" "${args[@]}"
