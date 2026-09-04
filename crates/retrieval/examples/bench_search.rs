//! Benchmark end-to-end fused search with optional per-stage timings.
//!
//! Usage:
//!   cargo run --release -p retrieval --example bench_search -- <store-path> \
//!     --queries 200 --stage-timings
//!
//! Uses MockEmbedder matched to the store's model_id (CPU). Measure absolute
//! dense quality with a CUDA OnnxEmbedder against a store built by the same model.

use std::env;
use std::path::PathBuf;
use std::process;
use std::time::Instant;

use inference::MockEmbedder;
use retrieval::dense::{DenseBackend, DenseIndex};
use retrieval::{FusionConfig, LexicalIndex, Searcher};
use storage::ChunkStore;

const WARMUP_QUERIES: usize = 20;

const QUERY_FIXTURES: &[&str] = &[
    "decoder timestamps",
    "normalize timestamp",
    "connection reset by peer",
    "parseHTTPResponse",
    "monotonic order",
    "error handling",
    "where are decoder timestamps clamped",
    "flush pending writes",
];

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (store_path, query_count, stage_timings) = parse_args()?;
    let store = ChunkStore::open(&store_path)?;
    let live = store.stats()?.live;
    let lexical = LexicalIndex::open(store.dir())?;
    let dense = DenseIndex::from_store(&store, DenseBackend::Cpu)?;
    let embedder =
        MockEmbedder::new(store.matrix().dims()).with_model_id(store.matrix().model_id());
    let searcher = Searcher::new(&lexical, &dense, &store, &embedder);
    let config = FusionConfig::default();

    for query_number in 0..WARMUP_QUERIES {
        let query = QUERY_FIXTURES[query_number % QUERY_FIXTURES.len()];
        let _ = searcher.search(query, 10, &config)?;
    }

    let mut totals = Vec::with_capacity(query_count);
    let mut embeds = Vec::with_capacity(query_count);
    let mut lexicals = Vec::with_capacity(query_count);
    let mut denses = Vec::with_capacity(query_count);
    let mut fuses = Vec::with_capacity(query_count);
    let mut assembles = Vec::with_capacity(query_count);
    let mut union_sizes = Vec::with_capacity(query_count);

    for query_number in 0..query_count {
        let query = QUERY_FIXTURES[query_number % QUERY_FIXTURES.len()];
        let started = Instant::now();
        let response = searcher.search(query, 10, &config)?;
        let wall = started.elapsed().as_secs_f64() * 1_000.0;
        let stages = &response.diagnostics.stage_millis;
        totals.push(wall);
        embeds.push(stages.embed as f64);
        lexicals.push(stages.lexical as f64);
        denses.push(stages.dense as f64);
        fuses.push(stages.fuse as f64);
        assembles.push(stages.assemble as f64);
        // Approximate union size from returned fused candidates before top_k trim
        // is not exposed; report result count as a lower bound and candidate depths.
        union_sizes.push(response.results.len() as f64);
        let _ = stages.total;
    }

    totals.sort_by(f64::total_cmp);
    println!(
        "queries={} warmup={} live_chunks={} median_millis={:.3} p95_millis={:.3}",
        query_count,
        WARMUP_QUERIES,
        live,
        percentile(&totals, 0.50),
        percentile(&totals, 0.95)
    );
    println!(
        "fusion_defaults rrf_k={} lexical_depth={} dense_depth={}",
        config.rrf_k, config.lexical_depth, config.dense_depth
    );
    println!(
        "mean_returned_results={:.2}",
        union_sizes.iter().sum::<f64>() / union_sizes.len().max(1) as f64
    );

    if stage_timings {
        embeds.sort_by(f64::total_cmp);
        lexicals.sort_by(f64::total_cmp);
        denses.sort_by(f64::total_cmp);
        fuses.sort_by(f64::total_cmp);
        assembles.sort_by(f64::total_cmp);
        println!(
            "stage_median_millis embed={:.3} lexical={:.3} dense={:.3} fuse={:.3} assemble={:.3}",
            percentile(&embeds, 0.50),
            percentile(&lexicals, 0.50),
            percentile(&denses, 0.50),
            percentile(&fuses, 0.50),
            percentile(&assembles, 0.50)
        );
        println!(
            "stage_p95_millis embed={:.3} lexical={:.3} dense={:.3} fuse={:.3} assemble={:.3}",
            percentile(&embeds, 0.95),
            percentile(&lexicals, 0.95),
            percentile(&denses, 0.95),
            percentile(&fuses, 0.95),
            percentile(&assembles, 0.95)
        );
    }
    Ok(())
}

fn parse_args() -> Result<(PathBuf, usize, bool), Box<dyn std::error::Error>> {
    let mut arguments = env::args().skip(1);
    let store_path = PathBuf::from(
        arguments
            .next()
            .ok_or("usage: bench_search <store-path> [--queries N] [--stage-timings]")?,
    );
    let mut query_count = 200;
    let mut stage_timings = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--stage-timings" => stage_timings = true,
            "--queries" => {
                query_count = arguments
                    .next()
                    .ok_or("missing value for --queries")?
                    .parse()?;
            }
            other => return Err(format!("unexpected argument: {other}").into()),
        }
    }
    Ok((store_path, query_count, stage_timings))
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (fraction * sorted.len() as f64).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}
