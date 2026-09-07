use crate::{
    api::types::{ApiError, IndexJob, JobState, OperationEvent, OperationPhase, OperationType},
    db::Database,
    registry::Registration,
};
use futures::StreamExt;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::{RwLock, broadcast};
#[derive(Clone)]
pub struct Jobs {
    db: Database,
    items: Arc<RwLock<BTreeMap<String, IndexJob>>>,
    events: broadcast::Sender<String>,
}
impl Jobs {
    pub async fn new(db: Database, events: broadcast::Sender<String>) -> Result<Self, ApiError> {
        let items = db
            .call(|c| {
                let mut s =
                    c.prepare("SELECT payload,state FROM jobs ORDER BY updated_at_unix_ms")?;
                let rows =
                    s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                let mut items = BTreeMap::new();
                for row in rows {
                    let (payload, state) = row?;
                    let mut j: IndexJob =
                        serde_json::from_str(&payload).map_err(|_| crate::db::DbError::Metadata)?;
                    if matches!(state.as_str(), "running" | "queued" | "interrupted") {
                        j.state = JobState::Interrupted;
                        j.error_code = Some("console_restarted".into());
                        j.error_message =
                            Some("The console restarted before this operation completed.".into());
                        j.phase = OperationPhase::Interrupted;
                        j.completed_at_unix_ms = Some(crate::now_ms());
                    }
                    items.insert(j.id.clone(), j);
                }
                Ok(items)
            })
            .await?;
        Ok(Self {
            db,
            items: Arc::new(RwLock::new(items)),
            events,
        })
    }
    pub async fn get(&self, id: &str) -> Result<IndexJob, ApiError> {
        self.items
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(ApiError::missing)
    }
    pub async fn list(&self, id: &str) -> Vec<IndexJob> {
        self.items
            .read()
            .await
            .values()
            .filter(|j| j.repository_id == id)
            .cloned()
            .collect()
    }
    pub async fn running(&self, id: &str) -> bool {
        self.items.read().await.values().any(|j| {
            j.repository_id == id && matches!(j.state, JobState::Queued | JobState::Running)
        })
    }
    pub async fn forget(&self, id: &str) {
        self.items
            .write()
            .await
            .retain(|_, j| j.repository_id != id);
    }
    pub async fn launch(
        &self,
        registration: Registration,
        mode: daemon::IndexMode,
    ) -> Result<IndexJob, ApiError> {
        // Keep the check and reservation together. Persisting while this write
        // guard is held prevents another launch from passing the check before
        // this job becomes visible to readers.
        let mut items = self.items.write().await;
        if items.values().any(|j| {
            j.repository_id == registration.id
                && matches!(j.state, JobState::Queued | JobState::Running)
        }) {
            return Err(daemon::DaemonError::IndexInProgress.into());
        }
        let now = crate::now_ms();
        let operation_type = match &mode {
            daemon::IndexMode::Update => OperationType::UpdateIndex,
            daemon::IndexMode::Full => OperationType::FullRebuild,
        };
        let job = IndexJob {
            id: uuid::Uuid::new_v4().to_string(),
            repository_id: registration.id.clone(),
            state: JobState::Queued,
            operation_type,
            phase: OperationPhase::Queued,
            progress: None,
            done: 0,
            total: None,
            report: None,
            error_code: None,
            error_message: None,
            daemon_instance_id: None,
            started_at_unix_ms: now,
            updated_at_unix_ms: now,
            completed_at_unix_ms: None,
            events: Vec::new(),
        };
        self.persist(&job).await?;
        items.insert(job.id.clone(), job.clone());
        drop(items);
        let this = self.clone();
        let id = job.id.clone();
        tokio::spawn(async move {
            this.run(registration, mode, id).await;
        });
        let _ = self.events.send(event_payload("operation", &job));
        Ok(job)
    }
    pub async fn launch_start(&self, registration: Registration) -> Result<IndexJob, ApiError> {
        let mut items = self.items.write().await;
        if items.values().any(|j| {
            j.repository_id == registration.id
                && matches!(j.state, JobState::Queued | JobState::Running)
        }) {
            return Err(daemon::DaemonError::Starting.into());
        }
        let now = crate::now_ms();
        let job = IndexJob {
            id: uuid::Uuid::new_v4().to_string(),
            repository_id: registration.id.clone(),
            state: JobState::Queued,
            operation_type: OperationType::StartDaemon,
            phase: OperationPhase::Queued,
            progress: None,
            done: 0,
            total: None,
            report: None,
            error_code: None,
            error_message: None,
            daemon_instance_id: None,
            started_at_unix_ms: now,
            updated_at_unix_ms: now,
            completed_at_unix_ms: None,
            events: Vec::new(),
        };
        self.persist(&job).await?;
        items.insert(job.id.clone(), job.clone());
        drop(items);
        let this = self.clone();
        let operation_id = job.id.clone();
        tokio::spawn(async move {
            this.run_start(registration, operation_id).await;
        });
        let _ = self.events.send(event_payload("operation", &job));
        Ok(job)
    }
    async fn run(&self, r: Registration, mode: daemon::IndexMode, id: String) {
        let mut job = self.get(&id).await.unwrap();
        job.state = JobState::Running;
        job.phase = OperationPhase::Connecting;
        let _ = self.update(&mut job).await;
        let result: Result<(), ApiError> = async {
            let mut client = connect(&r).await?;
            let stream = client
                .request_streaming(daemon::Request::Index {
                    mode,
                    repo_dir: r.config.repo_path,
                })
                .await
                .map_err(ApiError::from)?;
            futures::pin_mut!(stream);
            while let Some(response) = stream.next().await {
                match response {
                    daemon::Response::IndexProgress { phase, done, total } => {
                        job.progress = Some(phase);
                        job.phase = match phase {
                            daemon::IndexPhase::Walking => OperationPhase::Walking,
                            daemon::IndexPhase::Parsing => OperationPhase::Parsing,
                            daemon::IndexPhase::Embedding => OperationPhase::Embedding,
                            daemon::IndexPhase::Storing => OperationPhase::Storing,
                            daemon::IndexPhase::Compacting => OperationPhase::Compacting,
                        };
                        job.done = done;
                        job.total = total;
                        self.update(&mut job).await?;
                    }
                    daemon::Response::IndexDone(report) => {
                        job.state = JobState::Succeeded;
                        job.phase = OperationPhase::Completed;
                        job.report = Some(report);
                        return Ok(());
                    }
                    daemon::Response::Error(e) => return Err(e.into()),
                    _ => {
                        return Err(ApiError::new(
                            "connection_lost",
                            "Unexpected indexing response; the operation may still be running.",
                            true,
                        ));
                    }
                }
            }
            Err(ApiError::new(
                "connection_lost",
                "Indexing disconnected before completion; it may still be running.",
                true,
            ))
        }
        .await;
        if let Err(ref e) = result {
            job.state = if matches!(e.code.as_str(), "connection_lost" | "timeout") {
                JobState::Interrupted
            } else {
                JobState::Failed
            };
            job.phase = if job.state == JobState::Interrupted {
                OperationPhase::Interrupted
            } else {
                OperationPhase::Failed
            };
            job.error_code = Some(e.code.clone());
            job.error_message = Some(e.message.clone());
        }
        if result.is_ok() {
            job.completed_at_unix_ms = Some(crate::now_ms());
        }
        if self.update(&mut job).await.is_err() {
            let _ = self.events.send("health".into());
        }
        // Keep memory bounded by the same persisted terminal history limits.
        if let Ok(ids) = self
            .db
            .call(|c| {
                let mut s = c.prepare("SELECT id FROM jobs")?;
                Ok(s.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .await
        {
            self.items.write().await.retain(|id, j| {
                matches!(j.state, JobState::Queued | JobState::Running) || ids.contains(id)
            });
        }
        let _ = self.events.send(event_payload("operation", &job));
    }
    async fn run_start(&self, r: Registration, id: String) {
        let mut job = match self.get(&id).await {
            Ok(j) => j,
            Err(_) => return,
        };
        job.state = JobState::Running;
        job.phase = OperationPhase::SpawningDaemon;
        let _ = self.update(&mut job).await;
        let result: Result<(), ApiError> = async {
            job.phase = OperationPhase::WaitingForSocket;
            self.update(&mut job).await?;
            let mut client = connect(&r).await?;
            job.phase = OperationPhase::LoadingModel;
            self.update(&mut job).await?;
            loop {
                match client.request(daemon::Request::Status).await? {
                    daemon::Response::Status(status) => {
                        job.daemon_instance_id = Some(status.instance_id.clone());
                        if matches!(
                            status.lifecycle,
                            daemon::Lifecycle::Ready | daemon::Lifecycle::Indexing
                        ) {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    _ => {
                        return Err(ApiError::new(
                            "connection_lost",
                            "The daemon returned an unexpected startup response.",
                            true,
                        ));
                    }
                }
            }
            job.phase = OperationPhase::Ready;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                job.state = JobState::Succeeded;
                job.phase = OperationPhase::Completed;
                job.completed_at_unix_ms = Some(crate::now_ms());
            }
            Err(e) => {
                job.state = JobState::Failed;
                job.phase = OperationPhase::Failed;
                job.error_code = Some(e.code);
                job.error_message = Some(e.message);
                job.completed_at_unix_ms = Some(crate::now_ms());
            }
        }
        let _ = self.update(&mut job).await;
    }
    async fn update(&self, job: &mut IndexJob) -> Result<(), ApiError> {
        job.updated_at_unix_ms = crate::now_ms();
        job.events.push(OperationEvent {
            at_unix_ms: job.updated_at_unix_ms,
            state: job.state.clone(),
            phase: job.phase.clone(),
            message: event_message(job),
        });
        self.persist(job).await?;
        self.items.write().await.insert(job.id.clone(), job.clone());
        let _ = self.events.send(event_payload("operation", job));
        Ok(())
    }
    async fn persist(&self, job: &IndexJob) -> Result<(), ApiError> {
        let job = job.clone();
        let now = crate::now_ms();
        self.db.call(move|c|{let payload=serde_json::to_string(&job).map_err(|_|crate::db::DbError::Metadata)?;let state=serde_json::to_value(&job.state).unwrap().as_str().unwrap().to_owned();let tx=c.transaction()?;
 tx.execute("INSERT INTO jobs(id,repository_id,state,payload,updated_at_unix_ms) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET state=excluded.state,payload=excluded.payload,updated_at_unix_ms=excluded.updated_at_unix_ms",rusqlite::params![job.id,job.repository_id,state,payload,now])?;
        tx.execute("DELETE FROM jobs WHERE state NOT IN ('running','queued') AND updated_at_unix_ms<?1",[now.saturating_sub(crate::history::RETENTION_MILLIS)])?;
 tx.execute("DELETE FROM jobs WHERE id IN(SELECT id FROM(SELECT id,ROW_NUMBER() OVER(PARTITION BY repository_id ORDER BY updated_at_unix_ms DESC,id DESC) n FROM jobs WHERE state NOT IN ('running','queued')) WHERE n>100)",[])?;tx.commit()?;Ok(())}).await.map_err(ApiError::from)
    }
}
fn event_payload(kind: &str, job: &IndexJob) -> String {
    serde_json::json!({"kind":kind,"repository_id":job.repository_id,"operation_id":job.id,"state":job.state,"phase":job.phase,"updated_at_unix_ms":job.updated_at_unix_ms}).to_string()
}
fn event_message(job: &IndexJob) -> String {
    match &job.phase {
        OperationPhase::SpawningDaemon => "Starting daemon".into(),
        OperationPhase::WaitingForSocket => "Waiting for Unix socket".into(),
        OperationPhase::Connecting => "Connecting to daemon".into(),
        OperationPhase::LoadingModel => "Loading model".into(),
        OperationPhase::Walking => "Walking repository".into(),
        OperationPhase::Parsing => "Parsing repository".into(),
        OperationPhase::Embedding => "Embedding chunks".into(),
        OperationPhase::Storing => "Storing index".into(),
        OperationPhase::Compacting => "Compacting index".into(),
        OperationPhase::Ready => "Daemon ready".into(),
        OperationPhase::Completed => "Operation complete".into(),
        OperationPhase::Failed => job
            .error_message
            .clone()
            .unwrap_or_else(|| "Operation failed".into()),
        OperationPhase::Interrupted => "Operation interrupted".into(),
        OperationPhase::Queued => "Operation queued".into(),
        OperationPhase::InitializingCuda => "Initializing CUDA".into(),
        OperationPhase::OpeningStore => "Opening store".into(),
    }
}
pub async fn connect(r: &Registration) -> Result<daemon::DaemonClient, ApiError> {
    tokio::time::timeout(
        Duration::from_secs(120),
        daemon::DaemonClient::connect_or_spawn(
            &r.config.store_path,
            &r.config.repo_path,
            &r.config.model_path,
            Duration::from_secs(120),
            &r.config.daemon_path,
        ),
    )
    .await
    .map_err(|_| ApiError::timeout())?
    .map_err(ApiError::from)
}
