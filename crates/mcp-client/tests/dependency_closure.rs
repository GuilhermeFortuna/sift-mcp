//! Assert mcp-client's dependency closure stays free of inference/GPU/index crates.
//!
//! Uses `cargo tree -p mcp-client` rather than a workspace-wide `cargo metadata`
//! walk: metadata unifies features across all workspace members, so retrieval's
//! `engine` feature (enabled by indexing/daemon) would falsely pull `tantivy`
//! into this crate's reported closure.

use std::process::Command;

const FORBIDDEN: &[&str] = &["ort", "cudarc", "tokenizers", "tantivy"];

#[test]
fn mcp_client_dependency_closure_excludes_heavy_crates() {
    let output = Command::new("cargo")
        .args([
            "tree",
            "-p",
            "mcp-client",
            "-e",
            "normal",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo tree");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut found = Vec::new();
    for line in stdout.lines() {
        let name = line.split_whitespace().next().unwrap_or("");
        if FORBIDDEN.contains(&name) {
            found.push(name.to_owned());
        }
    }
    assert!(
        found.is_empty(),
        "mcp-client dependency closure includes forbidden crates: {found:?}\n{stdout}"
    );
}
