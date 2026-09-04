use half::f16;
use tempfile::tempdir;

use storage::{ChunkRecord, ChunkStore, ContentHash};

fn record_with_hash(hash: [u8; 32], symbol: &str) -> ChunkRecord {
    ChunkRecord {
        repository: "repo".into(),
        file: "a.rs".into(),
        language: "rust".into(),
        symbol: symbol.into(),
        symbol_type: "function".into(),
        signature: format!("fn {symbol}()"),
        doc_first_line: None,
        line_start: 1,
        line_end: 2,
        content_hash: ContentHash::from_bytes(hash),
    }
}

#[test]
fn batch_keeps_one_row_per_occurrence_with_identical_hashes() {
    let dir = tempdir().unwrap();
    let mut store = ChunkStore::create(dir.path(), 4, "m").unwrap();
    let hash = [7u8; 32];
    let r1 = record_with_hash(hash, "a");
    let r2 = record_with_hash(hash, "b"); // same body hash, distinct occurrence
    let v1 = vec![f16::from_f32(1.0); 4];
    let v2 = vec![f16::from_f32(2.0); 4];
    let rows = store.insert_batch(&[(r1, v1), (r2, v2)]).unwrap();
    assert_ne!(rows[0], rows[1]);
    assert_eq!(store.matrix().rows(), 2);
    assert_eq!(store.rows_for_file("a.rs").unwrap(), vec![rows[0], rows[1]]);
}

#[test]
fn insert_keeps_a_new_occurrence_for_an_existing_hash() {
    let dir = tempdir().unwrap();
    let mut store = ChunkStore::create(dir.path(), 4, "m").unwrap();
    let hash = [9u8; 32];
    let first = record_with_hash(hash, "a");
    let rows1 = store
        .insert_batch(&[(first, vec![f16::from_f32(1.0); 4])])
        .unwrap();
    assert_eq!(store.matrix().rows(), 1);

    let second = record_with_hash(hash, "b");
    let rows2 = store
        .insert_batch(&[(second, vec![f16::from_f32(99.0); 4])])
        .unwrap();
    assert_ne!(rows2[0], rows1[0]);
    assert_eq!(store.matrix().rows(), 2);
}
