#!/usr/bin/env bash
# Package the sift-console binary and frontend assets for standalone local distribution.
# Creates an unpacked bundle in the named directory and an archive alongside it.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

OUTPUT_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output)
      OUTPUT_DIR="$2"
      shift 2
      ;;
    -h|--help)
      echo "usage: $0 --output <directory>"
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      echo "usage: $0 --output <directory>" >&2
      exit 2
      ;;
  esac
done

if [[ -z "$OUTPUT_DIR" ]]; then
  echo "error: missing required --output <directory>" >&2
  exit 2
fi

# Ensure output directory is absolute or relative to invocation
if [[ ! "$OUTPUT_DIR" = /* ]]; then
  OUTPUT_DIR="$(pwd)/$OUTPUT_DIR"
fi

if [[ -d "$OUTPUT_DIR" ]]; then
  if [[ "$(ls -A "$OUTPUT_DIR")" ]]; then
    echo "error: refusing to overwrite existing non-empty directory: $OUTPUT_DIR" >&2
    exit 1
  fi
else
  mkdir -p "$OUTPUT_DIR"
fi

echo "==> Building frontend assets (pnpm build)..."
corepack pnpm@10.11.0 --dir "$ROOT/ui" install --frozen-lockfile
corepack pnpm@10.11.0 --dir "$ROOT/ui" build

echo "==> Building release binary (sift-console)..."
cargo build -p console --bin sift-console --release

echo "==> Populating packaged bundle..."
cp "$ROOT/target/release/sift-console" "$OUTPUT_DIR/sift-console"
chmod +x "$OUTPUT_DIR/sift-console"

mkdir -p "$OUTPUT_DIR/ui"
cp -r "$ROOT/ui/dist/"* "$OUTPUT_DIR/ui/"

cat << 'EOF' > "$OUTPUT_DIR/README.md"
# Sift Console

Standalone code intelligence console and search lab for local codebases.

## Requirements

The packaged console runs self-contained:
- No Node.js or pnpm is required to run the console or serve assets.
- No Python runtime is required.
- No GPU, CUDA toolkit, or model files are required merely to open the console and inspect metadata.
- To execute search or indexing against an active codebase, connect to a running `sift-daemon`.

## Usage

Start the console listening on loopback (default 127.0.0.1:7331):

```bash
./sift-console --listen 127.0.0.1:7331
```

Options:
- `--listen <IP:PORT>`: Loopback socket address to bind (default: 127.0.0.1:7331).
- `--database <PATH>`: SQLite history database path (default: console.sqlite3).
- `--assets <PATH>`: Directory containing UI assets (default: detects sibling ui/ directory).
- `--collect <on|off>`: Enable or disable background daemon metrics collection (default: on).

Open http://127.0.0.1:7331/search in your browser.
EOF

cat << 'EOF' > "$OUTPUT_DIR/NOTICES.md"
# Third-Party Notices and Licenses

The Sift Console incorporates software components subject to third-party open-source licenses.

## Rust Ecosystem
- axum (MIT License) - Copyright (c) 2021 Axum Contributors
- tokio (MIT License) - Copyright (c) 2021 Tokio Contributors
- rusqlite (MIT License) - Copyright (c) 2014 The Rusqlite Developers
- serde & serde_json (MIT / Apache-2.0) - Copyright (c) 2014-2023 Serde Developers
- tower & tower-http (MIT License) - Copyright (c) 2019-2023 Tower Contributors

## Frontend Ecosystem
- React & React DOM (MIT License) - Copyright (c) Meta Platforms, Inc. and affiliates
- Vite (MIT License) - Copyright (c) 2019-present Yuxi (Evan) You and Vite contributors
- Inter font (SIL Open Font License 1.1) - Copyright (c) 2016-2020 The Inter Project Authors
EOF

echo "==> Creating archive..."
ARCHIVE_PATH="$OUTPUT_DIR/sift-console.tar.gz"
tar -czf "$ARCHIVE_PATH" -C "$OUTPUT_DIR" sift-console ui README.md NOTICES.md

echo "==> Sift Console packaged successfully at: $OUTPUT_DIR"
echo "    Archive: $ARCHIVE_PATH"
echo "    Launch:  $OUTPUT_DIR/sift-console --listen 127.0.0.1:7331"
