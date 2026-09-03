//! Symbol-aware chunking, exclusions, and content hashing.

mod chunker;
mod error;
mod exclusions;
mod hash;
mod language;

pub use chunker::{
    Chunk, Chunker, ERROR_NODE_RATIO_THRESHOLD, FILE_PRELUDE_MIN_CHARS, FileChunks,
    OVERSIZE_CHAR_THRESHOLD,
};
pub use error::{ChunkDiagnostic, ChunkError};
pub use exclusions::{Exclusions, HEAD_SNIFF_BYTES, MAX_FILE_BYTES, SkipReason};
pub use hash::{HASH_SCHEME_VERSION, content_hash, normalize_body};
pub use language::Language;
