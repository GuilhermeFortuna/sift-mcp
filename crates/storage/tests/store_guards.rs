use half::f16;
use tempfile::tempdir;

use storage::{ChunkRecord, ChunkStore, ContentHash, StoreError, SCHEMA_VERSION};

fn rec() -> ChunkRecord {
    ChunkRecord {
        repository: "r".into(),
        file: "f.rs".into(),
        language: "rust".into(),
        symbol: "s".into(),
        symbol_type: "fn".into(),
        signature: "fn s()".into(),
        doc_first_line: None,
        line_start: 1,
        line_end: 2,
        content_hash: ContentHash::from_bytes([1u8; 32]),
    }
}

#[test]
fn require_model_rejects_mismatch() {
    let dir = tempdir().unwrap();
    let mut store = ChunkStore::create(dir.path(), 4, "model-a").unwrap();
    store
        .insert_batch(&[(rec(), vec![f16::from_f32(1.0); 4])])
        .unwrap();
    match store.require_model("model-b").unwrap_err() {
        StoreError::ModelMismatch { expected, got } => {
            assert_eq!(expected, "model-b");
            assert_eq!(got, "model-a");
        }
        other => panic!("expected ModelMismatch, got {other:?}"),
    }
    store.require_model("model-a").unwrap();
}

#[test]
fn open_rejects_wrong_schema_version() {
    let dir = tempdir().unwrap();
    {
        let mut store = ChunkStore::create(dir.path(), 4, "m").unwrap();
        store
            .insert_batch(&[(rec(), vec![f16::from_f32(1.0); 4])])
            .unwrap();
        store
            .conn_mut()
            .execute(
                "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
                [(SCHEMA_VERSION + 1).to_string()],
            )
            .unwrap();
    }
    match ChunkStore::open(dir.path()).unwrap_err() {
        StoreError::SchemaVersion { expected, got } => {
            assert_eq!(expected, SCHEMA_VERSION);
            assert_eq!(got, SCHEMA_VERSION + 1);
        }
        other => panic!("expected SchemaVersion, got {other:?}"),
    }
}
