#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

require_cmd gcloud
require_cmd tar

ensure_gcloud_context
instance_exists "$PROBE_FORGE_VM" || die "VM does not exist: ${PROBE_FORGE_VM}. Run 01-provision-baseline.sh first."

TMP_PROBE_SOURCE="$(mktemp -t probe-forge-worker-src.XXXXXX.tar.gz)"
TMP_REMOTE_SCRIPT="$(mktemp -t probe-forge-worker-remote.XXXXXX.sh)"
TMP_ENV_FILE="$(mktemp -t probe-forge-worker-env.XXXXXX)"
trap 'rm -f "$TMP_PROBE_SOURCE" "$TMP_REMOTE_SCRIPT" "$TMP_ENV_FILE"' EXIT

COPYFILE_DISABLE=1 tar \
  --exclude='.git' \
  --exclude='target' \
  --exclude='.DS_Store' \
  -czf "$TMP_PROBE_SOURCE" \
  -C "$ROOT_DIR" .

write_worker_env_file "$TMP_ENV_FILE"

HAS_LOCAL_AUTH=0
HAS_LOCAL_SERVER_CONFIG=0
[[ -f "$PROBE_FORGE_LOCAL_AUTH_FILE" ]] && HAS_LOCAL_AUTH=1
[[ -f "$PROBE_FORGE_LOCAL_SERVER_CONFIG_FILE" ]] && HAS_LOCAL_SERVER_CONFIG=1

cat >"$TMP_REMOTE_SCRIPT" <<'REMOTE'
#!/usr/bin/env bash
set -euo pipefail

PROBE_HOME="$1"
DATA_DISK_DEVICE_NAME="$2"
SOURCE_DIR="$3"
PROBE_BINARY="$4"
LAUNCHER_PATH="$5"
ENV_FILE_PATH="$6"
SERVICE_UNIT_PATH="$7"
SERVICE_NAME="$8"
REMOTE_USER="$9"
SOURCE_TARBALL="${10}"
LAUNCHER_SOURCE="${11}"
SERVICE_SOURCE="${12}"
ENV_FILE_SOURCE="${13}"
AUTH_SOURCE="${14}"
SERVER_CONFIG_SOURCE="${15}"
HAS_LOCAL_AUTH="${16}"
HAS_LOCAL_SERVER_CONFIG="${17}"

export DEBIAN_FRONTEND=noninteractive

sudo apt-get update -y
sudo apt-get install -y \
  build-essential \
  pkg-config \
  libssl-dev \
  ca-certificates \
  curl \
  git \
  jq

if ! id -u "$REMOTE_USER" >/dev/null 2>&1; then
  sudo useradd --system --create-home --home-dir "$PROBE_HOME" --shell /bin/bash "$REMOTE_USER"
fi

DATA_DISK_PATH="/dev/disk/by-id/google-${DATA_DISK_DEVICE_NAME}"
if [[ ! -b "$DATA_DISK_PATH" ]]; then
  echo "Could not locate Probe data disk by-id path: ${DATA_DISK_PATH}" >&2
  exit 1
fi

if ! sudo blkid "$DATA_DISK_PATH" >/dev/null 2>&1; then
  sudo mkfs.ext4 -F "$DATA_DISK_PATH"
fi

sudo mkdir -p "$PROBE_HOME"
if ! grep -q "${DATA_DISK_PATH} ${PROBE_HOME} ext4" /etc/fstab; then
  echo "${DATA_DISK_PATH} ${PROBE_HOME} ext4 defaults,nofail 0 2" | sudo tee -a /etc/fstab >/dev/null
fi
sudo mount -a

if [[ ! -x "${HOME}/.cargo/bin/cargo" ]]; then
  curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal --default-toolchain stable
fi
export PATH="${HOME}/.cargo/bin:${PATH}"

sudo rm -rf "$SOURCE_DIR"
sudo mkdir -p "$SOURCE_DIR"
sudo chown "$(id -u)":"$(id -g)" "$SOURCE_DIR"
tar -xzf "$SOURCE_TARBALL" -C "$SOURCE_DIR"

pushd "$SOURCE_DIR" >/dev/null
cargo build --release -p probe-cli
popd >/dev/null

sudo install -m 0755 "$SOURCE_DIR/target/release/probe-cli" "$PROBE_BINARY"
sudo install -m 0755 "$LAUNCHER_SOURCE" "$LAUNCHER_PATH"
sudo install -m 0644 "$SERVICE_SOURCE" "$SERVICE_UNIT_PATH"
sudo install -m 0600 "$ENV_FILE_SOURCE" "$ENV_FILE_PATH"

