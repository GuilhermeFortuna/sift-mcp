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
# Sift's production runtime targets CUDA 12.x. ort-sys uses this to select its
# CUDA 12 ONNX Runtime distribution deterministically on machines with multiple
# CUDA toolkits or incomplete version metadata.
export ORT_CUDA_VERSION=12

cargo fmt --all -- --check
cargo check -p daemon --features cuda --bin sift-daemon
cargo check -p eval --features cuda --example evaluate
cargo check -p eval --features cuda --example proxy_kpi
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
corepack pnpm@10.11.0 --dir ui install --frozen-lockfile
corepack pnpm@10.11.0 --dir ui typecheck
corepack pnpm@10.11.0 --dir ui test
corepack pnpm@10.11.0 --dir ui build
