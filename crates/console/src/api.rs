pub mod types;

use crate::{
    ConsoleConfig,
    api::types::*,
    assets::Assets,
    db::Database,
    freshness::FreshnessCache,
    jobs::Jobs,
    registry::{Registration, RegistrationInput},
};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{Request, StatusCode},
    middleware,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use serde::Deserialize;
use std::{sync::Arc, time::Duration};
use tokio::sync::broadcast;

pub const LISTEN_HOST: &str = "127.0.0.1:7331";

#[derive(Clone)]
struct AppState {
    db: Database,
    jobs: Jobs,
    freshness: FreshnessCache,
    assets: Assets,
    csrf: String,
    events: broadcast::Sender<String>,
}

pub fn router() -> Router {
    let security = crate::security::Security::new(LISTEN_HOST.into());
    let token = security.csrf.clone();
    Router::new()
        .route("/api/v1/session", get(move || session(token.clone())))
        .route("/api/v1/health", get(health))
        .layer(middleware::from_fn_with_state(
            security,
            crate::security::guard,
        ))
}

pub async fn application(config: ConsoleConfig) -> Result<Router, Box<dyn std::error::Error>> {
    if !config.listen.ip().is_loopback() {
        return Err("console listen address must be loopback".into());
    }
    let assets = Assets::open(&config.asset_path)?;
    let db = Database::open(&config.database_path, crate::now_ms()).await?;
    let (event_tx, _) = broadcast::channel(128);
    let jobs = Jobs::new(db.clone(), event_tx.clone())
        .await
        .map_err(|e| std::io::Error::other(e.message))?;
    let security = crate::security::Security::new(config.listen.to_string());
    let state = Arc::new(AppState {
        db: db.clone(),
        jobs,
        freshness: FreshnessCache::default(),
        assets,
        csrf: security.csrf.clone(),
        events: event_tx.clone(),
    });
    start_collector(Arc::downgrade(&state));
    let api = Router::new()
        .route("/session", get(get_session))
        .route("/health", get(health))
        .route(
            "/repositories",
            get(list_repositories).post(create_repository),
        )
        .route(
            "/repositories/{id}",
            get(get_repository)
                .patch(replace_repository)
                .delete(remove_repository),
        )
        .route("/repositories/{id}/start", post(start_repository))
        .route("/repositories/{id}/index", post(index_repository))
        .route("/repositories/{id}/jobs", get(list_jobs))
        .route("/repositories/{id}/status", get(repository_status))
        .route("/repositories/{id}/freshness", get(freshness))
        .route("/repositories/{id}/search", post(search))
        .route("/repositories/{id}/similar", post(similar))
        .route("/repositories/{id}/symbol", post(symbol))
        .route("/jobs/{id}", get(get_job))
        .route("/activity", get(activity))
        .route("/metrics", get(get_metrics))
        .route("/events", get(events));
    Ok(Router::new()
        .nest("/api/v1", api)
        .fallback(serve_asset)
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            security,
            crate::security::guard,
        )))
}

