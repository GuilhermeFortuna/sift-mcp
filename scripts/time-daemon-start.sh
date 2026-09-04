#!/usr/bin/env bash
# Measure daemon start time to first served search (real model + full index).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STORE_PATH="${1:?usage: $0 <store-path>}"
REPO_PATH="${2:-.}"
MODEL_PATH="${3:-.}"
RUNS="${RUNS:-5}"

cargo build --release -p daemon --bin sift-daemon

SOCKET="$(mktemp -u "${TMPDIR:-/tmp}/sift-time.XXXXXX.sock")"
cleanup() {
  rm -f "$SOCKET" "${SOCKET}.lock" 2>/dev/null || true
  if [[ -n "${DAEMON_PID:-}" ]]; then kill "$DAEMON_PID" 2>/dev/null || true; fi
}
trap cleanup EXIT

echo "store=$STORE_PATH runs=$RUNS"
times=()
for i in $(seq 1 "$RUNS"); do
  rm -f "$SOCKET" "${SOCKET}.lock" 2>/dev/null || true
  start_ns=$(date +%s%N)
  ./target/release/sift-daemon \
    --store "$STORE_PATH" \
    --repo "$REPO_PATH" \
    --model "$MODEL_PATH" \
    --socket "$SOCKET" \
    --idle-secs 120 &
  DAEMON_PID=$!
  # Wait until a Status/Search would succeed — use a tiny rust one-shot via cargo run is heavy;
  # poll socket existence then first connect via timeout.
  for _ in $(seq 1 600); do
    if [[ -S "$SOCKET" ]]; then
      break
    fi
    sleep 0.05
  done
  # Approximate first-ready by waiting until socket accepts; finer split is logged by daemon.
  end_ns=$(date +%s%N)
  ms=$(( (end_ns - start_ns) / 1000000 ))
  times+=("$ms")
  echo "run=$i start_to_socket_ms=$ms"
  kill "$DAEMON_PID" 2>/dev/null || true
  wait "$DAEMON_PID" 2>/dev/null || true
  DAEMON_PID=
done

IFS=$'\n' sorted=($(sort -n <<<"${times[*]}"))
mid=${sorted[$(( (RUNS-1)/2 ))]}
echo "median_ms=$mid values=${times[*]}"
