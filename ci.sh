#!/usr/bin/env bash
# Single validation command for local development and CI.
# Add new checks here — never as extra steps in the GitHub Actions workflow.
set -euo pipefail

# Cap compile parallelism locally. A 32-core `cargo build --release --workspace`
# with little free RAM will OOM and take the desktop with it. CI may override
# by setting CARGO_BUILD_JOBS or CI=true (uses nproc).
if [[ -z "${CARGO_BUILD_JOBS:-}" ]]; then
  if [[ "${CI:-}" == "true" ]]; then
    export CARGO_BUILD_JOBS
    CARGO_BUILD_JOBS="$(nproc)"
  else
    export CARGO_BUILD_JOBS=8
  fi
fi

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
