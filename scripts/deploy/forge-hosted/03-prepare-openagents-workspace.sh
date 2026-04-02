#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

require_cmd gcloud

ensure_gcloud_context
resolve_openagents_checkout_defaults

instance_exists "$PROBE_FORGE_VM" || die "VM does not exist: ${PROBE_FORGE_VM}. Run 01-provision-baseline.sh first."

TMP_REMOTE_SCRIPT="$(mktemp -t probe-forge-workspace.XXXXXX.sh)"
trap 'rm -f "$TMP_REMOTE_SCRIPT"' EXIT

cat >"$TMP_REMOTE_SCRIPT" <<'REMOTE'
#!/usr/bin/env bash
set -euo pipefail

PROBE_HOME="$1"
BASELINE_ID="$2"
WORKSPACE_ROOT="$3"
OPENAGENTS_DIR="$4"
REPO_URL="$5"
REPO_REF="$6"

sudo mkdir -p "$PROBE_HOME/hosted/baselines" "$WORKSPACE_ROOT"
sudo chown -R probe-hosted:probe-hosted "$PROBE_HOME/hosted"

if [[ ! -d "$OPENAGENTS_DIR/.git" ]]; then
  sudo -u probe-hosted git clone "$REPO_URL" "$OPENAGENTS_DIR"
fi

pushd "$OPENAGENTS_DIR" >/dev/null
sudo -u probe-hosted git fetch --all --tags --prune
sudo -u probe-hosted git checkout --detach "$REPO_REF"
sudo -u probe-hosted git reset --hard "$REPO_REF"
popd >/dev/null

HEAD_REF="$(sudo -u probe-hosted git -C "$OPENAGENTS_DIR" rev-parse --abbrev-ref HEAD || true)"
if [[ "$HEAD_REF" == "HEAD" || -z "$HEAD_REF" ]]; then
  HEAD_REF="$(sudo -u probe-hosted git -C "$OPENAGENTS_DIR" rev-parse HEAD)"
fi

sudo tee "$PROBE_HOME/hosted/baselines/${BASELINE_ID}.json" >/dev/null <<JSON
{
  "baseline_id": "${BASELINE_ID}",
  "repo_identity": "${REPO_URL}",
  "base_ref": "${HEAD_REF}",
  "stale": false
}
JSON
sudo chown probe-hosted:probe-hosted "$PROBE_HOME/hosted/baselines/${BASELINE_ID}.json"

printf 'remote_workspace=%s\n' "$OPENAGENTS_DIR"
printf 'baseline_manifest=%s\n' "$PROBE_HOME/hosted/baselines/${BASELINE_ID}.json"
printf 'head_ref=%s\n' "$HEAD_REF"
REMOTE

chmod +x "$TMP_REMOTE_SCRIPT"
remote_scp_to "$TMP_REMOTE_SCRIPT" "/tmp/probe-forge-workspace.sh"
remote_ssh "chmod +x /tmp/probe-forge-workspace.sh && /tmp/probe-forge-workspace.sh \
  '$PROBE_FORGE_PROBE_HOME' \
  '$PROBE_FORGE_BASELINE_ID' \
  '$PROBE_FORGE_REMOTE_WORKSPACE_ROOT' \
  '$PROBE_FORGE_REMOTE_OPENAGENTS_DIR' \
  '$OPENAGENTS_REPO_URL' \
  '$OPENAGENTS_REF'"
