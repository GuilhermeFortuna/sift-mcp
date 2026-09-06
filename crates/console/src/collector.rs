use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use daemon::{DaemonClient, DaemonError, DaemonStatus, EventCursor, Observation};

struct ObserverSession {
    socket: PathBuf,
    client: DaemonClient,
}

#[derive(Default)]
pub struct ObserverPool {
    sessions: BTreeMap<String, ObserverSession>,
}

impl ObserverPool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn remove(&mut self, repository_id: &str) {
        self.sessions.remove(repository_id);
    }

    pub async fn observe(
        &mut self,
        repository_id: &str,
        socket: &Path,
        after: Option<EventCursor>,
    ) -> Result<Observation, DaemonError> {
        let needs_connection = self
            .sessions
            .get(repository_id)
            .is_none_or(|session| session.socket != socket);
        if needs_connection {
            let client = DaemonClient::connect_observer(socket).await?;
            self.sessions.insert(
                repository_id.into(),
                ObserverSession {
                    socket: socket.to_path_buf(),
                    client,
                },
            );
        }

        let result = self
            .sessions
            .get_mut(repository_id)
            .expect("observer session inserted above")
            .client
            .observe(after)
            .await;
        if result.is_err() {
            self.remove(repository_id);
        }
        result
    }
}

#[derive(Debug, Clone)]
pub struct CollectedStatus {
    pub status: DaemonStatus,
    pub collected_at_unix_ms: u64,
    pub stale: bool,
    pub error_code: Option<String>,
}
#[derive(Default)]
pub struct Collector {
    cursors: BTreeMap<String, EventCursor>,
    statuses: BTreeMap<String, CollectedStatus>,
}
impl Collector {
    pub fn new() -> Self {
        Self::default()
    }
    pub async fn observe_path(
        &mut self,
        repository_id: &str,
        socket: &Path,
        now_unix_ms: u64,
    ) -> Result<CollectedStatus, DaemonError> {
        let mut client = DaemonClient::connect_observer(socket).await?;
        let observation = client
            .observe(self.cursors.get(repository_id).cloned())
            .await;
        self.apply(repository_id, observation, now_unix_ms)
    }
    pub fn apply(
        &mut self,
        repository_id: &str,
        observation: Result<Observation, DaemonError>,
        now_unix_ms: u64,
    ) -> Result<CollectedStatus, DaemonError> {
        match observation {
            Ok(observation) => {
                self.cursors
                    .insert(repository_id.into(), observation.next_cursor);
                let status = CollectedStatus {
                    status: observation.status,
                    collected_at_unix_ms: now_unix_ms,
                    stale: false,
                    error_code: None,
                };
                self.statuses.insert(repository_id.into(), status.clone());
                Ok(status)
            }
            Err(error) => {
                if let Some(previous) = self.statuses.get_mut(repository_id) {
                    previous.stale = true;
                    previous.error_code = Some(error_code(&error));
                    return Ok(previous.clone());
                }
                Err(error)
            }
        }
    }
    pub fn status(&self, repository_id: &str) -> Option<&CollectedStatus> {
        self.statuses.get(repository_id)
    }
}
fn error_code(error: &DaemonError) -> String {
    match error {
        DaemonError::ProtocolVersion { .. } => "protocol_incompatible",
        DaemonError::Starting => "starting",
        DaemonError::StoreStale { .. } => "stale",
        _ => "unreachable",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemon::{Lifecycle, ResourceSnapshot};
    fn observation() -> Observation {
        Observation {
            status: DaemonStatus {
                lifecycle: Lifecycle::Ready,
                instance_id: "a".into(),
                observed_at_unix_ms: 10,
                model_id: None,
                chunks_live: None,
                chunks_dead: None,
                indexed_commit: None,
                idle_seconds: 0,
                uptime_seconds: 0,
                current_progress: None,
                last_index: None,
                resources: ResourceSnapshot::unavailable(10),
            },
            events: vec![],
            next_cursor: EventCursor {
                instance_id: "a".into(),
                sequence: 1,
            },
            gap: false,
            more: false,
        }
    }
    #[test]
    fn failed_observation_keeps_the_last_state_and_marks_it_stale() {
        let mut collector = Collector::new();
        collector.apply("repo", Ok(observation()), 10).unwrap();
        let stale = collector
            .apply("repo", Err(DaemonError::Starting), 12)
            .unwrap();
        assert!(stale.stale);
        assert_eq!(stale.error_code.as_deref(), Some("starting"));
        assert_eq!(stale.status.lifecycle, Lifecycle::Ready);
    }
}
