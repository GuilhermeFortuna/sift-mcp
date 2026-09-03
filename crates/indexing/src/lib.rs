//! Symbol-aware chunking, exclusions, and content hashing.

mod hash;
mod language;

pub use hash::{content_hash, normalize_body, HASH_SCHEME_VERSION};
pub use language::Language;

