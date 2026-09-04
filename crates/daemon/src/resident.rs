//! Resident model/index state and prepare-then-swap indexing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use indexing::{IndexConfig, IndexReport, Indexer, Phase, Progress};
use inference::{Embedder, InferError, Role};
use parking_lot::Mutex as ParkingMutex;
use retrieval::dense::{DenseBackend, DenseIndex, LiveMask};
use retrieval::fusion::{FusionConfig, fuse};
use retrieval::lexical::{LexicalIndex, LexicalSearchHandle};
use retrieval::result::preview_from_body;
use retrieval::{SearchDiagnostics, SearchResponse, SearchResult, Searcher, StageTimings};
use storage::{ChunkRecord, ChunkStore, Integrity, RowId, SCHEMA_VERSION};

use crate::protocol::{
    DaemonError, DaemonStatus, IndexReportWire, LastIndexCompletion, Lifecycle, ResourceSnapshot,
    Response,
};

/// Identity used to detect store delete/replace under a running daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreIdentity {
    pub canonical_path: PathBuf,
    pub schema_version: u32,
    pub model_id: String,
    pub dir_dev: u64,
    pub dir_ino: u64,
}

impl StoreIdentity {
    pub fn capture(store_dir: &Path, model_id: &str) -> Result<Self, DaemonError> {
        let canonical = store_dir
            .canonicalize()
            .map_err(|e| DaemonError::Internal {
                detail: format!("canonicalize {}: {e}", store_dir.display()),
            })?;
        let meta = std::fs::metadata(&canonical).map_err(|e| DaemonError::Internal {
            detail: format!("stat {}: {e}", canonical.display()),
        })?;
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            canonical_path: canonical,
            schema_version: SCHEMA_VERSION,
            model_id: model_id.to_owned(),
            dir_dev: meta.dev(),
            dir_ino: meta.ino(),
        })
    }

    pub fn check_still_valid(&self) -> Result<(), DaemonError> {
        let meta =
            std::fs::metadata(&self.canonical_path).map_err(|e| DaemonError::StoreStale {
                reason: format!("store missing: {e}"),
            })?;
        use std::os::unix::fs::MetadataExt;
        if meta.dev() != self.dir_dev || meta.ino() != self.dir_ino {
            return Err(DaemonError::StoreStale {
                reason: "store directory replaced".into(),
            });
        }
        Ok(())
    }
}

pub struct Resident {
    pub store: ChunkStore,
    pub lexical: LexicalIndex,
    pub dense: DenseIndex,
    pub embedder: Arc<dyn Embedder>,
    pub identity: StoreIdentity,
    pub repo_dir: PathBuf,
}

fn dense_backend() -> DenseBackend {
    #[cfg(feature = "cuda")]
    {
        DenseBackend::Cuda
    }
    #[cfg(not(feature = "cuda"))]
    {
        DenseBackend::Cpu
    }
}

/// Immutable search view used while an index runs against owned store parts.
pub struct FrozenSearch {
    dense: DenseIndex,
    lexical: LexicalSearchHandle,
    records: HashMap<u64, ChunkRecord>,
    bodies: HashMap<u64, String>,
    embedder: Arc<dyn Embedder>,
    identity: StoreIdentity,
    pub model_id: String,
    pub chunks_live: u64,
    pub chunks_dead: u64,
    pub indexed_commit: Option<String>,
}

/// Ready-state ownership split between immutable search data and the single
/// mutable indexing owner. Searches clone only `search`, so they never hold
/// the indexing mutex while doing inference or retrieval.
pub struct ReadyResident {
    pub search: Arc<FrozenSearch>,
    pub parts: Mutex<ResidentParts>,
}

pub struct ResidentParts {
    pub store: ChunkStore,
    pub lexical: LexicalIndex,
    pub embedder: Arc<dyn Embedder>,
    pub repo_dir: PathBuf,
}

pub type IndexParts = (
    Arc<FrozenSearch>,
    ChunkStore,
    LexicalIndex,
    PathBuf,
    Arc<dyn Embedder>,
);

