#[cfg(feature = "engine")]
pub mod dense;
#[cfg(all(feature = "engine", test))]
mod dense_reference;
#[cfg(feature = "engine")]
mod error;
#[cfg(feature = "engine")]
pub mod fusion;
#[cfg(feature = "engine")]
pub mod lexical;
pub mod result;
#[cfg(feature = "engine")]
pub mod search;
#[cfg(feature = "engine")]
pub mod tokenize;

pub use result::{
    PREVIEW_MAX_BYTES, SearchDiagnostics, SearchResponse, SearchResult, StageTimings,
    preview_from_body,
};

#[cfg(feature = "engine")]
pub use error::RetrievalError;
#[cfg(feature = "engine")]
pub use fusion::{Contribution, FusedRow, FusionConfig, fuse};
#[cfg(feature = "engine")]
pub use lexical::{LexicalDoc, LexicalIndex, LexicalSearchHandle, ScoredRow};
#[cfg(feature = "engine")]
pub use search::Searcher;
