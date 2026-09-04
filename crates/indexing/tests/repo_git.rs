mod common;

use common::{CommitSpec, TempRepo};
use indexing::git::{FileChange, RepoGit};

#[test]
fn head_commit_returns_current_hash() {
    let repo = TempRepo::build(&[CommitSpec::new("init").file("a.rs", "fn a() {}\n")]);
    let git = RepoGit::open(repo.path()).unwrap();
    assert_eq!(git.head_commit().unwrap(), repo.head());
}

#[test]
fn changes_since_added_file() {
    let repo = TempRepo::build(&[
        CommitSpec::new("init").file("a.rs", "fn a() {}\n"),
        CommitSpec::new("add b").file("b.rs", "fn b() {}\n"),
    ]);
    let base = repo.commit_at("HEAD~1");
    let git = RepoGit::open(repo.path()).unwrap();
    let changes = git.changes_since(&base).unwrap();
    assert_eq!(changes, vec![FileChange::Added("b.rs".into())]);
}

#[test]
fn changes_since_modified_file() {
    let repo = TempRepo::build(&[
        CommitSpec::new("init").file("a.rs", "fn a() {}\n"),
        CommitSpec::new("edit a").file("a.rs", "fn a() { 1 }\n"),
    ]);
    let base = repo.commit_at("HEAD~1");
    let git = RepoGit::open(repo.path()).unwrap();
    let changes = git.changes_since(&base).unwrap();
    assert_eq!(changes, vec![FileChange::Modified("a.rs".into())]);
}

#[test]
fn changes_since_deleted_file() {
    let repo = TempRepo::build(&[
        CommitSpec::new("init")
            .file("a.rs", "fn a() {}\n")
            .file("b.rs", "fn b() {}\n"),
        CommitSpec::new("delete b").delete("b.rs"),
    ]);
    let base = repo.commit_at("HEAD~1");
    let git = RepoGit::open(repo.path()).unwrap();
    let changes = git.changes_since(&base).unwrap();
    assert_eq!(changes, vec![FileChange::Deleted("b.rs".into())]);
}

#[test]
fn changes_since_renamed_file() {
    let repo = TempRepo::build(&[
        CommitSpec::new("init").file("old.rs", "fn renamed() {}\n"),
        CommitSpec::new("rename").rename("old.rs", "new.rs"),
    ]);
    let base = repo.commit_at("HEAD~1");
    let git = RepoGit::open(repo.path()).unwrap();
    let changes = git.changes_since(&base).unwrap();
    assert_eq!(
        changes,
        vec![FileChange::Renamed {
            from: "old.rs".into(),
            to: "new.rs".into(),
        }]
    );
}

#[test]
fn is_dirty_false_on_clean_tree() {
    let repo = TempRepo::build(&[CommitSpec::new("init").file("a.rs", "fn a() {}\n")]);
    let git = RepoGit::open(repo.path()).unwrap();
    assert!(!git.is_dirty().unwrap());
}

#[test]
fn is_dirty_true_after_uncommitted_edit() {
    let repo = TempRepo::build(&[CommitSpec::new("init").file("a.rs", "fn a() {}\n")]);
    repo.write_uncommitted("a.rs", "fn a() { dirty }\n");
    let git = RepoGit::open(repo.path()).unwrap();
    assert!(git.is_dirty().unwrap());
}