pub enum ServingState {
    Starting,
    Ready(Arc<ReadyResident>),
    Indexing(Arc<FrozenSearch>),
    Stale(String),
}

pub struct SharedState {
    pub serving: RwLock<ServingState>,
    pub indexing: AtomicBool,
    pub fusion: FusionConfig,
    pub max_concurrent_searches: usize,
    pub search_sem: tokio::sync::Semaphore,
    pub started_at: Instant,
    pub last_request_at: ParkingMutex<Instant>,
    pub connected_clients: ParkingMutex<usize>,
    pub idle_timeout: Duration,
    pub load_delay: ParkingMutex<Option<Duration>>,
    pub search_delay: ParkingMutex<Option<Duration>>,
    pub index_phase_delay: ParkingMutex<Option<Duration>>,
    pub shutdown: tokio::sync::Notify,
    pub shutting_down: ParkingMutex<bool>,
    pub instance_id: String,
    pub current_progress: ParkingMutex<Option<crate::protocol::IndexProgressSnapshot>>,
    pub last_index: ParkingMutex<Option<LastIndexCompletion>>,
}

impl SharedState {
    pub fn new(
        fusion: FusionConfig,
        max_concurrent_searches: usize,
        idle_timeout: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            serving: RwLock::new(ServingState::Starting),
            indexing: AtomicBool::new(false),
            fusion,
            max_concurrent_searches,
            search_sem: tokio::sync::Semaphore::new(max_concurrent_searches),
            started_at: Instant::now(),
            last_request_at: ParkingMutex::new(Instant::now()),
            connected_clients: ParkingMutex::new(0),
            idle_timeout,
            load_delay: ParkingMutex::new(None),
            search_delay: ParkingMutex::new(None),
            index_phase_delay: ParkingMutex::new(None),
            shutdown: tokio::sync::Notify::new(),
            shutting_down: ParkingMutex::new(false),
            instance_id: new_instance_id(),
            current_progress: ParkingMutex::new(None),
            last_index: ParkingMutex::new(None),
        })
    }

    pub fn touch(&self) {
        *self.last_request_at.lock() = Instant::now();
    }

    pub fn observed_at_unix_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    pub fn status(&self) -> DaemonStatus {
        let uptime = self.started_at.elapsed().as_secs();
        let idle = self.last_request_at.lock().elapsed().as_secs();
        let observed_at_unix_ms = Self::observed_at_unix_ms();
        let resources = ResourceSnapshot::unavailable(observed_at_unix_ms);
        let current_progress = self.current_progress.lock().clone();
        let last_index = self.last_index.lock().clone();
        let shutting_down = *self.shutting_down.lock();
        let guard = self.serving.read().unwrap();
        let lifecycle = if shutting_down {
            Lifecycle::ShuttingDown
        } else {
            match &*guard {
                ServingState::Starting => Lifecycle::Starting,
                ServingState::Ready(_) => Lifecycle::Ready,
                ServingState::Indexing(_) => Lifecycle::Indexing,
                ServingState::Stale(_) => Lifecycle::Stale,
            }
        };
        match &*guard {
            ServingState::Starting => DaemonStatus {
                lifecycle,
                instance_id: self.instance_id.clone(),
                observed_at_unix_ms,
                model_id: None,
                chunks_live: None,
                chunks_dead: None,
                indexed_commit: None,
                idle_seconds: idle,
                uptime_seconds: uptime,
                current_progress,
                last_index,
                resources,
            },
            ServingState::Ready(r) => {
                let parts = r.parts.lock().unwrap();
                let stats = parts.store.stats().unwrap_or(storage::StoreStats {
                    live: 0,
                    dead: 0,
                    dead_fraction: 0.0,
                });
                DaemonStatus {
                    lifecycle,
                    instance_id: self.instance_id.clone(),
                    observed_at_unix_ms,
                    model_id: Some(r.search.model_id.clone()),
                    chunks_live: Some(stats.live),
                    chunks_dead: Some(stats.dead),
                    indexed_commit: parts.store.indexed_commit().ok().flatten(),
                    idle_seconds: idle,
                    uptime_seconds: uptime,
                    current_progress,
                    last_index,
                    resources,
                }
            }
            ServingState::Indexing(f) => DaemonStatus {
                lifecycle,
                instance_id: self.instance_id.clone(),
                observed_at_unix_ms,
                model_id: Some(f.model_id.clone()),
                chunks_live: Some(f.chunks_live),
                chunks_dead: Some(f.chunks_dead),
                indexed_commit: f.indexed_commit.clone(),
                idle_seconds: idle,
                uptime_seconds: uptime,
                current_progress,
                last_index,
                resources,
            },
            ServingState::Stale(_) => DaemonStatus {
                lifecycle,
                instance_id: self.instance_id.clone(),
                observed_at_unix_ms,
                model_id: None,
                chunks_live: None,
                chunks_dead: None,
                indexed_commit: None,
                idle_seconds: idle,
                uptime_seconds: uptime,
                current_progress,
                last_index,
                resources,
            },
        }
    }
}

