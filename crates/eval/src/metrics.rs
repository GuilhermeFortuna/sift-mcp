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

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BytesBeforeHit {
    pub mcp_median: u64,
    /// Grep-style baseline over the same repository.
    pub baseline_median: u64,
    pub baseline_command: String,
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

pub fn reciprocal_rank(ranked: &[(String, String)], expected: &[(String, String)]) -> f64 {
    for (index, candidate) in ranked.iter().enumerate() {
        if expected.iter().any(|e| e == candidate) {
            return 1.0 / (index as f64 + 1.0);
        }
    }
    0.0
}

/// Fraction of labels whose expected symbol appears in the top `k` ranked results.
pub fn top_k_accuracy(
    rankings: &[Vec<(String, String)>],
    expected: &[Vec<(String, String)>],
    k: usize,
) -> f64 {
    assert_eq!(rankings.len(), expected.len());
    if rankings.is_empty() {
        return 0.0;
    }
    let hits = rankings
        .iter()
        .zip(expected.iter())
        .filter(|(ranked, exp)| {
            ranked
                .iter()
                .take(k)
                .any(|candidate| exp.iter().any(|e| e == candidate))
        })
        .count();
    hits as f64 / rankings.len() as f64
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

    fn pair(file: &str, symbol: &str) -> (String, String) {
        (file.to_string(), symbol.to_string())
    }

    #[test]
    fn reciprocal_rank_expected_at_rank_one_is_one() {
        let ranked = vec![pair("a.rs", "foo"), pair("b.rs", "bar")];
        let expected = vec![pair("a.rs", "foo")];
        assert_eq!(reciprocal_rank(&ranked, &expected), 1.0);
    }

    #[test]
    fn reciprocal_rank_expected_at_rank_four_is_quarter() {
        let ranked = vec![
            pair("a.rs", "a"),
            pair("b.rs", "b"),
            pair("c.rs", "c"),
            pair("d.rs", "d"),
        ];
        let expected = vec![pair("d.rs", "d")];
        assert_eq!(reciprocal_rank(&ranked, &expected), 0.25);
    }

    #[test]
    fn reciprocal_rank_absent_is_zero() {
        let ranked = vec![pair("a.rs", "foo")];
        let expected = vec![pair("b.rs", "bar")];
        assert_eq!(reciprocal_rank(&ranked, &expected), 0.0);
    }

    #[test]
    fn reciprocal_rank_empty_ranking_is_zero() {
        let expected = vec![pair("a.rs", "foo")];
        assert_eq!(reciprocal_rank(&[], &expected), 0.0);
    }

    #[test]
    fn reciprocal_rank_first_of_two_expected_determines_rank() {
        let ranked = vec![pair("a.rs", "a"), pair("b.rs", "b"), pair("c.rs", "c")];
        // "c" at rank 3 and "b" at rank 2 — first expected to appear is "b" → 0.5
        let expected = vec![pair("c.rs", "c"), pair("b.rs", "b")];
        assert_eq!(reciprocal_rank(&ranked, &expected), 0.5);
    }

    #[test]
    fn reciprocal_rank_requires_both_file_and_symbol() {
        let ranked = vec![pair("same.rs", "other_symbol")];
        let expected = vec![pair("same.rs", "wanted")];
        assert_eq!(reciprocal_rank(&ranked, &expected), 0.0);
    }

    #[test]
    fn top_k_accuracy_counts_hits_in_window() {
        let rankings = vec![
            vec![pair("a.rs", "a"), pair("b.rs", "b")], // hit at 1
            vec![pair("x.rs", "x"), pair("y.rs", "y"), pair("z.rs", "z")], // miss in top-2
            vec![pair("p.rs", "p"), pair("q.rs", "q")], // hit at 2
        ];
        let expected = vec![
            vec![pair("a.rs", "a")],
            vec![pair("z.rs", "z")],
            vec![pair("q.rs", "q")],
        ];
        assert_eq!(top_k_accuracy(&rankings, &expected, 1), 1.0 / 3.0);
        assert_eq!(top_k_accuracy(&rankings, &expected, 2), 2.0 / 3.0);
        assert_eq!(top_k_accuracy(&rankings, &expected, 3), 1.0);
    }

    #[test]
    fn top_k_accuracy_empty_results_are_misses() {
        let rankings = vec![vec![], vec![pair("a.rs", "a")]];
        let expected = vec![vec![pair("a.rs", "a")], vec![pair("a.rs", "a")]];
        assert_eq!(top_k_accuracy(&rankings, &expected, 3), 0.5);
    }
}
