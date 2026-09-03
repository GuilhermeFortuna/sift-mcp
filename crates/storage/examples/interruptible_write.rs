//! Large batch writer used by scripts/kill-during-write.sh.
//!
//! Writes forever in batches until killed. Store directory is argv[1].

use half::f16;
use std::env;
use storage::{ChunkRecord, ChunkStore, ContentHash};

fn main() {
    let dir = env::args().nth(1).expect("store directory");
    let dims = 64u32;
    let mut store = ChunkStore::create(dir.as_ref(), dims, "kill-test-model").expect("create");
    let mut i = 0u64;
    loop {
        let batch: Vec<_> = (0..500)
            .map(|j| {
                let n = i + j;
                let mut hash = [0u8; 32];
                hash[..8].copy_from_slice(&n.to_le_bytes());
                let rec = ChunkRecord {
                    repository: "kill".into(),
                    file: format!("f{}.rs", n % 50),
                    language: "rust".into(),
                    symbol: format!("s{n}"),
                    symbol_type: "function".into(),
                    signature: format!("fn s{n}()"),
                    doc_first_line: None,
                    line_start: 1,
                    line_end: 2,
                    content_hash: ContentHash::from_bytes(hash),
                };
                (rec, vec![f16::from_f32((n % 100) as f32); dims as usize])
            })
            .collect();
        store.insert_batch(&batch).expect("insert");
        i += 500;
        if i % 5000 == 0 {
            eprintln!("wrote {i} chunks");
        }
    }
}
