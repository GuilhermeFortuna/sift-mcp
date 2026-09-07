#!/usr/bin/env bash
# Paired console overhead measurement (collection off vs on).
# Drives 3-pair aggregation rotating off/on order to limit bias.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

REPO=""
STORE=""
MODEL=""
DAEMON=""
CONSOLE=""
RUNS=3
OUTPUT=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo) REPO="$2"; shift 2 ;;
    --store) STORE="$2"; shift 2 ;;
    --model) MODEL="$2"; shift 2 ;;
    --daemon) DAEMON="$2"; shift 2 ;;
    --console) CONSOLE="$2"; shift 2 ;;
    --runs) RUNS="$2"; shift 2 ;;
    --output) OUTPUT="$2"; shift 2 ;;
    -h|--help)
      echo "usage: $0 --repo PATH --store PATH --model PATH --daemon PATH --console PATH --runs N --output DIR"
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [[ -z "$REPO" || -z "$STORE" || -z "$MODEL" || -z "$DAEMON" || -z "$CONSOLE" || -z "$OUTPUT" ]]; then
  echo "usage: $0 --repo PATH --store PATH --model PATH --daemon PATH --console PATH --runs N --output DIR" >&2
  exit 2
fi

mkdir -p "$OUTPUT"

echo "==> Building measure_console_overhead example..."
cargo build -p daemon --example measure_console_overhead --features resident

echo "==> Running paired console overhead measurements..."
"$ROOT/target/debug/examples/measure_console_overhead" \
  --repo "$REPO" --store "$STORE" --model "$MODEL" --daemon "$DAEMON" \
  --console "$CONSOLE" --runs "$RUNS" --output "$OUTPUT"

echo "==> Reports generated at $OUTPUT"
if command -v nvidia-smi >/dev/null 2>&1; then
  nvidia-smi --query-gpu=uuid,memory.used,memory.total --format=csv || true
fi
