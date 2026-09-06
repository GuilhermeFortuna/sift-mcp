//! Client that connects to a resident daemon, spawning one if needed.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use futures::stream::{self, Stream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::process::Command;

const MAX_STARTUP_STDERR_BYTES: usize = 64 * 1024;

use crate::codec::encode;
use crate::paths::socket_path_for_store;
use crate::protocol::{
    DaemonError, Envelope, EventCursor, OBSERVER_CLIENT, Observation, PROTOCOL_VERSION, Request,
    Response,
};

pub struct DaemonClient {
    stream: UnixStream,
    next_id: u64,
    #[allow(dead_code)]
    socket_path: PathBuf,
}

impl DaemonClient {
    pub async fn connect(socket_path: &Path) -> Result<Self, DaemonError> {
        Self::connect_with_client(socket_path, "daemon-client").await
    }

    /// Connect as a passive observer. Does not spawn a daemon.
    pub async fn connect_observer(socket_path: &Path) -> Result<Self, DaemonError> {
        Self::connect_with_client(socket_path, OBSERVER_CLIENT).await
    }

    async fn connect_with_client(socket_path: &Path, client: &str) -> Result<Self, DaemonError> {
        let stream = UnixStream::connect(socket_path)
            .await
            .map_err(|e| DaemonError::Internal {
                detail: format!("connect {}: {e}", socket_path.display()),
            })?;
        let mut client_conn = Self {
            stream,
            next_id: 1,
            socket_path: socket_path.to_path_buf(),
        };
        let resp = client_conn
            .request(Request::Hello {
                protocol_version: PROTOCOL_VERSION,
                client: client.into(),
            })
            .await?;
        match resp {
            Response::Hello { .. } => Ok(client_conn),
            Response::Error(e) => Err(e),
            other => Err(DaemonError::Malformed {
                detail: format!("unexpected hello response: {other:?}"),
            }),
        }
    }

    /// Poll diagnostics and recent request events.
    pub async fn observe(
        &mut self,
        after: Option<EventCursor>,
    ) -> Result<Observation, DaemonError> {
        match self.request(Request::Observe { after }).await? {
            Response::Observation(obs) => Ok(obs),
            other => Err(DaemonError::Malformed {
                detail: format!("unexpected observe response: {other:?}"),
            }),
        }
    }

