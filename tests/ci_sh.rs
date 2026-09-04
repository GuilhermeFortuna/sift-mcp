//! `ci.sh` must not fan cargo out to enough parallel rustc processes that
//! the last compile units — the daemon binary, its integration test, and
//! examples such as `index_repo` — link at once and OOM a workstation.

use std::fs;
use std::path::PathBuf;

fn ci_sh() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ci.sh");
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("ci.sh must exist at {}: {e}", path.display()))
}

#[test]
fn local_compile_jobs_default_to_one() {
    let text = ci_sh();

    assert!(
        text.contains("CARGO_BUILD_JOBS=1"),
        "ci.sh must default local CARGO_BUILD_JOBS to 1 so clippy --all-targets \
         cannot compile sift-daemon, daemon_integration, and index_repo together"
    );
    assert!(
        !text.contains("CARGO_BUILD_JOBS=8"),
        "a local cap of 8 still allows those three artifacts to compile together"
    );
}
