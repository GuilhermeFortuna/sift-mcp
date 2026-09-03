//! Replay the last N commits as incremental updates and print per-commit cost.
//!
//! Usage:
//!   cargo run -p indexing --example replay_commits -- <repo-path> [--count N]

use std::env;
use std::path::PathBuf;
use std::process::{self, Command};

use indexing::{IndexConfig, Indexer, NullProgress, require_verify_ok};
use inference::{Embedder, MockEmbedder};
use storage::ChunkStore;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut count: usize = 10;
    let mut repo: Option<PathBuf> = None;
    let mut args = env::args().skip(1).peekable();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--count" => {
                let n = args
                    .next()
                    .ok_or("--count requires a value")?
                    .parse::<usize>()?;
                count = n.max(1);
            }
            _ if repo.is_none() => repo = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected arg: {a}").into()),
        }
    }
    let repo = repo.ok_or("usage: replay_commits <repo-path> [--count N]")?;

    let log = Command::new("git")
        .args(["rev-list", "--reverse", &format!("-n{count}"), "HEAD"])
        .current_dir(&repo)
        .output()?;
    if !log.status.success() {
        return Err(String::from_utf8_lossy(&log.stderr).into());
    }
    let commits: Vec<String> = String::from_utf8(log.stdout)?
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if commits.is_empty() {
        return Err("no commits found".into());
    }

    let store_dir = tempfile::tempdir()?;
    let embedder = MockEmbedder::new(384).with_batch_limit(32);
    let store = ChunkStore::create(store_dir.path(), embedder.dims(), embedder.model_id())?;

    // Detach HEAD to the first commit, full index, then walk forward with update.
    let original = git_out(&repo, &["rev-parse", "HEAD"])?;
    let start = &commits[0];
    git_ok(&repo, &["checkout", "--detach", start])?;

    let config = IndexConfig::default();
    let mut indexer = Indexer::open(store, &embedder, &repo, config)?;
    let first = indexer.index_all(&mut NullProgress)?;
    require_verify_ok(indexer.store())?;
    println!(
        "commit={} wall_millis={} embeddings_computed={} (full index)",
        &first.commit[..first.commit.len().min(12)],
        first.wall_millis,
        first.embeddings_computed
    );

    for commit in commits.iter().skip(1) {
        git_ok(&repo, &["checkout", "--detach", commit])?;
        // Refresh RepoGit view: reopen indexer around same store.
        let store = indexer.into_store();
        let mut indexer_next = Indexer::open(store, &embedder, &repo, IndexConfig::default())?;
        let report = indexer_next.update(&mut NullProgress)?;
        require_verify_ok(indexer_next.store())?;
        println!(
            "commit={} wall_millis={} embeddings_computed={} files_indexed={} chunks_added={} chunks_removed={}",
            &commit[..commit.len().min(12)],
            report.wall_millis,
            report.embeddings_computed,
            report.files_indexed,
            report.chunks_added,
            report.chunks_removed
        );
        indexer = indexer_next;
    }

    // Restore original HEAD.
    let _ = indexer;
    git_ok(&repo, &["checkout", "-"])?;
    // If checkout - failed (detached), try original hash.
    let _ = Command::new("git")
        .args(["checkout", original.trim()])
        .current_dir(&repo)
        .status();

    Ok(())
}

fn git_ok(repo: &std::path::Path, args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("git").args(args).current_dir(repo).status()?;
    if !status.success() {
        return Err(format!("git {args:?} failed").into());
    }
    Ok(())
}

fn git_out(repo: &std::path::Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new("git").args(args).current_dir(repo).output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into());
    }
    Ok(String::from_utf8(out.stdout)?)
}
