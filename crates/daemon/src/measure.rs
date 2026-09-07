//! Observability overhead measurement helpers (CPU-testable aggregation).

use std::path::PathBuf;

/// Nearest-rank percentile. For n=100 and p=95 returns index 94 (0-based) value
/// after sorting ascending — i.e. the 95th value in 1..=100 is 95.
pub fn nearest_rank_percentile(sorted_asc: &[u64], percentile: u8) -> Option<u64> {
    if sorted_asc.is_empty() || percentile == 0 || percentile > 100 {
        return None;
    }
    let n = sorted_asc.len();
    let rank = ((percentile as usize) * n).div_ceil(100);
    let idx = rank.saturating_sub(1).min(n - 1);
    Some(sorted_asc[idx])
}

pub fn median_u64(values: &[u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    let mid = v.len() / 2;
    if v.len().is_multiple_of(2) {
        Some((v[mid - 1] + v[mid]) / 2)
    } else {
        Some(v[mid])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasureArgs {
    pub repo: PathBuf,
    pub store: PathBuf,
    pub model: PathBuf,
    pub daemon: PathBuf,
    pub runs: u32,
    pub output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeasureArgsError {
    Missing(&'static str),
    InvalidRuns(String),
    UnknownFlag(String),
}

impl std::fmt::Display for MeasureArgsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(name) => write!(f, "missing required argument --{name}"),
            Self::InvalidRuns(s) => write!(f, "invalid --runs value: {s}"),
            Self::UnknownFlag(s) => write!(f, "unknown argument: {s}"),
        }
    }
}

pub fn parse_measure_args(args: &[String]) -> Result<MeasureArgs, MeasureArgsError> {
    let mut repo = None;
    let mut store = None;
    let mut model = None;
    let mut daemon = None;
    let mut runs = 3u32;
    let mut output = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--repo" => {
                i += 1;
                repo = Some(PathBuf::from(
                    args.get(i).ok_or(MeasureArgsError::Missing("repo"))?,
                ));
            }
            "--store" => {
                i += 1;
                store = Some(PathBuf::from(
                    args.get(i).ok_or(MeasureArgsError::Missing("store"))?,
                ));
            }
            "--model" => {
                i += 1;
                model = Some(PathBuf::from(
                    args.get(i).ok_or(MeasureArgsError::Missing("model"))?,
                ));
            }
            "--daemon" => {
                i += 1;
                daemon = Some(PathBuf::from(
                    args.get(i).ok_or(MeasureArgsError::Missing("daemon"))?,
                ));
            }
            "--runs" => {
                i += 1;
                let raw = args.get(i).ok_or(MeasureArgsError::Missing("runs"))?;
                runs = raw
                    .parse()
                    .map_err(|_| MeasureArgsError::InvalidRuns(raw.clone()))?;
                if runs == 0 {
                    return Err(MeasureArgsError::InvalidRuns(raw.clone()));
                }
            }
            "--output" => {
                i += 1;
                output = Some(PathBuf::from(
                    args.get(i).ok_or(MeasureArgsError::Missing("output"))?,
                ));
            }
            other => return Err(MeasureArgsError::UnknownFlag(other.to_owned())),
        }
        i += 1;
    }
    Ok(MeasureArgs {
        repo: repo.ok_or(MeasureArgsError::Missing("repo"))?,
        store: store.ok_or(MeasureArgsError::Missing("store"))?,
        model: model.ok_or(MeasureArgsError::Missing("model"))?,
        daemon: daemon.ok_or(MeasureArgsError::Missing("daemon"))?,
        runs,
        output: output.ok_or(MeasureArgsError::Missing("output"))?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleMeasureArgs {
    pub repo: PathBuf,
    pub store: PathBuf,
    pub model: PathBuf,
    pub daemon: PathBuf,
    pub console: PathBuf,
    pub runs: u32,
    pub output: PathBuf,
}

pub fn parse_console_measure_args(args: &[String]) -> Result<ConsoleMeasureArgs, MeasureArgsError> {
    let mut repo = None;
    let mut store = None;
    let mut model = None;
    let mut daemon = None;
    let mut console = None;
    let mut runs = 3u32;
    let mut output = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--repo" => {
                i += 1;
                repo = Some(PathBuf::from(
                    args.get(i).ok_or(MeasureArgsError::Missing("repo"))?,
                ));
            }
            "--store" => {
                i += 1;
                store = Some(PathBuf::from(
                    args.get(i).ok_or(MeasureArgsError::Missing("store"))?,
                ));
            }
            "--model" => {
                i += 1;
                model = Some(PathBuf::from(
                    args.get(i).ok_or(MeasureArgsError::Missing("model"))?,
                ));
            }
            "--daemon" => {
                i += 1;
                daemon = Some(PathBuf::from(
                    args.get(i).ok_or(MeasureArgsError::Missing("daemon"))?,
                ));
            }
            "--console" => {
                i += 1;
                console = Some(PathBuf::from(
                    args.get(i).ok_or(MeasureArgsError::Missing("console"))?,
                ));
            }
            "--runs" => {
                i += 1;
                let raw = args.get(i).ok_or(MeasureArgsError::Missing("runs"))?;
                runs = raw
                    .parse()
                    .map_err(|_| MeasureArgsError::InvalidRuns(raw.clone()))?;
                if runs == 0 {
                    return Err(MeasureArgsError::InvalidRuns(raw.clone()));
                }
            }
            "--output" => {
                i += 1;
                output = Some(PathBuf::from(
                    args.get(i).ok_or(MeasureArgsError::Missing("output"))?,
                ));
            }
            other => return Err(MeasureArgsError::UnknownFlag(other.to_owned())),
        }
        i += 1;
    }
    Ok(ConsoleMeasureArgs {
        repo: repo.ok_or(MeasureArgsError::Missing("repo"))?,
        store: store.ok_or(MeasureArgsError::Missing("store"))?,
        model: model.ok_or(MeasureArgsError::Missing("model"))?,
        daemon: daemon.ok_or(MeasureArgsError::Missing("daemon"))?,
        console: console.ok_or(MeasureArgsError::Missing("console"))?,
        runs,
        output: output.ok_or(MeasureArgsError::Missing("output"))?,
    })
}

