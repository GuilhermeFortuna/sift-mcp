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
}
