#!/usr/bin/env bash
# Single validation command for local development and CI.
# Add new checks here — never as extra steps in the GitHub Actions workflow.
set -euo pipefail

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
