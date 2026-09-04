//! Git-mined evaluation harness for retrieval quality.

pub mod corpus;
pub mod error;
pub mod metrics;
pub mod mine;

pub use corpus::{
    FIRST_PARTY_CORPUS_DEFAULT_PATH, HARNESS_VERSION, MINED_CORPUS_DEFAULT_PATH,
    MINED_CORPUS_PINNED_REVISION, expand_home, require_mined_revision,
};
pub use error::EvalError;
pub use metrics::{BytesBeforeHit, Metrics, percentile, reciprocal_rank, top_k_accuracy};
pub use mine::{
    Label, LabelSource, MiningConfig, MiningReport, RejectReason, build_held_out_index,
    mine_commits, mine_docstrings, strip_doc_comments,
};
