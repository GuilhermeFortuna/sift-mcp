//! Unix-socket daemon bind, accept loop, and request handling.

use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use fs4::fs_std::FileExt;
use inference::Embedder;
use retrieval::FusionConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinSet;
use tracing::{info, warn};

use crate::codec::encode;
use crate::handshake::handle_hello;
use crate::paths::{
    assert_socket_permissions, lock_path_for_store, open_lock_file, prepare_socket_path,
    tighten_socket_permissions,
};
use crate::protocol::{DaemonError, Envelope, IndexMode, Request, Response};
use crate::resident::{
    ProgressForwarder, Resident, ServingState, SharedState, index_report_response,
    rebuild_resident, run_index, split_for_index,
};

pub struct DaemonConfig {
    pub store_dir: PathBuf,
    pub model_dir: PathBuf,
    pub repo_dir: PathBuf,
    pub socket_path: PathBuf,
    pub idle_timeout: Duration,
    pub max_concurrent_searches: usize,
    pub fusion: FusionConfig,
}

impl DaemonConfig {
    pub fn for_store(
        store_dir: PathBuf,
        repo_dir: PathBuf,
        model_dir: PathBuf,
    ) -> Result<Self, DaemonError> {
        let socket_path = crate::paths::socket_path_for_store(&store_dir)?;
        Ok(Self {
            store_dir,
            model_dir,
            repo_dir,
            socket_path,
            idle_timeout: Duration::from_secs(15 * 60),
            max_concurrent_searches: 4,
            fusion: FusionConfig::default(),
        })
    }
}

pub struct Daemon {
    pub config: DaemonConfig,
    pub listener: UnixListener,
    pub lock_file: File,
    pub state: Arc<SharedState>,
    pub embedder: Arc<dyn Embedder>,
}

pub enum BindOutcome {
    Bound(Box<Daemon>),
    LockHeld,
}

impl Daemon {
    /// Acquires the single-instance lock, then binds. Loading happens after bind.
    pub async fn try_bind(
        config: DaemonConfig,
        embedder: Arc<dyn Embedder>,
    ) -> Result<BindOutcome, DaemonError> {
        let lock_path = config
            .socket_path
            .parent()
            .map(|p| p.join("daemon.lock"))
            .unwrap_or(lock_path_for_store(&config.store_dir)?);

        let lock_file = open_lock_file(&lock_path)?;
        let got = lock_file
            .try_lock_exclusive()
            .map_err(|e| DaemonError::Internal {
                detail: format!("try_lock: {e}"),
            })?;
        if !got {
            return Ok(BindOutcome::LockHeld);
        }

        prepare_socket_path(&config.socket_path)?;
        let listener =
            UnixListener::bind(&config.socket_path).map_err(|e| DaemonError::Internal {
                detail: format!("bind {}: {e}", config.socket_path.display()),
            })?;
        tighten_socket_permissions(&config.socket_path)?;
        assert_socket_permissions(&config.socket_path)?;

        let state = SharedState::new(
            config.fusion,
            config.max_concurrent_searches,
            config.idle_timeout,
        );

        Ok(BindOutcome::Bound(Box::new(Daemon {
            config,
            listener,
            lock_file,
            state,
            embedder,
        })))
    }

    pub async fn bind(
        config: DaemonConfig,
        embedder: Arc<dyn Embedder>,
    ) -> Result<Self, DaemonError> {
        match Self::try_bind(config, embedder).await? {
            BindOutcome::Bound(d) => Ok(*d),
            BindOutcome::LockHeld => Err(DaemonError::Internal {
                detail: "lock held by another daemon".into(),
            }),
        }
    }

