//! Symbol-aware chunking, exclusions, and content hashing.

mod error;
mod exclusions;
mod hash;
mod language;

pub use error::{ChunkDiagnostic, ChunkError};
pub use exclusions::{Exclusions, SkipReason, HEAD_SNIFF_BYTES, MAX_FILE_BYTES};
pub use hash::{content_hash, normalize_body, HASH_SCHEME_VERSION};
pub use language::Language;
