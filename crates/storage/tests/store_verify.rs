use half::f16;
use tempfile::tempdir;

use storage::{ChunkRecord, ChunkStore, ContentHash, EmbeddingMatrix, Integrity, StoreError};

fn rec(seed: u8) -> ChunkRecord {
    let mut hash = [0u8; 32];
    hash[0] = seed;
    ChunkRecord {
        repository: "r".into(),
        file: "f.rs".into(),
        language: "rust".into(),
        symbol: format!("s{seed}"),
        symbol_type: "fn".into(),
        signature: format!("fn s{seed}()"),
        doc_first_line: None,
        line_start: 1,
        line_end: 2,
        content_hash: ContentHash::from_bytes(hash),
    }
}

#[test]
fn verify_ok_on_healthy_store() {
    let dir = tempdir().unwrap();
    let mut store = ChunkStore::create(dir.path(), 4, "m").unwrap();
    store
        .insert_batch(&[(rec(1), vec![f16::from_f32(1.0); 4])])
        .unwrap();
    match store.verify().unwrap() {
        Integrity::Ok { live } => assert_eq!(live, 1),
        other => panic!("expected Ok, got {other:?}"),
    }
}

#[test]
fn verify_reports_orphan_when_metadata_row_removed() {
    let dir = tempdir().unwrap();
    let mut store = ChunkStore::create(dir.path(), 4, "m").unwrap();
    let rows = store
        .insert_batch(&[
            (rec(1), vec![f16::from_f32(1.0); 4]),
            (rec(2), vec![f16::from_f32(2.0); 4]),
        ])
        .unwrap();
    store
        .conn_mut()
        .execute("DELETE FROM chunks WHERE rowid = ?1", [rows[1].get() as i64])
        .unwrap();
    match store.verify().unwrap() {
        Integrity::Broken { orphan_rows, .. } => {
            assert!(
                orphan_rows.contains(&rows[1]),
                "expected orphan {:?}, got {orphan_rows:?}",
                rows[1]
            );
        }
        other => panic!("expected Broken, got {other:?}"),
    }
}

#[test]
fn verify_reports_missing_when_matrix_truncated() {
    let dir = tempdir().unwrap();
    let store_dir = dir.path().to_path_buf();
    {
        let mut store = ChunkStore::create(&store_dir, 4, "m").unwrap();
        store
            .insert_batch(&[
                (rec(1), vec![f16::from_f32(1.0); 4]),
                (rec(2), vec![f16::from_f32(2.0); 4]),
            ])
            .unwrap();
    }
    EmbeddingMatrix::rewrite_header_for_test(&store_dir.join("embeddings.f16"), 4, "m", 1)
        .unwrap();

    let err = ChunkStore::open(&store_dir).unwrap_err();
    match err {
        StoreError::Corrupt(Integrity::Broken { missing_rows, .. }) => {
            assert!(
                missing_rows.iter().any(|r| r.get() == 1),
                "expected missing row 1, got {missing_rows:?}"
            );
        }
        other => panic!("expected Corrupt(Broken missing), got {other:?}"),
    }
}
