#!/usr/bin/env bash
# Emit a coding-agent MCP server registration snippet for sift mcp-client.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STORE="${SIFT_STORE:-$ROOT/.sift-store}"
REPO="${SIFT_REPO:-$ROOT}"
MODEL="${SIFT_MODEL:-$ROOT}"
BIN="${MCP_CLIENT_BIN:-$ROOT/target/release/mcp-client}"

if [[ ! -x "$BIN" ]]; then
  echo "Building mcp-client release binary…" >&2
  cargo build -q -p mcp-client --release
  BIN="$ROOT/target/release/mcp-client"
fi

cat <<EOF
# Add to your coding agent's MCP server config (Cursor example):
#
# {
#   "mcpServers": {
#     "sift": {
#       "command": "$BIN",
#       "args": ["--store", "$STORE", "--repo", "$REPO", "--model", "$MODEL"]
#     }
#   }
# }

{
  "mcpServers": {
    "sift": {
      "command": "$BIN",
      "args": ["--store", "$STORE", "--repo", "$REPO", "--model", "$MODEL"]
    }
  }
}
EOF
