//! Tokenization with role-based prefixes and truncation reporting.

use std::path::Path;

use tokenizers::{Tokenizer, TruncationDirection, TruncationParams, TruncationStrategy};

use crate::embedder::{InferError, Role};
use crate::metadata::{ModelMetadata, Pooling};

pub struct TextTokenizer {
    tokenizer: Tokenizer,
    metadata: ModelMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedInput {
    pub input_ids: Vec<u32>,
    pub attention_mask: Vec<u32>,
    pub truncated: bool,
}

impl TextTokenizer {
    pub fn from_dir(model_dir: &Path, metadata: ModelMetadata) -> Result<Self, InferError> {
        let path = model_dir.join("tokenizer.json");
        let tokenizer =
            Tokenizer::from_file(&path).map_err(|e| InferError::Tokenizer(e.to_string()))?;
        Ok(Self {
            tokenizer,
            metadata,
        })
    }

    pub fn metadata(&self) -> &ModelMetadata {
        &self.metadata
    }

    pub fn apply_prefix(&self, text: &str, role: Role) -> String {
        match role {
            Role::Query => {
                if let Some(prefix) = &self.metadata.query_prefix {
                    return format!("{prefix}{text}");
                }
            }
            Role::Document => {
                if let Some(prefix) = &self.metadata.document_prefix {
                    return format!("{prefix}{text}");
                }
            }
        }
        text.to_string()
    }

    /// Unpadded token ids for `text` after applying the role prefix.
    /// Truncation is detected against the untruncated encode length.
    pub fn encode_unpadded(&self, text: &str, role: Role) -> Result<(Vec<u32>, bool), InferError> {
        let prefixed = self.apply_prefix(text, role);
        let max_len = self.metadata.max_sequence_length;

        let full = self
            .tokenizer
            .encode(prefixed.as_str(), true)
            .map_err(|e| InferError::Tokenizer(e.to_string()))?;
        let full_ids = full.get_ids().to_vec();
        let truncated = full_ids.len() > max_len;

        let ids = if truncated {
            // Match HF `truncation=True`: keep special tokens (e.g. trailing EOS).
            let mut tok = self.tokenizer.clone();
            tok.with_truncation(Some(TruncationParams {
                max_length: max_len,
                strategy: TruncationStrategy::LongestFirst,
                stride: 0,
                direction: TruncationDirection::Right,
            }))
            .map_err(|e| InferError::Tokenizer(e.to_string()))?;
            let enc = tok
                .encode(prefixed.as_str(), true)
                .map_err(|e| InferError::Tokenizer(e.to_string()))?;
            enc.get_ids().to_vec()
        } else {
            full_ids
        };
        Ok((ids, truncated))
    }

    /// Batch-encode with padding and attention masks.
    /// Uses left padding for [`Pooling::LastToken`], right otherwise.
    pub fn encode_batch_padded(
        &self,
        texts: &[&str],
        role: Role,
    ) -> Result<Vec<EncodedInput>, InferError> {
        let max_len = self.metadata.max_sequence_length;
        let left_pad = matches!(self.metadata.pooling, Pooling::LastToken);

        let mut encoded = Vec::with_capacity(texts.len());
        let mut max_in_batch = 0usize;
        for text in texts {
            let (ids, truncated) = self.encode_unpadded(text, role)?;
            max_in_batch = max_in_batch.max(ids.len().min(max_len));
            encoded.push((ids, truncated));
        }

        let pad_id = self.pad_token_id();

        let mut out = Vec::with_capacity(texts.len());
        for (ids, truncated) in encoded {
            let seq_len = ids.len().min(max_len);
            let pad = max_in_batch.saturating_sub(seq_len);
            let mut input_ids = Vec::with_capacity(max_in_batch);
            let mut attention_mask = Vec::with_capacity(max_in_batch);
            if left_pad {
                input_ids.extend(std::iter::repeat_n(pad_id, pad));
                attention_mask.extend(std::iter::repeat_n(0u32, pad));
                input_ids.extend_from_slice(&ids[..seq_len]);
                attention_mask.extend(std::iter::repeat_n(1u32, seq_len));
            } else {
                input_ids.extend_from_slice(&ids[..seq_len]);
                attention_mask.extend(std::iter::repeat_n(1u32, seq_len));
                input_ids.extend(std::iter::repeat_n(pad_id, pad));
                attention_mask.extend(std::iter::repeat_n(0u32, pad));
            }
            out.push(EncodedInput {
                input_ids,
                attention_mask,
                truncated,
            });
        }
        Ok(out)
    }

    fn pad_token_id(&self) -> u32 {
        if let Some(padding) = self.tokenizer.get_padding() {
            return padding.pad_id;
        }
        // Match Hugging Face Qwen3: pad_token = <|endoftext|>
        self.tokenizer
            .token_to_id("<|endoftext|>")
            .or_else(|| self.tokenizer.token_to_id("[PAD]"))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::path::PathBuf;

    #[derive(Deserialize)]
    struct Case {
        name: String,
        text: String,
        role: String,
        tokens: Vec<u32>,
        truncated: bool,
    }

    #[derive(Deserialize)]
    struct ReferenceFixture {
        cases: Vec<Case>,
    }

    fn role_from(s: &str) -> Role {
        match s {
            "query" => Role::Query,
            "document" => Role::Document,
            other => panic!("unknown role {other}"),
        }
    }

    fn tokenizer_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join("primary-tokenizer")
    }

    fn load_tokenizer() -> TextTokenizer {
        let dir = tokenizer_dir();
        let meta_json = std::fs::read_to_string(dir.join("metadata.json")).expect("metadata");
        let meta = ModelMetadata::from_json(&meta_json).expect("parse metadata");
        TextTokenizer::from_dir(&dir, meta).expect("load tokenizer")
    }

    #[test]
    fn fixture_token_sequences_match_exactly() {
        let tok = load_tokenizer();
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("primary-reference.json");
        let fixture: ReferenceFixture =
            serde_json::from_str(&std::fs::read_to_string(fixture_path).unwrap()).unwrap();

        for case in &fixture.cases {
            let (ids, truncated) = tok
                .encode_unpadded(&case.text, role_from(&case.role))
                .expect("encode");
            assert_eq!(
                ids, case.tokens,
                "token mismatch for case {}",
                case.name
            );
            assert_eq!(
                truncated, case.truncated,
                "truncation flag mismatch for {}",
                case.name
            );
        }
    }

    #[test]
    fn over_length_truncates_to_max_sequence_length() {
        let tok = load_tokenizer();
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("primary-reference.json");
        let fixture: ReferenceFixture =
            serde_json::from_str(&std::fs::read_to_string(fixture_path).unwrap()).unwrap();
        let case = fixture
            .cases
            .iter()
            .find(|c| c.name == "over_length")
            .expect("over_length case");
        let (ids, truncated) = tok
            .encode_unpadded(&case.text, role_from(&case.role))
            .unwrap();
        assert!(truncated);
        assert_eq!(ids.len(), tok.metadata().max_sequence_length);
        assert_eq!(ids, case.tokens);
    }
}
