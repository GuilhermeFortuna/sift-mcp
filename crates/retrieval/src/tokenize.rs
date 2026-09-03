//! Code-aware lexical tokenization.

/// Return the whole identifier followed by its searchable components.
pub fn split_identifier(token: &str) -> Vec<String> {
    let has_namespace_separator = token
        .chars()
        .any(|character| !character.is_ascii_alphanumeric() && character != '_' && character != '-');
    let mut terms = Vec::new();

    if !token.is_empty() && !has_namespace_separator {
        push_unique(&mut terms, token.to_ascii_lowercase());
    }

    for component in token.split(|character: char| {
        character == '_' || character == '-' || !character.is_ascii_alphanumeric()
    }) {
        let characters: Vec<char> = component.chars().collect();
        let mut start = 0;
        for index in 1..characters.len() {
            let previous = characters[index - 1];
            let current = characters[index];
            let next = characters.get(index + 1).copied();
            let lower_to_upper = previous.is_ascii_lowercase() && current.is_ascii_uppercase();
            let acronym_to_word = previous.is_ascii_uppercase()
                && current.is_ascii_uppercase()
                && next.is_some_and(|character| character.is_ascii_lowercase());
            if lower_to_upper || acronym_to_word {
                let part: String = characters[start..index].iter().collect();
                push_unique(&mut terms, part.to_ascii_lowercase());
                start = index;
            }
        }
        if start < characters.len() {
            let part: String = characters[start..].iter().collect();
            push_unique(&mut terms, part.to_ascii_lowercase());
        }
    }

    terms
}

fn push_unique(terms: &mut Vec<String>, term: String) {
    if !term.is_empty() && !terms.iter().any(|existing| existing == &term) {
        terms.push(term);
    }
}

#[cfg(test)]
mod tests {
    use super::split_identifier;

    #[test]
    fn splits_camel_case_with_whole_token() {
        assert_eq!(
            split_identifier("normalizeTimestamp"),
            ["normalizetimestamp", "normalize", "timestamp"]
        );
    }

    #[test]
    fn splits_snake_case_with_whole_token() {
        assert_eq!(
            split_identifier("normalize_timestamp"),
            ["normalize_timestamp", "normalize", "timestamp"]
        );
    }

    #[test]
    fn splits_acronym_run() {
        assert_eq!(
            split_identifier("parseHTTPResponse"),
            ["parsehttpresponse", "parse", "http", "response"]
        );
    }

    #[test]
    fn splits_namespace_separators() {
        assert_eq!(split_identifier("Tracker::update"), ["tracker", "update"]);
    }

    #[test]
    fn keeps_digits_attached() {
        assert_eq!(split_identifier("sha256"), ["sha256"]);
    }

    #[test]
    fn splits_snake_case_with_digit_token() {
        assert_eq!(
            split_identifier("read_utf8_bom"),
            ["read_utf8_bom", "read", "utf8", "bom"]
        );
    }

    #[test]
    fn lowercases_tokens_without_boundaries() {
        assert_eq!(split_identifier("EAGAIN"), ["eagain"]);
    }
}
