use half::f16;
use tempfile::tempdir;

use storage::{ChunkRecord, ChunkStore, ContentHash};

fn sample_record(hash_seed: u8) -> ChunkRecord {
    let mut bytes = [0u8; 32];
    bytes[0] = hash_seed;
    ChunkRecord {
        repository: "repo".into(),
        file: "src/lib.rs".into(),
        language: "rust".into(),
        symbol: "foo".into(),
        symbol_type: "function".into(),
        signature: "fn foo()".into(),
        doc_first_line: Some("Foo does a thing.".into()),
        line_start: 10,
        line_end: 20,
        content_hash: ContentHash::from_bytes(bytes),
    }
}

fn sample_vec(dims: u32, fill: f32) -> Vec<f16> {
    vec![f16::from_f32(fill); dims as usize]
}

#[test]
fn single_chunk_round_trip_by_row_hash_and_file() {
    let dir = tempdir().unwrap();
    let mut store = ChunkStore::create(dir.path(), 4, "model-a").unwrap();
    let record = sample_record(1);
    let vector = sample_vec(4, 0.5);
    let rows = store
        .insert_batch(&[(record.clone(), vector)])
        .unwrap();
    assert_eq!(rows.len(), 1);
    let row = rows[0];

    let by_row = store.get(row).unwrap().expect("present by row");
    assert_eq!(by_row, record);

    let (hash_row, by_hash) = store
        .get_by_hash(&record.content_hash)
        .unwrap()
        .expect("present by hash");
    assert_eq!(hash_row, row);
    assert_eq!(by_hash, record);

    let file_rows = store.rows_for_file("src/lib.rs").unwrap();
    assert_eq!(file_rows, vec![row]);
}
