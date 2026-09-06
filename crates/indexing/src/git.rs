//! Git repository access for incremental indexing.

use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;
use gix::diff::Options as DiffOptions;
use gix::object::tree::diff::ChangeDetached;

use crate::error::IndexError;

/// One entry of `git diff --name-status` between two commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    Added(String),
    Modified(String),
    Deleted(String),
    Renamed { from: String, to: String },
}

/// Thin wrapper around a `gix` repository handle.
pub struct RepoGit {
    repo: gix::Repository,
    root: PathBuf,
}

impl RepoGit {
    pub fn open(root: &Path) -> Result<Self, IndexError> {
        let repo = gix::open(root).map_err(|e| IndexError::Git(e.to_string()))?;
        Ok(Self {
            root: root.to_path_buf(),
            repo,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn head_commit(&self) -> Result<String, IndexError> {
        let id = self
            .repo
            .head_id()
            .map_err(|e| IndexError::Git(e.to_string()))?;
        Ok(id.to_string())
    }

    /// Return every path in the current HEAD tree, including paths absent from
    /// the working directory.
    pub fn head_files(&self) -> Result<Vec<String>, IndexError> {
        let output = std::process::Command::new("git")
            .args(["ls-tree", "-r", "-z", "--name-only", "HEAD"])
            .current_dir(&self.root)
            .output()
            .map_err(|e| IndexError::Git(e.to_string()))?;
        if !output.status.success() {
            return Err(IndexError::Git(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        Ok(output
            .stdout
            .split(|&byte| byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).replace('\\', "/"))
            .collect())
    }

    pub fn changes_since(&self, base: &str) -> Result<Vec<FileChange>, IndexError> {
        let base_id = self
            .repo
            .rev_parse_single(base)
            .map_err(|e| IndexError::Git(format!("resolve base {base}: {e}")))?;
        let base_commit = base_id
            .object()
            .map_err(|e| IndexError::Git(e.to_string()))?
            .peel_to_commit()
            .map_err(|e| IndexError::Git(e.to_string()))?;
        let head_commit = self
            .repo
            .head_commit()
            .map_err(|e| IndexError::Git(e.to_string()))?;

        let old_tree = base_commit
            .tree()
            .map_err(|e| IndexError::Git(e.to_string()))?;
        let new_tree = head_commit
            .tree()
            .map_err(|e| IndexError::Git(e.to_string()))?;

        let mut opts = DiffOptions::default();
        opts.track_path();
        opts.track_rewrites(Some(Default::default()));

        let changes = self
            .repo
            .diff_tree_to_tree(Some(&old_tree), Some(&new_tree), Some(opts))
            .map_err(|e| IndexError::Git(e.to_string()))?;

        Ok(changes
            .into_iter()
            .filter_map(change_to_file_change)
            .collect())
    }

    pub fn is_dirty(&self) -> Result<bool, IndexError> {
        self.repo
            .is_dirty()
            .map_err(|e| IndexError::Git(e.to_string()))
    }

    /// Paths that differ from HEAD (staged or unstaged), for Refuse policy messages.
    pub fn dirty_paths(&self) -> Result<Vec<String>, IndexError> {
        // Use porcelain status via the git CLI only when listing; is_dirty uses gix.
        // Prefer gix status when available; fall back to comparing worktree is enough
        // for Refuse messaging once is_dirty is true.
        let status = std::process::Command::new("git")
            .args(["status", "--porcelain", "-z"])
            .current_dir(&self.root)
            .output()
            .map_err(|e| IndexError::Git(e.to_string()))?;
        if !status.status.success() {
            return Err(IndexError::Git(
                String::from_utf8_lossy(&status.stderr).into_owned(),
            ));
        }
        let mut paths = Vec::new();
        for entry in status.stdout.split(|&b| b == 0) {
            if entry.len() < 3 {
                continue;
            }
            // Format: XY <path> or XY <old>\0<new> for renames; -z uses NUL.
            let path = String::from_utf8_lossy(&entry[3..]).into_owned();
            if !path.is_empty() {
                paths.push(path);
            }
        }
        Ok(paths)
    }
}

fn location_to_string(location: &gix::bstr::BStr) -> Option<String> {
    let s = location.to_str().ok()?;
    if s.is_empty() {
        None
    } else {
        Some(s.replace('\\', "/"))
    }
}

fn change_to_file_change(change: ChangeDetached) -> Option<FileChange> {
    match change {
        ChangeDetached::Addition {
            location,
            entry_mode,
            ..
        } => {
            if !entry_mode.is_blob() && !entry_mode.is_link() {
                return None;
            }
            location_to_string(location.as_ref()).map(FileChange::Added)
        }
        ChangeDetached::Deletion {
            location,
            entry_mode,
            ..
        } => {
            if !entry_mode.is_blob() && !entry_mode.is_link() {
                return None;
            }
            location_to_string(location.as_ref()).map(FileChange::Deleted)
        }
        ChangeDetached::Modification {
            location,
            previous_entry_mode,
            entry_mode,
            ..
        } => {
            if !entry_mode.is_blob()
                && !entry_mode.is_link()
                && !previous_entry_mode.is_blob()
                && !previous_entry_mode.is_link()
            {
                return None;
            }
            location_to_string(location.as_ref()).map(FileChange::Modified)
        }
        ChangeDetached::Rewrite {
            source_location,
            location,
            entry_mode,
            source_entry_mode,
            ..
        } => {
            if !entry_mode.is_blob()
                && !entry_mode.is_link()
                && !source_entry_mode.is_blob()
                && !source_entry_mode.is_link()
            {
                return None;
            }
            let from = location_to_string(source_location.as_ref())?;
            let to = location_to_string(location.as_ref())?;
            Some(FileChange::Renamed { from, to })
        }
    }
}
