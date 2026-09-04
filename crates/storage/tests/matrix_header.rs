use tempfile::tempdir;

use storage::{EmbeddingMatrix, MATRIX_FORMAT_VERSION, StoreError};

#[test]
fn create_and_open_preserves_dims_and_model_id() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("embeddings.f16");
    let matrix = EmbeddingMatrix::create(&path, 1024, "test-model-v1").unwrap();
    assert_eq!(matrix.dims(), 1024);
    assert_eq!(matrix.model_id(), "test-model-v1");
    drop(matrix);

    let reopened = EmbeddingMatrix::open(&path).unwrap();
    assert_eq!(reopened.dims(), 1024);
    assert_eq!(reopened.model_id(), "test-model-v1");
}

#[test]
fn open_rejects_wrong_magic() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad.f16");
    EmbeddingMatrix::create(&path, 4, "m").unwrap();
    // Corrupt the magic bytes.
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.write_all(b"BADMAGIC").unwrap();
    }
    let err = EmbeddingMatrix::open(&path).unwrap_err();
    assert!(
        matches!(err, StoreError::Corrupt(_)),
        "expected Corrupt, got {err:?}"
    );
}

#[test]
fn open_rejects_wrong_format_version() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ver.f16");
    EmbeddingMatrix::create(&path, 4, "m").unwrap();
    // Overwrite format_version (offset 8, little-endian u32).
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.seek(SeekFrom::Start(8)).unwrap();
        let bad = (MATRIX_FORMAT_VERSION + 1).to_le_bytes();
        f.write_all(&bad).unwrap();
    }
    let err = EmbeddingMatrix::open(&path).unwrap_err();
    match err {
        StoreError::SchemaVersion { expected, got } => {
            assert_eq!(expected, MATRIX_FORMAT_VERSION);
            assert_eq!(got, MATRIX_FORMAT_VERSION + 1);
        }
        other => panic!("expected SchemaVersion, got {other:?}"),
    }
}
