use half::f16;
use tempfile::tempdir;

use storage::{EmbeddingMatrix, StoreError};

fn vec4(a: f32, b: f32, c: f32, d: f32) -> Vec<f16> {
    [a, b, c, d].into_iter().map(f16::from_f32).collect()
}

#[test]
fn append_assigns_dense_row_ids_and_reads_back() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("m.f16");
    let mut matrix = EmbeddingMatrix::create(&path, 4, "m").unwrap();
    let v0 = vec4(1.0, 2.0, 3.0, 4.0);
    let v1 = vec4(5.0, 6.0, 7.0, 8.0);
    let v2 = vec4(9.0, 10.0, 11.0, 12.0);
    let r0 = matrix.append(&v0).unwrap();
    let r1 = matrix.append(&v1).unwrap();
    let r2 = matrix.append(&v2).unwrap();
    assert_eq!(r0.get(), 0);
    assert_eq!(r1.get(), 1);
    assert_eq!(r2.get(), 2);
    assert_eq!(matrix.row(r1).unwrap(), v1.as_slice());
    assert_eq!(matrix.as_slice().len(), 3 * 4);
}

#[test]
fn append_rejects_wrong_width() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("m.f16");
    let mut matrix = EmbeddingMatrix::create(&path, 4, "m").unwrap();
    let bad: Vec<f16> = (0..5).map(|i| f16::from_f32(i as f32)).collect();
    let err = matrix.append(&bad).unwrap_err();
    match err {
        StoreError::DimensionMismatch { expected, got } => {
            assert_eq!(expected, 4);
            assert_eq!(got, 5);
        }
        other => panic!("expected DimensionMismatch, got {other:?}"),
    }
}
