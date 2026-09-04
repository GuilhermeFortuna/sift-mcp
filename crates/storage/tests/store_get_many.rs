use half::f16;
use tempfile::tempdir;

use storage::{ChunkRecord, ChunkStore, ContentHash};

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
fn get_many_preserves_order_and_none_for_tombstoned() {
    let dir = tempdir().unwrap();
    let mut store = ChunkStore::create(dir.path(), 4, "m").unwrap();
    let batch: Vec<_> = (0..5)
        .map(|i| (rec(i), vec![f16::from_f32(i as f32); 4]))
        .collect();
    let rows = store.insert_batch(&batch).unwrap();
    store.tombstone(&[rows[2]]).unwrap();

    let requested = [rows[4], rows[2], rows[0], rows[1], rows[3]];
    let got = store.get_many(&requested).unwrap();
    assert_eq!(got.len(), 5);
    assert_eq!(got[0].as_ref().unwrap().symbol, "s4");
    assert!(got[1].is_none());
    assert_eq!(got[2].as_ref().unwrap().symbol, "s0");
    assert_eq!(got[3].as_ref().unwrap().symbol, "s1");
    assert_eq!(got[4].as_ref().unwrap().symbol, "s3");
}

#[test]
fn get_many_prepare_count_independent_of_row_count() {
    let dir = tempdir().unwrap();
    let mut store = ChunkStore::create(dir.path(), 4, "m").unwrap();
    let batch: Vec<_> = (0..100)
        .map(|i| (rec(i as u8), vec![f16::from_f32(i as f32); 4]))
        .collect();
    let rows = store.insert_batch(&batch).unwrap();

    let mut counts = Vec::new();
    for n in [1usize, 10, 100] {
        let _ = store.take_statements_prepared();
        let subset: Vec<_> = rows.iter().take(n).copied().collect();
        store.get_many(&subset).unwrap();
        counts.push(store.take_statements_prepared());
    }
    assert_eq!(counts[0], counts[1], "1 vs 10: {counts:?}");
    assert_eq!(counts[1], counts[2], "10 vs 100: {counts:?}");
}
