//! Fill a store with synthetic chunks and report on-disk size and timings.
//!
//! Usage: cargo run --release -p storage --example fill_and_report -- --chunks 200000

use half::f16;
use std::env;
use std::time::Instant;
use storage::{ChunkRecord, ChunkStore, ContentHash};
use tempfile::tempdir;

fn main() {
    let chunks = parse_chunks();
    let dims = 1024u32;
    let dir = tempdir().expect("tempdir");
    let path = dir.path().to_path_buf();

    let t0 = Instant::now();
    let mut store = ChunkStore::create(&path, dims, "fill-and-report-model").expect("create");
    let create_ms = t0.elapsed();

    let batch_size = 1000usize;
    let mut seed = 0u64;
    while seed < chunks {
        let end = (seed + batch_size as u64).min(chunks);
        let batch: Vec<_> = (seed..end)
            .map(|i| {
                let mut hash = [0u8; 32];
                hash[..8].copy_from_slice(&i.to_le_bytes());
                let rec = ChunkRecord {
                    repository: "bench".into(),
                    file: format!("f{}.rs", i % 1000),
                    language: "rust".into(),
                    symbol: format!("s{i}"),
                    symbol_type: "function".into(),
                    signature: format!("fn s{i}()"),
                    doc_first_line: None,
                    line_start: 1,
                    line_end: 10,
                    content_hash: ContentHash::from_bytes(hash),
                };
                let mut vec = vec![f16::from_f32(0.0); dims as usize];
                vec[0] = f16::from_f32((i % 1000) as f32);
                (rec, vec)
            })
            .collect();
        store.insert_batch(&batch).expect("insert");
        seed = end;
    }

    drop(store);

    let mut open_times = Vec::new();
    let mut verify_times = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let store = ChunkStore::open(&path).expect("open");
        open_times.push(t.elapsed());
        let t = Instant::now();
        let integrity = store.verify().expect("verify");
        verify_times.push(t.elapsed());
        match integrity {
            storage::Integrity::Ok { live } => assert_eq!(live, chunks),
            other => panic!("verify failed: {other:?}"),
        }
    }

    let matrix_path = path.join("embeddings.f16");
    let db_path = path.join("chunks.db");
    let matrix_bytes = std::fs::metadata(&matrix_path).unwrap().len();
    let db_bytes = std::fs::metadata(&db_path).unwrap().len();
    // Also count WAL/SHM if present.
    let db_wal = std::fs::metadata(path.join("chunks.db-wal"))
        .map(|m| m.len())
        .unwrap_or(0);
    let db_shm = std::fs::metadata(path.join("chunks.db-shm"))
        .map(|m| m.len())
        .unwrap_or(0);

    let predicted = chunks * u64::from(dims) * 2;
    let mut open_ms: Vec<f64> = open_times
        .iter()
        .map(|d| d.as_secs_f64() * 1000.0)
        .collect();
    let mut verify_ms: Vec<f64> = verify_times
        .iter()
        .map(|d| d.as_secs_f64() * 1000.0)
        .collect();
    println!("chunks={chunks} dims={dims}");
    println!("create_elapsed_ms={}", create_ms.as_secs_f64() * 1000.0);
    println!(
        "matrix_bytes={matrix_bytes} ({:.2} MiB) predicted_payload_bytes={predicted} ({:.2} MiB)",
        matrix_bytes as f64 / (1024.0 * 1024.0),
        predicted as f64 / (1024.0 * 1024.0)
    );
    println!(
        "database_bytes={} ({:.2} MiB) including_wal_shm={}",
        db_bytes,
        db_bytes as f64 / (1024.0 * 1024.0),
        db_bytes + db_wal + db_shm
    );
    println!("open_times_ms={open_ms:?}");
    println!("verify_times_ms={verify_ms:?}");
    open_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    verify_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("open_median_ms={}", open_ms[open_ms.len() / 2]);
    println!("verify_median_ms={}", verify_ms[verify_ms.len() / 2]);
}

fn parse_chunks() -> u64 {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--chunks" {
            return args
                .next()
                .expect("--chunks value")
                .parse()
                .expect("chunks u64");
        }
    }
    1_000
}
