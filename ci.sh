#!/usr/bin/env bash
# Single validation command for local development and CI.
# Add new checks here — never as extra steps in the GitHub Actions workflow.
set -euo pipefail

# Cap compile parallelism locally. `cargo clippy --all-targets` finishes by
# compiling several large artifacts at once (sift-daemon, daemon_integration,
# and examples such as index_repo). Each rustc/link of those can take multiple
# GiB, so a cap of 8 still OOMs a 32-core workstation. One job serializes
# them. Override with CARGO_BUILD_JOBS, or CI=true (uses nproc).
if [[ -z "${CARGO_BUILD_JOBS:-}" ]]; then
  if [[ "${CI:-}" == "true" ]]; then
    export CARGO_BUILD_JOBS
    CARGO_BUILD_JOBS="$(nproc)"
  else
    export CARGO_BUILD_JOBS=1
  fi
fi

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
