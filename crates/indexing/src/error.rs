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

/// A recoverable problem while chunking a single file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkDiagnostic {
    pub file: String,
    pub message: String,
}
