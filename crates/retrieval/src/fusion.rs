//! Reciprocal rank fusion over lexical and dense rankings.

use storage::RowId;

use crate::ScoredRow;

/// Reciprocal rank fusion. `k` damps the influence of top ranks; see decisions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FusionConfig {
    /// Candidates taken from the lexical retriever.
    pub lexical_depth: usize,
    /// Candidates taken from the dense retriever.
    pub dense_depth: usize,
    /// RRF damping constant.
    pub rrf_k: f32,
}

impl Default for FusionConfig {
    fn default() -> Self {
        Self {
            lexical_depth: 50,
            dense_depth: 50,
            rrf_k: 60.0,
        }
    }
}

/// One retriever's contribution to a fused row. None means "did not return it",
/// which is distinct from a score of zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contribution {
    /// 1-based within that retriever's list.
    pub rank: Option<u32>,
    pub score: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FusedRow {
    pub row: RowId,
    pub lexical: Contribution,
    pub dense: Contribution,
    pub fused_score: f32,
}

/// Fuse lexical and dense candidate lists by reciprocal rank.
pub fn fuse(lexical: &[ScoredRow], dense: &[ScoredRow], config: &FusionConfig) -> Vec<FusedRow> {
    use std::collections::HashMap;

    let lexical = &lexical[..lexical.len().min(config.lexical_depth)];
    let dense = &dense[..dense.len().min(config.dense_depth)];

    let mut by_row: HashMap<RowId, FusedRow> = HashMap::new();

    for (index, scored) in lexical.iter().enumerate() {
        let rank = (index + 1) as u32;
        let entry = by_row.entry(scored.row).or_insert(FusedRow {
            row: scored.row,
            lexical: Contribution {
                rank: None,
                score: None,
            },
            dense: Contribution {
                rank: None,
                score: None,
            },
            fused_score: 0.0,
        });
        entry.lexical = Contribution {
            rank: Some(rank),
            score: Some(scored.score),
        };
        entry.fused_score += 1.0 / (config.rrf_k + rank as f32);
    }

    for (index, scored) in dense.iter().enumerate() {
        let rank = (index + 1) as u32;
        let entry = by_row.entry(scored.row).or_insert(FusedRow {
            row: scored.row,
            lexical: Contribution {
                rank: None,
                score: None,
            },
            dense: Contribution {
                rank: None,
                score: None,
            },
            fused_score: 0.0,
        });
        entry.dense = Contribution {
            rank: Some(rank),
            score: Some(scored.score),
        };
        entry.fused_score += 1.0 / (config.rrf_k + rank as f32);
    }

    let mut fused: Vec<FusedRow> = by_row.into_values().collect();
    fused.sort_by(|left, right| {
        right
            .fused_score
            .total_cmp(&left.fused_score)
            .then_with(|| left.row.cmp(&right.row))
    });
    fused
}

#[cfg(test)]
mod tests {
    use storage::RowId;

    use super::{Contribution, FusionConfig, fuse};
    use crate::ScoredRow;

    fn scored(row: u64, score: f32) -> ScoredRow {
        ScoredRow {
            row: RowId::from_u64(row),
            score,
        }
    }

    #[test]
    fn fuse_rrf_arithmetic_and_missing_contribution() {
        let config = FusionConfig {
            lexical_depth: 50,
            dense_depth: 50,
            rrf_k: 60.0,
        };
        // Lexical: row 10 rank 1, row 20 rank 2
        // Dense:   row 30 rank 1, row 40 rank 2, row 10 rank 3
        let lexical = vec![scored(10, 9.0), scored(20, 5.0)];
        let dense = vec![scored(30, 0.9), scored(40, 0.8), scored(10, 0.7)];

        let fused = fuse(&lexical, &dense, &config);

        let row10 = fused
            .iter()
            .find(|r| r.row.get() == 10)
            .expect("row 10 present");
        let expected = 1.0 / 61.0 + 1.0 / 63.0;
        assert!(
            (row10.fused_score - expected).abs() < 1e-6,
            "expected {expected:.6}, got {:.6}",
            row10.fused_score
        );
        assert_eq!(row10.lexical.rank, Some(1));
        assert_eq!(row10.lexical.score, Some(9.0));
        assert_eq!(row10.dense.rank, Some(3));
        assert_eq!(row10.dense.score, Some(0.7));

        // Lexical-only at rank 1 scores 1/61 and carries missing dense contribution.
        let lex_only = fuse(&[scored(99, 4.0)], &[], &config);
        assert_eq!(lex_only.len(), 1);
        let only = &lex_only[0];
        let expected_lex_only = 1.0 / 61.0;
        assert!(
            (only.fused_score - expected_lex_only).abs() < 1e-6,
            "expected {expected_lex_only:.6}, got {:.6}",
            only.fused_score
        );
        assert_eq!(
            only.dense,
            Contribution {
                rank: None,
                score: None
            }
        );

        // Hand-computed order (descending fused_score, ties by ascending RowId):
        // row10: 1/61 + 1/63 ≈ 0.032276
        // row20: 1/62 ≈ 0.016129
        // row30: 1/61 ≈ 0.016393
        // row40: 1/62 ≈ 0.016129  (tie with 20 → RowId 20 before 40)
        let order: Vec<u64> = fused.iter().map(|r| r.row.get()).collect();
        assert_eq!(order, vec![10, 30, 20, 40]);
    }
}
