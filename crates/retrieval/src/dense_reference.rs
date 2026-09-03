use storage::RowId;

use crate::dense::LiveMask;
use crate::ScoredRow;

pub fn reference_search(
    matrix_f32: &[f32],
    live: &LiveMask,
    dims: usize,
    query: &[f32],
    limit: usize,
) -> Vec<ScoredRow> {
    assert!(dims > 0, "dims must be positive");
    assert_eq!(query.len(), dims, "query width must match dims");
    assert_eq!(matrix_f32.len() % dims, 0, "matrix must contain whole rows");
    assert_eq!(matrix_f32.len() / dims, live.len());

    let mut results = matrix_f32
        .chunks_exact(dims)
        .enumerate()
        .filter_map(|(row, vector)| {
            let row = RowId::from_u64(row as u64);
            live.is_live(row).then(|| ScoredRow {
                row,
                score: vector
                    .iter()
                    .zip(query)
                    .map(|(value, query_value)| value * query_value)
                    .sum(),
            })
        })
        .collect::<Vec<_>>();
    results.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.row.cmp(&right.row))
    });
    results.truncate(limit);
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_ranks_hand_computed_scores_and_excludes_dead_rows() {
        let matrix = [
            1.0, 0.0, 0.0, // row 0: 1.0
            0.0, 1.0, 0.0, // row 1: 0.5
            0.0, 0.0, 1.0, // row 2: -1.0
            1.0, 1.0, 0.0, // row 3: 1.5, but dead
            -1.0, 0.0, 0.0, // row 4: -1.0
        ];
        let query = [1.0, 0.5, -1.0];
        let live = LiveMask::from_bits(vec![true, true, true, false, true]);

        let results = reference_search(&matrix, &live, 3, &query, 10);

        assert_eq!(
            results,
            vec![
                ScoredRow {
                    row: RowId::from_u64(0),
                    score: 1.0,
                },
                ScoredRow {
                    row: RowId::from_u64(1),
                    score: 0.5,
                },
                ScoredRow {
                    row: RowId::from_u64(2),
                    score: -1.0,
                },
                ScoredRow {
                    row: RowId::from_u64(4),
                    score: -1.0,
                },
            ]
        );
    }

    #[test]
    fn reference_respects_limit() {
        let live = LiveMask::from_bits(vec![true, true, true]);
        let results = reference_search(&[1.0, 2.0, 3.0], &live, 1, &[1.0], 2);
        assert_eq!(
            results.iter().map(|result| result.row).collect::<Vec<_>>(),
            [RowId::from_u64(2), RowId::from_u64(1)]
        );
    }
}
