use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvalError {
    #[error("{0}")]
    Message(String),
    #[error("corpus revision mismatch: expected {expected}, got {actual}")]
    RevisionMismatch { expected: String, actual: String },
    #[error("git: {0}")]
    Git(String),
    #[error("index: {0}")]
    Index(String),
    #[error("retrieval: {0}")]
    Retrieval(String),
    #[error("store: {0}")]
    Store(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

impl EvalError {
    pub fn message(msg: impl Into<String>) -> Self {
        Self::Message(msg.into())
    }
}
