use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use retrieval::LexicalIndex;

const WARMUP_QUERIES: usize = 20;
const QUERY_FIXTURES: &[&str] = &[
    "decoder timestamps",
    "normalize timestamp",
    "connection reset by peer",
    "parseHTTPResponse",
    "monotonic order",
    "error handling",
];

fn main() {
    let (store_path, query_count, open_only) = parse_args();
    let lexical_path = store_path.join("lexical");

    if open_only {
        let started = Instant::now();
        let index = LexicalIndex::open(&store_path).unwrap_or_else(|error| {
            eprintln!("failed to open lexical index: {error}");
            std::process::exit(1);
        });
        let elapsed = started.elapsed();
        println!(
            "open_only=true open_millis={:.3} index_docs={} lexical_bytes={}",
            elapsed.as_secs_f64() * 1_000.0,
            index.num_docs(),
            regular_file_bytes(&lexical_path)
        );
        return;
    }

    let index = LexicalIndex::open(&store_path).unwrap_or_else(|error| {
        eprintln!("failed to open lexical index: {error}");
        std::process::exit(1);
    });
    for query_number in 0..WARMUP_QUERIES {
        let query = QUERY_FIXTURES[query_number % QUERY_FIXTURES.len()];
        let _ = index.search(query, 20).unwrap_or_else(|error| {
            eprintln!("warm-up search failed: {error}");
            std::process::exit(1);
        });
    }

    let mut samples = Vec::with_capacity(query_count);
    for query_number in 0..query_count {
        let query = QUERY_FIXTURES[query_number % QUERY_FIXTURES.len()];
        let started = Instant::now();
        let _ = index.search(query, 20).unwrap_or_else(|error| {
            eprintln!("timed search failed: {error}");
            std::process::exit(1);
        });
        samples.push(started.elapsed());
    }

    samples.sort_unstable();
    let median = percentile(&samples, 0.50);
    let p95 = percentile(&samples, 0.95);
    println!(
        "queries={} warmup={} median_millis={:.3} p95_millis={:.3} index_docs={}",
        query_count,
        WARMUP_QUERIES,
        median.as_secs_f64() * 1_000.0,
        p95.as_secs_f64() * 1_000.0,
        index.num_docs()
    );
}

fn parse_args() -> (PathBuf, usize, bool) {
    let mut arguments = env::args().skip(1);
    let Some(store_path) = arguments.next() else {
        usage_and_exit();
    };
    let mut query_count = 200;
    let mut open_only = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--open-only" => open_only = true,
            "--queries" => {
                query_count = arguments
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_else(|| usage_and_exit());
            }
            _ => usage_and_exit(),
        }
    }
    (PathBuf::from(store_path), query_count, open_only)
}

fn usage_and_exit() -> ! {
    eprintln!("usage: bench_lexical <store-path> [--queries N] [--open-only]");
    std::process::exit(2);
}

fn percentile(samples: &[std::time::Duration], fraction: f64) -> std::time::Duration {
    if samples.is_empty() {
        return std::time::Duration::ZERO;
    }
    let rank = (fraction * samples.len() as f64).ceil() as usize;
    samples[rank.saturating_sub(1).min(samples.len() - 1)]
}

fn regular_file_bytes(path: &Path) -> u64 {
    let Ok(metadata) = fs::metadata(path) else {
        return 0;
    };
    if metadata.is_file() {
        return metadata.len();
    }
    if !metadata.is_dir() {
        return 0;
    }
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| regular_file_bytes(&entry.path()))
        .sum()
}
