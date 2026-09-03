use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use half::f16;
use inference::{Embedder, OnnxEmbedder};
use retrieval::dense::{DenseBackend, DenseIndex, LiveMask};

fn argument_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((sorted.len() as f64 * fraction).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[rank]
}

fn gpu_used_bytes() -> Option<u64> {
    let output = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|mib| mib * 1024 * 1024)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let sizes = argument_value(&args, "--sizes")
        .unwrap_or("200000")
        .split(',')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()?;
    let queries = argument_value(&args, "--queries")
        .map(str::parse::<usize>)
        .transpose()?
        .unwrap_or(0);
    let report_vram = args.iter().any(|argument| argument == "--report-vram");
    if queries == 0 && !report_vram {
        return Err("usage: bench_dense -- --sizes N[,N] [--queries N] [--report-vram]".into());
    }

    let embedder = if report_vram {
        let model_dir = env::var("SIFT_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models/primary")
            });
        Some(OnnxEmbedder::load(&model_dir, 32).map_err(|error| {
            format!("load embedding model from {}: {error}", model_dir.display())
        })?)
    } else {
        None
    };
    let dims = embedder.as_ref().map_or(1024, Embedder::dims);
    let model_id = embedder
        .as_ref()
        .map_or_else(|| "bench-dense".to_owned(), |model| model.model_id().to_owned());
    let value = f16::from_f32(1.0 / (dims as f32).sqrt());
    let query = vec![value; dims as usize];

    for rows in sizes {
        let matrix = vec![value; rows as usize * dims as usize];
        let live = LiveMask::all_live(rows);
        let gpu_before = report_vram.then(gpu_used_bytes).flatten();
        let prepare_started = Instant::now();
        let index = DenseIndex::prepare_slice(
            &matrix,
            rows,
            dims,
            &model_id,
            &live,
            DenseBackend::Cuda,
        )?;
        let prepare_ms = prepare_started.elapsed().as_secs_f64() * 1_000.0;
        drop(matrix);

        if queries > 0 {
            for _ in 0..10 {
                let _ = index.search(&query, &model_id, 50)?;
            }
            let mut samples = Vec::with_capacity(queries);
            for _ in 0..queries {
                let started = Instant::now();
                let _ = index.search(&query, &model_id, 50)?;
                samples.push(started.elapsed().as_secs_f64() * 1_000.0);
            }
            samples.sort_by(f64::total_cmp);
            println!(
                "rows={rows} dims={dims} queries={queries} warmup=10 prepare_ms={prepare_ms:.3} median_ms={:.3} p95_ms={:.3} budget_ms=10",
                percentile(&samples, 0.50),
                percentile(&samples, 0.95)
            );
        }

        if report_vram {
            let gpu_after = gpu_used_bytes();
            println!("rows={rows} dense_resident_bytes={}", index.resident_bytes());
            match (gpu_before, gpu_after) {
                (Some(before), Some(after)) => println!(
                    "rows={rows} gpu_used_before_bytes={before} gpu_used_after_bytes={after} dense_attributable_bytes={} usable_budget_bytes={}",
                    after.saturating_sub(before),
                    5_u64 * 1024 * 1024 * 1024
                ),
                _ => println!("rows={rows} gpu_memory=n/a (nvidia-smi unavailable)"),
            }
        }
    }
    Ok(())
}
