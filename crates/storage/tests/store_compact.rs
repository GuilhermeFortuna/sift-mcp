use half::f16;
use tempfile::tempdir;

use storage::{ChunkRecord, ChunkStore, ContentHash, Integrity};

fn rec(seed: u16) -> ChunkRecord {
    let mut hash = [0u8; 32];
    hash[..2].copy_from_slice(&seed.to_le_bytes());
    ChunkRecord {
        repository: "r".into(),
        file: "f.rs".into(),
        language: "rust".into(),
        symbol: format!("s{seed}"),
        symbol_type: "fn".into(),
        signature: format!("fn s{seed}()"),
        doc_first_line: Some(format!("doc {seed}")),
        line_start: seed as u32,
        line_end: seed as u32 + 1,
        content_hash: ContentHash::from_bytes(hash),
    }
}

#[test]
fn compact_reclaims_dead_rows_preserving_live_data() {
    let dir = tempdir().unwrap();
    let mut store = ChunkStore::create(dir.path(), 4, "m").unwrap();
    let batch: Vec<_> = (0..1000)
        .map(|i| (rec(i as u16), vec![f16::from_f32(i as f32); 4]))
        .collect();
    let rows = store.insert_batch(&batch).unwrap();

    let dead: Vec<_> = rows.iter().take(300).copied().collect();
    let survivors: Vec<_> = rows.iter().skip(300).copied().collect();
    let survivor_records: Vec<_> = survivors
        .iter()
        .map(|r| store.get(*r).unwrap().unwrap())
        .collect();
    let survivor_vectors: Vec<_> = survivors
        .iter()
        .map(|r| store.matrix().row(*r).unwrap().to_vec())
        .collect();

    store.tombstone(&dead).unwrap();
    let report = store.compact().unwrap();
    assert_eq!(report.live_after, 700);
    assert_eq!(store.matrix().rows(), 700);
    match store.verify().unwrap() {
        Integrity::Ok { live } => assert_eq!(live, 700),
        other => panic!("expected Ok, got {other:?}"),
    }

    let mut new_ids = Vec::new();
    for (rec, vec) in survivor_records.iter().zip(survivor_vectors.iter()) {
        let (row, got) = store
            .get_by_hash(&rec.content_hash)
            .unwrap()
            .expect("survivor present");
        assert_eq!(&got, rec);
        assert_eq!(store.matrix().row(row).unwrap(), vec.as_slice());
        new_ids.push(row.get());
    }
    new_ids.sort_unstable();
    assert_eq!(new_ids, (0..700).collect::<Vec<_>>());
}
