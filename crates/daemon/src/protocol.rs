//! Wire protocol types for the resident daemon.

use serde::{Deserialize, Serialize};

/// Bumped on any wire-incompatible change. Checked during Hello.
pub const PROTOCOL_VERSION: u32 = 1;
/// Requests above this are rejected without being buffered.
pub const MAX_REQUEST_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Envelope<T> {
    pub request_id: u64,
    pub payload: T,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IndexMode {
    Full,
    Update,
}

/// Wire mirror of indexing phases so thin clients need not depend on `indexing`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IndexPhase {
    Walking,
    Parsing,
    Embedding,
    Storing,
    Compacting,
}

#[cfg(feature = "resident")]
impl From<indexing::Phase> for IndexPhase {
    fn from(p: indexing::Phase) -> Self {
        match p {
            indexing::Phase::Walking => Self::Walking,
            indexing::Phase::Parsing => Self::Parsing,
            indexing::Phase::Embedding => Self::Embedding,
            indexing::Phase::Storing => Self::Storing,
            indexing::Phase::Compacting => Self::Compacting,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Request {
    Hello {
        protocol_version: u32,
        client: String,
    },
    Search {
        query: String,
        top_k: usize,
    },
    SearchSimilar {
        code: String,
        top_k: usize,
    },
    GetSymbol {
        file: String,
        symbol: String,
    },
    Index {
        mode: IndexMode,
        repo_dir: std::path::PathBuf,
    },
    Status,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DaemonError {
    ProtocolVersion {
        daemon: u32,
        client: u32,
    },
    Starting,
    IndexInProgress,
    SymbolNotFound {
        file: String,
        symbol: String,
    },
    SymbolAmbiguous {
        file: String,
        symbol: String,
        candidates: Vec<String>,
    },
    StoreStale {
        reason: String,
    },
    GpuUnavailable {
        detail: String,
    },
    RequestTooLarge {
        bytes: usize,
        limit: usize,
    },
    Malformed {
        detail: String,
    },
    Internal {
        detail: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DaemonStatus {
    pub model_id: String,
    pub chunks_live: u64,
    pub chunks_dead: u64,
    pub indexed_commit: Option<String>,
    pub indexing: bool,
    pub resident_gpu_bytes: u64,
    pub idle_seconds: u64,
    pub uptime_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Response {
    Hello {
        protocol_version: u32,
        model_id: String,
        chunks: u64,
    },
    Search(retrieval::SearchResponse),
    Symbol {
        file: String,
        symbol: String,
        language: String,
        signature: String,
        lines: [u32; 2],
        body: String,
    },
    IndexProgress {
        phase: IndexPhase,
        done: u64,
        total: Option<u64>,
    },
    IndexDone(IndexReportWire),
    Status(DaemonStatus),
    Error(DaemonError),
}

/// Serializable mirror of [`indexing::IndexReport`] for the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexReportWire {
    pub commit: String,
    pub files_seen: u64,
    pub files_indexed: u64,
    pub files_excluded: u64,
    pub files_unsupported: u64,
    pub files_unparsed: u64,
    pub chunks_added: u64,
    pub chunks_reused: u64,
    pub chunks_removed: u64,
    pub embeddings_computed: u64,
    pub chunks_truncated: u64,
    pub parse_millis: u64,
    pub embed_millis: u64,
    pub store_millis: u64,
    pub wall_millis: u64,
    pub live_before: u64,
    pub live_after: u64,
}

#[cfg(feature = "resident")]
impl From<&indexing::IndexReport> for IndexReportWire {
    fn from(r: &indexing::IndexReport) -> Self {
        Self {
            commit: r.commit.clone(),
            files_seen: r.files_seen,
            files_indexed: r.files_indexed,
            files_excluded: r.files_excluded,
            files_unsupported: r.files_unsupported,
            files_unparsed: r.files_unparsed,
            chunks_added: r.chunks_added,
            chunks_reused: r.chunks_reused,
            chunks_removed: r.chunks_removed,
            embeddings_computed: r.embeddings_computed,
            chunks_truncated: r.chunks_truncated,
            parse_millis: r.parse_millis,
            embed_millis: r.embed_millis,
            store_millis: r.store_millis,
            wall_millis: r.wall_millis,
            live_before: r.live_before,
            live_after: r.live_after,
        }
    }
}
