#[derive(Debug, thiserror::Error)]
pub enum RetrievalError {
    #[error("store error: {0}")]
    Store(#[from] storage::StoreError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("tantivy error: {0}")]
    Tantivy(String),
}
