use crate::{
    api::types::{ApiError, IndexJob, JobState},
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
                    if state == "interrupted" {
                        j.state = JobState::Interrupted;
                        j.error_code = Some("console_restarted".into());
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
        self.items
            .read()
            .await
            .values()
            .any(|j| j.repository_id == id && j.state == JobState::Running)
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
        if items
            .values()
            .any(|j| j.repository_id == registration.id && j.state == JobState::Running)
        {
            return Err(daemon::DaemonError::IndexInProgress.into());
        }
        let job = IndexJob {
            id: uuid::Uuid::new_v4().to_string(),
            repository_id: registration.id.clone(),
            state: JobState::Running,
            progress: None,
            done: 0,
            total: None,
            report: None,
            error_code: None,
        };
        self.persist(&job).await?;
        items.insert(job.id.clone(), job.clone());
        drop(items);
        let this = self.clone();
        let id = job.id.clone();
        let (accepted, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            this.run(registration, mode, id, accepted).await;
        });
        rx.await.unwrap_or_else(|_| {
            Err(ApiError::new(
                "connection_lost",
                "The job connection was interrupted.",
                true,
            ))
        })
    }
    async fn run(
        &self,
        r: Registration,
        mode: daemon::IndexMode,
        id: String,
        accepted: tokio::sync::oneshot::Sender<Result<IndexJob, ApiError>>,
    ) {
        let mut accepted = Some(accepted);
        let mut job = self.get(&id).await.unwrap();
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
                        job.done = done;
                        job.total = total;
                        self.items.write().await.insert(id.clone(), job.clone());
                        let _ = self.events.send("indexing".into());
                        if let Some(tx) = accepted.take() {
                            let _ = tx.send(Ok(job.clone()));
                        }
                    }
                    daemon::Response::IndexDone(report) => {
                        job.state = JobState::Succeeded;
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
            job.error_code = Some(e.code.clone());
        }
        if self.persist(&job).await.is_err() {
            let _ = self.events.send("health".into());
        }
        self.items.write().await.insert(id, job.clone());
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
            self.items
                .write()
                .await
                .retain(|id, j| j.state == JobState::Running || ids.contains(id));
        }
        let _ = self.events.send("indexing".into());
        if let Some(tx) = accepted.take() {
            let _ = tx.send(result.map(|_| job));
        }
    }
    async fn persist(&self, job: &IndexJob) -> Result<(), ApiError> {
        let job = job.clone();
        let now = crate::now_ms();
        self.db.call(move|c|{let payload=serde_json::to_string(&job).map_err(|_|crate::db::DbError::Metadata)?;let state=serde_json::to_value(&job.state).unwrap().as_str().unwrap().to_owned();let tx=c.transaction()?;
 tx.execute("INSERT INTO jobs(id,repository_id,state,payload,updated_at_unix_ms) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET state=excluded.state,payload=excluded.payload,updated_at_unix_ms=excluded.updated_at_unix_ms",rusqlite::params![job.id,job.repository_id,state,payload,now])?;
 tx.execute("DELETE FROM jobs WHERE state<>'running' AND updated_at_unix_ms<?1",[now.saturating_sub(crate::history::RETENTION_MILLIS)])?;
 tx.execute("DELETE FROM jobs WHERE id IN(SELECT id FROM(SELECT id,ROW_NUMBER() OVER(PARTITION BY repository_id ORDER BY updated_at_unix_ms DESC,id DESC) n FROM jobs WHERE state<>'running') WHERE n>100)",[])?;tx.commit()?;Ok(())}).await.map_err(ApiError::from)
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
