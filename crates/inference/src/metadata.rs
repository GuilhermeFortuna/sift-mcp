//! Model artifact metadata deserialized from `models/<key>/metadata.json`.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pooling {
    LastToken,
    Mean,
    Cls,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Normalize {
    L2,
    None,
}

/// Deserialized from models/<key>/metadata.json, written by SIFT-004.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelMetadata {
    pub model_id: String,
    pub dims: u32,
    pub max_sequence_length: usize,
    pub pooling: Pooling,
    pub normalize: Normalize,
    pub query_prefix: Option<String>,
    pub document_prefix: Option<String>,
    pub onnx_sha256: String,
    pub tokenizer_sha256: String,
}

impl ModelMetadata {
    pub fn from_json(json: &str) -> Result<Self, MetadataError> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|e| MetadataError::InvalidJson(e.to_string()))?;

        if let Some(pooling) = value.get("pooling") {
            match pooling.as_str() {
                Some("last_token") | Some("mean") | Some("cls") => {}
                Some(other) => return Err(MetadataError::UnknownPooling(other.to_string())),
                None => {
                    return Err(MetadataError::InvalidJson(
                        "pooling must be a string".into(),
                    ));
                }
            }
        }

        serde_json::from_value(value).map_err(|e| {
            let msg = e.to_string();
            if msg.contains("missing field") {
                MetadataError::MissingField("required field")
            } else {
                MetadataError::InvalidJson(msg)
            }
        })
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MetadataError {
    #[error("missing or invalid field: {0}")]
    MissingField(&'static str),
    #[error("unknown pooling strategy: {0}")]
    UnknownPooling(String),
    #[error("invalid metadata JSON: {0}")]
    InvalidJson(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join("sample-metadata.json")
    }

    #[test]
    fn sample_metadata_parses_every_field() {
        let json = std::fs::read_to_string(sample_path()).expect("read sample");
        let meta = ModelMetadata::from_json(&json).expect("parse sample");
        assert_eq!(meta.model_id, "test/model@abc123");
        assert_eq!(meta.dims, 1024);
        assert_eq!(meta.max_sequence_length, 512);
        assert_eq!(meta.pooling, Pooling::LastToken);
        assert_eq!(meta.normalize, Normalize::L2);
        assert_eq!(
            meta.query_prefix.as_deref(),
            Some("Instruct: query\nQuery: ")
        );
        assert_eq!(meta.document_prefix, None);
        assert_eq!(
            meta.onnx_sha256,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(
            meta.tokenizer_sha256,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
    }

    #[test]
    fn pooling_maps_last_token() {
        let json = r#"{
            "model_id": "m",
            "dims": 8,
            "max_sequence_length": 16,
            "pooling": "last_token",
            "normalize": "l2",
            "query_prefix": null,
            "document_prefix": null,
            "onnx_sha256": "aa",
            "tokenizer_sha256": "bb"
        }"#;
        let meta = ModelMetadata::from_json(json).expect("parse");
        assert_eq!(meta.pooling, Pooling::LastToken);
    }

    #[test]
    fn unknown_pooling_is_an_error() {
        let json = r#"{
            "model_id": "m",
            "dims": 8,
            "max_sequence_length": 16,
            "pooling": "max",
            "normalize": "l2",
            "query_prefix": null,
            "document_prefix": null,
            "onnx_sha256": "aa",
            "tokenizer_sha256": "bb"
        }"#;
        let err = ModelMetadata::from_json(json).expect_err("unknown pooling");
        match err {
            MetadataError::UnknownPooling(s) => assert_eq!(s, "max"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn missing_required_field_is_an_error() {
        let json = r#"{
            "model_id": "m",
            "dims": 8,
            "max_sequence_length": 16,
            "pooling": "mean",
            "normalize": "l2",
            "query_prefix": null,
            "document_prefix": null,
            "onnx_sha256": "aa"
        }"#;
        let err = ModelMetadata::from_json(json).expect_err("missing tokenizer_sha256");
        match err {
            MetadataError::MissingField(_) | MetadataError::InvalidJson(_) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
