use thiserror::Error;

#[derive(Debug, Error)]
pub enum ChunkError {
    #[error("glob pattern error: {0}")]
    Glob(String),
    #[error("ignore rules error: {0}")]
    Ignore(String),
    #[error("tree-sitter error: {0}")]
    Parser(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Errors from the indexing pipeline and git helpers.
#[derive(Debug, Error)]
pub enum IndexError {
    #[error("chunking error: {0}")]
    Chunk(#[from] ChunkError),
    #[error("store error: {0}")]
    Store(#[from] storage::StoreError),
    #[error("inference error: {0}")]
    Infer(#[from] inference::InferError),
    #[error("git error: {0}")]
    Git(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("dirty worktree refused: {0}")]
    DirtyRefused(String),
    #[error("interrupted: {0}")]
    Interrupted(String),
    #[error("{0}")]
    Other(String),
}

/// A recoverable problem while chunking a single file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkDiagnostic {
    pub file: String,
    pub message: String,
}