    /// Spawn background load of Resident, then serve until idle/shutdown.
    pub async fn serve(self) -> Result<(), DaemonError> {
        let state = Arc::clone(&self.state);
        let store_dir = self.config.store_dir.clone();
        let repo_dir = self.config.repo_dir.clone();
        let embedder = Arc::clone(&self.embedder);
        let load_state = Arc::clone(&self.state);
        tokio::task::spawn_blocking(move || {
            if let Some(delay) = *load_state.load_delay.lock() {
                std::thread::sleep(delay);
            }
            match Resident::load(&store_dir, &repo_dir, embedder) {
                Ok(resident) => match resident.into_ready() {
                    Ok(ready) => {
                        *load_state.serving.write().unwrap() = ServingState::Ready(Arc::new(ready));
                    }
                    Err(e) => {
                        warn!(error = ?e, "resident snapshot failed");
                        *load_state.serving.write().unwrap() =
                            ServingState::Stale(format!("snapshot failed: {e:?}"));
                    }
                },
                Err(e) => {
                    warn!(error = ?e, "resident load failed");
                    *load_state.serving.write().unwrap() =
                        ServingState::Stale(format!("load failed: {e:?}"));
                }
            }
        });

        let mut jobs = JoinSet::new();
        let listener = self.listener;
        let socket_path = self.config.socket_path.clone();
        let _lock_file = self.lock_file;

        loop {
            let idle = state.idle_timeout;
            let shutdown = state.shutdown.notified();
            tokio::select! {
                biased;
                _ = shutdown => {
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    let clients = *state.connected_clients.lock();
                    let idle_elapsed = state.last_request_at.lock().elapsed() >= idle;
                    let not_starting = !matches!(
                        *state.serving.read().unwrap(),
                        ServingState::Starting
                    );
                    if clients == 0 && idle_elapsed && not_starting {
                        info!("idle timeout reached; shutting down");
                        break;
                    }
                }
                accept = listener.accept() => {
                    match accept {
                        Ok((stream, _)) => {
                            if *state.shutting_down.lock() {
                                drop(stream);
                                continue;
                            }
                            *state.connected_clients.lock() += 1;
                            let st = Arc::clone(&state);
                            jobs.spawn(async move {
                                if let Err(e) = handle_connection(stream, st.clone()).await {
                                    warn!(error = ?e, "connection error");
                                }
                                *st.connected_clients.lock() -= 1;
                                st.touch();
                            });
                        }
                        Err(e) => {
                            warn!(error = %e, "accept failed");
                        }
                    }
                }
            }
        }

        *state.shutting_down.lock() = true;
        while jobs.join_next().await.is_some() {}
        *state.serving.write().unwrap() = ServingState::Starting;
        let _ = std::fs::remove_file(&socket_path);
        Ok(())
    }
}

async fn handle_connection(stream: UnixStream, state: Arc<SharedState>) -> Result<(), DaemonError> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(reader);
    let mut hello_done = false;

    loop {
        let env: Envelope<Request> = match read_envelope(&mut reader).await {
            Ok(e) => e,
            Err(DaemonError::Malformed { detail }) if detail.contains("truncated") => {
                break;
            }
            Err(e) => {
                let resp = Response::Error(e.clone());
                write_response(&mut writer, 0, resp).await?;
                if matches!(
                    e,
                    DaemonError::RequestTooLarge { .. } | DaemonError::Malformed { .. }
                ) {
                    continue;
                }
                break;
            }
        };
        state.touch();
        let request_id = env.request_id;
        let req_type = request_type_name(&env.payload);

        if !hello_done {
            let starting = matches!(*state.serving.read().unwrap(), ServingState::Starting);
            let stale_reason = match &*state.serving.read().unwrap() {
                ServingState::Stale(r) => Some(r.clone()),
                _ => None,
            };
            if let Some(reason) = stale_reason {
                let resp = Response::Error(DaemonError::StoreStale { reason });
                write_response(&mut writer, request_id, resp).await?;
                continue;
            }
            if starting {
                match &env.payload {
                    Request::Hello { .. } => {}
                    Request::Status => {
                        let resp = Response::Status(state.status());
                        log_request(request_id, req_type, "ok", None);
                        write_response(&mut writer, request_id, resp).await?;
                        continue;
                    }
                    _ => {
                        let resp = Response::Error(DaemonError::Starting);
                        log_request(request_id, req_type, "starting", None);
                        write_response(&mut writer, request_id, resp).await?;
                        continue;
                    }
                }
            }

            let (model_id, chunks) = {
                let guard = state.serving.read().unwrap();
                match &*guard {
                    ServingState::Ready(r) => {
                        let parts = r.parts.lock().unwrap();
                        let live = parts.store.stats().map(|s| s.live).unwrap_or(0);
                        (r.search.model_id.clone(), live)
                    }
                    ServingState::Indexing(f) => (f.model_id.clone(), f.chunks_live),
                    ServingState::Starting => (String::new(), 0),
                    ServingState::Stale(_) => (String::new(), 0),
                }
            };

            match handle_hello(&env, &model_id, chunks) {
                Ok((_ok, resp)) => {
                    hello_done = true;
                    log_request(request_id, "Hello", "ok", None);
                    write_response(&mut writer, request_id, *resp).await?;
                }
                Err(resp) => {
                    log_request(request_id, req_type, "error", None);
                    write_response(&mut writer, request_id, *resp).await?;
                    break;
                }
            }
            continue;
        }

        let outcome = dispatch_request(&env.payload, &state, &mut writer, request_id).await;
        match outcome {
            Ok(stage) => log_request(request_id, req_type, "ok", stage),
            Err(e) => {
                log_request(request_id, req_type, "error", None);
                write_response(&mut writer, request_id, Response::Error(e)).await?;
            }
        }

        if matches!(env.payload, Request::Shutdown) {
            state.shutdown.notify_waiters();
            break;
        }
    }
    Ok(())
}

