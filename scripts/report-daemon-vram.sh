#!/usr/bin/env bash
# Report resident GPU memory with embedder loaded (desktop session attached).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STORE_PATH="${1:?usage: $0 <store-path>}"
REPO_PATH="${2:-.}"
MODEL_PATH="${3:-.}"

cargo build --release -p daemon --bin sift-daemon --features cuda

SOCKET="$(mktemp -u "${TMPDIR:-/tmp}/sift-vram.XXXXXX.sock")"
cleanup() {
  rm -f "$SOCKET" "${SOCKET}.lock" 2>/dev/null || true
  if [[ -n "${DAEMON_PID:-}" ]]; then kill "$DAEMON_PID" 2>/dev/null || true; fi
}
trap cleanup EXIT

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
sleep 2

if command -v nvidia-smi >/dev/null; then
  nvidia-smi --query-compute-apps=pid,used_gpu_memory --format=csv,noheader || true
  nvidia-smi --query-gpu=memory.used,memory.total --format=csv,noheader
  echo "budget_gb_approx=5.0"
else
  echo "nvidia-smi not available; cannot report VRAM"
  exit 1
fi
