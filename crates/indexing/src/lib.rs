//! Symbol-aware chunking, exclusions, and content hashing.

mod chunker;
mod error;
mod exclusions;
mod hash;
mod language;

pub use chunker::{
    Chunk, Chunker, FileChunks, ERROR_NODE_RATIO_THRESHOLD, FILE_PRELUDE_MIN_CHARS,
    OVERSIZE_CHAR_THRESHOLD,
};
pub use error::{ChunkDiagnostic, ChunkError};
pub use exclusions::{Exclusions, SkipReason, HEAD_SNIFF_BYTES, MAX_FILE_BYTES};
pub use hash::{content_hash, normalize_body, HASH_SCHEME_VERSION};
pub use language::Language;
