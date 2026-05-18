#!/usr/bin/env bash
set -euo pipefail

# Configure these for your server.
SSH_HOST="193.40.157.136"
SSH_PORT="22"
SSH_USER="ubuntu"
SSH_KEY="/home/d125709f573075/.ssh/id_ed25519"
REMOTE_PG_HOST="127.0.0.1"
REMOTE_PG_PORT="5432"
LOCAL_FORWARD_PORT="15432"
CONTROL_SOCKET="/tmp/cm_pg_tunnel.sock"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PHP_BIN="$(command -v php)"

if [[ -z "${PHP_BIN}" ]]; then
  echo "php binary not found" >&2
  exit 1
fi

cleanup() {
  ssh -S "${CONTROL_SOCKET}" -O exit -p "${SSH_PORT}" "${SSH_USER}@${SSH_HOST}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

ssh -M -S "${CONTROL_SOCKET}" -fnNT \
  -i "${SSH_KEY}" \
  -o ExitOnForwardFailure=yes \
  -o StrictHostKeyChecking=accept-new \
  -p "${SSH_PORT}" \
  -L "${LOCAL_FORWARD_PORT}:${REMOTE_PG_HOST}:${REMOTE_PG_PORT}" \
  "${SSH_USER}@${SSH_HOST}"

PG_HOST=127.0.0.1 \
PG_PORT="${LOCAL_FORWARD_PORT}" \
"${PHP_BIN}" "${SCRIPT_DIR}/sync_readings.php"
