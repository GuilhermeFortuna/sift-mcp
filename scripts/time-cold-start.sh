#!/usr/bin/env bash
# Measure mcp-client cold start to completed MCP handshake.
# Usage: scripts/time-cold-start.sh [N]
# With the daemon already warm, reports median and worst case against the 200 ms budget.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
N="${1:-20}"
STORE="${SIFT_STORE:-$ROOT/.sift-store}"
REPO="${SIFT_REPO:-$ROOT}"
MODEL="${SIFT_MODEL:-$ROOT}"
BIN="${MCP_CLIENT_BIN:-}"

if [[ -z "$BIN" ]]; then
  cargo build -q -p mcp-client --release
  BIN="$ROOT/target/release/mcp-client"
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

times_file="$tmpdir/times.txt"
: >"$times_file"

handshake_once() {
  local out="$tmpdir/out.jsonl"
  : >"$out"
  # Drive initialize + initialized; measure wall time to first initialize result.
  local start end
  start="$(date +%s%N)"
  (
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"time-cold-start","version":"0.0.0"}}}'
    # Give the server a moment to respond before closing stdin.
    sleep 0.05
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    sleep 0.05
  ) | "$BIN" --store "$STORE" --repo "$REPO" --model "$MODEL" >"$out" 2>/dev/null || true
  end="$(date +%s%N)"
  # Require an initialize result on stdout.
  if ! grep -q '"id":1' "$out" 2>/dev/null; then
    echo "handshake failed; stdout:" >&2
    cat "$out" >&2 || true
    return 1
  fi
  python3 - <<PY
start=$start
end=$end
print((end - start) / 1_000_000.0)
PY
}

echo "Measuring $N cold starts (daemon warm assumed)…"
for i in $(seq 1 "$N"); do
  ms="$(handshake_once)"
  echo "$ms" >>"$times_file"
  echo "  run $i: ${ms} ms"
done

python3 - <<'PY' "$times_file"
import sys
path = sys.argv[1]
vals = sorted(float(line) for line in open(path) if line.strip())
n = len(vals)
median = vals[n // 2] if n % 2 == 1 else 0.5 * (vals[n // 2 - 1] + vals[n // 2])
worst = vals[-1]
budget = 200.0
print(f"n={n}")
print(f"median_ms={median:.2f}")
print(f"worst_ms={worst:.2f}")
print(f"budget_ms={budget:.0f}")
print(f"median_ok={median <= budget}")
print(f"worst_ok={worst <= budget}")
PY
