//! Chunk metadata (SQLite) paired with an fp16 embedding matrix.

mod error;
mod matrix;
mod record;
mod store;

pub use error::StoreError;
pub use matrix::{EmbeddingMatrix, MatrixHeader, MATRIX_FORMAT_VERSION};
pub use record::{ChunkRecord, ContentHash, RowId};
pub use store::{ChunkStore, CompactionReport, Integrity, StoreStats};
