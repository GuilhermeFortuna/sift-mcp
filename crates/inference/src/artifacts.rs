//! Load-time artifact verification (CPU-safe, no ORT).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::embedder::InferError;
use crate::metadata::ModelMetadata;

const ONNX_NAME: &str = "model.onnx";
const ONNX_DATA_NAME: &str = "model.onnx.data";
const METADATA_NAME: &str = "metadata.json";
const TOKENIZER_NAME: &str = "tokenizer.json";

/// Tokenizer filenames that may participate in the SIFT-004 hash
/// (sorted by name when hashing).
const TOKENIZER_HASH_CANDIDATES: &[&str] = &[
    "tokenizer.json",
    "tokenizer_config.json",
    "special_tokens_map.json",
    "vocab.txt",
    "vocab.json",
    "merges.txt",
    "added_tokens.json",
    "tokenizer.model",
];

/// Verify model directory files and metadata hashes. Does not load ORT.
pub fn verify_model_dir(model_dir: &Path) -> Result<ModelMetadata, InferError> {
    let meta_path = model_dir.join(METADATA_NAME);
    if !meta_path.is_file() {
        return Err(InferError::ModelFilesMissing { path: meta_path });
    }
    let meta_json =
        fs::read_to_string(&meta_path).map_err(|e| InferError::Runtime(e.to_string()))?;
    let meta = ModelMetadata::from_json(&meta_json)?;

    let onnx_path = model_dir.join(ONNX_NAME);
    if !onnx_path.is_file() {
        return Err(InferError::ModelFilesMissing { path: onnx_path });
    }

    let tokenizer_path = model_dir.join(TOKENIZER_NAME);
    if !tokenizer_path.is_file() {
        return Err(InferError::ModelFilesMissing {
            path: tokenizer_path,
        });
    }

    let mut onnx_parts = vec![onnx_path.clone()];
    let onnx_data = model_dir.join(ONNX_DATA_NAME);
    if onnx_data.is_file() {
        onnx_parts.push(onnx_data);
    }
    let onnx_hash = sha256_paths(&onnx_parts)?;
    if onnx_hash != meta.onnx_sha256 {
        return Err(InferError::ArtifactHashMismatch {
            expected: meta.onnx_sha256.clone(),
            got: onnx_hash,
        });
    }

    let tok_paths: Vec<PathBuf> = TOKENIZER_HASH_CANDIDATES
        .iter()
        .map(|name| model_dir.join(name))
        .filter(|p| p.is_file())
        .collect();
    let tok_hash = sha256_paths(&tok_paths)?;
    if tok_hash != meta.tokenizer_sha256 {
        return Err(InferError::ArtifactHashMismatch {
            expected: meta.tokenizer_sha256.clone(),
            got: tok_hash,
        });
    }

    Ok(meta)
}

fn sha256_paths(paths: &[PathBuf]) -> Result<String, InferError> {
    let mut sorted = paths.to_vec();
    sorted.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .cmp(b.file_name().unwrap_or_default())
    });
    let mut hasher = Sha256::new();
    for path in &sorted {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| InferError::Runtime(format!("bad path {}", path.display())))?;
        hasher.update(name.as_bytes());
        hasher.update([0u8]);
        let mut file = fs::File::open(path).map_err(|e| InferError::Runtime(e.to_string()))?;
        let mut buf = [0u8; 1024 * 1024];
        loop {
            let n = file
                .read(&mut buf)
                .map_err(|e| InferError::Runtime(e.to_string()))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Construct an allocation failure for tests / callers that detect OOM.
pub fn allocation_error(requested_bytes: u64) -> InferError {
    InferError::Allocation { requested_bytes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_minimal_metadata(dir: &Path, onnx_hash: &str, tok_hash: &str) {
        let json = format!(
            r#"{{
                "model_id": "test@abc",
                "dims": 8,
                "max_sequence_length": 16,
                "pooling": "mean",
                "normalize": "l2",
                "query_prefix": null,
                "document_prefix": null,
                "onnx_sha256": "{onnx_hash}",
                "tokenizer_sha256": "{tok_hash}"
            }}"#
        );
        fs::write(dir.join(METADATA_NAME), json).unwrap();
    }

    #[test]
    fn missing_graph_returns_model_files_missing() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_metadata(dir.path(), "aa", "bb");
        fs::write(dir.path().join(TOKENIZER_NAME), b"{}").unwrap();
        // no model.onnx
        let err = verify_model_dir(dir.path()).expect_err("missing onnx");
        match err {
            InferError::ModelFilesMissing { path } => {
                assert!(path.ends_with(ONNX_NAME), "path={path:?}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn hash_mismatch_returns_artifact_hash_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(ONNX_NAME), b"onnx-bytes").unwrap();
        fs::write(dir.path().join(TOKENIZER_NAME), b"tok-bytes").unwrap();
        let real_onnx = sha256_paths(&[dir.path().join(ONNX_NAME)]).unwrap();
        let real_tok = sha256_paths(&[dir.path().join(TOKENIZER_NAME)]).unwrap();
        write_minimal_metadata(dir.path(), &real_onnx, "deadbeef");
        let _ = real_tok;
        let err = verify_model_dir(dir.path()).expect_err("hash mismatch");
        match err {
            InferError::ArtifactHashMismatch { expected, got } => {
                assert_eq!(expected, "deadbeef");
                assert_ne!(got, expected);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn allocation_error_is_distinguishable() {
        let err = allocation_error(1024);
        match err {
            InferError::Allocation { requested_bytes } => assert_eq!(requested_bytes, 1024),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn model_files_missing_gpu_and_allocation_are_distinct() {
        use std::mem::discriminant;
        let a = InferError::ModelFilesMissing {
            path: PathBuf::from("/missing"),
        };
        let b = InferError::GpuUnavailable {
            detail: "no cuda".into(),
        };
        let c = InferError::Allocation { requested_bytes: 1 };
        assert_ne!(discriminant(&a), discriminant(&b));
        assert_ne!(discriminant(&b), discriminant(&c));
        assert_ne!(discriminant(&a), discriminant(&c));
    }

    #[test]
    fn live_primary_model_dir_verifies_when_present() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models/primary");
        if !dir.join("model.onnx").is_file() {
            return;
        }
        verify_model_dir(&dir).expect("primary model dir should verify");
    }
}
