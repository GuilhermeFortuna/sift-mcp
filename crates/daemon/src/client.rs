//! Client that connects to a resident daemon, spawning one if needed.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use futures::stream::{self, Stream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::process::Command;

use crate::codec::encode;
use crate::paths::socket_path_for_store;
use crate::protocol::{DaemonError, Envelope, PROTOCOL_VERSION, Request, Response};

pub struct DaemonClient {
    stream: UnixStream,
    next_id: u64,
    #[allow(dead_code)]
    socket_path: PathBuf,
}

impl DaemonClient {
    pub async fn connect(socket_path: &Path) -> Result<Self, DaemonError> {
        let stream = UnixStream::connect(socket_path)
            .await
            .map_err(|e| DaemonError::Internal {
                detail: format!("connect {}: {e}", socket_path.display()),
            })?;
        let mut client = Self {
            stream,
            next_id: 1,
            socket_path: socket_path.to_path_buf(),
        };
        let resp = client
            .request(Request::Hello {
                protocol_version: PROTOCOL_VERSION,
                client: "daemon-client".into(),
            })
            .await?;
        match resp {
            Response::Hello { .. } => Ok(client),
            Response::Error(e) => Err(e),
            other => Err(DaemonError::Malformed {
                detail: format!("unexpected hello response: {other:?}"),
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
        let mut spawned = false;
        loop {
            if UnixStream::connect(&socket).await.is_ok() {
                return Self::connect(&socket).await;
            }
            if !spawned {
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
                    .stderr(Stdio::null());
                cmd.spawn().map_err(|e| DaemonError::Internal {
                    detail: format!("spawn daemon: {e}"),
                })?;
                spawned = true;
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
