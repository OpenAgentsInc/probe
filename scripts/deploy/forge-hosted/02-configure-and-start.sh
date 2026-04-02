#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

require_cmd gcloud
require_cmd tar

ensure_gcloud_context

instance_exists "$PROBE_FORGE_VM" || die "VM does not exist: ${PROBE_FORGE_VM}. Run 01-provision-baseline.sh first."
[[ -f "$PROBE_FORGE_LOCAL_AUTH_FILE" ]] || die "Missing local Probe auth file: ${PROBE_FORGE_LOCAL_AUTH_FILE}"

TMP_PROBE_SOURCE="$(mktemp -t probe-forge-src.XXXXXX.tar.gz)"
TMP_REMOTE_SCRIPT="$(mktemp -t probe-forge-remote.XXXXXX.sh)"
trap 'rm -f "$TMP_PROBE_SOURCE" "$TMP_REMOTE_SCRIPT"' EXIT

COPYFILE_DISABLE=1 tar \
  --exclude='.git' \
  --exclude='target' \
  --exclude='.DS_Store' \
  -czf "$TMP_PROBE_SOURCE" \
  -C "$ROOT_DIR" .

cat >"$TMP_REMOTE_SCRIPT" <<'REMOTE'
#!/usr/bin/env bash
set -euo pipefail

PROBE_HOME="$1"
DATA_DISK_DEVICE_NAME="$2"
SOURCE_DIR="$3"
REMOTE_BINARY="$4"
LISTEN_ADDR="$5"
OWNER_ID="$6"
DISPLAY_NAME="$7"
AUTH_AUTHORITY="$8"
AUTH_SUBJECT="$9"
AUTH_KIND="${10}"
AUTH_SCOPE="${11}"
WATCHDOG_POLL_MS="${12}"
WATCHDOG_STALL_MS="${13}"
WATCHDOG_TIMEOUT_MS="${14}"
AUTH_FILE_SOURCE="${15}"
SERVER_CONFIG_SOURCE="${16}"
SOURCE_TARBALL="${17}"

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

if ! id -u probe-hosted >/dev/null 2>&1; then
  sudo useradd --system --create-home --home-dir /home/probe-hosted --shell /bin/bash probe-hosted
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
cargo build --release -p probe-server
popd >/dev/null

sudo install -m 0755 "$SOURCE_DIR/target/release/probe-server" "$REMOTE_BINARY"

sudo mkdir -p \
  "$PROBE_HOME/auth" \
  "$PROBE_HOME/server" \
  "$PROBE_HOME/hosted/baselines" \
  "$PROBE_HOME/hosted/workspaces"

sudo install -m 0600 "$AUTH_FILE_SOURCE" "$PROBE_HOME/auth/openai-codex.json"
if [[ -f "$SERVER_CONFIG_SOURCE" ]]; then
  sudo install -m 0600 "$SERVER_CONFIG_SOURCE" "$PROBE_HOME/server/openai-codex-subscription.json"
fi
sudo chown -R probe-hosted:probe-hosted "$PROBE_HOME"

sudo tee /etc/systemd/system/probe-hosted.service >/dev/null <<UNIT
[Unit]
Description=Probe hosted Forge dogfood server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=probe-hosted
Group=probe-hosted
Environment=HOME=${PROBE_HOME}
Environment=PROBE_HOME=${PROBE_HOME}
WorkingDirectory=${PROBE_HOME}
ExecStart=${REMOTE_BINARY} --probe-home ${PROBE_HOME} --listen-tcp ${LISTEN_ADDR} --hosted-owner-id ${OWNER_ID} --hosted-display-name "${DISPLAY_NAME}" --hosted-auth-authority ${AUTH_AUTHORITY} --hosted-auth-subject ${AUTH_SUBJECT} --hosted-auth-kind ${AUTH_KIND} --hosted-auth-scope ${AUTH_SCOPE} --watchdog-poll-ms ${WATCHDOG_POLL_MS} --watchdog-stall-ms ${WATCHDOG_STALL_MS} --watchdog-timeout-ms ${WATCHDOG_TIMEOUT_MS}
Restart=always
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full
ProtectHome=false
ReadWritePaths=${PROBE_HOME}
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
UNIT

sudo systemctl daemon-reload
sudo systemctl enable probe-hosted
sudo systemctl restart probe-hosted
sudo systemctl --no-pager --full status probe-hosted | sed -n '1,60p'
REMOTE

chmod +x "$TMP_REMOTE_SCRIPT"

remote_scp_to "$TMP_PROBE_SOURCE" "/tmp/probe-forge-src.tar.gz"
remote_scp_to "$TMP_REMOTE_SCRIPT" "/tmp/probe-forge-configure.sh"
remote_scp_to "$PROBE_FORGE_LOCAL_AUTH_FILE" "/tmp/openai-codex.json"
if [[ -f "$PROBE_FORGE_LOCAL_SERVER_CONFIG_FILE" ]]; then
  remote_scp_to "$PROBE_FORGE_LOCAL_SERVER_CONFIG_FILE" "/tmp/openai-codex-subscription.json"
fi

remote_ssh "chmod +x /tmp/probe-forge-configure.sh && /tmp/probe-forge-configure.sh \
  '$PROBE_FORGE_PROBE_HOME' \
  '$PROBE_FORGE_DATA_DISK_DEVICE_NAME' \
  '$PROBE_FORGE_REMOTE_SOURCE_DIR' \
  '$PROBE_FORGE_REMOTE_BINARY' \
  '$PROBE_FORGE_LISTEN_ADDR' \
  '$PROBE_FORGE_OWNER_ID' \
  '$PROBE_FORGE_DISPLAY_NAME' \
  '$PROBE_FORGE_AUTH_AUTHORITY' \
  '$PROBE_FORGE_AUTH_SUBJECT' \
  '$PROBE_FORGE_AUTH_KIND' \
  '$PROBE_FORGE_AUTH_SCOPE' \
  '$PROBE_FORGE_WATCHDOG_POLL_MS' \
  '$PROBE_FORGE_WATCHDOG_STALL_MS' \
  '$PROBE_FORGE_WATCHDOG_TIMEOUT_MS' \
  '/tmp/openai-codex.json' \
  '/tmp/openai-codex-subscription.json' \
  '/tmp/probe-forge-src.tar.gz'"

log "Probe hosted server configured on ${PROBE_FORGE_VM}"
log "listen_addr=${PROBE_FORGE_LISTEN_ADDR}"
