//! Search result records and preview truncation.
//!
//! Wire types in this module compile without the `engine` feature so thin
//! clients can deserialize search responses without linking tantivy/tokenizers.

use serde::{Deserialize, Serialize};

/// Preview ceiling in bytes. Roughly three-to-four lines of code — enough to
/// recognize the chunk, short enough that ten results stay small.
pub const PREVIEW_MAX_BYTES: usize = 320;

/// The serialized shape returned to the agent. Field names and order are the
/// locked snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub file: String,
    pub symbol: String,
    pub signature: String,
    pub doc: Option<String>,
    /// At most [`PREVIEW_MAX_BYTES`], char-boundary safe.
    pub preview: String,
    /// `[line_start, line_end]`, 1-based inclusive.
    pub lines: [u32; 2],
    pub lexical_score: Option<f32>,
    pub dense_score: Option<f32>,
    pub fused_score: f32,
}

/// Which retrievers ran and which failed. Degradation is data, not a log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchDiagnostics {
    pub lexical_ok: bool,
    pub dense_ok: bool,
    pub lexical_error: Option<String>,
    pub dense_error: Option<String>,
    pub stage_millis: StageTimings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageTimings {
    pub embed: u64,
    pub lexical: u64,
    pub dense: u64,
    pub fuse: u64,
    pub assemble: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub diagnostics: SearchDiagnostics,
}

/// Truncate `body` to at most [`PREVIEW_MAX_BYTES`], ending on a UTF-8 character
/// boundary. No ellipsis is appended.
pub fn preview_from_body(body: &str) -> String {
    if body.len() <= PREVIEW_MAX_BYTES {
        return body.to_owned();
    }
    let mut end = PREVIEW_MAX_BYTES;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    body[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::{PREVIEW_MAX_BYTES, SearchResult, preview_from_body};

    #[test]
    fn truncates_long_body_to_preview_max_bytes() {
        let body = "a".repeat(PREVIEW_MAX_BYTES + 50);
        let preview = preview_from_body(&body);
        assert!(preview.len() <= PREVIEW_MAX_BYTES);
        assert_eq!(preview.len(), PREVIEW_MAX_BYTES);
        assert!(!preview.contains('…'));
        assert!(!preview.ends_with("..."));
    }

    #[test]
    fn truncates_before_multibyte_character_boundary() {
        // 319 ASCII bytes, then a 3-byte character that would cross the limit.
        let mut body = "x".repeat(319);
        body.push('€'); // U+20AC, three UTF-8 bytes
        body.push_str("more");
        let preview = preview_from_body(&body);
        assert!(preview.len() < PREVIEW_MAX_BYTES);
        assert_eq!(preview.len(), 319);
        assert!(preview.is_char_boundary(preview.len()));
        assert_eq!(preview, "x".repeat(319));
        assert!(std::str::from_utf8(preview.as_bytes()).is_ok());
    }

    #[test]
    fn short_body_returned_whole() {
        let body = "fn short() {}";
        let preview = preview_from_body(body);
        assert_eq!(preview, body);
    }

    #[test]
    fn no_ellipsis_appended() {
        let body = "b".repeat(PREVIEW_MAX_BYTES + 10);
        let preview = preview_from_body(&body);
        assert!(!preview.contains('…'));
        assert!(!preview.ends_with("..."));
        assert_eq!(preview, "b".repeat(PREVIEW_MAX_BYTES));
    }

    #[test]
    fn search_result_serialization_snapshot() {
        let both = SearchResult {
            file: "src/timestamp.rs".into(),
            symbol: "normalize_timestamp".into(),
            signature: "fn normalize_timestamp(pts: i64, last: i64) -> i64".into(),
            doc: Some("Clamp regressing decoder timestamps to monotonic order.".into()),
            preview: "let mut t = pts;\nif t < last {\n    t = last + 1;".into(),
            lines: [82, 117],
            lexical_score: Some(0.44),
            dense_score: Some(0.81),
            fused_score: 0.032276,
        };
        let missing_lexical = SearchResult {
            file: "src/other.rs".into(),
            symbol: "helper".into(),
            signature: "fn helper()".into(),
            doc: None,
            preview: "fn helper() {}".into(),
            lines: [1, 3],
            lexical_score: None,
            dense_score: Some(0.0),
            fused_score: 0.016393,
        };
        let payload = vec![both, missing_lexical];
        let json = serde_json::to_string_pretty(&payload).unwrap();
        // Absent lexical_score must serialize as null, not 0.0.
        assert!(json.contains("\"lexical_score\": null"));
        assert!(json.contains("\"dense_score\": 0.0") || json.contains("\"dense_score\": 0.0"));
        insta::assert_snapshot!(json);
    }
}
