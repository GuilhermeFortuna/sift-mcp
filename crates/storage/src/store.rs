use crate::error::StoreError;
use crate::matrix::EmbeddingMatrix;
use crate::record::{ChunkRecord, ContentHash, RowId};
use half::f16;
use std::path::Path;

/// Counts used by SIFT-006 to decide when to compact.
#[derive(Debug, Clone, PartialEq)]
pub struct StoreStats {
    pub live: u64,
    pub dead: u64,
    pub dead_fraction: f64,
}

/// Result of the correspondence check. Never panics on a corrupt store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Integrity {
    Ok { live: u64 },
    Broken {
        orphan_rows: Vec<RowId>,
        missing_rows: Vec<RowId>,
        duplicate_rows: Vec<RowId>,
    },
}

/// Report from compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionReport {
    pub live_before: u64,
    pub dead_reclaimed: u64,
    pub live_after: u64,
}

/// Durable chunk metadata + embedding matrix.
pub struct ChunkStore {
    // Populated in later steps.
    #[allow(dead_code)]
    dir: std::path::PathBuf,
    #[allow(dead_code)]
    matrix: EmbeddingMatrix,
}

impl ChunkStore {
    pub fn create(_dir: &Path, _dims: u32, _model_id: &str) -> Result<Self, StoreError> {
        Err(StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ChunkStore::create not yet implemented",
        )))
    }

    pub fn open(_dir: &Path) -> Result<Self, StoreError> {
        Err(StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ChunkStore::open not yet implemented",
        )))
    }

    pub fn insert_batch(
        &mut self,
        _chunks: &[(ChunkRecord, Vec<f16>)],
    ) -> Result<Vec<RowId>, StoreError> {
        unimplemented!("insert_batch")
    }

    pub fn get(&self, _row: RowId) -> Result<Option<ChunkRecord>, StoreError> {
        unimplemented!("get")
    }

    pub fn get_many(&self, _rows: &[RowId]) -> Result<Vec<Option<ChunkRecord>>, StoreError> {
        unimplemented!("get_many")
    }

    pub fn get_by_hash(
        &self,
        _hash: &ContentHash,
    ) -> Result<Option<(RowId, ChunkRecord)>, StoreError> {
        unimplemented!("get_by_hash")
    }

    pub fn rows_for_file(&self, _file: &str) -> Result<Vec<RowId>, StoreError> {
        unimplemented!("rows_for_file")
    }

    pub fn tombstone(&mut self, _rows: &[RowId]) -> Result<(), StoreError> {
        unimplemented!("tombstone")
    }

    pub fn stats(&self) -> Result<StoreStats, StoreError> {
        unimplemented!("stats")
    }

    pub fn verify(&self) -> Result<Integrity, StoreError> {
        unimplemented!("verify")
    }

    pub fn compact(&mut self) -> Result<CompactionReport, StoreError> {
        unimplemented!("compact")
    }

    pub fn matrix(&self) -> &EmbeddingMatrix {
        &self.matrix
    }

    pub fn indexed_commit(&self) -> Result<Option<String>, StoreError> {
        unimplemented!("indexed_commit")
    }

    pub fn set_indexed_commit(&mut self, _commit: &str) -> Result<(), StoreError> {
        unimplemented!("set_indexed_commit")
    }
}
