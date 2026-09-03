//! Build temporary git repositories for indexing tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// One file write relative to the repo root.
#[derive(Debug, Clone)]
pub struct FileSpec {
    pub path: String,
    pub contents: String,
}

impl FileSpec {
    pub fn new(path: impl Into<String>, contents: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            contents: contents.into(),
        }
    }
}

/// One commit to apply in order.
#[derive(Debug, Clone)]
pub struct CommitSpec {
    pub message: String,
    pub files: Vec<FileSpec>,
    /// Paths to delete before committing.
    pub deletes: Vec<String>,
    /// Renames as (from, to); content is preserved from the previous tree.
    pub renames: Vec<(String, String)>,
}

impl CommitSpec {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            files: Vec::new(),
            deletes: Vec::new(),
            renames: Vec::new(),
        }
    }

    pub fn file(mut self, path: impl Into<String>, contents: impl Into<String>) -> Self {
        self.files.push(FileSpec::new(path, contents));
        self
    }

    pub fn delete(mut self, path: impl Into<String>) -> Self {
        self.deletes.push(path.into());
        self
    }

    pub fn rename(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.renames.push((from.into(), to.into()));
        self
    }
}

/// A disposable git repository built from a sequence of commits.
pub struct TempRepo {
    dir: TempDir,
}

impl TempRepo {
    pub fn build(commits: &[CommitSpec]) -> Self {
        let dir = TempDir::new().expect("tempdir");
        git(dir.path(), &["init", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        // Disable gpg signing for test commits.
        git(dir.path(), &["config", "commit.gpgsign", "false"]);

        let mut repo = Self { dir };
        for commit in commits {
            repo.apply_commit(commit);
        }
        repo
    }

    pub fn apply_commit(&self, commit: &CommitSpec) {
        for path in &commit.deletes {
            let full = self.dir.path().join(path);
            if full.exists() {
                fs::remove_file(&full).expect("delete file");
            }
            git(
                self.dir.path(),
                &["rm", "--quiet", "--ignore-unmatch", "-f", path],
            );
        }
        for (from, to) in &commit.renames {
            let from_full = self.dir.path().join(from);
            let to_full = self.dir.path().join(to);
            if let Some(parent) = to_full.parent() {
                fs::create_dir_all(parent).expect("mkdir rename dest");
            }
            fs::rename(&from_full, &to_full).expect("rename file");
            git(self.dir.path(), &["add", "-A"]);
        }
        for file in &commit.files {
            let full = self.dir.path().join(&file.path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("mkdir");
            }
            fs::write(&full, &file.contents).expect("write file");
            git(self.dir.path(), &["add", "--", &file.path]);
        }
        git(
            self.dir.path(),
            &["commit", "--allow-empty", "-m", &commit.message],
        );
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    #[allow(dead_code)]
    pub fn path_buf(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    /// Write an uncommitted change (does not stage unless `stage` is true).
    #[allow(dead_code)]
    pub fn write_uncommitted(&self, path: &str, contents: &str) {
        let full = self.dir.path().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(&full, contents).expect("write");
    }

    pub fn head(&self) -> String {
        let out = git_output(self.dir.path(), &["rev-parse", "HEAD"]);
        String::from_utf8(out).expect("utf8").trim().to_string()
    }

    #[allow(dead_code)]
    pub fn commit_at(&self, rev: &str) -> String {
        let out = git_output(self.dir.path(), &["rev-parse", rev]);
        String::from_utf8(out).expect("utf8").trim().to_string()
    }
}

fn git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {args:?} failed");
}

fn git_output(cwd: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}
