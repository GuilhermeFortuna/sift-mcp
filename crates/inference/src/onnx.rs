//! ONNX Runtime embedder behind the `cuda` feature.

use std::path::Path;
use std::sync::Mutex;

use half::f16;
use ort::ep::CUDA;
use ort::session::Session;
use ort::value::Tensor;

use crate::artifacts::verify_model_dir;
use crate::embedder::{Embedder, Embedding, InferError, Role, embed_with_batch_limit};
use crate::metadata::{ModelMetadata, Normalize};
use crate::pooling::{l2_normalize_rows, pool};
use crate::tokenize::TextTokenizer;

pub struct OnnxEmbedder {
    session: Mutex<Session>,
    tokenizer: TextTokenizer,
    metadata: ModelMetadata,
    max_batch: usize,
    peak_gpu_bytes: Mutex<u64>,
}

impl OnnxEmbedder {
    pub fn load(model_dir: &Path, max_batch: usize) -> Result<Self, InferError> {
        let metadata = verify_model_dir(model_dir)?;
        let tokenizer = TextTokenizer::from_dir(model_dir, metadata.clone())?;
        let onnx_path = model_dir.join("model.onnx");

        let mut builder = Session::builder()
            .map_err(|e| InferError::Runtime(e.to_string()))?
            .with_execution_providers([CUDA::default().build()])
            .map_err(|e| InferError::GpuUnavailable {
                detail: e.to_string(),
            })?;
        let session = builder.commit_from_file(&onnx_path).map_err(|e| {
            let msg = e.to_string();
            if msg.to_lowercase().contains("cuda")
                || msg.to_lowercase().contains("gpu")
                || msg.to_lowercase().contains("provider")
            {
                InferError::GpuUnavailable { detail: msg }
            } else {
                InferError::Runtime(msg)
            }
        })?;

        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            metadata,
            max_batch: max_batch.max(1),
            peak_gpu_bytes: Mutex::new(0),
        })
    }

    /// Load with an intentionally invalid CUDA device to force provider failure.
    #[cfg(test)]
    pub fn load_with_bad_cuda_device(
        model_dir: &Path,
        max_batch: usize,
    ) -> Result<Self, InferError> {
        let metadata = verify_model_dir(model_dir)?;
        let tokenizer = TextTokenizer::from_dir(model_dir, metadata.clone())?;
        let onnx_path = model_dir.join("model.onnx");

        let mut builder = Session::builder()
            .map_err(|e| InferError::Runtime(e.to_string()))?
            .with_execution_providers([CUDA::default().with_device_id(99_999).build()])
            .map_err(|e| InferError::GpuUnavailable {
                detail: e.to_string(),
            })?;
        let session =
            builder
                .commit_from_file(&onnx_path)
                .map_err(|e| InferError::GpuUnavailable {
                    detail: e.to_string(),
                })?;

        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            metadata,
            max_batch: max_batch.max(1),
            peak_gpu_bytes: Mutex::new(0),
        })
    }

    pub fn peak_gpu_bytes(&self) -> u64 {
        *self
            .peak_gpu_bytes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_peak_gpu_bytes(&self, bytes: u64) {
        let mut guard = self
            .peak_gpu_bytes
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *guard = (*guard).max(bytes);
    }

    fn embed_batch(&self, texts: &[&str], role: Role) -> Result<Vec<Embedding>, InferError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let encoded = self.tokenizer.encode_batch_padded(texts, role)?;
        let batch = encoded.len();
        let seq = encoded[0].input_ids.len();
        let dims = self.metadata.dims as usize;

        let mut truncated = Vec::with_capacity(batch);
        let mut ids = Vec::with_capacity(batch * seq);
        let mut mask_i64 = Vec::with_capacity(batch * seq);
        for enc in &encoded {
            debug_assert_eq!(enc.input_ids.len(), seq);
            ids.extend(enc.input_ids.iter().map(|&x| x as i64));
            mask_i64.extend(enc.attention_mask.iter().map(|&x| x as i64));
            truncated.push(enc.truncated);
        }

        let ids_tensor = Tensor::from_array(([batch, seq], ids))
            .map_err(|e| InferError::Runtime(e.to_string()))?;
        let mask_tensor = Tensor::from_array(([batch, seq], mask_i64))
            .map_err(|e| InferError::Runtime(e.to_string()))?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| InferError::Runtime("session mutex poisoned".into()))?;

        let outputs = session
            .run(ort::inputs![
                "input_ids" => ids_tensor,
                "attention_mask" => mask_tensor
            ])
            .map_err(|e| {
                let msg = e.to_string().to_lowercase();
                if msg.contains("alloc") || msg.contains("memory") || msg.contains("oom") {
                    InferError::Allocation {
                        requested_bytes: (batch * seq * dims * 2) as u64,
                    }
                } else {
                    InferError::Runtime(e.to_string())
                }
            })?;

        let hidden_f16 = outputs["last_hidden_state"]
            .try_extract_array::<f16>()
            .map_err(|e| InferError::Runtime(e.to_string()))?;

        let shape = hidden_f16.shape();
        if shape.len() != 3 || shape[0] != batch || shape[2] != dims {
            return Err(InferError::Runtime(format!(
                "unexpected hidden shape {shape:?}, expected [{batch}, ?, {dims}]"
            )));
        }
        let out_seq = shape[1];
        if out_seq != seq {
            return Err(InferError::Runtime(format!(
                "sequence length mismatch: encoded {seq} vs hidden {out_seq}"
            )));
        }

        let mut hidden_f32 = Vec::with_capacity(batch * out_seq * dims);
        for v in hidden_f16.iter() {
            hidden_f32.push(v.to_f32());
        }

        let mask_u32: Vec<u32> = encoded
            .iter()
            .flat_map(|e| e.attention_mask.iter().copied())
            .collect();

        let mut pooled = pool(
            &hidden_f32,
            &mask_u32,
            batch,
            out_seq,
            dims,
            self.metadata.pooling.clone(),
        );

        if matches!(self.metadata.normalize, Normalize::L2) {
            l2_normalize_rows(&mut pooled, dims);
        }

        let mut out = Vec::with_capacity(batch);
        for (i, trunc) in truncated.into_iter().enumerate() {
            let row = &pooled[i * dims..(i + 1) * dims];
            out.push(Embedding {
                vector: row.iter().copied().map(f16::from_f32).collect(),
                truncated: trunc,
            });
        }
        Ok(out)
    }
}

