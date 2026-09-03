//! Pooling and L2 normalization over per-token hidden states.

use crate::metadata::Pooling;

/// `hidden`: [batch, seq, dims] row-major. `mask`: [batch, seq], 1 for real tokens.
/// Returns [batch, dims]. Padded positions never contribute.
pub fn pool(
    hidden: &[f32],
    mask: &[u32],
    batch: usize,
    seq: usize,
    dims: usize,
    strategy: Pooling,
) -> Vec<f32> {
    let mut out = vec![0.0f32; batch * dims];
    for b in 0..batch {
        let row_out = &mut out[b * dims..(b + 1) * dims];
        match strategy {
            Pooling::Mean => {
                let mut count = 0u32;
                for s in 0..seq {
                    if mask[b * seq + s] == 0 {
                        continue;
                    }
                    let base = (b * seq + s) * dims;
                    for d in 0..dims {
                        row_out[d] += hidden[base + d];
                    }
                    count += 1;
                }
                if count > 0 {
                    let inv = 1.0 / count as f32;
                    for v in row_out.iter_mut() {
                        *v *= inv;
                    }
                } else {
                    // No real tokens: average all positions (matches verify_export.py).
                    for s in 0..seq {
                        let base = (b * seq + s) * dims;
                        for d in 0..dims {
                            row_out[d] += hidden[base + d];
                        }
                    }
                    if seq > 0 {
                        let inv = 1.0 / seq as f32;
                        for v in row_out.iter_mut() {
                            *v *= inv;
                        }
                    }
                }
            }
            Pooling::LastToken => {
                let mut idx: Option<usize> = None;
                for s in 0..seq {
                    if mask[b * seq + s] != 0 {
                        idx = Some(s);
                    }
                }
                let s = idx.unwrap_or(seq.saturating_sub(1));
                let base = (b * seq + s) * dims;
                row_out.copy_from_slice(&hidden[base..base + dims]);
            }
            Pooling::Cls => {
                let base = b * seq * dims;
                row_out.copy_from_slice(&hidden[base..base + dims]);
            }
        }
    }
    out
}

/// In-place L2 normalization per row. A zero row is left as zeros rather than
/// producing NaN.
pub fn l2_normalize_rows(vectors: &mut [f32], dims: usize) {
    assert!(dims > 0);
    assert_eq!(vectors.len() % dims, 0);
    let rows = vectors.len() / dims;
    for r in 0..rows {
        let row = &mut vectors[r * dims..(r + 1) * dims];
        let mut sum_sq = 0.0f32;
        for &v in row.iter() {
            sum_sq += v * v;
        }
        if sum_sq == 0.0 {
            continue;
        }
        let inv = 1.0 / sum_sq.sqrt();
        for v in row.iter_mut() {
            *v *= inv;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// batch 2, seq 3, dims 2:
    /// row0: [(1,2), (3,4), (5,6)] mask all 1 → mean = (3,4)
    /// row1: [(10,20), pad, pad] mask length 1 → mean = (10,20)
    fn synthetic() -> (Vec<f32>, Vec<u32>) {
        let hidden = vec![
            1.0, 2.0, // b0 s0
            3.0, 4.0, // b0 s1
            5.0, 6.0, // b0 s2
            10.0, 20.0, // b1 s0
            99.0, 99.0, // b1 s1 pad
            88.0, 88.0, // b1 s2 pad
        ];
        let mask = vec![1, 1, 1, 1, 0, 0];
        (hidden, mask)
    }

    #[test]
    fn mean_pool_hand_computed() {
        let (hidden, mask) = synthetic();
        let out = pool(&hidden, &mask, 2, 3, 2, Pooling::Mean);
        assert_eq!(out.len(), 4);
        assert!((out[0] - 3.0).abs() < 1e-6);
        assert!((out[1] - 4.0).abs() < 1e-6);
        assert!((out[2] - 10.0).abs() < 1e-6);
        assert!((out[3] - 20.0).abs() < 1e-6);
    }

    #[test]
    fn mean_masked_row_equals_alone_at_seq1() {
        let (hidden, mask) = synthetic();
        let batched = pool(&hidden, &mask, 2, 3, 2, Pooling::Mean);
        let alone_hidden = vec![10.0, 20.0];
        let alone_mask = vec![1u32];
        let alone = pool(&alone_hidden, &alone_mask, 1, 1, 2, Pooling::Mean);
        assert_eq!(&batched[2..4], &alone[..]);
    }

    #[test]
    fn last_token_selects_last_unmasked() {
        let (hidden, mask) = synthetic();
        let out = pool(&hidden, &mask, 2, 3, 2, Pooling::LastToken);
        // row0 last unmasked = s2 → (5,6); row1 last unmasked = s0 → (10,20)
        assert_eq!(out, vec![5.0, 6.0, 10.0, 20.0]);
    }

    #[test]
    fn last_token_does_not_select_last_column_when_padded() {
        // Right-padded: last column is pad; must not return pad embedding.
        let hidden = vec![
            1.0, 2.0, // s0 real
            3.0, 4.0, // s1 pad
        ];
        let mask = vec![1u32, 0];
        let out = pool(&hidden, &mask, 1, 2, 2, Pooling::LastToken);
        assert_eq!(out, vec![1.0, 2.0]);
        assert_ne!(out, vec![3.0, 4.0], "must not pick last column pad");
    }

    #[test]
    fn cls_selects_first_token() {
        let (hidden, mask) = synthetic();
        let out = pool(&hidden, &mask, 2, 3, 2, Pooling::Cls);
        assert_eq!(out, vec![1.0, 2.0, 10.0, 20.0]);
    }

    #[test]
    fn l2_normalizes_three_four_to_point_six_point_eight() {
        let mut v = vec![3.0f32, 4.0];
        l2_normalize_rows(&mut v, 2);
        assert!((v[0] - 0.6).abs() < 1e-6);
        assert!((v[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn l2_leaves_zero_row_as_zeros_without_nan() {
        let mut v = vec![0.0f32, 0.0];
        l2_normalize_rows(&mut v, 2);
        assert_eq!(v, vec![0.0, 0.0]);
        assert!(v.iter().all(|x| !x.is_nan()));
    }

    #[test]
    fn l2_normalizes_rows_independently() {
        let mut v = vec![3.0f32, 4.0, 0.0, 5.0];
        l2_normalize_rows(&mut v, 2);
        assert!((v[0] - 0.6).abs() < 1e-6);
        assert!((v[1] - 0.8).abs() < 1e-6);
        assert!((v[2] - 0.0).abs() < 1e-6);
        assert!((v[3] - 1.0).abs() < 1e-6);
    }
}
