#!/usr/bin/env bash
# Index a repository through the daemon while issuing searches throughout.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

REPO_PATH="${1:?usage: $0 <repo-path>}"
STORE_PATH="${2:-$(mktemp -d "${TMPDIR:-/tmp}/sift-under-load.XXXXXX")}"
MODEL_PATH="${3:-.}"

echo "repo=$REPO_PATH store=$STORE_PATH"
cargo build --release -p daemon --bin sift-daemon

SOCKET="$(mktemp -u "${TMPDIR:-/tmp}/sift-load.XXXXXX.sock")"
cleanup() {
  rm -f "$SOCKET" "${SOCKET}.lock" 2>/dev/null || true
  if [[ -n "${DAEMON_PID:-}" ]]; then kill "$DAEMON_PID" 2>/dev/null || true; fi
}
trap cleanup EXIT

# Store must already exist and be indexed once for the daemon to load; create if empty.
if [[ ! -f "$STORE_PATH/chunks.db" ]]; then
  echo "store looks empty; create/index it first (SIFT-006 path), then re-run"
  exit 2
fi

./target/release/sift-daemon \
  --store "$STORE_PATH" \
  --repo "$REPO_PATH" \
  --model "$MODEL_PATH" \
  --socket "$SOCKET" \
  --idle-secs 600 &
DAEMON_PID=$!

for _ in $(seq 1 600); do
  [[ -S "$SOCKET" ]] && break
  sleep 0.05
done

echo "daemon up; issue Index Full and concurrent Status polls (progress visible in daemon logs)"
echo "manual: connect with DaemonClient / future MCP tools to stream IndexProgress while searching"
echo "socket=$SOCKET pid=$DAEMON_PID"
# Keep alive briefly for operator observation.
sleep "${OBSERVE_SECS:-10}"
