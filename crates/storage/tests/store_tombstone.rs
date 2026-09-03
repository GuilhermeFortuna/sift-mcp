use half::f16;
use tempfile::tempdir;

use storage::{ChunkRecord, ChunkStore, ContentHash};

fn rec(seed: u16, file: &str) -> ChunkRecord {
    let mut hash = [0u8; 32];
    hash[..2].copy_from_slice(&seed.to_le_bytes());
    ChunkRecord {
        repository: "r".into(),
        file: file.into(),
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
fn tombstone_updates_stats_and_hides_rows() {
    let dir = tempdir().unwrap();
    let mut store = ChunkStore::create(dir.path(), 4, "m").unwrap();
    let batch: Vec<_> = (0..1000)
        .map(|i| {
            (
                rec(i as u16, if i < 500 { "a.rs" } else { "b.rs" }),
                vec![f16::from_f32(i as f32); 4],
            )
        })
        .collect();
    let rows = store.insert_batch(&batch).unwrap();
    let to_delete: Vec<_> = rows.iter().take(200).copied().collect();
    store.tombstone(&to_delete).unwrap();

    let stats = store.stats().unwrap();
    assert_eq!(stats.dead, 200);
    assert_eq!(stats.live, 800);
    assert!((stats.dead_fraction - 0.2).abs() < 1e-9);

    assert!(store.get(rows[0]).unwrap().is_none());
    let file_rows = store.rows_for_file("a.rs").unwrap();
    assert!(!file_rows.contains(&rows[0]));
    assert_eq!(file_rows.len(), 300); // 500 in a.rs minus 200 deleted from start
}
