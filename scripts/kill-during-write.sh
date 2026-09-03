#!/usr/bin/env bash
# Start a large batch write, SIGKILL it mid-write, then reopen and verify.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STORE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/sift-kill-write.XXXXXX")"
cleanup() { rm -rf "$STORE_DIR"; }
trap cleanup EXIT

echo "store_dir=$STORE_DIR"

cargo build --release -p storage --example interruptible_write --example verify_store

cargo run --release -p storage --example interruptible_write -- "$STORE_DIR" &
PID=$!
echo "writer_pid=$PID"

# Let it write for a bit, then SIGKILL.
sleep 2
kill -9 "$PID" || true
wait "$PID" 2>/dev/null || true
echo "sent SIGKILL"

set +e
cargo run --release -p storage --example verify_store -- "$STORE_DIR"
STATUS=$?
set -e
echo "verify_exit=$STATUS"
exit "$STATUS"