fn new_instance_id() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    Instant::now().hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

impl Resident {
    pub fn load(
        store_dir: &Path,
        repo_dir: &Path,
        embedder: Arc<dyn Embedder>,
    ) -> Result<Self, DaemonError> {
        let db_exists = store_dir.join("chunks.db").exists();
        let matrix_exists = store_dir.join("embeddings.f16").exists();
        let manifest_exists = store_dir.join("store.manifest").exists();
        let store = if !db_exists && !matrix_exists && !manifest_exists {
            ChunkStore::create(store_dir, embedder.dims(), embedder.model_id())
        } else {
            ChunkStore::open(store_dir)
        }
        .map_err(|e| DaemonError::Internal {
            detail: format!("open or create store: {e}"),
        })?;
        store
            .require_model(embedder.model_id())
            .map_err(|e| DaemonError::Internal {
                detail: format!("model: {e}"),
            })?;
        let lexical = LexicalIndex::open(store.dir()).map_err(|e| DaemonError::Internal {
            detail: format!("open lexical: {e}"),
        })?;
        let dense =
            DenseIndex::from_store(&store, dense_backend()).map_err(|e| DaemonError::Internal {
                detail: format!("open dense: {e}"),
            })?;
        let identity = StoreIdentity::capture(store_dir, embedder.model_id())?;
        Ok(Self {
            store,
            lexical,
            dense,
            embedder,
            identity,
            repo_dir: repo_dir.to_path_buf(),
        })
    }

    pub fn search(
        &self,
        query: &str,
        top_k: usize,
        fusion: &FusionConfig,
    ) -> Result<SearchResponse, DaemonError> {
        self.identity.check_still_valid()?;
        let searcher = Searcher::new(
            &self.lexical,
            &self.dense,
            &self.store,
            self.embedder.as_ref(),
        );
        searcher
            .search(query, top_k, fusion)
            .map_err(map_retrieval_err)
    }

    pub fn search_similar(
        &self,
        code: &str,
        top_k: usize,
        fusion: &FusionConfig,
    ) -> Result<SearchResponse, DaemonError> {
        self.identity.check_still_valid()?;
        let searcher = Searcher::new(
            &self.lexical,
            &self.dense,
            &self.store,
            self.embedder.as_ref(),
        );
        searcher
            .search_similar(code, top_k, fusion)
            .map_err(map_retrieval_err)
    }

    pub fn get_symbol(&self, file: &str, symbol: &str) -> Result<Response, DaemonError> {
        self.identity.check_still_valid()?;
        resolve_symbol(&self.store, &self.lexical, file, symbol)
    }
}

impl Resident {
    pub fn into_ready(self) -> Result<ReadyResident, DaemonError> {
        let Resident {
            store,
            lexical,
            dense,
            embedder,
            identity,
            repo_dir,
        } = self;
        let frozen = freeze_search(&store, &lexical, dense, Arc::clone(&embedder), identity)?;
        Ok(ReadyResident {
            search: Arc::new(frozen),
            parts: Mutex::new(ResidentParts {
                store,
                lexical,
                embedder,
                repo_dir,
            }),
        })
    }
}

