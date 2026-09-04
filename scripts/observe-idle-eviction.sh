#!/usr/bin/env bash
# Leave the daemon idle past its timeout and confirm exit + GPU release.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STORE_PATH="${1:?usage: $0 <store-path>}"
REPO_PATH="${2:-.}"
MODEL_PATH="${3:-.}"
IDLE_SECS="${IDLE_SECS:-5}"

cargo build --release -p daemon --bin sift-daemon

SOCKET="$(mktemp -u "${TMPDIR:-/tmp}/sift-idle.XXXXXX.sock")"
cleanup() {
  rm -f "$SOCKET" "${SOCKET}.lock" 2>/dev/null || true
}
trap cleanup EXIT

./target/release/sift-daemon \
  --store "$STORE_PATH" \
  --repo "$REPO_PATH" \
  --model "$MODEL_PATH" \
  --socket "$SOCKET" \
  --idle-secs "$IDLE_SECS" &
DAEMON_PID=$!
echo "daemon_pid=$DAEMON_PID idle_secs=$IDLE_SECS"

for _ in $(seq 1 600); do
  [[ -S "$SOCKET" ]] && break
  sleep 0.05
done

# No clients; wait for idle exit.
deadline=$((SECONDS + IDLE_SECS + 30))
while kill -0 "$DAEMON_PID" 2>/dev/null; do
  if (( SECONDS > deadline )); then
    echo "daemon did not exit after idle"
    kill "$DAEMON_PID" 2>/dev/null || true
    exit 1
  fi
  sleep 0.5
done
wait "$DAEMON_PID" 2>/dev/null || true
echo "daemon_exited=1"
if [[ -S "$SOCKET" ]]; then
  echo "warning: socket still present"
else
  echo "socket_unlinked=1"
fi

if command -v nvidia-smi >/dev/null; then
  nvidia-smi --query-compute-apps=pid,used_gpu_memory --format=csv,noheader || true
fi
