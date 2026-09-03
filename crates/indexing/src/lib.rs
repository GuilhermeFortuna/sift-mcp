//! Symbol-aware chunking, exclusions, content hashing, and repository indexing.

mod chunker;
mod error;
mod exclusions;
pub mod git;
mod hash;
mod language;
pub mod pipeline;

pub use chunker::{
    Chunk, Chunker, ERROR_NODE_RATIO_THRESHOLD, FILE_PRELUDE_MIN_CHARS, FileChunks,
    OVERSIZE_CHAR_THRESHOLD,
};
pub use error::{ChunkDiagnostic, ChunkError, IndexError};
pub use exclusions::{Exclusions, HEAD_SNIFF_BYTES, MAX_FILE_BYTES, SkipReason};
pub use git::{FileChange, RepoGit};
pub use hash::{HASH_SCHEME_VERSION, content_hash, normalize_body};
pub use language::Language;
pub use pipeline::{
    DIRTY_COMMIT_SUFFIX, DirtyPolicy, IndexConfig, IndexReport, Indexer, NullProgress, Phase,
    Progress, require_verify_ok,
};
