//! Chunk metadata (SQLite) paired with an fp16 embedding matrix.

mod error;
mod matrix;
mod record;
mod store;

pub use error::StoreError;
pub use matrix::{EmbeddingMatrix, MATRIX_FORMAT_VERSION, MatrixHeader};
pub use record::{ChunkRecord, ContentHash, RowId};
pub use store::{
    ChunkStore, CompactionReport, Integrity, SCHEMA_VERSION, SnapshotReader, StoreStats,
};
