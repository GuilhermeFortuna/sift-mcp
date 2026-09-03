//! Symbol-aware chunking, exclusions, and content hashing.

mod hash;

pub use hash::{content_hash, normalize_body, HASH_SCHEME_VERSION};
