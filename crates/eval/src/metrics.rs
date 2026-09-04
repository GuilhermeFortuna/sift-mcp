//! Accuracy and latency metrics for evaluation runs.

/// A hit is an expected (file, symbol) appearing in the top n results.
#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    /// Reported beside every figure, per the spec.
    pub labels_scored: u64,
    /// Expected symbols absent from the index.
    pub labels_discarded: u64,
    pub top_1: f64,
    pub top_3: f64,
    pub top_10: f64,
    pub mrr: f64,
    pub latency_p50_ms: f64,
    pub latency_p95_ms: f64,
    pub peak_gpu_bytes: u64,
    pub bytes_before_hit: BytesBeforeHit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BytesBeforeHit {
    pub mcp_median: u64,
    /// Grep-style baseline over the same repository.
    pub baseline_median: u64,
    pub baseline_command: String,
}

impl Default for BytesBeforeHit {
    fn default() -> Self {
        Self {
            mcp_median: 0,
            baseline_median: 0,
            baseline_command: String::new(),
        }
    }
}

/// Nearest-rank on sorted samples, so a reported percentile means one thing.
pub fn percentile(sorted_ms: &[f64], fraction: f64) -> f64 {
    if sorted_ms.is_empty() {
        return 0.0;
    }
    let n = sorted_ms.len();
    let rank = ((fraction * n as f64).ceil() as usize).max(1).min(n);
    sorted_ms[rank - 1]
}

pub fn reciprocal_rank(
    _ranked: &[(String, String)],
    _expected: &[(String, String)],
) -> f64 {
    todo!("reciprocal_rank")
}

/// Fraction of labels whose expected symbol appears in the top `k` ranked results.
pub fn top_k_accuracy(
    _rankings: &[Vec<(String, String)>],
    _expected: &[Vec<(String, String)>],
    _k: usize,
) -> f64 {
    todo!("top_k_accuracy")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_p95_of_one_to_hundred_is_ninety_five() {
        let samples: Vec<f64> = (1..=100).map(|n| n as f64).collect();
        assert_eq!(percentile(&samples, 0.95), 95.0);
    }

    #[test]
    fn percentile_single_sample_returns_that_sample() {
        assert_eq!(percentile(&[42.0], 0.95), 42.0);
        assert_eq!(percentile(&[42.0], 0.50), 42.0);
    }

    #[test]
    fn percentile_empty_returns_zero() {
        assert_eq!(percentile(&[], 0.95), 0.0);
    }

    #[test]
    fn percentile_p50_even_length_uses_nearest_rank_not_interpolation() {
        // Sorted length 4: nearest-rank p50 is index ceil(0.5*4)-1 = 1 → 20.0
        // Interpolation would average 20 and 30 → 25.0.
        let samples = [10.0, 20.0, 30.0, 40.0];
        assert_eq!(percentile(&samples, 0.50), 20.0);
    }
}
