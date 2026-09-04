#!/usr/bin/env bash
# Paired observability overhead measurement (recording off vs on).
# Environment variables name owner-supplied absolute paths.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

REPO=""
STORE=""
MODEL=""
DAEMON=""
RUNS=3
OUTPUT=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo) REPO="$2"; shift 2 ;;
    --store) STORE="$2"; shift 2 ;;
    --model) MODEL="$2"; shift 2 ;;
    --daemon) DAEMON="$2"; shift 2 ;;
    --runs) RUNS="$2"; shift 2 ;;
    --output) OUTPUT="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [[ -z "$REPO" || -z "$STORE" || -z "$MODEL" || -z "$DAEMON" || -z "$OUTPUT" ]]; then
  echo "usage: $0 --repo PATH --store PATH --model PATH --daemon PATH --runs N --output DIR" >&2
  exit 2
fi

mkdir -p "$OUTPUT"

cargo build -p daemon --example measure_ui_observability --features resident

# The example loops recording off then on; SIFT_RECORD_EVENTS is consulted by
# sift-daemon / sift-daemon-test when spawned via connect_or_spawn.
export SIFT_RECORD_EVENTS=1
"$ROOT/target/debug/examples/measure_ui_observability" \
  --repo "$REPO" --store "$STORE" --model "$MODEL" --daemon "$DAEMON" \
  --runs "$RUNS" --output "$OUTPUT"

echo "reports in $OUTPUT"
if command -v nvidia-smi >/dev/null 2>&1; then
  nvidia-smi --query-gpu=uuid,memory.used,memory.total --format=csv || true
fi
