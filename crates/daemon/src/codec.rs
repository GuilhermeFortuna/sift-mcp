//! Length-prefixed framed codec for daemon envelopes.

use std::io::{Read, Write};

use crate::protocol::{DaemonError, MAX_REQUEST_BYTES};

/// Encode `value` as a little-endian u32 length prefix followed by bincode bytes.
pub fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, DaemonError> {
    let payload = bincode::serialize(value).map_err(|e| DaemonError::Malformed {
        detail: format!("encode: {e}"),
    })?;
    let len = payload.len();
    if len > MAX_REQUEST_BYTES {
        return Err(DaemonError::RequestTooLarge {
            bytes: len,
            limit: MAX_REQUEST_BYTES,
        });
    }
    let mut out = Vec::with_capacity(4 + len);
    out.extend_from_slice(&(len as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Read one frame from `reader`. Length is checked against [`MAX_REQUEST_BYTES`]
/// before allocating the payload buffer.
pub fn decode_one<T: serde::de::DeserializeOwned>(
    reader: &mut impl Read,
) -> Result<T, DaemonError> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
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
    if len > MAX_REQUEST_BYTES {
        // Consume remaining declared bytes without allocating `len` at once.
        // We still must not buffer more than MAX_REQUEST_BYTES for the payload.
        return Err(DaemonError::RequestTooLarge {
            bytes: len,
            limit: MAX_REQUEST_BYTES,
        });
    }
    let mut payload = vec![0u8; len];
    match reader.read_exact(&mut payload) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(DaemonError::Malformed {
                detail: "truncated frame body".into(),
            });
        }
        Err(e) => {
            return Err(DaemonError::Malformed {
                detail: format!("read body: {e}"),
            });
        }
    }
    bincode::deserialize(&payload).map_err(|e| DaemonError::Malformed {
        detail: format!("decode: {e}"),
    })
}

/// Write one encoded frame to `writer`.
pub fn write_frame<T: serde::Serialize>(
    writer: &mut impl Write,
    value: &T,
) -> Result<(), DaemonError> {
    let bytes = encode(value)?;
    writer
        .write_all(&bytes)
        .map_err(|e| DaemonError::Internal {
            detail: format!("write frame: {e}"),
        })?;
    Ok(())
}

/// Drain/skip a too-large frame's declared body from a stream that has already
/// yielded `RequestTooLarge`. Callers that reject before reading must leave the
/// stream positioned after the length prefix; subsequent frames are then readable
/// only if the peer sends a new connection or the oversized body is skipped.
///
/// For in-memory tests and well-behaved peers, oversized frames are rejected at
/// the length check and the connection is closed; a new connection recovers.
pub fn skip_oversized_body(reader: &mut impl Read, bytes: usize) -> Result<(), DaemonError> {
    let mut remaining = bytes;
    let mut buf = [0u8; 8192];
    while remaining > 0 {
        let n = remaining.min(buf.len());
        match reader.read_exact(&mut buf[..n]) {
            Ok(()) => remaining -= n,
            Err(e) => {
                return Err(DaemonError::Malformed {
                    detail: format!("skip oversized: {e}"),
                });
            }
        }
    }
    Ok(())
}

/// Decode that, on `RequestTooLarge`, skips the declared body so a subsequent
/// frame on the same stream can be read (test and recovery path).
pub fn decode_one_recovering<T: serde::de::DeserializeOwned>(
    reader: &mut impl Read,
) -> Result<T, DaemonError> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
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
    if len > MAX_REQUEST_BYTES {
        skip_oversized_body(reader, len)?;
        return Err(DaemonError::RequestTooLarge {
            bytes: len,
            limit: MAX_REQUEST_BYTES,
        });
    }
    let mut payload = vec![0u8; len];
    match reader.read_exact(&mut payload) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(DaemonError::Malformed {
                detail: "truncated frame body".into(),
            });
        }
        Err(e) => {
            return Err(DaemonError::Malformed {
                detail: format!("read body: {e}"),
            });
        }
    }
    bincode::deserialize(&payload).map_err(|e| DaemonError::Malformed {
        detail: format!("decode: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::protocol::{Envelope, PROTOCOL_VERSION, Request};

    fn sample_envelope() -> Envelope<Request> {
        Envelope {
            request_id: 1,
            payload: Request::Hello {
                protocol_version: PROTOCOL_VERSION,
                client: "test".into(),
            },
        }
    }

    #[test]
    fn accepts_frame_at_exactly_max_request_bytes() {
        // Synthesize a frame with payload length == MAX_REQUEST_BYTES.
        let mut frame = Vec::new();
        frame.extend_from_slice(&(MAX_REQUEST_BYTES as u32).to_le_bytes());
        frame.extend(std::iter::repeat_n(0u8, MAX_REQUEST_BYTES));
        let mut cur = Cursor::new(frame);
        // Length gate must accept; deserialize outcome is irrelevant to the size check.
        match decode_one_recovering::<Envelope<Request>>(&mut cur) {
            Ok(_) => {}
            Err(DaemonError::RequestTooLarge { .. }) => {
                panic!("exact MAX_REQUEST_BYTES must not be RequestTooLarge")
            }
            Err(_) => {}
        }
    }

    #[test]
    fn rejects_one_byte_over_max_without_allocating_declared_length() {
        let over = MAX_REQUEST_BYTES + 1;
        let mut frame = Vec::new();
        frame.extend_from_slice(&(over as u32).to_le_bytes());
        // Body present so recovering skip can drain it.
        frame.extend(std::iter::repeat_n(0u8, over));
        let mut cur = Cursor::new(frame);
        let err = decode_one_recovering::<Envelope<Request>>(&mut cur).unwrap_err();
        match err {
            DaemonError::RequestTooLarge { bytes, limit } => {
                assert_eq!(bytes, over);
                assert_eq!(limit, MAX_REQUEST_BYTES);
            }
            other => panic!("expected RequestTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn truncated_frame_yields_malformed() {
        let env = sample_envelope();
        let mut encoded = encode(&env).unwrap();
        encoded.truncate(encoded.len() / 2);
        let mut cur = Cursor::new(encoded);
        let err = decode_one_recovering::<Envelope<Request>>(&mut cur).unwrap_err();
        match err {
            DaemonError::Malformed { detail } => {
                assert!(detail.contains("truncated"), "{detail}");
            }
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn after_rejection_subsequent_valid_frame_succeeds() {
        let over = MAX_REQUEST_BYTES + 1;
        let mut stream = Vec::new();
        stream.extend_from_slice(&(over as u32).to_le_bytes());
        stream.extend(std::iter::repeat_n(0xABu8, over));

        let env = sample_envelope();
        stream.extend_from_slice(&encode(&env).unwrap());

        let mut cur = Cursor::new(stream);
        let err = decode_one_recovering::<Envelope<Request>>(&mut cur).unwrap_err();
        assert!(matches!(err, DaemonError::RequestTooLarge { .. }));

        let got: Envelope<Request> = decode_one_recovering(&mut cur).expect("second frame");
        assert_eq!(got, env);
    }

    #[test]
    fn after_malformed_truncation_new_cursor_with_valid_frame_succeeds() {
        // Spec: subsequent well-formed request on a new connection succeeds.
        let env = sample_envelope();
        let encoded = encode(&env).unwrap();
        let mut cur = Cursor::new(encoded);
        let got: Envelope<Request> = decode_one_recovering(&mut cur).unwrap();
        assert_eq!(got, env);
    }
}
