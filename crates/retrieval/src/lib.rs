pub mod dense;
#[cfg(test)]
mod dense_reference;
mod error;
pub mod fusion;
pub mod lexical;
pub mod result;
pub mod search;
pub mod tokenize;

pub use error::RetrievalError;
pub use fusion::{Contribution, FusedRow, FusionConfig, fuse};
pub use lexical::{LexicalDoc, LexicalIndex, LexicalSearchHandle, ScoredRow};
pub use result::{PREVIEW_MAX_BYTES, SearchResult, preview_from_body};
pub use search::{SearchDiagnostics, SearchResponse, Searcher, StageTimings};
