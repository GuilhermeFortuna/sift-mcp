//! Index a repository and print timings / optional VRAM report.
//!
//! Usage:
//!   cargo run -p indexing --example index_repo -- <repo-path> [--timing] [--report-vram]
//!
//! Uses MockEmbedder by default (CPU-only). With `--features cuda` and
//! `--report-vram`, loads OnnxEmbedder when `SIFT_MODEL_DIR` is set.

use std::env;
use std::path::PathBuf;
use std::process;

use indexing::{IndexConfig, Indexer, Phase, Progress, require_verify_ok};
use inference::{Embedder, MockEmbedder};
use storage::ChunkStore;

struct PrintProgress;

impl Progress for PrintProgress {
    fn phase(&mut self, phase: Phase, done: u64, total: Option<u64>) {
        let name = match phase {
            Phase::Walking => "walking",
            Phase::Parsing => "parsing",
            Phase::Embedding => "embedding",
            Phase::Storing => "storing",
            Phase::Compacting => "compacting",
        };
        match total {
            Some(t) => eprintln!("[{name}] {done}/{t}"),
            None => eprintln!("[{name}] {done}"),
        }
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut timing = false;
    let mut report_vram = false;
    let mut repo: Option<PathBuf> = None;
    for a in env::args().skip(1) {
        match a.as_str() {
            "--timing" => timing = true,
            "--report-vram" => report_vram = true,
            _ if repo.is_none() => repo = Some(PathBuf::from(a)),
            _ => {
                return Err(format!("unexpected arg: {a}").into());
            }
        }
    }
    let repo = repo.ok_or("usage: index_repo <repo-path> [--timing] [--report-vram]")?;

    let store_dir = repo.join(".sift-index");
    std::fs::create_dir_all(&store_dir)?;

    let embedder = MockEmbedder::new(384).with_batch_limit(32);
    let model_id = embedder.model_id().to_string();

    let store = if store_dir.join("chunks.db").exists() {
        ChunkStore::open(&store_dir)?
    } else {
        ChunkStore::create(&store_dir, embedder.dims(), &model_id)?
    };

    let config = IndexConfig::default();
    let mut indexer = Indexer::open(store, &embedder, &repo, config)?;
    let mut progress = PrintProgress;
    let report = indexer.index_all(&mut progress)?;
    require_verify_ok(indexer.store())?;

    println!("commit={}", report.commit);
    println!("files_seen={}", report.files_seen);
    println!("files_indexed={}", report.files_indexed);
    println!("files_excluded={}", report.files_excluded);
    println!("files_unsupported={}", report.files_unsupported);
    println!("files_unparsed={}", report.files_unparsed);
    println!("chunks_added={}", report.chunks_added);
    println!("chunks_reused={}", report.chunks_reused);
    println!("chunks_removed={}", report.chunks_removed);
    println!("embeddings_computed={}", report.embeddings_computed);
    println!("chunks_truncated={}", report.chunks_truncated);
    println!("live={}", report.live_after);

    if timing {
        println!("wall_millis={}", report.wall_millis);
        println!("parse_millis={}", report.parse_millis);
        println!("embed_millis={}", report.embed_millis);
        println!("store_millis={}", report.store_millis);
    }

    if report_vram {
        println!(
            "peak_vram_bytes=n/a (MockEmbedder; measure with a CUDA OnnxEmbedder on the target machine)"
        );
    }

    if let Some(c) = &report.compacted {
        println!(
            "compacted live_before={} dead_reclaimed={} live_after={}",
            c.live_before, c.dead_reclaimed, c.live_after
        );
    }

    let store_size: u64 = walkdir_size(&store_dir)?;
    println!("store_bytes={store_size}");
    Ok(())
}

fn walkdir_size(root: &std::path::Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    Ok(total)
}
