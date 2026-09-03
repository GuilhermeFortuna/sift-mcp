//! Inference crate — batched embedding behind [`Embedder`].

pub mod embedder;
pub mod metadata;
pub mod mock;
pub mod pooling;
pub mod tokenize;

pub use embedder::{Embedder, Embedding, InferError, Role};
pub use metadata::{MetadataError, ModelMetadata, Normalize, Pooling};
pub use mock::MockEmbedder;
pub use pooling::{l2_normalize_rows, pool};
pub use tokenize::TextTokenizer;

#[cfg(test)]
mod fixture_tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct Tolerance {
        metric: String,
        max: f64,
        basis: String,
    }

    #[derive(Debug, Deserialize)]
    struct Case {
        name: String,
        text: String,
        role: String,
        tokens: Vec<u32>,
        truncated: bool,
        vector: Vec<f64>,
    }

    #[derive(Debug, Deserialize)]
    struct ReferenceFixture {
        model_id: String,
        dims: u32,
        tolerance: Tolerance,
        cases: Vec<Case>,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum FixtureError {
        InvalidJson(String),
        MissingField(&'static str),
        MissingCase(&'static str),
        BadCase { name: String, reason: &'static str },
    }

    const REQUIRED_CASES: &[&str] = &[
        "code_snippet",
        "prose_sentence",
        "bare_identifier",
        "empty_string",
        "over_length",
        "non_ascii",
    ];

    fn validate_reference_fixture(json: &str) -> Result<ReferenceFixture, FixtureError> {
        let fixture: ReferenceFixture =
            serde_json::from_str(json).map_err(|e| FixtureError::InvalidJson(e.to_string()))?;

        if fixture.model_id.is_empty() {
            return Err(FixtureError::MissingField("model_id"));
        }
        if fixture.dims == 0 {
            return Err(FixtureError::MissingField("dims"));
        }
        if fixture.tolerance.metric.is_empty() || fixture.tolerance.basis.is_empty() {
            return Err(FixtureError::MissingField("tolerance"));
        }
        if fixture.tolerance.max <= 0.0 {
            return Err(FixtureError::MissingField("tolerance.max"));
        }

        let names: HashSet<&str> = fixture.cases.iter().map(|c| c.name.as_str()).collect();
        for required in REQUIRED_CASES {
            if !names.contains(required) {
                return Err(FixtureError::MissingCase(required));
            }
        }

        for case in &fixture.cases {
            if case.tokens.is_empty() && case.name != "empty_string" {
                // empty_string may still have special tokens; require a vector always.
            }
            if case.vector.len() != fixture.dims as usize {
                return Err(FixtureError::BadCase {
                    name: case.name.clone(),
                    reason: "vector width does not match dims",
                });
            }
            if case.name != "empty_string" && case.tokens.is_empty() {
                return Err(FixtureError::BadCase {
                    name: case.name.clone(),
                    reason: "missing token sequence",
                });
            }
            // empty_string must still carry a token sequence (BOS/EOS etc.) and a vector.
            if case.name == "empty_string" && case.tokens.is_empty() {
                return Err(FixtureError::BadCase {
                    name: case.name.clone(),
                    reason: "missing token sequence",
                });
            }
            let _ = (&case.text, &case.role, case.truncated);
        }

        Ok(fixture)
    }

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name)
    }

    #[test]
    fn truncated_fixture_fails_validation() {
        let truncated = r#"{
            "model_id": "test@abc",
            "dims": 768,
            "tolerance": { "metric": "cosine_distance", "max": 1e-3, "basis": "test" },
            "cases": [
                {
                    "name": "code_snippet",
                    "text": "fn x() {}",
                    "role": "document",
                    "tokens": [1, 2],
                    "truncated": false,
                    "vector": [0.0]
                }
            ]
        }"#;
        let err = validate_reference_fixture(truncated).expect_err("truncated fixture must fail");
        match err {
            FixtureError::MissingCase(_) | FixtureError::BadCase { .. } => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn fallback_reference_fixture_is_structurally_valid() {
        let path = fixture_path("fallback-reference.json");
        let json = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let fixture = validate_reference_fixture(&json).expect("fallback fixture must validate");
        assert_eq!(fixture.dims, 768);
        assert_eq!(fixture.cases.len(), REQUIRED_CASES.len());
        for case in &fixture.cases {
            assert_eq!(case.vector.len(), fixture.dims as usize);
            assert!(
                !case.tokens.is_empty(),
                "case {} must pin a token sequence",
                case.name
            );
        }
    }

    #[test]
    fn primary_reference_fixture_is_structurally_valid() {
        let path = fixture_path("primary-reference.json");
        let json = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let fixture = validate_reference_fixture(&json).expect("primary fixture must validate");
        assert_eq!(fixture.dims, 1024);
        assert_eq!(fixture.cases.len(), REQUIRED_CASES.len());
        for case in &fixture.cases {
            assert_eq!(case.vector.len(), fixture.dims as usize);
            assert!(
                !case.tokens.is_empty(),
                "case {} must pin a token sequence",
                case.name
            );
        }
    }

    #[test]
    #[ignore = "requires CUDA hardware and ONNX Runtime"]
    fn gpu_inference_is_only_meaningful_with_hardware() {
        panic!("GPU inference tests are only meaningful with CUDA hardware and ONNX Runtime");
    }
}
