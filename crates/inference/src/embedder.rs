//! Core embedding types shared by mock and ONNX backends.

use std::path::PathBuf;

use half::f16;
use thiserror::Error;

/// Queries and documents differ only by prefix, applied from metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Query,
    Document,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Embedding {
    /// Pooled, L2-normalized, length == dims.
    pub vector: Vec<f16>,
    /// Input exceeded max_sequence_length.
    pub truncated: bool,
}

/// Live GPU/process resource sample. Unavailable fields stay `None`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourceUsage {
    pub device_id: Option<String>,
    pub device_used_bytes: Option<u64>,
    pub device_total_bytes: Option<u64>,
    pub process_used_bytes: Option<u64>,
    /// Model-attributable bytes; stays unavailable without allocator-level data.
    pub model_used_bytes: Option<u64>,
}

impl ResourceUsage {
    pub fn unavailable() -> Self {
        Self::default()
    }
}

#[derive(Debug, Error)]
pub enum InferError {
    #[error("model files missing at {path}")]
    ModelFilesMissing { path: PathBuf },
    #[error("GPU unavailable: {detail}")]
    GpuUnavailable { detail: String },
    #[error("allocation failed for {requested_bytes} bytes")]
    Allocation { requested_bytes: u64 },
    #[error("artifact hash mismatch: expected {expected}, got {got}")]
    ArtifactHashMismatch { expected: String, got: String },
    #[error("tokenizer error: {0}")]
    Tokenizer(String),
    #[error("runtime error: {0}")]
    Runtime(String),
    #[error("metadata error: {0}")]
    Metadata(#[from] crate::metadata::MetadataError),
}

/// The abstraction every consumer depends on.
pub trait Embedder: Send + Sync {
    fn model_id(&self) -> &str;
    fn dims(&self) -> u32;
    /// Splits internally at the configured batch limit. Output order matches
    /// input order. `role` selects the prefix convention from metadata.
    fn embed(&self, texts: &[&str], role: Role) -> Result<Vec<Embedding>, InferError>;

    /// Sample device/process resource usage. Default is all-unavailable.
    /// Must not fail the caller; return unavailable fields on error.
    fn resource_usage(&self) -> ResourceUsage {
        ResourceUsage::unavailable()
    }
}

/// Shared batch-splitting wrapper used by backends with a configured limit.
pub fn embed_with_batch_limit<F>(
    texts: &[&str],
    role: Role,
    max_batch: usize,
    mut embed_batch: F,
) -> Result<Vec<Embedding>, InferError>
where
    F: FnMut(&[&str], Role) -> Result<Vec<Embedding>, InferError>,
{
    assert!(max_batch > 0, "max_batch must be positive");
    let mut out = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(max_batch) {
        out.extend(embed_batch(chunk, role)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockEmbedder;

    #[test]
    fn mock_resource_usage_is_unavailable() {
        let m = MockEmbedder::new(8);
        let u = m.resource_usage();
        assert!(u.device_id.is_none());
        assert!(u.device_used_bytes.is_none());
        assert!(u.device_total_bytes.is_none());
        assert!(u.process_used_bytes.is_none());
        assert!(u.model_used_bytes.is_none());
    }

    #[test]
    fn measured_zero_stays_zero() {
        let u = ResourceUsage {
            device_id: Some("GPU-test".into()),
            device_used_bytes: Some(0),
            device_total_bytes: Some(1),
            process_used_bytes: Some(0),
            model_used_bytes: None,
        };
        assert_eq!(u.device_used_bytes, Some(0));
        assert_eq!(u.process_used_bytes, Some(0));
        assert!(u.model_used_bytes.is_none());
    }
}
