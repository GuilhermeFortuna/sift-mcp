//! Print fused search results as JSON for a query.
//!
//! Usage:
//!   cargo run -p retrieval --example search -- <store-path> --query "<question>"
//!
//! Uses MockEmbedder with the store's model_id/dims (CPU). For meaningful dense
//! ranking against a GPU-indexed store, rebuild with `--features cuda` and set
//! `SIFT_MODEL_DIR`.

use std::env;
use std::path::PathBuf;
use std::process;

use inference::MockEmbedder;
use retrieval::dense::{DenseBackend, DenseIndex};
use retrieval::{FusionConfig, LexicalIndex, Searcher};
use storage::ChunkStore;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut store_path: Option<PathBuf> = None;
    let mut query: Option<String> = None;
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--query" => {
                query = Some(args.next().ok_or("missing value for --query")?);
            }
            other if store_path.is_none() => store_path = Some(PathBuf::from(other)),
            other => return Err(format!("unexpected argument: {other}").into()),
        }
    }
    let store_path = store_path.ok_or("usage: search <store-path> --query \"<question>\"")?;
    let query = query.ok_or("missing --query")?;

    let store = ChunkStore::open(&store_path)?;
    let lexical = LexicalIndex::open(store.dir())?;
    let dense = DenseIndex::from_store(&store, DenseBackend::Cpu)?;
    let embedder =
        MockEmbedder::new(store.matrix().dims()).with_model_id(store.matrix().model_id());

    let searcher = Searcher::new(&lexical, &dense, &store, &embedder);
    let response = searcher.search(&query, 10, &FusionConfig::default())?;
    println!("{}", serde_json::to_string_pretty(&response.results)?);
    eprintln!(
        "diagnostics lexical_ok={} dense_ok={} total_millis={}",
        response.diagnostics.lexical_ok,
        response.diagnostics.dense_ok,
        response.diagnostics.stage_millis.total
    );
    if let Some(error) = response.diagnostics.lexical_error {
        eprintln!("lexical_error={error}");
    }
    if let Some(error) = response.diagnostics.dense_error {
        eprintln!("dense_error={error}");
    }
    Ok(())
}
