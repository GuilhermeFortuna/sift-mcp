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
use tracing::{Level, debug, error, info, warn};

use crate::codec::encode;
use crate::handshake::handle_hello;
use crate::paths::{
    assert_socket_permissions, lock_path_for_store, open_lock_file, prepare_socket_path,
    tighten_socket_permissions,
};
use crate::protocol::{ClientRole, DaemonError, Envelope, IndexMode, Request, Response};
use crate::resident::{
    ProgressForwarder, Resident, ServingState, SharedState, index_report_response,
    rebuild_resident, restore_after_failed_refresh, run_index, split_for_index,
};

/// How long a connection may sit before completing Hello.
const PROVISIONAL_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct DaemonConfig {
    pub store_dir: PathBuf,
    pub model_dir: PathBuf,
    pub repo_dir: PathBuf,
    pub socket_path: PathBuf,
    pub idle_timeout: Duration,
    pub max_concurrent_searches: usize,
    pub fusion: FusionConfig,
    /// When false, status/observe still work but terminal events are not recorded.
    pub record_events: bool,
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
            record_events: true,
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
        state
            .record_events
            .store(config.record_events, std::sync::atomic::Ordering::Relaxed);

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
            info!("loading resident model and index");
            if let Some(delay) = *load_state.load_delay.lock() {
                std::thread::sleep(delay);
            }
            match Resident::load(&store_dir, &repo_dir, embedder) {
                Ok(resident) => match resident.into_ready() {
                    Ok(ready) => {
                        info!("resident model and index ready");
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
        let connection_embedder = Arc::clone(&self.embedder);
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
                            // Provisional: do not count as a worker until Hello classifies.
                            let st = Arc::clone(&state);
                            let emb = Arc::clone(&connection_embedder);
                            jobs.spawn(async move {
                                let role = match handle_connection(stream, st.clone(), emb).await {
                                    Ok(role) => role,
                                    Err(e) => {
                                        warn!(error = ?e, "connection error");
                                        None
                                    }
                                };
                                if role == Some(ClientRole::Worker) {
                                    *st.connected_clients.lock() -= 1;
                                    st.touch();
                                }
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
        state.connection_shutdown.notify_waiters();
        while jobs.join_next().await.is_some() {}
        *state.serving.write().unwrap() = ServingState::Starting;
        let _ = std::fs::remove_file(&socket_path);
        Ok(())
    }
}

async fn handle_connection(
    stream: UnixStream,
    state: Arc<SharedState>,
    embedder: Arc<dyn Embedder>,
) -> Result<Option<ClientRole>, DaemonError> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(reader);

    // Provisional handshake deadline: incomplete Hello must not pin residency.
    let hello_env =
        match read_envelope_until(&mut reader, &state, PROVISIONAL_HANDSHAKE_TIMEOUT).await {
            Ok(Some(env)) => env,
            Ok(None) => return Ok(None),
            Err(DaemonError::Malformed { detail }) if detail.contains("truncated") => {
                return Ok(None);
            }
            Err(e) => {
                let _ = write_response(&mut writer, 0, Response::Error(e.clone())).await;
                return Ok(None);
            }
        };

    let request_id = hello_env.request_id;
    let req_type = request_type_name(&hello_env.payload);

    let is_observer_hello = matches!(
        &hello_env.payload,
        Request::Hello { client, .. } if ClientRole::from_hello_client(client) == ClientRole::Observer
    );

    // Workers are rejected during Starting/Stale; observers may diagnose those states.
    if !is_observer_hello {
        let starting = matches!(*state.serving.read().unwrap(), ServingState::Starting);
        let stale_reason = match &*state.serving.read().unwrap() {
            ServingState::Stale(r) => Some(r.clone()),
            _ => None,
        };
        if let Some(reason) = stale_reason {
            let resp = Response::Error(DaemonError::StoreStale { reason });
            write_response(&mut writer, request_id, resp).await?;
            return Ok(None);
        }
        if starting {
            match &hello_env.payload {
                Request::Hello { .. } => {}
                Request::Status => {
                    let resp = Response::Status(state.status());
                    log_request(request_id, req_type, "ok", None);
                    write_response(&mut writer, request_id, resp).await?;
                    // Status without Hello does not classify; stay provisional and exit.
                    return Ok(None);
                }
                _ => {
                    let resp = Response::Error(DaemonError::Starting);
                    log_request(request_id, req_type, "starting", None);
                    write_response(&mut writer, request_id, resp).await?;
                    return Ok(None);
                }
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

    let hello_ok = match handle_hello(&hello_env, &model_id, chunks) {
        Ok((ok, resp)) => {
            log_request(request_id, "Hello", "ok", None);
            write_response(&mut writer, request_id, *resp).await?;
            ok
        }
        Err(resp) => {
            log_request(request_id, req_type, "error", None);
            write_response(&mut writer, request_id, *resp).await?;
            return Ok(None);
        }
    };

    let role = hello_ok.role;
    let connection_id = state.alloc_connection_id();
    if role == ClientRole::Worker {
        *state.connected_clients.lock() += 1;
        state.touch();
    }

    loop {
        if *state.shutting_down.lock() {
            break;
        }
        let env: Envelope<Request> =
            match read_envelope_until(&mut reader, &state, Duration::from_secs(3600)).await {
                Ok(Some(e)) => e,
                Ok(None) => break,
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

        if role == ClientRole::Worker {
            state.touch();
        }

        let request_id = env.request_id;
        let req_type = request_type_name(&env.payload);

        if role == ClientRole::Observer {
            match &env.payload {
                Request::Status | Request::Observe { .. } => {}
                other => {
                    let operation = request_type_name(other).to_owned();
                    let err = DaemonError::ObserverForbidden { operation };
                    log_request(request_id, req_type, "forbidden", None);
                    write_response(&mut writer, request_id, Response::Error(err)).await?;
                    continue;
                }
            }
        }

        let outcome = dispatch_request(
            &env.payload,
            &state,
            &mut writer,
            request_id,
            connection_id,
            embedder.as_ref(),
        )
        .await;
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
    Ok(Some(role))
}

/// Read one envelope, aborting early on connection shutdown or timeout.
async fn read_envelope_until(
    reader: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    state: &SharedState,
    timeout: Duration,
) -> Result<Option<Envelope<Request>>, DaemonError> {
    tokio::select! {
        biased;
        _ = state.connection_shutdown.notified() => Ok(None),
        _ = tokio::time::sleep(timeout) => Err(DaemonError::Malformed {
            detail: "handshake or read timeout".into(),
        }),
        result = read_envelope(reader) => result.map(Some),
    }
}

async fn dispatch_request(
    req: &Request,
    state: &Arc<SharedState>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    request_id: u64,
    connection_id: u64,
    embedder: &dyn Embedder,
) -> Result<Option<String>, DaemonError> {
    match req {
        Request::Hello { .. } => Err(DaemonError::Malformed {
            detail: "duplicate Hello".into(),
        }),
        Request::Status => {
            state.refresh_resources(embedder);
            write_response(writer, request_id, Response::Status(state.status())).await?;
            Ok(None)
        }
        Request::Shutdown => {
            write_response(writer, request_id, Response::Status(state.status())).await?;
            Ok(None)
        }
        Request::Search { query, top_k } => {
            let started = std::time::Instant::now();
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
                    ServingState::Starting => {
                        let err = DaemonError::Starting;
                        state.record_terminal(
                            crate::observe::TerminalEventDraft {
                                connection_id,
                                request_id,
                                operation: "Search",
                                outcome: "error",
                                error_code: Some(crate::observe::safe_error_code(&err)),
                                result_count: None,
                                stage_millis: None,
                            },
                            started.elapsed(),
                        );
                        return Err(err);
                    }
                    ServingState::Stale(r) => {
                        let err = DaemonError::StoreStale { reason: r.clone() };
                        state.record_terminal(
                            crate::observe::TerminalEventDraft {
                                connection_id,
                                request_id,
                                operation: "Search",
                                outcome: "error",
                                error_code: Some(crate::observe::safe_error_code(&err)),
                                result_count: None,
                                stage_millis: None,
                            },
                            started.elapsed(),
                        );
                        return Err(err);
                    }
                    ServingState::Ready(r) => Target::Ready(Arc::clone(&r.search)),
                    ServingState::Indexing(f) => Target::Frozen(Arc::clone(f)),
                }
            };
            let result = match target {
                Target::Frozen(f) => tokio::task::spawn_blocking(move || {
                    if let Some(d) = delay {
                        std::thread::sleep(d);
                    }
                    f.search(&query, top_k, &fusion)
                })
                .await
                .map_err(|e| DaemonError::Internal {
                    detail: format!("join: {e}"),
                })?,
                Target::Ready(ready) => tokio::task::spawn_blocking(move || {
                    if let Some(d) = delay {
                        std::thread::sleep(d);
                    }
                    ready.search(&query, top_k, &fusion)
                })
                .await
                .map_err(|e| DaemonError::Internal {
                    detail: format!("join: {e}"),
                })?,
            };
            match result {
                Ok(search) => {
                    let stage = Some(format!("{:?}", search.diagnostics.stage_millis));
                    let count = search.results.len() as u64;
                    let timings = search.diagnostics.stage_millis.clone();
                    state.record_terminal(
                        crate::observe::TerminalEventDraft {
                            connection_id,
                            request_id,
                            operation: "Search",
                            outcome: "ok",
                            error_code: None,
                            result_count: Some(count),
                            stage_millis: Some(timings),
                        },
                        started.elapsed(),
                    );
                    write_response(writer, request_id, Response::Search(search)).await?;
                    Ok(stage)
                }
                Err(e) => {
                    state.record_terminal(
                        crate::observe::TerminalEventDraft {
                            connection_id,
                            request_id,
                            operation: "Search",
                            outcome: "error",
                            error_code: Some(crate::observe::safe_error_code(&e)),
                            result_count: None,
                            stage_millis: None,
                        },
                        started.elapsed(),
                    );
                    Err(e)
                }
            }
        }
        Request::SearchSimilar { code, top_k } => {
            let started = std::time::Instant::now();
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
                    ServingState::Starting => {
                        let err = DaemonError::Starting;
                        state.record_terminal(
                            crate::observe::TerminalEventDraft {
                                connection_id,
                                request_id,
                                operation: "SearchSimilar",
                                outcome: "error",
                                error_code: Some(crate::observe::safe_error_code(&err)),
                                result_count: None,
                                stage_millis: None,
                            },
                            started.elapsed(),
                        );
                        return Err(err);
                    }
                    ServingState::Stale(r) => {
                        let err = DaemonError::StoreStale { reason: r.clone() };
                        state.record_terminal(
                            crate::observe::TerminalEventDraft {
                                connection_id,
                                request_id,
                                operation: "SearchSimilar",
                                outcome: "error",
                                error_code: Some(crate::observe::safe_error_code(&err)),
                                result_count: None,
                                stage_millis: None,
                            },
                            started.elapsed(),
                        );
                        return Err(err);
                    }
                    ServingState::Ready(r) => Target::Ready(Arc::clone(&r.search)),
                    ServingState::Indexing(f) => Target::Frozen(Arc::clone(f)),
                }
            };
            let result = match target {
                Target::Frozen(f) => tokio::task::spawn_blocking(move || {
                    if let Some(d) = delay {
                        std::thread::sleep(d);
                    }
                    f.search_similar(&code, top_k, &fusion)
                })
                .await
                .map_err(|e| DaemonError::Internal {
                    detail: format!("join: {e}"),
                })?,
                Target::Ready(ready) => tokio::task::spawn_blocking(move || {
                    if let Some(d) = delay {
                        std::thread::sleep(d);
                    }
                    ready.search_similar(&code, top_k, &fusion)
                })
                .await
                .map_err(|e| DaemonError::Internal {
                    detail: format!("join: {e}"),
                })?,
            };
            match result {
                Ok(search) => {
                    let stage = Some(format!("{:?}", search.diagnostics.stage_millis));
                    let count = search.results.len() as u64;
                    let timings = search.diagnostics.stage_millis.clone();
                    state.record_terminal(
                        crate::observe::TerminalEventDraft {
                            connection_id,
                            request_id,
                            operation: "SearchSimilar",
                            outcome: "ok",
                            error_code: None,
                            result_count: Some(count),
                            stage_millis: Some(timings),
                        },
                        started.elapsed(),
                    );
                    write_response(writer, request_id, Response::Search(search)).await?;
                    Ok(stage)
                }
                Err(e) => {
                    state.record_terminal(
                        crate::observe::TerminalEventDraft {
                            connection_id,
                            request_id,
                            operation: "SearchSimilar",
                            outcome: "error",
                            error_code: Some(crate::observe::safe_error_code(&e)),
                            result_count: None,
                            stage_millis: None,
                        },
                        started.elapsed(),
                    );
                    Err(e)
                }
            }
        }
        Request::GetSymbol { file, symbol } => {
            let started = std::time::Instant::now();
            let file = file.clone();
            let symbol = symbol.clone();
            enum Target {
                Ready(Arc<crate::resident::FrozenSearch>),
                Frozen(Arc<crate::resident::FrozenSearch>),
            }
            let target = {
                let guard = state.serving.read().unwrap();
                match &*guard {
                    ServingState::Starting => {
                        let err = DaemonError::Starting;
                        state.record_terminal(
                            crate::observe::TerminalEventDraft {
                                connection_id,
                                request_id,
                                operation: "GetSymbol",
                                outcome: "error",
                                error_code: Some(crate::observe::safe_error_code(&err)),
                                result_count: None,
                                stage_millis: None,
                            },
                            started.elapsed(),
                        );
                        return Err(err);
                    }
                    ServingState::Stale(r) => {
                        let err = DaemonError::StoreStale { reason: r.clone() };
                        state.record_terminal(
                            crate::observe::TerminalEventDraft {
                                connection_id,
                                request_id,
                                operation: "GetSymbol",
                                outcome: "error",
                                error_code: Some(crate::observe::safe_error_code(&err)),
                                result_count: None,
                                stage_millis: None,
                            },
                            started.elapsed(),
                        );
                        return Err(err);
                    }
                    ServingState::Ready(r) => Target::Ready(Arc::clone(&r.search)),
                    ServingState::Indexing(f) => Target::Frozen(Arc::clone(f)),
                }
            };
            let result = match target {
                Target::Frozen(f) => f.get_symbol(&file, &symbol),
                Target::Ready(ready) => {
                    tokio::task::spawn_blocking(move || ready.get_symbol(&file, &symbol))
                        .await
                        .map_err(|e| DaemonError::Internal {
                            detail: format!("join: {e}"),
                        })?
                }
            };
            match result {
                Ok(resp) => {
                    state.record_terminal(
                        crate::observe::TerminalEventDraft {
                            connection_id,
                            request_id,
                            operation: "GetSymbol",
                            outcome: "ok",
                            error_code: None,
                            result_count: Some(1),
                            stage_millis: None,
                        },
                        started.elapsed(),
                    );
                    write_response(writer, request_id, resp).await?;
                    Ok(None)
                }
                Err(e) => {
                    state.record_terminal(
                        crate::observe::TerminalEventDraft {
                            connection_id,
                            request_id,
                            operation: "GetSymbol",
                            outcome: "error",
                            error_code: Some(crate::observe::safe_error_code(&e)),
                            result_count: None,
                            stage_millis: None,
                        },
                        started.elapsed(),
                    );
                    Err(e)
                }
            }
        }
        Request::Index { mode, repo_dir } => {
            let started = std::time::Instant::now();
            if state
                .indexing
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                let err = DaemonError::IndexInProgress;
                state.record_terminal(
                    crate::observe::TerminalEventDraft {
                        connection_id,
                        request_id,
                        operation: "Index",
                        outcome: "error",
                        error_code: Some(crate::observe::safe_error_code(&err)),
                        result_count: None,
                        stage_millis: None,
                    },
                    started.elapsed(),
                );
                return Err(err);
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
                let old_snapshot = Arc::clone(&frozen);
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
                        match rebuild_resident(
                            store,
                            lexical,
                            Arc::clone(&embedder),
                            repo_dir.clone(),
                        ) {
                            Ok(resident) => match resident.into_ready() {
                                Ok(ready) => {
                                    *state_c.serving.write().unwrap() =
                                        ServingState::Ready(Arc::new(ready));
                                    clear_indexing();
                                    let _ = done_tx.send(Ok(report));
                                }
                                Err(e) => {
                                    let fallback = restore_after_failed_refresh(
                                        Arc::clone(&old_snapshot),
                                        repo_dir.clone(),
                                        Arc::clone(&embedder),
                                    )
                                    .map(Arc::new)
                                    .map(ServingState::Ready)
                                    .unwrap_or_else(|_| ServingState::Indexing(old_snapshot));
                                    *state_c.serving.write().unwrap() = fallback;
                                    clear_indexing();
                                    let _ = done_tx.send(Err(e));
                                }
                            },
                            Err(e) => {
                                let fallback = restore_after_failed_refresh(
                                    Arc::clone(&old_snapshot),
                                    repo_dir.clone(),
                                    Arc::clone(&embedder),
                                )
                                .map(Arc::new)
                                .map(ServingState::Ready)
                                .unwrap_or_else(|_| ServingState::Indexing(old_snapshot));
                                *state_c.serving.write().unwrap() = fallback;
                                clear_indexing();
                                let _ = done_tx.send(Err(e));
                            }
                        }
                    }
                    Err(e) => {
                        let fallback = restore_after_failed_refresh(
                            Arc::clone(&old_snapshot),
                            repo_dir.clone(),
                            Arc::clone(&embedder),
                        )
                        .map(Arc::new)
                        .map(ServingState::Ready)
                        .unwrap_or(ServingState::Indexing(old_snapshot));
                        *state_c.serving.write().unwrap() = fallback;
                        clear_indexing();
                        let _ = done_tx.send(Err(e));
                    }
                }
            });

            let final_res = loop {
                tokio::select! {
                    Some((phase, done, total)) = progress_rx.recv() => {
                        *state.current_progress.lock() = Some(crate::protocol::IndexProgressSnapshot {
                            phase: crate::protocol::IndexPhase::from(phase),
                            done,
                            total,
                            connection_id,
                            request_id,
                        });
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
            *state.current_progress.lock() = None;
            match final_res {
                Ok(report) => {
                    let wire = crate::protocol::IndexReportWire::from(&report);
                    *state.last_index.lock() = Some(crate::protocol::LastIndexCompletion {
                        completed_at_unix_ms: SharedState::observed_at_unix_ms(),
                        outcome: "ok".into(),
                        error_code: None,
                        connection_id,
                        request_id,
                        report: Some(wire),
                    });
                    state.record_terminal(
                        crate::observe::TerminalEventDraft {
                            connection_id,
                            request_id,
                            operation: "Index",
                            outcome: "ok",
                            error_code: None,
                            result_count: Some(report.chunks_added),
                            stage_millis: None,
                        },
                        started.elapsed(),
                    );
                    write_response(writer, request_id, index_report_response(&report)).await?;
                    Ok(None)
                }
                Err(e) => {
                    *state.last_index.lock() = Some(crate::protocol::LastIndexCompletion {
                        completed_at_unix_ms: SharedState::observed_at_unix_ms(),
                        outcome: "error".into(),
                        error_code: Some(crate::observe::safe_error_code(&e)),
                        connection_id,
                        request_id,
                        report: None,
                    });
                    state.record_terminal(
                        crate::observe::TerminalEventDraft {
                            connection_id,
                            request_id,
                            operation: "Index",
                            outcome: "error",
                            error_code: Some(crate::observe::safe_error_code(&e)),
                            result_count: None,
                            stage_millis: None,
                        },
                        started.elapsed(),
                    );
                    Err(e)
                }
            }
        }
        Request::Observe { after } => {
            state.refresh_resources(embedder);
            let status = state.status();
            let mut obs = {
                let ring = state.event_ring.lock();
                crate::observe::build_observation(status, &ring, after.as_ref())
            };
            while encode(&Envelope {
                request_id,
                payload: Response::Observation(obs.clone()),
            })
            .map(|b| b.len() > crate::protocol::MAX_REQUEST_BYTES + 4)
            .unwrap_or(true)
                && !obs.events.is_empty()
            {
                obs.events.pop();
                obs.more = true;
                if let Some(last) = obs.events.last() {
                    obs.next_cursor = last.cursor.clone();
                }
            }
            write_response(writer, request_id, Response::Observation(obs)).await?;
            Ok(None)
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
        Request::Observe { .. } => "Observe",
    }
}

fn request_log_level(req_type: &str, outcome: &str) -> Level {
    match outcome {
        "ok" if matches!(req_type, "Hello" | "Observe" | "Status") => Level::DEBUG,
        "ok" => Level::INFO,
        "starting" | "forbidden" => Level::WARN,
        _ => Level::ERROR,
    }
}

fn log_request(request_id: u64, req_type: &str, outcome: &str, stage: Option<String>) {
    match request_log_level(req_type, outcome) {
        Level::DEBUG => debug!(
            request_id,
            req_type,
            outcome,
            stage = stage.as_deref().unwrap_or(""),
            "daemon request"
        ),
        Level::INFO => info!(
            request_id,
            req_type,
            outcome,
            stage = stage.as_deref().unwrap_or(""),
            "daemon request"
        ),
        Level::WARN => warn!(
            request_id,
            req_type,
            outcome,
            stage = stage.as_deref().unwrap_or(""),
            "daemon request"
        ),
        _ => error!(
            request_id,
            req_type,
            outcome,
            stage = stage.as_deref().unwrap_or(""),
            "daemon request"
        ),
    }
}

#[cfg(test)]
mod request_logging_tests {
    use super::*;

    #[test]
    fn routine_successes_are_debug_and_operations_are_info() {
        assert_eq!(request_log_level("Hello", "ok"), Level::DEBUG);
        assert_eq!(request_log_level("Observe", "ok"), Level::DEBUG);
        assert_eq!(request_log_level("Status", "ok"), Level::DEBUG);
        assert_eq!(request_log_level("Search", "ok"), Level::INFO);
        assert_eq!(request_log_level("Index", "ok"), Level::INFO);
    }

    #[test]
    fn recoverable_conditions_warn_and_failures_error() {
        assert_eq!(request_log_level("Search", "starting"), Level::WARN);
        assert_eq!(request_log_level("Search", "forbidden"), Level::WARN);
        assert_eq!(request_log_level("Search", "error"), Level::ERROR);
    }
}