sudo mkdir -p "$PROBE_HOME/auth" "$PROBE_HOME/server" "$PROBE_HOME/workspaces/default"
if [[ "$HAS_LOCAL_AUTH" == "1" ]]; then
  sudo install -m 0600 "$AUTH_SOURCE" "$PROBE_HOME/auth/openai-codex.json"
fi
if [[ "$HAS_LOCAL_SERVER_CONFIG" == "1" ]]; then
  sudo install -m 0600 "$SERVER_CONFIG_SOURCE" "$PROBE_HOME/server/openai-codex-subscription.json"
fi
sudo chown -R "$REMOTE_USER:$REMOTE_USER" "$PROBE_HOME"

sudo systemctl daemon-reload
sudo systemctl enable "$SERVICE_NAME" >/dev/null

set -a
source "$ENV_FILE_PATH"
set +a

CAN_START=1
if [[ -z "${PROBE_FORGE_BASE_URL:-}" || -z "${PROBE_FORGE_WORKER_ID:-}" ]]; then
  CAN_START=0
fi
if [[ ! -f "$PROBE_HOME/auth/forge-worker.json" && -z "${PROBE_FORGE_BOOTSTRAP_TOKEN:-}" ]]; then
  CAN_START=0
fi
if [[ "${PROBE_FORGE_PROFILE:-openai-codex-subscription}" == "openai-codex-subscription" ]] && [[ ! -f "$PROBE_HOME/auth/openai-codex.json" ]]; then
  CAN_START=0
fi

if [[ "$CAN_START" == "1" ]]; then
  sudo systemctl restart "$SERVICE_NAME"
  sudo systemctl --no-pager --full status "$SERVICE_NAME" | sed -n '1,80p'
else
  sudo systemctl stop "$SERVICE_NAME" >/dev/null 2>&1 || true
  printf 'service_installed_not_started=true\n'
  printf 'env_file=%s\n' "$ENV_FILE_PATH"
  printf 'codex_auth_present=%s\n' "$([[ -f "$PROBE_HOME/auth/openai-codex.json" ]] && echo true || echo false)"
  printf 'forge_session_present=%s\n' "$([[ -f "$PROBE_HOME/auth/forge-worker.json" ]] && echo true || echo false)"
fi
REMOTE

chmod +x "$TMP_REMOTE_SCRIPT"

remote_scp_to "$TMP_PROBE_SOURCE" "/tmp/probe-forge-worker-src.tar.gz"
remote_scp_to "$TMP_REMOTE_SCRIPT" "/tmp/probe-forge-worker-configure.sh"
remote_scp_to "${SCRIPT_DIR}/probe-forge-worker.sh" "/tmp/probe-forge-worker.sh"
remote_scp_to "${SCRIPT_DIR}/probe-forge-worker.service" "/tmp/probe-forge-worker.service"
remote_scp_to "$TMP_ENV_FILE" "/tmp/probe-forge-worker.env"
if [[ "$HAS_LOCAL_AUTH" == "1" ]]; then
  remote_scp_to "$PROBE_FORGE_LOCAL_AUTH_FILE" "/tmp/openai-codex.json"
fi
if [[ "$HAS_LOCAL_SERVER_CONFIG" == "1" ]]; then
  remote_scp_to "$PROBE_FORGE_LOCAL_SERVER_CONFIG_FILE" "/tmp/openai-codex-subscription.json"
fi

remote_ssh "chmod +x /tmp/probe-forge-worker-configure.sh && /tmp/probe-forge-worker-configure.sh \
  '$PROBE_FORGE_PROBE_HOME' \
  '$PROBE_FORGE_DATA_DISK_DEVICE_NAME' \
  '$PROBE_FORGE_REMOTE_SOURCE_DIR' \
  '$PROBE_FORGE_REMOTE_BINARY' \
  '$PROBE_FORGE_REMOTE_LAUNCHER' \
  '$PROBE_FORGE_REMOTE_ENV_FILE' \
  '$PROBE_FORGE_REMOTE_SERVICE_UNIT' \
  '$PROBE_FORGE_SERVICE_NAME' \
  '$PROBE_FORGE_REMOTE_USER' \
  '/tmp/probe-forge-worker-src.tar.gz' \
  '/tmp/probe-forge-worker.sh' \
  '/tmp/probe-forge-worker.service' \
  '/tmp/probe-forge-worker.env' \
  '/tmp/openai-codex.json' \
  '/tmp/openai-codex-subscription.json' \
  '${HAS_LOCAL_AUTH}' \
  '${HAS_LOCAL_SERVER_CONFIG}'"

log "Forge worker deploy lane configured on ${PROBE_FORGE_VM}"
log "service_name=${PROBE_FORGE_SERVICE_NAME}"
log "env_file=${PROBE_FORGE_REMOTE_ENV_FILE}"
log "probe_home=${PROBE_FORGE_PROBE_HOME}"