fn freeze_search(
    store: &ChunkStore,
    lexical: &LexicalIndex,
    dense: DenseIndex,
    embedder: Arc<dyn Embedder>,
    identity: StoreIdentity,
) -> Result<FrozenSearch, DaemonError> {
    let stats = store.stats().map_err(|e| DaemonError::Internal {
        detail: format!("stats: {e}"),
    })?;
    let indexed_commit = store.indexed_commit().ok().flatten();
    let live_rows = store.live_rows().map_err(|e| DaemonError::Internal {
        detail: format!("live_rows: {e}"),
    })?;
    let got = store
        .get_many(&live_rows)
        .map_err(|e| DaemonError::Internal {
            detail: format!("get_many: {e}"),
        })?;
    let frozen_lexical = lexical
        .frozen_search_handle()
        .map_err(|e| DaemonError::Internal {
            detail: format!("frozen lexical reader: {e}"),
        })?;
    let body_list = frozen_lexical
        .bodies(&live_rows)
        .map_err(|e| DaemonError::Internal {
            detail: format!("bodies: {e}"),
        })?;
    let mut records = HashMap::new();
    let mut bodies = HashMap::new();
    for ((row, rec), body) in live_rows.iter().zip(got).zip(body_list) {
        if let Some(rec) = rec {
            let id = row.get();
            if let Some(b) = body {
                bodies.insert(id, b);
            }
            records.insert(id, rec);
        }
    }
    Ok(FrozenSearch {
        dense,
        lexical: frozen_lexical,
        records,
        bodies,
        model_id: embedder.model_id().to_owned(),
        embedder,
        identity,
        chunks_live: stats.live,
        chunks_dead: stats.dead,
        indexed_commit,
    })
}

/// Split a ready resident into the immutable search view and mutable index owner.
pub fn split_for_index(ready: ReadyResident) -> Result<IndexParts, DaemonError> {
    let search = ready.search;
    let parts = ready
        .parts
        .into_inner()
        .map_err(|_| DaemonError::Internal {
            detail: "resident parts lock poisoned".into(),
        })?;
    Ok((
        search,
        parts.store,
        parts.lexical,
        parts.repo_dir,
        parts.embedder,
    ))
}

pub fn rebuild_resident(
    store: ChunkStore,
    lexical: LexicalIndex,
    embedder: Arc<dyn Embedder>,
    repo_dir: PathBuf,
) -> Result<Resident, DaemonError> {
    let live = LiveMask::from_store(&store).map_err(|e| DaemonError::Internal {
        detail: format!("live mask: {e}"),
    })?;
    let mut dense =
        DenseIndex::from_store(&store, dense_backend()).map_err(|e| DaemonError::Internal {
            detail: format!("dense: {e}"),
        })?;
    dense
        .refresh(store.matrix(), &live)
        .map_err(|e| DaemonError::Internal {
            detail: format!("dense refresh: {e}"),
        })?;
    let identity = StoreIdentity::capture(store.dir(), embedder.model_id())?;
    Ok(Resident {
        store,
        lexical,
        dense,
        embedder,
        identity,
        repo_dir,
    })
}

impl FrozenSearch {
    pub fn search(
        &self,
        query: &str,
        top_k: usize,
        fusion: &FusionConfig,
    ) -> Result<SearchResponse, DaemonError> {
        self.identity.check_still_valid()?;
        search_with_parts(
            self.embedder.as_ref(),
            &self.lexical,
            &self.dense,
            &self.records,
            &self.bodies,
            query,
            top_k,
            fusion,
            Role::Query,
            true,
        )
    }

    pub fn search_similar(
        &self,
        code: &str,
        top_k: usize,
        fusion: &FusionConfig,
    ) -> Result<SearchResponse, DaemonError> {
        self.identity.check_still_valid()?;
        search_with_parts(
            self.embedder.as_ref(),
            &self.lexical,
            &self.dense,
            &self.records,
            &self.bodies,
            code,
            top_k,
            fusion,
            Role::Document,
            false,
        )
    }