impl Embedder for OnnxEmbedder {
    fn model_id(&self) -> &str {
        &self.metadata.model_id
    }

    fn dims(&self) -> u32 {
        self.metadata.dims
    }

    fn embed(&self, texts: &[&str], role: Role) -> Result<Vec<Embedding>, InferError> {
        embed_with_batch_limit(texts, role, self.max_batch, |chunk, role| {
            self.embed_batch(chunk, role)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::path::PathBuf;

    fn model_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models/primary")
    }

    fn cosine_distance(a: &[f16], b: &[f64]) -> f64 {
        let mut dot = 0.0f64;
        let mut na = 0.0f64;
        let mut nb = 0.0f64;
        for (x, y) in a.iter().zip(b.iter()) {
            let xf = x.to_f32() as f64;
            dot += xf * y;
            na += xf * xf;
            nb += y * y;
        }
        1.0 - dot / (na.sqrt() * nb.sqrt())
    }

    #[test]
    #[ignore = "requires CUDA hardware and ONNX Runtime"]
    fn gpu_unavailable_is_distinguishable() {
        let dir = model_dir();
        if !dir.join("model.onnx").is_file() {
            panic!("models/primary missing; export first");
        }
        let err = OnnxEmbedder::load_with_bad_cuda_device(&dir, 1)
            .err()
            .expect("bad device must fail");
        match err {
            InferError::GpuUnavailable { detail } => {
                assert!(!detail.is_empty());
            }
            other => panic!("expected GpuUnavailable, got {other:?}"),
        }
    }

    #[derive(Deserialize)]
    struct Tolerance {
        max: f64,
    }

    #[derive(Deserialize)]
    struct Case {
        name: String,
        text: String,
        role: String,
        vector: Vec<f64>,
        truncated: bool,
    }

    #[derive(Deserialize)]
    struct ReferenceFixture {
        tolerance: Tolerance,
        cases: Vec<Case>,
    }

    #[test]
    #[ignore = "requires CUDA hardware and ONNX Runtime"]
    fn fixture_parity() {
        let dir = model_dir();
        if !dir.join("model.onnx").is_file() {
            panic!("models/primary missing; export first");
        }
        let embedder = OnnxEmbedder::load(&dir, 8).expect("load OnnxEmbedder");
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("primary-reference.json");
        let fixture: ReferenceFixture =
            serde_json::from_str(&std::fs::read_to_string(fixture_path).unwrap()).unwrap();

        for case in &fixture.cases {
            let role = match case.role.as_str() {
                "query" => Role::Query,
                "document" => Role::Document,
                other => panic!("unknown role {other}"),
            };
            let out = embedder
                .embed(&[case.text.as_str()], role)
                .unwrap_or_else(|e| panic!("embed {}: {e}", case.name));
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].truncated, case.truncated, "truncation {}", case.name);
            let dist = cosine_distance(&out[0].vector, &case.vector);
            assert!(
                dist <= fixture.tolerance.max,
                "case {} cosine_distance {dist} exceeds {}",
                case.name,
                fixture.tolerance.max
            );
            eprintln!(
                "fixture_parity {}: cosine_distance={dist:.6e} (max={})",
                case.name, fixture.tolerance.max
            );
        }
    }
}
