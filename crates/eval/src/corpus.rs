//! Pinned evaluation corpora. The mined corpus revision is fixed before the
//! first measured run and must not change afterward.

use std::path::{Path, PathBuf};

use indexing::RepoGit;

use crate::error::EvalError;

/// Harness version; bump when mining or metric definitions change.
pub const HARNESS_VERSION: u32 = 1;

/// Default path for the third-party mined corpus (not committed here).
pub const MINED_CORPUS_DEFAULT_PATH: &str = "~/llama.cpp";

/// Pinned `llama.cpp` revision. Mining refuses any other HEAD.
pub const MINED_CORPUS_PINNED_REVISION: &str = "c589f0ed10c643678c4707dd160c21ac7633ebc0";

/// First-party repository used for docstring and hand-written label sets.
pub const FIRST_PARTY_CORPUS_DEFAULT_PATH: &str = "~/projects/job-engine";

/// Expand `~` in a path string.
pub fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    if path == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home);
    }
    PathBuf::from(path)
}

/// Refuse to proceed when the checkout HEAD is not the pinned mined revision.
pub fn require_mined_revision(repo: &Path) -> Result<String, EvalError> {
    let git = RepoGit::open(repo).map_err(|e| EvalError::Git(e.to_string()))?;
    let actual = git
        .head_commit()
        .map_err(|e| EvalError::Git(e.to_string()))?;
    if actual != MINED_CORPUS_PINNED_REVISION {
        return Err(EvalError::RevisionMismatch {
            expected: MINED_CORPUS_PINNED_REVISION.to_string(),
            actual,
        });
    }
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn refuses_checkout_whose_head_differs_from_pinned_revision() {
        let dir = TempDir::new().expect("tempdir");
        git(dir.path(), &["init", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        fs::write(dir.path().join("README"), "fixture").expect("write");
        git(dir.path(), &["add", "README"]);
        git(dir.path(), &["commit", "-m", "initial"]);

        let err = require_mined_revision(dir.path()).expect_err("must refuse wrong HEAD");
        let msg = err.to_string();
        assert!(
            msg.contains(MINED_CORPUS_PINNED_REVISION),
            "error must name pinned revision: {msg}"
        );
        let actual = indexing::RepoGit::open(dir.path())
            .unwrap()
            .head_commit()
            .unwrap();
        assert!(
            msg.contains(&actual),
            "error must name actual HEAD {actual}: {msg}"
        );
        match err {
            EvalError::RevisionMismatch {
                expected,
                actual: a,
            } => {
                assert_eq!(expected, MINED_CORPUS_PINNED_REVISION);
                assert_eq!(a, actual);
            }
            other => panic!("expected RevisionMismatch, got {other:?}"),
        }
    }
}