    pub fn get_symbol(&self, file: &str, symbol: &str) -> Result<Response, DaemonError> {
        self.identity.check_still_valid()?;
        let matches: Vec<(u64, &ChunkRecord)> = self
            .records
            .iter()
            .filter(|(_, r)| r.file == file && r.symbol == symbol)
            .map(|(id, r)| (*id, r))
            .collect();
        match matches.len() {
            0 => Err(DaemonError::SymbolNotFound {
                file: file.into(),
                symbol: symbol.into(),
            }),
            1 => {
                let (row, rec) = matches[0];
                let body = self.bodies.get(&row).cloned().unwrap_or_default();
                Ok(Response::Symbol {
                    file: rec.file.clone(),
                    symbol: rec.symbol.clone(),
                    language: rec.language.clone(),
                    signature: rec.signature.clone(),
                    lines: [rec.line_start, rec.line_end],
                    body,
                })
            }
            _ => {
                let candidates: Vec<String> = matches
                    .iter()
                    .map(|(_, r)| format!("{}:{}", r.file, r.signature))
                    .collect();
                Err(DaemonError::SymbolAmbiguous {
                    file: file.into(),
                    symbol: symbol.into(),
                    candidates,
                })
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn search_with_parts(
    embedder: &dyn Embedder,
    lexical: &LexicalSearchHandle,
    dense: &DenseIndex,
    records: &HashMap<u64, ChunkRecord>,
    bodies: &HashMap<u64, String>,
    text: &str,
    top_k: usize,
    fusion: &FusionConfig,
    role: Role,
    run_lexical: bool,
) -> Result<SearchResponse, DaemonError> {
    let total = Instant::now();
    let embed_started = Instant::now();
    let embedded = embedder.embed(&[text], role).map_err(map_infer_err)?;
    let embed_millis = embed_started.elapsed().as_millis() as u64;
    let vector = &embedded[0].vector;

    let (lexical_rows, lexical_error, lexical_millis) = if run_lexical {
        let lex_started = Instant::now();
        let (rows, error) = match lexical.search(text, fusion.lexical_depth) {
            Ok(rows) => (rows, None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        (rows, error, lex_started.elapsed().as_millis() as u64)
    } else {
        (Vec::new(), None, 0)
    };

    let dense_started = Instant::now();
    let (dense_rows, dense_error) =
        match dense.search(vector, embedder.model_id(), fusion.dense_depth) {
            Ok(rows) => (rows, None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
    let dense_millis = dense_started.elapsed().as_millis() as u64;

    let fuse_started = Instant::now();
    let fused = fuse(&lexical_rows, &dense_rows, fusion);
    let fuse_millis = fuse_started.elapsed().as_millis() as u64;

    let assemble_started = Instant::now();
    let mut results = Vec::new();
    for row in fused.into_iter().take(top_k) {
        let id = row.row.get();
        let Some(record) = records.get(&id) else {
            continue;
        };
        let body = bodies.get(&id).map(String::as_str).unwrap_or("");
        results.push(SearchResult {
            file: record.file.clone(),
            symbol: record.symbol.clone(),
            signature: record.signature.clone(),
            doc: record.doc_first_line.clone(),
            preview: preview_from_body(body),
            lines: [record.line_start, record.line_end],
            lexical_score: row.lexical.score,
            dense_score: row.dense.score,
            fused_score: row.fused_score,
        });
    }
    let assemble_millis = assemble_started.elapsed().as_millis() as u64;

    Ok(SearchResponse {
        results,
        diagnostics: SearchDiagnostics {
            lexical_ok: lexical_error.is_none(),
            dense_ok: dense_error.is_none(),
            lexical_error,
            dense_error,
            stage_millis: StageTimings {
                embed: embed_millis,
                lexical: lexical_millis,
                dense: dense_millis,
                fuse: fuse_millis,
                assemble: assemble_millis,
                total: total.elapsed().as_millis() as u64,
            },
        },
    })
}

pub fn resolve_symbol(
    store: &ChunkStore,
    lexical: &LexicalIndex,
    file: &str,
    symbol: &str,
) -> Result<Response, DaemonError> {
    let rows = store
        .rows_for_file(file)
        .map_err(|e| DaemonError::Internal {
            detail: format!("rows_for_file: {e}"),
        })?;
    let mut matches: Vec<(RowId, ChunkRecord)> = Vec::new();
    for row in rows {
        if let Some(rec) = store.get(row).map_err(|e| DaemonError::Internal {
            detail: format!("get: {e}"),
        })? && rec.symbol == symbol
        {
            matches.push((row, rec));
        }
    }
    match matches.len() {
        0 => Err(DaemonError::SymbolNotFound {
            file: file.into(),
            symbol: symbol.into(),
        }),
        1 => {
            let (row, rec) = matches.pop().unwrap();
            let bodies = lexical.bodies(&[row]).map_err(|e| DaemonError::Internal {
                detail: format!("bodies: {e}"),
            })?;
            let body = bodies.into_iter().next().flatten().unwrap_or_default();
            Ok(Response::Symbol {
                file: rec.file,
                symbol: rec.symbol,
                language: rec.language,
                signature: rec.signature,
                lines: [rec.line_start, rec.line_end],
                body,
            })
        }
        _ => {
            let candidates: Vec<String> = matches
                .iter()
                .map(|(_, r)| format!("{}:{}", r.file, r.signature))
                .collect();
            Err(DaemonError::SymbolAmbiguous {
                file: file.into(),
                symbol: symbol.into(),
                candidates,
            })
        }
    }
}

pub struct ProgressForwarder {
    pub tx: tokio::sync::mpsc::UnboundedSender<(Phase, u64, Option<u64>)>,
    pub delay: Option<Duration>,
}

impl Progress for ProgressForwarder {
    fn phase(&mut self, phase: Phase, done: u64, total: Option<u64>) {
        if let Some(d) = self.delay {
            std::thread::sleep(d);
        }
        let _ = self.tx.send((phase, done, total));
    }
}

pub fn run_index(
    store: ChunkStore,
    embedder: &dyn Embedder,
    repo: &Path,
    full: bool,
    progress: &mut dyn Progress,
) -> Result<(ChunkStore, LexicalIndex, IndexReport), DaemonError> {
    let mut indexer =
        Indexer::open(store, embedder, repo, IndexConfig::default()).map_err(|e| {
            DaemonError::Internal {
                detail: format!("indexer open: {e}"),
            }
        })?;
    let report = if full {
        indexer.index_all(progress)
    } else {
        indexer.update(progress)
    }
    .map_err(|e| DaemonError::Internal {
        detail: format!("index: {e}"),
    })?;
    match indexer.store().verify() {
        Ok(Integrity::Ok { .. }) => {}
        Ok(other) => {
            return Err(DaemonError::Internal {
                detail: format!("verify after index: {other:?}"),
            });
        }
        Err(e) => {
            return Err(DaemonError::Internal {
                detail: format!("verify: {e}"),
            });
        }
    }
    let (store, lexical) = indexer.into_parts();
    Ok((store, lexical, report))
}

pub fn map_infer_err(e: InferError) -> DaemonError {
    match e {
        InferError::GpuUnavailable { detail } => DaemonError::GpuUnavailable { detail },
        other => DaemonError::Internal {
            detail: other.to_string(),
        },
    }
}

fn map_retrieval_err(e: retrieval::RetrievalError) -> DaemonError {
    match e {
        retrieval::RetrievalError::Inference(InferError::GpuUnavailable { detail }) => {
            DaemonError::GpuUnavailable { detail }
        }
        other => DaemonError::Internal {
            detail: other.to_string(),
        },
    }
}

pub fn index_report_response(report: &IndexReport) -> Response {
    Response::IndexDone(IndexReportWire::from(report))
}
