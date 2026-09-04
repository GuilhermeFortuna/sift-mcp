#[derive(Debug, thiserror::Error)]
pub enum RetrievalError {
    #[error("inference error: {0}")]
    Inference(#[from] inference::InferError),

    #[error("store error: {0}")]
    Store(#[from] storage::StoreError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("tantivy error: {0}")]
    Tantivy(String),

    #[error("embedding dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: u32, got: u32 },

    #[error("model identity mismatch: expected {expected}, got {got}")]
    ModelMismatch { expected: String, got: String },

    #[error("dense index error: {0}")]
    Dense(String),

    #[error("both retrievers failed: lexical={lexical}; dense={dense}")]
    BothRetrieversFailed { lexical: String, dense: String },
}
