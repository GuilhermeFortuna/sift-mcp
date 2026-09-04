use crate::Integrity;

/// Errors produced by the chunk store and embedding matrix.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("embedding dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: u32, got: u32 },

    #[error("model identity mismatch: expected {expected}, got {got}")]
    ModelMismatch { expected: String, got: String },

    #[error("schema or format version mismatch: expected {expected}, got {got}")]
    SchemaVersion { expected: u32, got: u32 },

    #[error("store correspondence broken: {0:?}")]
    Corrupt(Integrity),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}
