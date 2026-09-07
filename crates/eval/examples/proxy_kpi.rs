//! Proxy efficiency KPI: MCP bytes vs keyword baseline.
//!
//! ```text
//! cargo run --release -p eval --features cuda --example proxy_kpi -- <repo-path> <store-path> --model <model-dir>
//! ```

use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::process;

use eval::{MiningConfig, median_bytes_before_hit, mine_commits};
use inference::{Embedder, OnnxEmbedder};
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
    let args: Vec<String> = env::args().skip(1).collect();
    let repo = PathBuf::from(
        args.first()
            .ok_or("usage: proxy_kpi <repo-path> <store-path> --model <model-dir>")?,
    );
    let store_path = PathBuf::from(
        args.get(1)
            .ok_or("usage: proxy_kpi <repo-path> <store-path> --model <model-dir>")?,
    );
    let model_pos = args
        .iter()
        .position(|arg| arg == "--model")
        .ok_or("proxy_kpi requires --model <model-dir>")?;
    let model_dir = PathBuf::from(
        args.get(model_pos + 1)
            .ok_or("proxy_kpi requires --model <model-dir>")?,
    );

    let store = ChunkStore::open(&store_path)?;
    let embedder = OnnxEmbedder::load(&model_dir, 32)?;
    store.require_model(embedder.model_id())?;
    let lexical = LexicalIndex::open(store.dir())?;
    let dense = DenseIndex::from_store(&store, DenseBackend::Cuda)?;
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