/// Rotates off/on order between paired runs to limit ordering bias.
/// Run 0: [false, true] (off then on)
/// Run 1: [true, false] (on then off)
/// Run 2: [false, true] (off then on)
pub fn pair_order(run_index: u32) -> [bool; 2] {
    if run_index.is_multiple_of(2) {
        [false, true]
    } else {
        [true, false]
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ConsoleRunReport {
    pub collection: bool,
    pub run_index: u32,
    pub latencies_micros: Vec<u64>,
    pub p50_micros: u64,
    pub p95_micros: u64,
    pub daemon_rss_bytes: Option<u64>,
    pub console_rss_bytes: Option<u64>,
    pub device_id: Option<String>,
    pub device_used_bytes: Option<u64>,
    pub process_used_bytes: Option<u64>,
    pub sample_count: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ConsoleSummary {
    pub runs_count: u32,
    pub collection_off_p50_median: Option<u64>,
    pub collection_off_p95_median: Option<u64>,
    pub collection_on_p50_median: Option<u64>,
    pub collection_on_p95_median: Option<u64>,
    pub daemon_rss_off_median: Option<u64>,
    pub daemon_rss_on_median: Option<u64>,
    pub console_rss_off_median: Option<u64>,
    pub console_rss_on_median: Option<u64>,
    pub relative_p95_change_percent: Option<f64>,
    pub runs: Vec<ConsoleRunReport>,
}

pub fn aggregate_console_summary(runs: Vec<ConsoleRunReport>) -> ConsoleSummary {
    let off_p50: Vec<u64> = runs
        .iter()
        .filter(|r| !r.collection)
        .map(|r| r.p50_micros)
        .collect();
    let off_p95: Vec<u64> = runs
        .iter()
        .filter(|r| !r.collection)
        .map(|r| r.p95_micros)
        .collect();
    let on_p50: Vec<u64> = runs
        .iter()
        .filter(|r| r.collection)
        .map(|r| r.p50_micros)
        .collect();
    let on_p95: Vec<u64> = runs
        .iter()
        .filter(|r| r.collection)
        .map(|r| r.p95_micros)
        .collect();

    let daemon_rss_off: Vec<u64> = runs
        .iter()
        .filter(|r| !r.collection)
        .filter_map(|r| r.daemon_rss_bytes)
        .collect();
    let daemon_rss_on: Vec<u64> = runs
        .iter()
        .filter(|r| r.collection)
        .filter_map(|r| r.daemon_rss_bytes)
        .collect();
    let console_rss_off: Vec<u64> = runs
        .iter()
        .filter(|r| !r.collection)
        .filter_map(|r| r.console_rss_bytes)
        .collect();
    let console_rss_on: Vec<u64> = runs
        .iter()
        .filter(|r| r.collection)
        .filter_map(|r| r.console_rss_bytes)
        .collect();

    let off_p95_med = median_u64(&off_p95);
    let on_p95_med = median_u64(&on_p95);

    let relative_p95 = match (off_p95_med, on_p95_med) {
        (Some(off), Some(on)) if off > 0 => Some(((on as f64 - off as f64) / off as f64) * 100.0),
        _ => None,
    };

    let runs_count = runs.len() as u32;

    ConsoleSummary {
        runs_count,
        collection_off_p50_median: median_u64(&off_p50),
        collection_off_p95_median: off_p95_med,
        collection_on_p50_median: median_u64(&on_p50),
        collection_on_p95_median: on_p95_med,
        daemon_rss_off_median: median_u64(&daemon_rss_off),
        daemon_rss_on_median: median_u64(&daemon_rss_on),
        console_rss_off_median: median_u64(&console_rss_off),
        console_rss_on_median: median_u64(&console_rss_on),
        relative_p95_change_percent: relative_p95,
        runs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_p95_of_1_to_100_is_95() {
        let values: Vec<u64> = (1..=100).collect();
        assert_eq!(nearest_rank_percentile(&values, 95), Some(95));
        assert_eq!(nearest_rank_percentile(&values, 50), Some(50));
    }

    #[test]
    fn parse_requires_paths_and_rejects_zero_runs() {
        let err = parse_measure_args(&[]).unwrap_err();
        assert!(matches!(err, MeasureArgsError::Missing("repo")));

        let args = [
            "--repo", "/r", "--store", "/s", "--model", "/m", "--daemon", "/d", "--runs", "0",
            "--output", "/o",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        assert!(matches!(
            parse_measure_args(&args),
            Err(MeasureArgsError::InvalidRuns(_))
        ));
    }

    #[test]
    fn parse_accepts_valid_args() {
        let args = [
            "--repo", "/r", "--store", "/s", "--model", "/m", "--daemon", "/d", "--runs", "3",
            "--output", "/o",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let parsed = parse_measure_args(&args).unwrap();
        assert_eq!(parsed.runs, 3);
        assert_eq!(parsed.repo, PathBuf::from("/r"));
    }

    #[test]
    fn pair_order_alternates_off_and_on() {
        assert_eq!(pair_order(0), [false, true]);
        assert_eq!(pair_order(1), [true, false]);
        assert_eq!(pair_order(2), [false, true]);
    }

    #[test]
    fn parse_console_measure_args_handles_valid_and_missing() {
        let err = parse_console_measure_args(&[]).unwrap_err();
        assert!(matches!(err, MeasureArgsError::Missing("repo")));

        let args = [
            "--repo",
            "/r",
            "--store",
            "/s",
            "--model",
            "/m",
            "--daemon",
            "/d",
            "--console",
            "/c",
            "--runs",
            "3",
            "--output",
            "/o",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let parsed = parse_console_measure_args(&args).unwrap();
        assert_eq!(parsed.runs, 3);
        assert_eq!(parsed.console, PathBuf::from("/c"));
    }

    #[test]
    fn aggregate_console_summary_computes_medians_and_p95_change() {
        let make_run = |collection: bool,
                        run_index: u32,
                        p50: u64,
                        p95: u64,
                        daemon_rss: u64,
                        console_rss: u64| ConsoleRunReport {
            collection,
            run_index,
            latencies_micros: vec![p50, p95],
            p50_micros: p50,
            p95_micros: p95,
            daemon_rss_bytes: Some(daemon_rss),
            console_rss_bytes: Some(console_rss),
            device_id: Some("GPU-uuid".into()),
            device_used_bytes: Some(1024),
            process_used_bytes: Some(512),
            sample_count: 20,
        };

        // 3 pairs (3 off, 3 on)
        let runs = vec![
            make_run(false, 0, 100, 200, 10_000, 5_000),
            make_run(true, 0, 110, 220, 10_500, 5_500),
            make_run(true, 1, 115, 230, 10_600, 5_600),
            make_run(false, 1, 105, 210, 10_100, 5_100),
            make_run(false, 2, 95, 190, 9_900, 4_900),
            make_run(true, 2, 108, 215, 10_400, 5_400),
        ];

        let summary = aggregate_console_summary(runs);
        assert_eq!(summary.runs_count, 6);
        // Off p50s: [95, 100, 105] -> median = 100
        assert_eq!(summary.collection_off_p50_median, Some(100));
        // Off p95s: [190, 200, 210] -> median = 200
        assert_eq!(summary.collection_off_p95_median, Some(200));

        // On p50s: [108, 110, 115] -> median = 110
        assert_eq!(summary.collection_on_p50_median, Some(110));
        // On p95s: [215, 220, 230] -> median = 220
        assert_eq!(summary.collection_on_p95_median, Some(220));

        // Daemon and console RSS tracked separately
        assert_eq!(summary.daemon_rss_off_median, Some(10_000));
        assert_eq!(summary.daemon_rss_on_median, Some(10_500));
        assert_eq!(summary.console_rss_off_median, Some(5_000));
        assert_eq!(summary.console_rss_on_median, Some(5_500));

        // Relative p95 change: (220 - 200) / 200 * 100 = 10%
        assert_eq!(summary.relative_p95_change_percent, Some(10.0));
    }

    #[test]
    fn aggregate_handles_unavailable_baseline_gracefully() {
        // When baseline off p95 is 0 or empty, relative change is None
        let make_run = |collection: bool, p95: u64| ConsoleRunReport {
            collection,
            run_index: 0,
            latencies_micros: vec![],
            p50_micros: 0,
            p95_micros: p95,
            daemon_rss_bytes: None,
            console_rss_bytes: None,
            device_id: None,
            device_used_bytes: None,
            process_used_bytes: None,
            sample_count: 0,
        };

        let summary = aggregate_console_summary(vec![make_run(false, 0), make_run(true, 100)]);
        assert_eq!(summary.relative_p95_change_percent, None);
    }
}
