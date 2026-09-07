//! `ci.sh` must bound both artifact production and test execution so the
//! validation suite cannot exhaust a developer workstation by default.

use std::fs;
use std::path::PathBuf;

fn ci_sh() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ci.sh");
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("ci.sh must exist at {}: {e}", path.display()))
}

fn position(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("ci.sh must contain `{needle}`"))
}

#[test]
fn resource_defaults_are_conservative_and_overridable() {
    let text = ci_sh();

    assert!(
        text.contains(r#": "${CARGO_BUILD_JOBS:=1}""#),
        "build concurrency must default to one without replacing an explicit override"
    );
    assert!(
        text.contains(r#": "${RUST_TEST_THREADS:=1}""#),
        "test concurrency must default to one without replacing an explicit override"
    );
    assert!(
        text.contains(r#": "${CARGO_PROFILE_DEV_DEBUG:=0}""#),
        "validation must default development artifacts to no debug information"
    );
    assert!(
        text.contains(r#": "${CARGO_PROFILE_TEST_DEBUG:=0}""#),
        "validation must default test artifacts to no debug information"
    );
    assert!(
        !text.contains("nproc"),
        "processor count is not a safe proxy for available memory"
    );
    assert!(
        !text.contains(r#"${CI:-}"#),
        "generic CI markers must not disable the conservative defaults"
    );
}

#[test]
fn validation_stages_remain_complete_and_ordered() {
    let text = ci_sh();

    let format = position(&text, "cargo fmt --all -- --check");
    let clippy = position(
        &text,
        "cargo clippy --workspace --all-targets -- -D warnings",
    );
    let test = position(&text, "cargo test --workspace");
    let release = position(&text, "cargo build --workspace --release");

    assert!(format < clippy && clippy < test && test < release);
}

#[test]
fn cuda_feature_targets_are_compile_checked() {
    let text = ci_sh();

    assert!(
        text.contains("cargo check -p daemon --features cuda --bin sift-daemon"),
        "CI must compile the daemon CUDA feature path"
    );
    assert!(
        text.contains("cargo check -p eval --features cuda --example evaluate"),
        "CI must compile the CUDA evaluator"
    );
    assert!(
        text.contains("cargo check -p eval --features cuda --example proxy_kpi"),
        "CI must compile the CUDA proxy evaluator"
    );
}
