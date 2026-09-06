#![allow(dead_code)]
use daemon::{
    DaemonStatus, Envelope, EventCursor, Lifecycle, Observation, Request, ResourceSnapshot,
    Response,
};
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
    sync::Notify,
};

pub fn observation(instance: &str, sequence: u64) -> Observation {
    Observation {
        status: DaemonStatus {
            lifecycle: Lifecycle::Ready,
            instance_id: instance.into(),
            observed_at_unix_ms: 100,
            model_id: Some("mock".into()),
            chunks_live: Some(2),
            chunks_dead: Some(0),
            indexed_commit: Some("abc".into()),
            idle_seconds: 0,
            uptime_seconds: 1,
            current_progress: None,
            last_index: None,
            resources: ResourceSnapshot::unavailable(100),
        },
        events: vec![],
        next_cursor: EventCursor {
            instance_id: instance.into(),
            sequence,
        },
        gap: false,
        more: false,
    }
}

pub struct MockDaemon {
    pub requests: Arc<Mutex<Vec<Request>>>,
    pub finish_index: Arc<Notify>,
    pub index_started: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
    socket: std::path::PathBuf,
}
impl Drop for MockDaemon {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.socket);
    }
}
impl MockDaemon {
    pub async fn bind(socket: std::path::PathBuf, reply: Response) -> Self {
        std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let finish_index = Arc::new(Notify::new());
        let index_started = Arc::new(Notify::new());
        let (r, f, s) = (
            requests.clone(),
            finish_index.clone(),
            index_started.clone(),
        );
        let task = tokio::spawn(async move {
            let mut children = tokio::task::JoinSet::new();
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (r, f, s, reply) = (r.clone(), f.clone(), s.clone(), reply.clone());
                children.spawn(async move {
                    while let Ok(len) = stream.read_u32_le().await {
                        if len as usize > daemon::MAX_REQUEST_BYTES {
                            break;
                        }
                        let mut bytes = vec![0; len as usize];
                        if stream.read_exact(&mut bytes).await.is_err() {
                            break;
                        }
                        let env: Envelope<Request> = bincode::deserialize(&bytes).unwrap();
                        r.lock().unwrap().push(env.payload.clone());
                        let response = match env.payload {
                            Request::Hello { .. } => {
                                if matches!(
                                    reply,
                                    Response::Error(daemon::DaemonError::ProtocolVersion { .. })
                                ) {
                                    reply.clone()
                                } else {
                                    Response::Hello {
                                        protocol_version: daemon::PROTOCOL_VERSION,
                                        model_id: "mock".into(),
                                        chunks: 2,
                                    }
                                }
                            }
                            Request::Observe { .. } => match &reply {
                                Response::Observation(_) => reply.clone(),
                                _ => Response::Observation(observation("mock", 0)),
                            },
                            Request::Status => Response::Status(observation("mock", 0).status),
                            Request::Index { .. } => {
                                s.notify_one();
                                let progress = daemon::codec::encode(&Envelope {
                                    request_id: env.request_id,
                                    payload: Response::IndexProgress {
                                        phase: daemon::IndexPhase::Parsing,
                                        done: 1,
                                        total: Some(2),
                                    },
                                })
                                .unwrap();
                                if stream.write_all(&progress).await.is_err() {
                                    break;
                                }
                                f.notified().await;
                                reply.clone()
                            }
                            _ => reply.clone(),
                        };
                        let bytes = daemon::codec::encode(&Envelope {
                            request_id: env.request_id,
                            payload: response,
                        })
                        .unwrap();
                        if stream.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        Self {
            requests,
            finish_index,
            index_started,
            task,
            socket,
        }
    }
}
