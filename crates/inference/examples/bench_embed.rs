//! Benchmark embedding latency and batch throughput.
//!
//! Usage:
//!   cargo run --release -p inference --features cuda --example bench_embed -- --queries 100
//!   cargo run --release -p inference --features cuda --example bench_embed -- --batch-sweep --report-vram

use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use inference::{Embedder, OnnxEmbedder, Role};

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((p * (sorted.len() as f64 - 1.0)).round() as usize).min(sorted.len() - 1);
    sorted[rank]
}

fn query_vram_used_bytes() -> Option<u64> {
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let mb: u64 = s.lines().next()?.trim().parse().ok()?;
    Some(mb * 1024 * 1024)
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let queries = args
        .windows(2)
        .find(|w| w[0] == "--queries")
        .and_then(|w| w[1].parse::<usize>().ok())
        .unwrap_or(0);
    let batch_sweep = args.iter().any(|a| a == "--batch-sweep");
    let report_vram = args.iter().any(|a| a == "--report-vram");

    let model_dir = env::var("SIFT_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models/primary"));

    let max_batch = 32usize;
    let embedder = OnnxEmbedder::load(&model_dir, max_batch).unwrap_or_else(|e| {
        eprintln!("failed to load model from {}: {e}", model_dir.display());
        std::process::exit(1);
    });

    if queries > 0 {
        let warm = 10usize;
        for _ in 0..warm {
            let _ = embedder.embed(&["warm-up query"], Role::Query).unwrap();
        }
        let mut samples = Vec::with_capacity(queries);
        for i in 0..queries {
            let q = format!("latency sample query {i}");
            let t0 = Instant::now();
            let _ = embedder.embed(&[q.as_str()], Role::Query).unwrap();
            samples.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = percentile(&samples, 0.50);
        let p95 = percentile(&samples, 0.95);
        println!("single-query latency over {queries} runs after {warm} warm-up:");
        println!("  median = {median:.3} ms");
        println!("  p95    = {p95:.3} ms");
        println!("  budget = 15 ms (query embedding)");
    }

    if batch_sweep {
        let text = "fn example() { let x = 1; x + 2 }";
        let sizes = [1usize, 2, 4, 8, 16, 32];
        println!("batch throughput sweep:");
        for &bs in &sizes {
            if bs > max_batch {
                continue;
            }
            let batch: Vec<&str> = std::iter::repeat_n(text, bs).collect();
            // warm
            let _ = embedder.embed(&batch, Role::Document).unwrap();
            let runs = 20usize;
            let t0 = Instant::now();
            for _ in 0..runs {
                let _ = embedder.embed(&batch, Role::Document).unwrap();
            }
            let elapsed = t0.elapsed().as_secs_f64();
            let thr = (runs * bs) as f64 / elapsed;
            println!("  batch={bs:>2}  throughput={thr:.1} texts/s");
        }
        if report_vram {
            let batch: Vec<&str> = std::iter::repeat_n(text, max_batch).collect();
            let before = query_vram_used_bytes();
            let _ = embedder.embed(&batch, Role::Document).unwrap();
            let after = query_vram_used_bytes();
            if let (Some(b), Some(a)) = (before, after) {
                let peak = a.max(b);
                embedder.set_peak_gpu_bytes(peak);
                println!(
                    "peak GPU memory (nvidia-smi used) at batch={max_batch}: {:.2} GB",
                    peak as f64 / (1024.0 * 1024.0 * 1024.0)
                );
                println!("budget usable VRAM ~5.0 GB with desktop attached");
            } else {
                println!(
                    "peak_gpu_bytes (embedder): {} (nvidia-smi unavailable)",
                    embedder.peak_gpu_bytes()
                );
            }
        }
    }

    if queries == 0 && !batch_sweep {
        eprintln!("usage: bench_embed -- --queries N | --batch-sweep [--report-vram]");
        std::process::exit(2);
    }
}