async fn session(token: String) -> Json<serde_json::Value> {
    Json(serde_json::json!({"csrf_token":token}))
}
async fn get_session(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    session(s.csrf.clone()).await
}
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status":"ok"}))
}
async fn list_repositories(
    State(s): State<Arc<AppState>>,
) -> Result<Json<Vec<Registration>>, ApiError> {
    Ok(Json(s.db.list().await?))
}
async fn create_repository(
    State(s): State<Arc<AppState>>,
    Json(input): Json<RegistrationInput>,
) -> Result<impl IntoResponse, ApiError> {
    Ok((StatusCode::CREATED, Json(s.db.register(input).await?)))
}
async fn get_repository(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Registration>, ApiError> {
    Ok(Json(s.db.get(&id).await?))
}
async fn replace_repository(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<RegistrationInput>,
) -> Result<Json<Registration>, ApiError> {
    ensure_idle(&s, &id).await?;
    let r = s.db.replace(&id, input).await?;
    s.freshness.forget(&id).await;
    Ok(Json(r))
}
async fn remove_repository(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    ensure_idle(&s, &id).await?;
    s.db.remove(&id).await?;
    s.jobs.forget(&id).await;
    s.freshness.forget(&id).await;
    Ok(StatusCode::NO_CONTENT)
}
async fn ensure_idle(s: &AppState, id: &str) -> Result<(), ApiError> {
    if s.jobs.running(id).await {
        Err(ApiError::new(
            "index_in_progress",
            "The registration cannot change while indexing is running.",
            true,
        ))
    } else {
        Ok(())
    }
}
async fn start_repository(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<daemon::DaemonStatus>, ApiError> {
    let r = s.db.get(&id).await?;
    let mut c = crate::jobs::connect(&r).await?;
    match c.request(daemon::Request::Status).await? {
        daemon::Response::Status(v) => Ok(Json(v)),
        _ => Err(unexpected()),
    }
}
async fn index_repository(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<IndexInput>,
) -> Result<impl IntoResponse, ApiError> {
    let r = s.db.get(&id).await?;
    let mode = match input.mode {
        IndexMode::Update => daemon::IndexMode::Update,
        IndexMode::Full => daemon::IndexMode::Full,
    };
    Ok((StatusCode::ACCEPTED, Json(s.jobs.launch(r, mode).await?)))
}
async fn list_jobs(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<IndexJob>>, ApiError> {
    s.db.get(&id).await?;
    Ok(Json(s.jobs.list(&id).await))
}
async fn get_job(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<IndexJob>, ApiError> {
    Ok(Json(s.jobs.get(&id).await?))
}
async fn freshness(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<crate::freshness::Freshness>, ApiError> {
    let r = s.db.get(&id).await?;
    Ok(Json(
        s.freshness.inspect(id, r.config.repo_path, None).await,
    ))
}
async fn repository_status(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let r = s.db.get(&id).await?;
    let socket = daemon::paths::socket_path_for_store(&r.config.store_path)?;
    if !socket.exists() {
        return Ok(Json(
            serde_json::json!({"status":null,"collected_at_unix_ms":crate::now_ms(),"stale":false,"connection_state":"stopped","error_code":null}),
        ));
    }
    let cursor = s.db.cursor(&id).await?;
    let mut client = daemon::DaemonClient::connect_observer(&socket).await?;
    match tokio::time::timeout(Duration::from_secs(2), client.observe(cursor)).await {
        Ok(Ok(o)) => {
            s.db.ingest(&id, o.clone(), crate::now_ms()).await?;
            Ok(Json(
                serde_json::json!({"status":o.status,"collected_at_unix_ms":crate::now_ms(),"stale":false,"connection_state":"connected","error_code":null}),
            ))
        }
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Err(ApiError::timeout()),
    }
}

#[derive(Deserialize)]
struct HistoryQuery {
    repository_id: Option<String>,
    from: Option<u64>,
    to: Option<u64>,
    limit: Option<usize>,
    cursor: Option<usize>,
}
fn window(q: &HistoryQuery) -> Result<(u64, u64), ApiError> {
    let now = crate::now_ms();
    let to = q.to.unwrap_or(now);
    let from = q.from.unwrap_or_else(|| to.saturating_sub(3_600_000));
    if from > to || to.saturating_sub(from) > crate::history::RETENTION_MILLIS {
        return Err(ApiError::invalid(
            "History windows must be ordered and no longer than seven days.",
        ));
    }
    Ok((from, to))
}
async fn activity(
    State(s): State<Arc<AppState>>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<ActivityPage>, ApiError> {
    let (from, to) = window(&q)?;
    if let Some(id) = &q.repository_id {
        s.db.get(id).await?;
    }
    let all = s.db.events(from, to, q.repository_id).await?;
    let offset = q.cursor.unwrap_or(0);
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let items = all.iter().skip(offset).take(limit).cloned().collect();
    let next_cursor = (offset + limit < all.len()).then(|| (offset + limit).to_string());
    Ok(Json(Page { items, next_cursor }))
}
async fn get_metrics(
    State(s): State<Arc<AppState>>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<crate::history::MetricsResponse>, ApiError> {
    let (from, to) = window(&q)?;
    if let Some(id) = &q.repository_id {
        s.db.get(id).await?;
    }
    Ok(Json(
        crate::history::metrics(&s.db, from, to, q.repository_id).await?,
    ))
}
async fn events(
    State(s): State<Arc<AppState>>,
) -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = s.events.subscribe();
    Sse::new(futures::stream::unfold(
        (rx, true),
        |(mut rx, initial)| async move {
            if initial {
                return Some((Ok(Event::default().event("reset").data("{}")), (rx, false)));
            }
            let event = match rx.recv().await {
                Ok(kind) => Event::default().event(kind).data("{}"),
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    Event::default().event("reset").data("{}")
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            };
            Some((Ok(event), (rx, false)))
        },
    ))
}

fn start_collector(state: std::sync::Weak<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let Some(state) = state.upgrade() else { break };
            let db = state.db.clone();
            let events = state.events.clone();
            let Ok(registrations) = db.list().await else {
                let _ = events.send("health".into());
                continue;
            };
            for r in registrations {
                let Ok(socket) = daemon::paths::socket_path_for_store(&r.config.store_path) else {
                    continue;
                };
                if !socket.exists() {
                    continue;
                }
                let mut cursor = db.cursor(&r.id).await.ok().flatten();
                for _ in 0..4 {
                    let observed = async {
                        let mut c = daemon::DaemonClient::connect_observer(&socket).await?;
                        c.observe(cursor.clone()).await
                    };
                    match tokio::time::timeout(Duration::from_secs(2), observed).await {
                        Ok(Ok(o)) => {
                            let more = o.more;
                            cursor = Some(o.next_cursor.clone());
                            if db.ingest(&r.id, o, crate::now_ms()).await.is_err() {
                                let _ = events.send("health".into());
                                break;
                            }
                            let _ = events.send("status".into());
                            let _ = events.send("activity".into());
                            if !more {
                                break;
                            }
                        }
                        _ => {
                            let now = crate::now_ms();
                            let _ = db
                                .gap(&r.id, now.saturating_sub(2_000), now, "collection_outage")
                                .await;
                            let _ = events.send("status".into());
                            break;
                        }
                    }
                }
            }
            drop(state);
        }
    });
}
async fn search(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(v): Json<SearchInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    search_action(
        &s,
        id,
        daemon::Request::Search {
            query: v.query,
            top_k: v.top_k,
        },
    )
    .await
}
async fn similar(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(v): Json<SimilarInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    search_action(
        &s,
        id,
        daemon::Request::SearchSimilar {
            code: v.code,
            top_k: v.top_k,
        },
    )
    .await
}
async fn search_action(
    s: &AppState,
    id: String,
    request: daemon::Request,
) -> Result<Json<serde_json::Value>, ApiError> {
    let r = s.db.get(&id).await?;
    let mut c = crate::jobs::connect(&r).await?;
    match tokio::time::timeout(Duration::from_secs(60), c.request(request))
        .await
        .map_err(|_| ApiError::timeout())??
    {
        daemon::Response::Search(v) => Ok(Json(serde_json::to_value(v).map_err(|_| unexpected())?)),
        _ => Err(unexpected()),
    }
}
async fn symbol(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(v): Json<SymbolInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let r = s.db.get(&id).await?;
    let socket = daemon::paths::socket_path_for_store(&r.config.store_path)?;
    let mut c = daemon::DaemonClient::connect_socket(&socket).await?;
    match tokio::time::timeout(
        Duration::from_secs(60),
        c.request(daemon::Request::GetSymbol {
            file: v.file,
            symbol: v.symbol,
        }),
    )
    .await
    .map_err(|_| ApiError::timeout())??
    {
        daemon::Response::Symbol {
            file,
            symbol,
            language,
            signature,
            lines,
            body,
        } => Ok(Json(
            serde_json::json!({"file":file,"symbol":symbol,"language":language,"signature":signature,"lines":lines,"body":body}),
        )),
        _ => Err(unexpected()),
    }
}
fn unexpected() -> ApiError {
    ApiError::new(
        "connection_lost",
        "The daemon returned an unexpected response.",
        true,
    )
}
async fn serve_asset(State(s): State<Arc<AppState>>, request: Request<Body>) -> Response {
    s.assets.serve(request).await
}
