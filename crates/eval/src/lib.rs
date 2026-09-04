//! Git-mined evaluation harness for retrieval quality.

pub mod corpus;
pub mod error;

pub use corpus::{
    FIRST_PARTY_CORPUS_DEFAULT_PATH, HARNESS_VERSION, MINED_CORPUS_DEFAULT_PATH,
    MINED_CORPUS_PINNED_REVISION, expand_home, require_mined_revision,
};
pub use error::EvalError;
