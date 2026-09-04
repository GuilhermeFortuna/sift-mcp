//! Proxy efficiency KPI: MCP bytes vs keyword baseline.
//!
//! ```text
//! cargo run --release -p eval --example proxy_kpi -- <repo-path> <store-path>
//! ```

use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::process;

use eval::{MiningConfig, median_bytes_before_hit, mine_commits};
use inference::MockEmbedder;
use retrieval::dense::{DenseBackend, DenseIndex};
use retrieval::{FusionConfig, LexicalIndex, Searcher};
use storage::ChunkStore;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let repo = PathBuf::from(
        args.next()
            .ok_or("usage: proxy_kpi <repo-path> <store-path>")?,
    );
    let store_path = PathBuf::from(
        args.next()
            .ok_or("usage: proxy_kpi <repo-path> <store-path>")?,
    );

    let store = ChunkStore::open(&store_path)?;
    let embedder =
        MockEmbedder::new(store.matrix().dims()).with_model_id(store.matrix().model_id());
    let lexical = LexicalIndex::open(store.dir())?;
    let dense = DenseIndex::from_store(&store, DenseBackend::Cpu)?;
    let searcher = Searcher::new(&lexical, &dense, &store, &embedder);

    let (labels, _) = mine_commits(
        &repo,
        &store,
        &MiningConfig {
            enforce_pinned_revision: false,
            max_commits: Some(200),
            ..MiningConfig::default()
        },
    )?;

    let kpi = median_bytes_before_hit(&searcher, &repo, &labels, &FusionConfig::default())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "labels": labels.len(),
            "mcp_median_bytes": kpi.mcp_median,
            "baseline_median_bytes": kpi.baseline_median,
            "baseline_command": kpi.baseline_command,
        }))?
    );
    Ok(())
}