async fn dispatch_request(
    req: &Request,
    state: &Arc<SharedState>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    request_id: u64,
) -> Result<Option<String>, DaemonError> {
    match req {
        Request::Hello { .. } => Err(DaemonError::Malformed {
            detail: "duplicate Hello".into(),
        }),
        Request::Status => {
            write_response(writer, request_id, Response::Status(state.status())).await?;
            Ok(None)
        }
        Request::Shutdown => {
            write_response(writer, request_id, Response::Status(state.status())).await?;
            Ok(None)
        }
        Request::Search { query, top_k } => {
            let delay = *state.search_delay.lock();
            let _permit = state
                .search_sem
                .acquire()
                .await
                .map_err(|_| DaemonError::Internal {
                    detail: "semaphore closed".into(),
                })?;
            let fusion = state.fusion;
            let top_k = *top_k;
            let query = query.clone();
            enum Target {
                Ready(Arc<crate::resident::FrozenSearch>),
                Frozen(Arc<crate::resident::FrozenSearch>),
            }
            let target = {
                let guard = state.serving.read().unwrap();
                match &*guard {
                    ServingState::Starting => return Err(DaemonError::Starting),
                    ServingState::Stale(r) => {
                        return Err(DaemonError::StoreStale { reason: r.clone() });
                    }
                    ServingState::Ready(r) => Target::Ready(Arc::clone(&r.search)),
                    ServingState::Indexing(f) => Target::Frozen(Arc::clone(f)),
                }
            };
            let resp = match target {
                Target::Frozen(f) => tokio::task::spawn_blocking(move || {
                    if let Some(d) = delay {
                        std::thread::sleep(d);
                    }
                    f.search(&query, top_k, &fusion).map(Response::Search)
                })
                .await
                .map_err(|e| DaemonError::Internal {
                    detail: format!("join: {e}"),
                })??,
                Target::Ready(ready) => tokio::task::spawn_blocking(move || {
                    if let Some(d) = delay {
                        std::thread::sleep(d);
                    }
                    ready.search(&query, top_k, &fusion).map(Response::Search)
                })
                .await
                .map_err(|e| DaemonError::Internal {
                    detail: format!("join: {e}"),
                })??,
            };
            let stage = match &resp {
                Response::Search(s) => Some(format!("{:?}", s.diagnostics.stage_millis)),
                _ => None,
            };
            write_response(writer, request_id, resp).await?;
            Ok(stage)
        }
        Request::SearchSimilar { code, top_k } => {
            let delay = *state.search_delay.lock();
            let _permit = state
                .search_sem
                .acquire()
                .await
                .map_err(|_| DaemonError::Internal {
                    detail: "semaphore closed".into(),
                })?;
            let fusion = state.fusion;
            let top_k = *top_k;
            let code = code.clone();
            enum Target {
                Ready(Arc<crate::resident::FrozenSearch>),
                Frozen(Arc<crate::resident::FrozenSearch>),
            }
            let target = {
                let guard = state.serving.read().unwrap();
                match &*guard {
                    ServingState::Starting => return Err(DaemonError::Starting),
                    ServingState::Stale(r) => {
                        return Err(DaemonError::StoreStale { reason: r.clone() });
                    }
                    ServingState::Ready(r) => Target::Ready(Arc::clone(&r.search)),
                    ServingState::Indexing(f) => Target::Frozen(Arc::clone(f)),
                }
            };
            let resp = match target {
                Target::Frozen(f) => tokio::task::spawn_blocking(move || {
                    if let Some(d) = delay {
                        std::thread::sleep(d);
                    }
                    f.search_similar(&code, top_k, &fusion)
                        .map(Response::Search)
                })
                .await
                .map_err(|e| DaemonError::Internal {
                    detail: format!("join: {e}"),
                })??,
                Target::Ready(ready) => tokio::task::spawn_blocking(move || {
                    if let Some(d) = delay {
                        std::thread::sleep(d);
                    }
                    ready
                        .search_similar(&code, top_k, &fusion)
                        .map(Response::Search)
                })
                .await
                .map_err(|e| DaemonError::Internal {
                    detail: format!("join: {e}"),
                })??,
            };
            write_response(writer, request_id, resp).await?;
            Ok(None)
        }
        Request::GetSymbol { file, symbol } => {
            let file = file.clone();
            let symbol = symbol.clone();
            enum Target {
                Ready(Arc<crate::resident::FrozenSearch>),
                Frozen(Arc<crate::resident::FrozenSearch>),
            }
            let target = {
                let guard = state.serving.read().unwrap();
                match &*guard {
                    ServingState::Starting => return Err(DaemonError::Starting),
                    ServingState::Stale(r) => {
                        return Err(DaemonError::StoreStale { reason: r.clone() });
                    }
                    ServingState::Ready(r) => Target::Ready(Arc::clone(&r.search)),
                    ServingState::Indexing(f) => Target::Frozen(Arc::clone(f)),
                }
            };
            let resp = match target {
                Target::Frozen(f) => f.get_symbol(&file, &symbol)?,
                Target::Ready(ready) => {
                    tokio::task::spawn_blocking(move || ready.get_symbol(&file, &symbol))
                        .await
                        .map_err(|e| DaemonError::Internal {
                            detail: format!("join: {e}"),
                        })??
                }
            };
            write_response(writer, request_id, resp).await?;
            Ok(None)
        }
        Request::Index { mode, repo_dir } => {
            if state
                .indexing
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return Err(DaemonError::IndexInProgress);
            }
            let delay = *state.index_phase_delay.lock();
            let full = matches!(mode, IndexMode::Full);
            let repo_dir = repo_dir.clone();
            let state_c = Arc::clone(state);

            let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
            let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();

            std::thread::spawn(move || {
                let clear_indexing = || {
                    state_c.indexing.store(false, Ordering::SeqCst);
                };
                let taken = {
                    let mut serve = state_c.serving.write().unwrap();
                    match std::mem::replace(&mut *serve, ServingState::Starting) {
                        ServingState::Ready(r) => match Arc::try_unwrap(r) {
                            Ok(ready) => ready,
                            Err(r) => {
                                *serve = ServingState::Ready(r);
                                clear_indexing();
                                let _ = done_tx.send(Err(DaemonError::Internal {
                                    detail: "resident still borrowed".into(),
                                }));
                                return;
                            }
                        },
                        other => {
                            *serve = other;
                            clear_indexing();
                            let _ = done_tx.send(Err(DaemonError::Internal {
                                detail: "not ready for index".into(),
                            }));
                            return;
                        }
                    }
                };
                let split = match split_for_index(taken) {
                    Ok(s) => s,
                    Err(e) => {
                        clear_indexing();
                        let _ = done_tx.send(Err(e));
                        return;
                    }
                };
                let (frozen, store, lexical, _resident_repo_dir, embedder) = split;
                *state_c.serving.write().unwrap() = ServingState::Indexing(frozen);

                if let Some(d) = delay {
                    std::thread::sleep(d);
                }

                let mut progress = ProgressForwarder {
                    tx: progress_tx,
                    delay,
                };
                drop(lexical);
                let result = run_index(store, embedder.as_ref(), &repo_dir, full, &mut progress);
                match result {
                    Ok((store, lexical, report)) => {
                        match rebuild_resident(store, lexical, embedder, repo_dir) {
                            Ok(resident) => match resident.into_ready() {
                                Ok(ready) => {
                                    *state_c.serving.write().unwrap() =
                                        ServingState::Ready(Arc::new(ready));
                                    clear_indexing();
                                    let _ = done_tx.send(Ok(report));
                                }
                                Err(e) => {
                                    *state_c.serving.write().unwrap() =
                                        ServingState::Stale(format!("snapshot: {e:?}"));
                                    clear_indexing();
                                    let _ = done_tx.send(Err(e));
                                }
                            },
                            Err(e) => {
                                *state_c.serving.write().unwrap() =
                                    ServingState::Stale(format!("rebuild: {e:?}"));
                                clear_indexing();
                                let _ = done_tx.send(Err(e));
                            }
                        }
                    }
                    Err(e) => {
                        *state_c.serving.write().unwrap() =
                            ServingState::Stale(format!("index: {e:?}"));
                        clear_indexing();
                        let _ = done_tx.send(Err(e));
                    }
                }
            });

            let final_res = loop {
                tokio::select! {
                    Some((phase, done, total)) = progress_rx.recv() => {
                        write_response(writer, request_id, Response::IndexProgress {
                            phase: crate::protocol::IndexPhase::from(phase),
                            done,
                            total,
                        }).await?;
                    }
                    result = &mut done_rx => {
                        break result.unwrap_or(Err(DaemonError::Internal {
                            detail: "index worker vanished".into(),
                        }));
                    }
                }
            };
            match final_res {
                Ok(report) => {
                    write_response(writer, request_id, index_report_response(&report)).await?;
                    Ok(None)
                }
                Err(e) => Err(e),
            }
        }
    }
}

