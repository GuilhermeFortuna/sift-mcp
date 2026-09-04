//! Search result records and preview truncation.

/// Preview ceiling in bytes. Roughly three-to-four lines of code — enough to
/// recognize the chunk, short enough that ten results stay small.
pub const PREVIEW_MAX_BYTES: usize = 320;

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
    use super::{PREVIEW_MAX_BYTES, preview_from_body};

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
}
