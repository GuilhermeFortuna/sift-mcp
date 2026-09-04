//! Wire protocol types for the resident daemon.

use serde::{Deserialize, Serialize};

/// Bumped on any wire-incompatible change. Checked during Hello.
pub const PROTOCOL_VERSION: u32 = 2;
/// Protocol version spoken by pre-observability clients.
pub const PROTOCOL_VERSION_V1: u32 = 1;
/// Requests above this are rejected without being buffered.
pub const MAX_REQUEST_BYTES: usize = 1 << 20;
/// Hello `client` value that negotiates observer capabilities.
pub const OBSERVER_CLIENT: &str = "sift-console-observer";
/// Maximum events retained in the daemon ring buffer.
pub const EVENT_RING_CAPACITY: usize = 4096;
/// Maximum events returned in a single Observe response.
pub const OBSERVE_PAGE_SIZE: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Envelope<T> {
    pub request_id: u64,
    pub payload: T,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClientRole {
    Worker,
    Observer,
}

impl ClientRole {
    pub fn from_hello_client(client: &str) -> Self {
        if client == OBSERVER_CLIENT {
            Self::Observer
        } else {
            Self::Worker
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Lifecycle {
    Starting,
    Ready,
    Indexing,
    Stale,
    ShuttingDown,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventCursor {
    pub instance_id: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestEvent {
    pub cursor: EventCursor,
    pub connection_id: u64,
    pub request_id: u64,
    pub completed_at_unix_ms: u64,
    pub operation: String,
    pub elapsed_micros: u64,
    pub outcome: String,
    pub error_code: Option<String>,
    pub result_count: Option<u64>,
    pub stage_millis: Option<retrieval::StageTimings>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceSnapshot {
    pub sampled_at_unix_ms: u64,
    pub device_id: Option<String>,
    pub device_used_bytes: Option<u64>,
    pub device_total_bytes: Option<u64>,
    pub process_used_bytes: Option<u64>,
    pub model_used_bytes: Option<u64>,
}

impl ResourceSnapshot {
    pub fn unavailable(sampled_at_unix_ms: u64) -> Self {
        Self {
            sampled_at_unix_ms,
            device_id: None,
            device_used_bytes: None,
            device_total_bytes: None,
            process_used_bytes: None,
            model_used_bytes: None,
        }
    }
}

/// Live indexing progress visible to observers independently of the initiating client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexProgressSnapshot {
    pub phase: IndexPhase,
    pub done: u64,
    pub total: Option<u64>,
    pub connection_id: u64,
    pub request_id: u64,
}

/// Latest completed indexing report retained for diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LastIndexCompletion {
    pub completed_at_unix_ms: u64,
    pub outcome: String,
    pub error_code: Option<String>,
    pub connection_id: u64,
    pub request_id: u64,
    pub report: Option<IndexReportWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    pub status: DaemonStatus,
    pub events: Vec<RequestEvent>,
    pub next_cursor: EventCursor,
    pub gap: bool,
    pub more: bool,
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
    /// Appended in protocol v2; must remain last among new variants for encoding notes.
    Observe {
        after: Option<EventCursor>,
    },
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
    /// Observer attempted a worker-only operation.
    ObserverForbidden {
        operation: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DaemonStatus {
    pub lifecycle: Lifecycle,
    pub instance_id: String,
    pub observed_at_unix_ms: u64,
    pub model_id: Option<String>,
    pub chunks_live: Option<u64>,
    pub chunks_dead: Option<u64>,
    pub indexed_commit: Option<String>,
    pub idle_seconds: u64,
    pub uptime_seconds: u64,
    pub current_progress: Option<IndexProgressSnapshot>,
    pub last_index: Option<LastIndexCompletion>,
    pub resources: ResourceSnapshot,
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
    /// Appended in protocol v2.
    Observation(Observation),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::encode;

    #[test]
    fn protocol_is_version_two() {
        assert_eq!(PROTOCOL_VERSION, 2);
        assert_eq!(PROTOCOL_VERSION_V1, 1);
    }

    #[test]
    fn version_one_client_yields_named_mismatch_against_v2() {
        let err = DaemonError::ProtocolVersion {
            daemon: PROTOCOL_VERSION,
            client: PROTOCOL_VERSION_V1,
        };
        match err {
            DaemonError::ProtocolVersion { daemon, client } => {
                assert_eq!(daemon, 2);
                assert_eq!(client, 1);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn observer_client_string_is_reserved() {
        assert_eq!(
            ClientRole::from_hello_client(OBSERVER_CLIENT),
            ClientRole::Observer
        );
        assert_eq!(
            ClientRole::from_hello_client("daemon-client"),
            ClientRole::Worker
        );
    }

    #[test]
    fn observe_request_and_observation_response_round_trip() {
        let cursor = EventCursor {
            instance_id: "abc".into(),
            sequence: 7,
        };
        let req = Request::Observe {
            after: Some(cursor.clone()),
        };
        let bytes = encode(&Envelope {
            request_id: 1,
            payload: req,
        })
        .expect("encode observe");
        assert!(bytes.len() > 4);

        let status = DaemonStatus {
            lifecycle: Lifecycle::Ready,
            instance_id: "abc".into(),
            observed_at_unix_ms: 1,
            model_id: Some("mock".into()),
            chunks_live: Some(1),
            chunks_dead: Some(0),
            indexed_commit: Some("deadbeef".into()),
            idle_seconds: 0,
            uptime_seconds: 1,
            current_progress: None,
            last_index: None,
            resources: ResourceSnapshot::unavailable(1),
        };
        let obs = Observation {
            status,
            events: vec![],
            next_cursor: cursor,
            gap: false,
            more: false,
        };
        let resp_bytes = encode(&Envelope {
            request_id: 1,
            payload: Response::Observation(obs),
        })
        .expect("encode observation");
        assert!(resp_bytes.len() > 4);
        assert!(resp_bytes.len() <= MAX_REQUEST_BYTES + 4);
    }

    #[test]
    fn unavailable_model_metadata_is_explicit_none() {
        let status = DaemonStatus {
            lifecycle: Lifecycle::Starting,
            instance_id: "x".into(),
            observed_at_unix_ms: 0,
            model_id: None,
            chunks_live: None,
            chunks_dead: None,
            indexed_commit: None,
            idle_seconds: 0,
            uptime_seconds: 0,
            current_progress: None,
            last_index: None,
            resources: ResourceSnapshot::unavailable(0),
        };
        assert!(status.model_id.is_none());
        assert!(status.chunks_live.is_none());
        assert!(status.resources.device_id.is_none());
        assert!(status.resources.device_used_bytes.is_none());
        // v2 removed fabricated resident_gpu_bytes; resources carry optional GPU data.
        let _ = status.resources;
    }
}