async fn read_envelope(
    reader: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
) -> Result<Envelope<Request>, DaemonError> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(DaemonError::Malformed {
                detail: "truncated length prefix".into(),
            });
        }
        Err(e) => {
            return Err(DaemonError::Malformed {
                detail: format!("read length: {e}"),
            });
        }
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > crate::protocol::MAX_REQUEST_BYTES {
        let mut left = len;
        let mut buf = vec![0u8; 8192];
        while left > 0 {
            let n = left.min(buf.len());
            reader
                .read_exact(&mut buf[..n])
                .await
                .map_err(|e| DaemonError::Malformed {
                    detail: format!("drain oversized: {e}"),
                })?;
            left -= n;
        }
        return Err(DaemonError::RequestTooLarge {
            bytes: len,
            limit: crate::protocol::MAX_REQUEST_BYTES,
        });
    }
    let mut payload = vec![0u8; len];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|e| DaemonError::Malformed {
            detail: format!("truncated frame body: {e}"),
        })?;
    bincode::deserialize(&payload).map_err(|e| DaemonError::Malformed {
        detail: format!("decode: {e}"),
    })
}

async fn write_response(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    request_id: u64,
    payload: Response,
) -> Result<(), DaemonError> {
    let env = Envelope {
        request_id,
        payload,
    };
    let bytes = encode(&env)?;
    writer
        .write_all(&bytes)
        .await
        .map_err(|e| DaemonError::Internal {
            detail: format!("write: {e}"),
        })?;
    Ok(())
}

fn request_type_name(req: &Request) -> &'static str {
    match req {
        Request::Hello { .. } => "Hello",
        Request::Search { .. } => "Search",
        Request::SearchSimilar { .. } => "SearchSimilar",
        Request::GetSymbol { .. } => "GetSymbol",
        Request::Index { .. } => "Index",
        Request::Status => "Status",
        Request::Shutdown => "Shutdown",
    }
}

fn log_request(request_id: u64, req_type: &str, outcome: &str, stage: Option<String>) {
    info!(
        request_id,
        req_type,
        outcome,
        stage = stage.as_deref().unwrap_or(""),
        "daemon request"
    );
}
