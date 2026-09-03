use half::f16;
use tempfile::tempdir;

use storage::{ChunkRecord, ChunkStore, ContentHash, Integrity};

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
fn failed_batch_leaves_store_clean() {
    let dir = tempdir().unwrap();
    let mut store = ChunkStore::create(dir.path(), 4, "m").unwrap();
    store
        .insert_batch(&[(rec(1), vec![f16::from_f32(1.0); 4])])
        .unwrap();

    store.set_fail_before_commit(true);
    let err = store
        .insert_batch(&[
            (rec(2), vec![f16::from_f32(2.0); 4]),
            (rec(3), vec![f16::from_f32(3.0); 4]),
        ])
        .unwrap_err();
    assert!(matches!(err, storage::StoreError::Io(_)));

    store.set_fail_before_commit(false);

    // Reopen and verify.
    drop(store);
    let store = ChunkStore::open(dir.path()).unwrap();
    match store.verify().unwrap() {
        Integrity::Ok { live } => assert_eq!(live, 1),
        other => panic!("expected Ok, got {other:?}"),
    }
    assert!(store.get_by_hash(&rec(2).content_hash).unwrap().is_none());
    assert!(store.get_by_hash(&rec(3).content_hash).unwrap().is_none());
    assert!(store.get_by_hash(&rec(1).content_hash).unwrap().is_some());
}
