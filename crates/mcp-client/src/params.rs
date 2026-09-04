//! Typed MCP tool parameters and bound-checking.

use schemars::JsonSchema;
use serde::Deserialize;
use thiserror::Error;

pub const TOP_K_MAX: usize = 20;
pub const QUERY_MAX_CHARS: usize = 1000;
pub const CODE_MAX_CHARS: usize = 20_000;
pub const DEFAULT_TOP_K: usize = 5;

#[derive(Debug, Error, PartialEq, Eq)]
#[error("{0}")]
pub struct ParamError(pub String);

pub trait Validate {
    fn validate(&self) -> Result<(), ParamError>;
}

pub fn validate<T: Validate>(params: &T) -> Result<(), ParamError> {
    params.validate()
}

fn default_top_k() -> usize {
    DEFAULT_TOP_K
}

fn default_full() -> bool {
    false
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SearchCodeParams {
    pub query: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}

impl Validate for SearchCodeParams {
    fn validate(&self) -> Result<(), ParamError> {
        if self.query.is_empty() {
            return Err(ParamError("query must be non-empty".into()));
        }
        if self.query.chars().count() > QUERY_MAX_CHARS {
            return Err(ParamError(format!(
                "query exceeds QUERY_MAX_CHARS bound of {QUERY_MAX_CHARS}"
            )));
        }
        validate_top_k(self.top_k)
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FindSimilarCodeParams {
    pub code: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}

impl Validate for FindSimilarCodeParams {
    fn validate(&self) -> Result<(), ParamError> {
        if self.code.is_empty() {
            return Err(ParamError("code must be non-empty".into()));
        }
        if self.code.chars().count() > CODE_MAX_CHARS {
            return Err(ParamError(format!(
                "code exceeds CODE_MAX_CHARS bound of {CODE_MAX_CHARS}"
            )));
        }
        validate_top_k(self.top_k)
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GetSymbolParams {
    pub file: String,
    pub symbol: String,
}

impl Validate for GetSymbolParams {
    fn validate(&self) -> Result<(), ParamError> {
        if self.file.is_empty() {
            return Err(ParamError("file must be non-empty".into()));
        }
        if self.symbol.is_empty() {
            return Err(ParamError("symbol must be non-empty".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct IndexRepositoryParams {
    pub path: String,
    #[serde(default = "default_full")]
    pub full: bool,
}

impl Validate for IndexRepositoryParams {
    fn validate(&self) -> Result<(), ParamError> {
        if self.path.is_empty() {
            return Err(ParamError("path must be non-empty".into()));
        }
        Ok(())
    }
}

fn validate_top_k(top_k: usize) -> Result<(), ParamError> {
    if !(1..=TOP_K_MAX).contains(&top_k) {
        return Err(ParamError(format!(
            "top_k out of range: must be 1..={TOP_K_MAX} (got {top_k})"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_k_zero_rejected_naming_bound() {
        let p = SearchCodeParams {
            query: "timestamps".into(),
            top_k: 0,
        };
        let err = validate(&p).unwrap_err().0;
        assert!(err.contains("1..="), "{err}");
        assert!(err.contains(&TOP_K_MAX.to_string()), "{err}");
    }

    #[test]
    fn top_k_above_max_rejected_naming_bound() {
        let p = SearchCodeParams {
            query: "timestamps".into(),
            top_k: TOP_K_MAX + 1,
        };
        let err = validate(&p).unwrap_err().0;
        assert!(err.contains(&TOP_K_MAX.to_string()), "{err}");
    }

    #[test]
    fn empty_query_rejected() {
        let p = SearchCodeParams {
            query: String::new(),
            top_k: 5,
        };
        let err = validate(&p).unwrap_err().0;
        assert!(err.contains("non-empty"), "{err}");
    }

    #[test]
    fn query_above_max_chars_rejected_naming_limit() {
        let p = SearchCodeParams {
            query: "a".repeat(QUERY_MAX_CHARS + 1),
            top_k: 5,
        };
        let err = validate(&p).unwrap_err().0;
        assert!(err.contains(&QUERY_MAX_CHARS.to_string()), "{err}");
    }

    #[test]
    fn code_above_max_chars_rejected() {
        let p = FindSimilarCodeParams {
            code: "x".repeat(CODE_MAX_CHARS + 1),
            top_k: 5,
        };
        let err = validate(&p).unwrap_err().0;
        assert!(err.contains(&CODE_MAX_CHARS.to_string()), "{err}");
    }

    #[test]
    fn omitted_top_k_defaults_to_five() {
        let p: SearchCodeParams = serde_json::from_str(r#"{"query":"timestamps"}"#).unwrap();
        assert_eq!(p.top_k, 5);
        validate(&p).unwrap();
    }

    #[test]
    fn omitted_full_defaults_to_false() {
        let p: IndexRepositoryParams = serde_json::from_str(r#"{"path":"/tmp/repo"}"#).unwrap();
        assert!(!p.full);
        validate(&p).unwrap();
    }

    #[test]
    fn wrong_typed_field_rejected() {
        let err = serde_json::from_str::<SearchCodeParams>(r#"{"query":"x","top_k":"five"}"#)
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid type") || msg.contains("u64") || msg.contains("integer"),
            "{msg}"
        );
    }
}
