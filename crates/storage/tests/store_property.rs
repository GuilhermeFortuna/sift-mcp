use half::f16;
use proptest::prelude::*;
use tempfile::tempdir;

use storage::{ChunkRecord, ChunkStore, ContentHash};

fn record_from_seed(seed: u64, file: &str) -> ChunkRecord {
    let mut hash = [0u8; 32];
    hash[..8].copy_from_slice(&seed.to_le_bytes());
    ChunkRecord {
        repository: "repo".into(),
        file: file.into(),
        language: "rust".into(),
        symbol: format!("sym_{seed}"),
        symbol_type: "function".into(),
        signature: format!("fn sym_{seed}()"),
        doc_first_line: None,
        line_start: 1,
        line_end: 2,
        content_hash: ContentHash::from_bytes(hash),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn insert_batch_round_trips_distinct_records(
        seeds in prop::collection::vec(0u64..10_000, 1..40)
    ) {
        // Distinct seeds => distinct hashes.
        let mut uniq = seeds;
        uniq.sort_unstable();
        uniq.dedup();
        prop_assume!(!uniq.is_empty());

        let dir = tempdir().unwrap();
        let mut store = ChunkStore::create(dir.path(), 4, "m").unwrap();
        let batch: Vec<(ChunkRecord, Vec<f16>)> = uniq
            .iter()
            .enumerate()
            .map(|(i, seed)| {
                let rec = record_from_seed(*seed, &format!("f{i}.rs"));
                let vec = vec![f16::from_f32(*seed as f32); 4];
                (rec, vec)
            })
            .collect();

        let rows = store.insert_batch(&batch).unwrap();
        prop_assert_eq!(rows.len(), batch.len());

        let mut seen = std::collections::BTreeSet::new();
        for (i, row) in rows.iter().enumerate() {
            prop_assert!(seen.insert(row.get()), "duplicate row id {}", row.get());
            prop_assert_eq!(row.get(), i as u64);
            let got = store.get(*row).unwrap().expect("live");
            prop_assert_eq!(got, batch[i].0.clone());
        }
    }
}
