#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

require_cmd() {
  local cmd="$1"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    printf '[probe-forge-worker-local-smoke] ERROR: missing required command: %s\n' "$cmd" >&2
    exit 1
  fi
}

require_cmd python3

PROBE_BINARY="${PROBE_BINARY:-${ROOT_DIR}/target/debug/probe-cli}"
if [[ ! -x "$PROBE_BINARY" ]]; then
  cargo build -p probe-cli --manifest-path "${ROOT_DIR}/Cargo.toml" >/dev/null
fi

TMP_DIR="$(mktemp -d -t probe-forge-worker-local.XXXXXX)"
trap 'if [[ -n "${SERVER_PID:-}" ]]; then kill "${SERVER_PID}" >/dev/null 2>&1 || true; wait "${SERVER_PID}" >/dev/null 2>&1 || true; fi; rm -rf "${TMP_DIR}"' EXIT

PROBE_HOME="${TMP_DIR}/probe-home"
WORKSPACE_ROOT="${TMP_DIR}/workspace"
REQUEST_LOG="${TMP_DIR}/requests.log"
PORT_FILE="${TMP_DIR}/port.txt"
mkdir -p "${PROBE_HOME}/auth" "${PROBE_HOME}/server" "${WORKSPACE_ROOT}"

cat >"${PROBE_HOME}/auth/openai-codex.json" <<'JSON'
{
  "refresh": "refresh-token",
  "access": "access-token",
  "expires": "2099-01-01T00:00:00Z",
  "account_id": "acct-smoke"
}
JSON

cat >"${TMP_DIR}/fake_forge.py" <<'PY'
import http.server
import json
import socketserver
import sys

request_log = sys.argv[1]
port_file = sys.argv[2]


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        return

    def _record(self, body):
        with open(request_log, "a", encoding="utf-8") as handle:
            handle.write(f"{self.command} {self.path} {body}\n")

    def _reply(self, payload, status=200):
        raw = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length).decode("utf-8")
        self._record(body)
        if self.path == "/worker/v1/attach":
            self._reply({
                "worker": {
                    "id": "forge-worker-1",
                    "org_id": "org-openagents-internal",
                    "project_id": "project-forge-mvp",
                    "runtime_kind": "probe",
                    "environment_class": "gcp-linux",
                    "state": "attached"
                },
                "session_id": "forge-worker-session-1",
                "session_token": "session-token-1",
                "expires_at": "2099-01-01T00:00:00Z"
            })
            return
        if self.path == "/worker/v1/runs/claim-next":
            self._reply({"request_id": "req-claim", "assignment": None})
            return
        self._reply({"error": "unexpected"}, status=404)

    def do_GET(self):
        self._record("")
        if self.path == "/worker/v1/me":
            self._reply({
                "request_id": "req-context",
                "worker_session": {
                    "worker_id": "forge-worker-1",
                    "org_id": "org-openagents-internal",
                    "project_id": "project-forge-mvp",
                    "runtime_kind": "probe",
                    "environment_class": "gcp-linux",
                    "session_id": "forge-worker-session-1"
                },
                "worker": {
                    "id": "forge-worker-1",
                    "org_id": "org-openagents-internal",
                    "project_id": "project-forge-mvp",
                    "runtime_kind": "probe",
                    "environment_class": "gcp-linux",
                    "state": "attached"
                }
            })
            return
        if self.path == "/worker/v1/runs/current":
            self._reply({"request_id": "req-current", "assignment": None})
            return
        self._reply({"error": "unexpected"}, status=404)


with socketserver.TCPServer(("127.0.0.1", 0), Handler) as server:
    with open(port_file, "w", encoding="utf-8") as handle:
        handle.write(str(server.server_address[1]))
    server.serve_forever()
PY

python3 "${TMP_DIR}/fake_forge.py" "${REQUEST_LOG}" "${PORT_FILE}" &
SERVER_PID=$!

for _ in $(seq 1 50); do
  if [[ -f "${PORT_FILE}" ]]; then
    break
  fi
  sleep 0.05
done
[[ -f "${PORT_FILE}" ]] || { printf '[probe-forge-worker-local-smoke] ERROR: fake Forge server did not start\n' >&2; exit 1; }
PORT="$(cat "${PORT_FILE}")"

export HOME="${PROBE_HOME}"
export PROBE_HOME="${PROBE_HOME}"
export PROBE_BINARY="${PROBE_BINARY}"
export PROBE_FORGE_BASE_URL="http://127.0.0.1:${PORT}"
export PROBE_FORGE_WORKER_ID="forge-worker-1"
export PROBE_FORGE_BOOTSTRAP_TOKEN="bootstrap-token"
export PROBE_FORGE_PROFILE="openai-codex-subscription"
export PROBE_FORGE_WORKSPACE_ROOT="${WORKSPACE_ROOT}"
export PROBE_FORGE_POLL_INTERVAL_MS="1"
export PROBE_FORGE_EXIT_ON_IDLE="true"
export PROBE_FORGE_MAX_ITERATIONS="1"
export PROBE_FORGE_HOSTNAME_OVERRIDE="probe-smoke.local"

"${SCRIPT_DIR}/probe-forge-worker.sh" | tee "${TMP_DIR}/worker-output.log"

grep -q 'POST /worker/v1/attach' "${REQUEST_LOG}"
grep -q 'POST /worker/v1/runs/claim-next' "${REQUEST_LOG}"
grep -q 'loop_completed=true' "${TMP_DIR}/worker-output.log"
grep -q 'exit_reason="idle"' "${TMP_DIR}/worker-output.log"

printf 'local_smoke_passed=true\n'
printf 'request_log=%s\n' "${REQUEST_LOG}"
printf 'probe_home=%s\n' "${PROBE_HOME}"
