#!/usr/bin/env bash
# Single validation command for local development and CI.
# Add new checks here — never as extra steps in the GitHub Actions workflow.
set -euo pipefail

# The daemon's all-targets test artifacts are large enough to exhaust a 32 GB
# workstation even when compilation is serialized if they carry full debug
# information. Keep validation conservative on every host: processor count is
# not evidence of memory headroom. Explicit environment values remain valid for
# dedicated runners that have measured capacity.
: "${CARGO_BUILD_JOBS:=1}"
: "${RUST_TEST_THREADS:=1}"
: "${CARGO_PROFILE_DEV_DEBUG:=0}"
: "${CARGO_PROFILE_TEST_DEBUG:=0}"
export CARGO_BUILD_JOBS RUST_TEST_THREADS
export CARGO_PROFILE_DEV_DEBUG CARGO_PROFILE_TEST_DEBUG

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
