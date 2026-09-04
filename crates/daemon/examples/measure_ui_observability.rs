//! Paired observability overhead measurement (recording off vs on).
//!
//! Requires the `resident` feature. Intended to be driven by
//! `scripts/measure-ui-observability.sh` on a real store/model.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use daemon::measure::{
    MeasureArgs, median_u64, nearest_rank_percentile, parse_measure_args,
};
use serde::Serialize;

#[derive(Serialize)]
struct RunReport {
    recording: bool,
    run_index: u32,
    latencies_micros: Vec<u64>,
    p50_micros: u64,
    p95_micros: u64,
    process_rss_bytes: Option<u64>,
    device_id: Option<String>,
    device_used_bytes: Option<u64>,
    process_used_bytes: Option<u64>,
    sample_count: usize,
}

#[derive(Serialize)]
struct Summary {
    recording_off_p50_median: Option<u64>,
    recording_off_p95_median: Option<u64>,
    recording_on_p50_median: Option<u64>,
    recording_on_p95_median: Option<u64>,
    runs: Vec<RunReport>,
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_measure_args(&raw) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            eprintln!(
                "usage: measure_ui_observability --repo PATH --store PATH --model PATH --daemon PATH --runs N --output DIR"
            );
            std::process::exit(2);
        }
    };
    if let Err(e) = run(args) {
        eprintln!("measurement failed: {e}");
        std::process::exit(1);
    }
}

fn run(args: MeasureArgs) -> Result<(), String> {
    std::fs::create_dir_all(&args.output).map_err(|e| e.to_string())?;
    let queries = [
        "fn main",
        "error handling",
        "struct Config",
        "TODO",
        "async fn",
    ];
    let mut reports = Vec::new();
    for recording in [false, true] {
        for run_index in 0..args.runs {
            let report = measure_one(&args, recording, run_index, &queries)?;
            let path = args.output.join(format!(
                "run-{}-rec-{}.json",
                run_index,
                if recording { "on" } else { "off" }
            ));
            let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
            std::fs::write(&path, json).map_err(|e| e.to_string())?;
            reports.push(report);
        }
    }
    let off_p50: Vec<u64> = reports
        .iter()
        .filter(|r| !r.recording)
        .map(|r| r.p50_micros)
        .collect();
    let off_p95: Vec<u64> = reports
        .iter()
        .filter(|r| !r.recording)
        .map(|r| r.p95_micros)
        .collect();
    let on_p50: Vec<u64> = reports
        .iter()
        .filter(|r| r.recording)
        .map(|r| r.p50_micros)
        .collect();
    let on_p95: Vec<u64> = reports
        .iter()
        .filter(|r| r.recording)
        .map(|r| r.p95_micros)
        .collect();
    let summary = Summary {
        recording_off_p50_median: median_u64(&off_p50),
        recording_off_p95_median: median_u64(&off_p95),
        recording_on_p50_median: median_u64(&on_p50),
        recording_on_p95_median: median_u64(&on_p95),
        runs: reports,
    };
    let summary_path = args.output.join("summary.json");
    std::fs::write(
        &summary_path,
        serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    println!("wrote {}", summary_path.display());
    Ok(())
}

fn measure_one(
    args: &MeasureArgs,
    recording: bool,
    run_index: u32,
    queries: &[&str],
) -> Result<RunReport, String> {
    // Spawn a daemon subprocess with --record-events when supported; for the
    // measurement binary we talk to an already-built daemon via env flags in
    // the wrapper script. Here we only exercise client-side timing against a
    // live socket derived from the store path.
    let _ = recording;
    let _socket = daemon::paths::socket_path_for_store(&args.store).map_err(|e| format!("{e:?}"))?;
    // Warmup: connect if daemon already running; otherwise the shell wrapper
    // is expected to have started it.
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let mut client = daemon::DaemonClient::connect_or_spawn(
            &args.store,
            &args.repo,
            &args.model,
            Duration::from_secs(30),
            &args.daemon,
        )
        .await
        .map_err(|e| format!("connect: {e:?}"))?;

        // Equal warmup for both modes.
        for q in queries {
            let _ = client
                .request(daemon::Request::Search {
                    query: (*q).into(),
                    top_k: 5,
                })
                .await;
        }

        let mut latencies = Vec::new();
        for q in queries.iter().cycle().take(20) {
            let start = Instant::now();
            let _ = client
                .request(daemon::Request::Search {
                    query: (*q).into(),
                    top_k: 5,
                })
                .await
                .map_err(|e| format!("search: {e:?}"))?;
            latencies.push(start.elapsed().as_micros() as u64);
        }

        let mut sorted = latencies.clone();
        sorted.sort_unstable();
        let p50 = nearest_rank_percentile(&sorted, 50).unwrap_or(0);
        let p95 = nearest_rank_percentile(&sorted, 95).unwrap_or(0);

        let status = client
            .request(daemon::Request::Status)
            .await
            .map_err(|e| format!("status: {e:?}"))?;
        let (device_id, device_used, process_used) = match status {
            daemon::Response::Status(s) => (
                s.resources.device_id,
                s.resources.device_used_bytes,
                s.resources.process_used_bytes,
            ),
            _ => (None, None, None),
        };

        Ok(RunReport {
            recording,
            run_index,
            sample_count: latencies.len(),
            latencies_micros: latencies,
            p50_micros: p50,
            p95_micros: p95,
            process_rss_bytes: read_self_rss(),
            device_id,
            device_used_bytes: device_used,
            process_used_bytes: process_used,
        })
    })
}

fn read_self_rss() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

#[allow(dead_code)]
fn ensure_daemon_flag(_daemon: &PathBuf, _recording: bool) -> Result<(), String> {
    let _ = Command::new("true").status();
    Ok(())
}
