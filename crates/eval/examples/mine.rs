//! Mine retrieval labels from a repository.
//!
//! ```text
//! cargo run --release -p eval --example mine -- <repo-path> [--report] [--no-pin]
//! ```

use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::process;

use eval::{MiningConfig, mine_commits};
use indexing::{IndexConfig, Indexer, NullProgress};
use inference::{Embedder, MockEmbedder};
use storage::ChunkStore;
use tempfile::TempDir;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let report = args.iter().any(|a| a == "--report");
    let no_pin = args.iter().any(|a| a == "--no-pin");
    args.retain(|a| a != "--report" && a != "--no-pin");

    let repo = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| eval::expand_home(eval::MINED_CORPUS_DEFAULT_PATH));

    let store_dir = TempDir::new()?;
    let embedder = MockEmbedder::new(8);
    {
        let store = ChunkStore::create(store_dir.path(), 8, embedder.model_id())?;
        let mut indexer = Indexer::open(store, &embedder, &repo, IndexConfig::default())?;
        eprintln!("indexing {} …", repo.display());
        indexer.index_all(&mut NullProgress)?;
    }
    let store = ChunkStore::open(store_dir.path())?;

    let config = MiningConfig {
        enforce_pinned_revision: !no_pin,
        ..MiningConfig::default()
    };
    let (labels, mining_report) = mine_commits(&repo, &store, &config)?;

    if report {
        println!("commits_examined={}", mining_report.commits_examined);
        println!("labels_accepted={}", mining_report.labels_accepted);
        for (rule, count) in &mining_report.rejected {
            println!("rejected.{rule}={count}");
        }
        println!("reconciles={}", mining_report.reconciles());
        let n = mining_report.labels_accepted;
        // Wilson-ish rough half-width for p≈0.80 at 95%: ~1.96*sqrt(pq/n)
        let half = if n > 0 {
            1.96 * (0.80 * 0.20 / n as f64).sqrt()
        } else {
            f64::NAN
        };
        println!("top3_near_0.80_ci_half_width≈{half:.4} (n={n})");
        println!("--- sample accepted (up to 10) ---");
        for label in labels.iter().take(10) {
            println!(
                "  [{}] {} -> {:?}",
                &label.provenance[..8.min(label.provenance.len())],
                label.query,
                label.expected
            );
        }
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "labels": labels.iter().map(|l| serde_json::json!({
                "query": l.query,
                "expected": l.expected,
                "provenance": l.provenance,
            })).collect::<Vec<_>>(),
            "report": {
                "commits_examined": mining_report.commits_examined,
                "labels_accepted": mining_report.labels_accepted,
                "rejected": mining_report.rejected,
            }
        }))?
    );
    Ok(())
}