    /// Connects, or spawns a daemon and retries with backoff until the deadline.
    pub async fn connect_or_spawn(
        store_dir: &Path,
        repo_dir: &Path,
        model_dir: &Path,
        deadline: Duration,
        binary: &Path,
    ) -> Result<Self, DaemonError> {
        let socket = socket_path_for_store(store_dir)?;
        let start = Instant::now();
        let mut child: Option<tokio::process::Child> = None;
        let mut stderr_task: Option<tokio::task::JoinHandle<Vec<u8>>> = None;
        loop {
            if UnixStream::connect(&socket).await.is_ok() {
                if let (Some(mut child), Some(stderr_task)) = (child.take(), stderr_task.take()) {
                    tokio::spawn(async move {
                        let _ = child.wait().await;
                        let _ = stderr_task.await;
                    });
                }
                return Self::connect(&socket).await;
            }
            if child.is_none() {
                let mut cmd = Command::new(binary);
                cmd.arg("--store")
                    .arg(store_dir)
                    .arg("--repo")
                    .arg(repo_dir)
                    .arg("--model")
                    .arg(model_dir)
                    .arg("--socket")
                    .arg(&socket)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped());
                let mut spawned = cmd.spawn().map_err(|e| DaemonError::Internal {
                    detail: format!("spawn daemon: {e}"),
                })?;
                let mut stderr = spawned.stderr.take().ok_or_else(|| DaemonError::Internal {
                    detail: "spawn daemon: stderr pipe unavailable".into(),
                })?;
                let reader_task = tokio::spawn(async move {
                    let mut captured = Vec::new();
                    let mut buffer = [0u8; 4096];
                    loop {
                        let read = match stderr.read(&mut buffer).await {
                            Ok(0) => break,
                            Ok(read) => read,
                            Err(_) => break,
                        };
                        if captured.len() < MAX_STARTUP_STDERR_BYTES {
                            let remaining = MAX_STARTUP_STDERR_BYTES - captured.len();
                            captured.extend_from_slice(&buffer[..read.min(remaining)]);
                        }
                    }
                    captured
                });
                child = Some(spawned);
                stderr_task = Some(reader_task);
            }
            if let Some(child) = child.as_mut()
                && let Some(status) = child.try_wait().map_err(|e| DaemonError::Internal {
                    detail: format!("wait for daemon: {e}"),
                })?
            {
                let stderr = match stderr_task.take() {
                    Some(task) => task
                        .await
                        .ok()
                        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
                        .filter(|text| !text.is_empty()),
                    None => None,
                };
                let diagnostic = stderr.map(|text| format!(": {text}")).unwrap_or_default();
                return Err(DaemonError::Internal {
                    detail: format!("daemon exited with {status}{diagnostic}"),
                });
            }
            if start.elapsed() >= deadline {
                return Err(DaemonError::Internal {
                    detail: "timed out waiting for daemon".into(),
                });
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn connect_socket(socket_path: &Path) -> Result<Self, DaemonError> {
        Self::connect(socket_path).await
    }

    pub async fn request(&mut self, req: Request) -> Result<Response, DaemonError> {
        let id = self.next_id;
        self.next_id += 1;
        let env = Envelope {
            request_id: id,
            payload: req,
        };
        let bytes = encode(&env)?;
        self.stream
            .write_all(&bytes)
            .await
            .map_err(|e| DaemonError::Internal {
                detail: format!("write: {e}"),
            })?;
        let resp_env: Envelope<Response> = read_envelope(&mut self.stream).await?;
        if resp_env.request_id != id {
            return Err(DaemonError::Malformed {
                detail: format!(
                    "response id mismatch: sent {id}, got {}",
                    resp_env.request_id
                ),
            });
        }
        match resp_env.payload {
            Response::Error(e) => Err(e),
            other => Ok(other),
        }
    }

    /// Yields IndexProgress frames as they arrive, followed by IndexDone or an
    /// Error response. The stream borrows this client until its terminal frame.
    pub async fn request_streaming(
        &mut self,
        req: Request,
    ) -> Result<impl Stream<Item = Response> + '_, DaemonError> {
        let id = self.next_id;
        self.next_id += 1;
        let env = Envelope {
            request_id: id,
            payload: req,
        };
        let bytes = encode(&env)?;
        self.stream
            .write_all(&bytes)
            .await
            .map_err(|e| DaemonError::Internal {
                detail: format!("write: {e}"),
            })?;

        Ok(stream::unfold(
            (self, id, false),
            |(client, id, done)| async move {
                if done {
                    return None;
                }
                let payload = match read_envelope(&mut client.stream).await {
                    Ok(response) if response.request_id == id => response.payload,
                    Ok(response) => Response::Error(DaemonError::Malformed {
                        detail: format!(
                            "streaming response id mismatch: expected {id}, got {}",
                            response.request_id
                        ),
                    }),
                    Err(error) => Response::Error(error),
                };
                let terminal = matches!(payload, Response::IndexDone(_) | Response::Error(_));
                Some((payload, (client, id, terminal)))
            },
        ))
    }
}

async fn read_envelope(stream: &mut UnixStream) -> Result<Envelope<Response>, DaemonError> {
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| DaemonError::Malformed {
            detail: format!("read length: {e}"),
        })?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > crate::protocol::MAX_REQUEST_BYTES {
        return Err(DaemonError::RequestTooLarge {
            bytes: len,
            limit: crate::protocol::MAX_REQUEST_BYTES,
        });
    }
    let mut payload = vec![0u8; len];
    stream
        .read_exact(&mut payload)
        .await
        .map_err(|e| DaemonError::Malformed {
            detail: format!("read body: {e}"),
        })?;
    bincode::deserialize(&payload).map_err(|e| DaemonError::Malformed {
        detail: format!("decode: {e}"),
    })
}
