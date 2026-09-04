//! Git-mined evaluation harness for retrieval quality.

pub mod corpus;
pub mod error;
pub mod handwritten;
pub mod metrics;
pub mod mine;
pub mod proxy;
pub mod run;

pub use corpus::{
    FIRST_PARTY_CORPUS_DEFAULT_PATH, HARNESS_VERSION, MINED_CORPUS_DEFAULT_PATH,
    MINED_CORPUS_PINNED_REVISION, expand_home, require_mined_revision,
};
pub use error::EvalError;
pub use handwritten::{default_handwritten_path, load_handwritten};
pub use metrics::{BytesBeforeHit, Metrics, percentile, reciprocal_rank, top_k_accuracy};
pub use mine::{
    Label, LabelSource, MiningConfig, MiningReport, RejectReason, build_held_out_index,
    mine_commits, mine_docstrings, strip_doc_comments,
};
pub use proxy::{BASELINE_COMMAND, bytes_before_hit, median_bytes_before_hit};
pub use run::{
    Ablation, EvalRun, FusionConfigSerde, LengthBucket, RunManifest, evaluate, partition_labels,
};
