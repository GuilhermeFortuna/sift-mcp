//! Per-connection handshake and request dispatch helpers.

use crate::protocol::{
    DaemonError, DaemonStatus, Envelope, Request, Response, PROTOCOL_VERSION,
};

/// Result of attempting to accept a Hello from a client.
#[derive(Debug, Clone, PartialEq)]
pub struct HelloOk {
    pub client: String,
}

/// Validate and answer a Hello. Any other first request is refused.
pub fn handle_hello(
    envelope: &Envelope<Request>,
    model_id: &str,
    chunks: u64,
) -> Result<(HelloOk, Box<Response>), Box<Response>> {
    match &envelope.payload {
        Request::Hello {
            protocol_version,
            client,
        } => {
            if *protocol_version != PROTOCOL_VERSION {
                return Err(Box::new(Response::Error(DaemonError::ProtocolVersion {
                    daemon: PROTOCOL_VERSION,
                    client: *protocol_version,
                })));
            }
            Ok((
                HelloOk {
                    client: client.clone(),
                },
                Box::new(Response::Hello {
                    protocol_version: PROTOCOL_VERSION,
                    model_id: model_id.to_owned(),
                    chunks,
                }),
            ))
        }
        _ => Err(Box::new(Response::Error(DaemonError::Malformed {
            detail: "Hello required before other requests".into(),
        }))),
    }
}

/// Build a Status response from a filled [`DaemonStatus`].
pub fn status_response(status: DaemonStatus) -> Response {
    Response::Status(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello_env(version: u32) -> Envelope<Request> {
        Envelope {
            request_id: 1,
            payload: Request::Hello {
                protocol_version: version,
                client: "test-client".into(),
            },
        }
    }

    #[test]
    fn matching_protocol_version_succeeds() {
        let (ok, resp) = handle_hello(&hello_env(PROTOCOL_VERSION), "mock", 42).unwrap();
        assert_eq!(ok.client, "test-client");
        match *resp {
            Response::Hello {
                protocol_version,
                model_id,
                chunks,
            } => {
                assert_eq!(protocol_version, PROTOCOL_VERSION);
                assert_eq!(model_id, "mock");
                assert_eq!(chunks, 42);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn mismatched_protocol_version_names_both() {
        let err = *handle_hello(&hello_env(PROTOCOL_VERSION + 1), "mock", 0).unwrap_err();
        match err {
            Response::Error(DaemonError::ProtocolVersion { daemon, client }) => {
                assert_eq!(daemon, PROTOCOL_VERSION);
                assert_eq!(client, PROTOCOL_VERSION + 1);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn request_before_hello_is_refused() {
        let env = Envelope {
            request_id: 1,
            payload: Request::Status,
        };
        let err = *handle_hello(&env, "mock", 0).unwrap_err();
        match err {
            Response::Error(DaemonError::Malformed { detail }) => {
                assert!(detail.contains("Hello"), "{detail}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
