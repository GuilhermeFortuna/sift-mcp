//! Guards that the workspace member list, `crates/` directories, and the
//! crate layout in `docs/tech-stack.md` stay in agreement.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_workspace_members(root: &Path) -> BTreeSet<String> {
    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read root Cargo.toml");
    let mut members = BTreeSet::new();
    let mut in_members = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("members") && trimmed.contains('[') {
            in_members = true;
            continue;
        }
        if !in_members {
            continue;
        }
        if trimmed.starts_with(']') {
            break;
        }
        if let Some(start) = trimmed.find('"') {
            let rest = &trimmed[start + 1..];
            if let Some(end) = rest.find('"') {
                let path = &rest[..end];
                let crate_name = path
                    .strip_prefix("crates/")
                    .expect("workspace member path must start with crates/");
                members.insert(crate_name.to_string());
            }
        }
    }
    members
}

fn crate_directories(root: &Path) -> BTreeSet<String> {
    let crates_dir = root.join("crates");
    fs::read_dir(&crates_dir)
        .unwrap_or_else(|e| panic!("read crates/: {e}"))
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if entry.file_type().ok()?.is_dir() {
                Some(entry.file_name().to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect()
}

fn documented_layout(root: &Path) -> BTreeSet<String> {
    let tech_stack =
        fs::read_to_string(root.join("docs/tech-stack.md")).expect("read docs/tech-stack.md");
    let mut crates = BTreeSet::new();
    let mut in_crate_layout = false;
    for line in tech_stack.lines() {
        if line.trim() == "### Crate layout" {
            in_crate_layout = true;
            continue;
        }
        if in_crate_layout && line.starts_with("## ") {
            break;
        }
        if !in_crate_layout {
            continue;
        }
        let trimmed = line.trim();
        for prefix in ["├── ", "└── "] {
            let Some(rest) = trimmed.strip_prefix(prefix) else {
                continue;
            };
            // Drop trailing comments, then require a directory name ending in `/`.
            let name_part = rest.split('#').next().unwrap_or(rest).trim();
            if let Some(name) = name_part.strip_suffix('/') {
                crates.insert(name.to_string());
            }
        }
    }
    crates
}

#[test]
fn every_crate_directory_is_a_declared_member() {
    let root = workspace_root();
    let members = read_workspace_members(&root);
    let dirs = crate_directories(&root);

    for dir in &dirs {
        assert!(
            members.contains(dir),
            "{dir} exists under crates/ but is not a declared workspace member"
        );
    }
    for member in &members {
        assert!(
            dirs.contains(member),
            "{member} is a declared workspace member but has no directory under crates/"
        );
    }
}

#[test]
fn member_list_matches_documented_layout() {
    let root = workspace_root();
    let members = read_workspace_members(&root);
    let documented = documented_layout(&root);

    assert!(
        !documented.is_empty(),
        "parsed no crates from docs/tech-stack.md crate layout"
    );

    for member in &members {
        assert!(
            documented.contains(member),
            "{member} is a workspace member but is missing from the tech-stack.md crate layout"
        );
    }
    for crate_name in &documented {
        assert!(
            members.contains(crate_name),
            "{crate_name} is in the tech-stack.md crate layout but is not a workspace member"
        );
    }
}
