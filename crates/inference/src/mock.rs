//! Deterministic CPU-only embedder for tests.

use half::f16;

use crate::embedder::{Embedder, Embedding, InferError, Role, embed_with_batch_limit};
use crate::pooling::l2_normalize_rows;

/// Deterministic, CPU-only, no model files. Same text always yields the same
/// vector; different texts yield different vectors.
pub struct MockEmbedder {
    dims: u32,
    model_id: String,
    max_batch: usize,
    seed: u64,
}

impl MockEmbedder {
    pub fn new(dims: u32) -> Self {
        Self {
            dims,
            model_id: "mock".to_string(),
            max_batch: usize::MAX,
            seed: 0xC0FFEE,
        }
    }

    pub fn with_batch_limit(mut self, max_batch: usize) -> Self {
        self.max_batch = max_batch.max(1);
        self
    }

    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = model_id.into();
        self
    }

    /// Returns a vector that is nearest to `text`'s own, for tests that need to
    /// construct a query with a known correct answer.
    pub fn query_matching(&self, text: &str) -> Vec<f16> {
        self.vector_for(text)
    }

    fn vector_for(&self, text: &str) -> Vec<f16> {
        let dims = self.dims as usize;
        let mut floats = vec![0.0f32; dims];
        let mut state = self.seed;
        for (i, b) in text.as_bytes().iter().enumerate() {
            state = state
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(*b as u64)
                .wrapping_add(i as u64);
            let idx = (state as usize) % dims;
            let bit = ((state >> 32) as f32) / (u32::MAX as f32) * 2.0 - 1.0;
            floats[idx] += bit;
        }
        // Ensure non-zero even for empty string.
        if floats.iter().all(|&v| v == 0.0) {
            floats[0] = 1.0;
        }
        l2_normalize_rows(&mut floats, dims);
        floats.into_iter().map(f16::from_f32).collect()
    }

    fn embed_batch(&self, texts: &[&str], _role: Role) -> Result<Vec<Embedding>, InferError> {
        Ok(texts
            .iter()
            .map(|t| Embedding {
                vector: self.vector_for(t),
                truncated: false,
            })
            .collect())
    }
}

impl Embedder for MockEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dims(&self) -> u32 {
        self.dims
    }

    fn embed(&self, texts: &[&str], role: Role) -> Result<Vec<Embedding>, InferError> {
        embed_with_batch_limit(texts, role, self.max_batch, |chunk, role| {
            self.embed_batch(chunk, role)
        })
    }

    fn resource_usage(&self) -> crate::embedder::ResourceUsage {
        crate::embedder::ResourceUsage {
            execution_provider: Some("cpu".into()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine_distance(a: &[f16], b: &[f16]) -> f32 {
        let mut dot = 0.0f32;
        let mut na = 0.0f32;
        let mut nb = 0.0f32;
        for (x, y) in a.iter().zip(b.iter()) {
            let xf = x.to_f32();
            let yf = y.to_f32();
            dot += xf * yf;
            na += xf * xf;
            nb += yf * yf;
        }
        1.0 - dot / (na.sqrt() * nb.sqrt())
    }

    fn norm(v: &[f16]) -> f32 {
        v.iter()
            .map(|x| {
                let f = x.to_f32();
                f * f
            })
            .sum::<f32>()
            .sqrt()
    }

    #[test]
    fn mock_satisfies_embedder_and_is_deterministic() {
        let a = MockEmbedder::new(8);
        let b = MockEmbedder::new(8);
        let va = a.embed(&["hello"], Role::Document).unwrap();
        let vb = b.embed(&["hello"], Role::Document).unwrap();
        assert_eq!(va[0].vector, vb[0].vector);
        assert_eq!(a.dims(), 8);
        assert_eq!(a.model_id(), "mock");
    }

    #[test]
    fn different_texts_yield_different_vectors() {
        let m = MockEmbedder::new(16);
        let a = m.embed(&["alpha"], Role::Query).unwrap();
        let b = m.embed(&["beta"], Role::Query).unwrap();
        assert_ne!(a[0].vector, b[0].vector);
    }

    #[test]
    fn vectors_have_declared_width_and_unit_norm() {
        let dims = 32;
        let m = MockEmbedder::new(dims);
        let out = m.embed(&["unit"], Role::Document).unwrap();
        assert_eq!(out[0].vector.len(), dims as usize);
        assert!((norm(&out[0].vector) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn query_matching_is_nearest_to_own_embedding() {
        let m = MockEmbedder::new(64);
        let texts = ["foo", "bar", "baz", "qux"];
        let embeds: Vec<_> = texts
            .iter()
            .map(|t| m.embed(&[*t], Role::Document).unwrap()[0].vector.clone())
            .collect();
        let matching = m.query_matching("foo");
        let dist_self = cosine_distance(&matching, &embeds[0]);
        for other in &embeds[1..] {
            assert!(
                dist_self < cosine_distance(&matching, other),
                "query_matching(foo) must be nearer to embed(foo)"
            );
        }
    }

    #[test]
    fn batch_split_preserves_order_and_matches_sub_batches() {
        let m = MockEmbedder::new(16).with_batch_limit(4);
        let texts: Vec<String> = (0..10).map(|i| format!("text-{i}")).collect();
        let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        let all = m.embed(&refs, Role::Document).unwrap();
        assert_eq!(all.len(), 10);

        let a = m.embed(&refs[0..4], Role::Document).unwrap();
        let b = m.embed(&refs[4..8], Role::Document).unwrap();
        let c = m.embed(&refs[8..10], Role::Document).unwrap();
        let concat: Vec<_> = a.into_iter().chain(b).chain(c).collect();
        assert_eq!(all.len(), concat.len());
        for (i, (got, expect)) in all.iter().zip(concat.iter()).enumerate() {
            assert_eq!(got.vector, expect.vector, "mismatch at index {i}");
        }
    }
}
