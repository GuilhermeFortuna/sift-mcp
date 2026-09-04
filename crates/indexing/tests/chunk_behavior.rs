use indexing::{Chunker, FILE_PRELUDE_MIN_CHARS, Language, OVERSIZE_CHAR_THRESHOLD};

#[test]
fn container_excludes_member_bodies_and_qualifies_names() {
    let source = r#"
struct Tracker {
    value: i32,
}

impl Tracker {
    fn new() -> Self {
        Self { value: 0 }
    }

    fn update(&mut self) {
        self.value += 1;
    }
}
"#;
    let mut chunker = Chunker::new().unwrap();
    let file = chunker.chunk_file("t.rs", Language::Rust, source);
    let symbols: Vec<_> = file
        .chunks
        .iter()
        .map(|c| c.record.symbol.as_str())
        .collect();
    assert!(symbols.contains(&"Tracker"), "got {symbols:?}");
    // impl + two methods = container handling yields three chunks for the impl group,
    // plus the struct itself.
    let impl_chunk = file
        .chunks
        .iter()
        .find(|c| c.record.symbol == "Tracker" && c.record.symbol_type == "impl")
        .expect("impl chunk");
    assert!(
        !impl_chunk.body.contains("fn new()"),
        "impl body should exclude methods: {}",
        impl_chunk.body
    );
    assert!(
        !impl_chunk.body.contains("fn update"),
        "impl body should exclude methods: {}",
        impl_chunk.body
    );

    let new_m = file
        .chunks
        .iter()
        .find(|c| c.record.symbol == "Tracker::new")
        .expect("new");
    let update_m = file
        .chunks
        .iter()
        .find(|c| c.record.symbol == "Tracker::update")
        .expect("update");
    assert!(new_m.body.contains("fn new()"));
    assert!(update_m.body.contains("fn update"));

    // Three chunks for the impl group: impl + new + update.
    let impl_group = file
        .chunks
        .iter()
        .filter(|c| {
            c.record.symbol == "Tracker" && c.record.symbol_type == "impl"
                || c.record.symbol.starts_with("Tracker::")
        })
        .count();
    assert_eq!(impl_group, 3, "expected impl + 2 methods");
}

#[test]
fn file_prelude_emitted_only_when_large_enough() {
    let imports_only = "use std::collections::HashMap;\nuse std::io;\n";
    assert!(imports_only.chars().count() < FILE_PRELUDE_MIN_CHARS);

    let mut chunker = Chunker::new().unwrap();
    let small = chunker.chunk_file("s.rs", Language::Rust, imports_only);
    assert!(
        small
            .chunks
            .iter()
            .all(|c| c.record.symbol_type != "file_prelude"),
        "imports-only should not emit prelude"
    );

    let mut big_prelude =
        String::from("// Substantial module-level setup that exceeds the threshold.\n");
    while big_prelude.chars().count() < FILE_PRELUDE_MIN_CHARS + 10 {
        big_prelude.push_str("const PAD: &str = \"xxxxxxxx\";\n");
    }
    big_prelude.push_str("\nfn later() {}\n");
    let large = chunker.chunk_file("b.rs", Language::Rust, &big_prelude);
    assert!(
        large
            .chunks
            .iter()
            .any(|c| c.record.symbol_type == "file_prelude"),
        "expected file_prelude, got {:?}",
        large
            .chunks
            .iter()
            .map(|c| &c.record.symbol_type)
            .collect::<Vec<_>>()
    );
}

#[test]
fn oversize_symbol_splits_on_statements_with_signature_prefix() {
    let mut body = String::from("fn huge() {\n");
    let mut i = 0;
    while body.chars().count() < OVERSIZE_CHAR_THRESHOLD + 200 {
        body.push_str(&format!("    let v{i} = {i};\n"));
        body.push_str(&format!("    let _ = v{i};\n"));
        i += 1;
    }
    body.push_str("}\n");
    assert!(body.chars().count() > OVERSIZE_CHAR_THRESHOLD);

    let mut chunker = Chunker::new().unwrap();
    let file = chunker.chunk_file("huge.rs", Language::Rust, &body);
    let frags: Vec<_> = file
        .chunks
        .iter()
        .filter(|c| c.record.symbol == "huge")
        .collect();
    assert!(
        frags.len() > 1,
        "expected multiple fragments, got {}",
        frags.len()
    );
    let mut seen = std::collections::HashSet::new();
    for (i, f) in frags.iter().enumerate() {
        assert_eq!(f.record.symbol, "huge");
        assert_eq!(f.fragment, Some(i as u32));
        assert!(
            f.body.contains("fn huge()"),
            "fragment missing signature: {}",
            f.body.chars().take(80).collect::<String>()
        );
        assert!(seen.insert(f.fragment));
    }
}

#[test]
fn move_preserves_hashes_edit_changes_one() {
    let source = include_str!("fixtures/rust/sample.rs");
    let mut chunker = Chunker::new().unwrap();
    let a = chunker.chunk_file("old/path.rs", Language::Rust, source);
    let b = chunker.chunk_file("new/other.rs", Language::Rust, source);
    assert_eq!(a.chunks.len(), b.chunks.len());
    for (ca, cb) in a.chunks.iter().zip(b.chunks.iter()) {
        assert_eq!(ca.record.symbol, cb.record.symbol);
        assert_eq!(
            ca.record.content_hash.as_bytes(),
            cb.record.content_hash.as_bytes(),
            "hash changed on move for {}",
            ca.record.symbol
        );
    }

    let edited = source.replacen("let x = 1;", "let x = 2;", 1);
    let c = chunker.chunk_file("old/path.rs", Language::Rust, &edited);
    let mut changed = 0;
    for (ca, cc) in a.chunks.iter().zip(c.chunks.iter()) {
        if ca.record.content_hash.as_bytes() != cc.record.content_hash.as_bytes() {
            changed += 1;
            assert_eq!(ca.record.symbol, "free_function");
        }
    }
    assert_eq!(changed, 1, "exactly one hash should change");
}

#[test]
fn malformed_file_yields_diagnostic_without_panic() {
    let broken = "fn oops( {\n this is {{{ not rust\n((((((****\n";
    let mut chunker = Chunker::new().unwrap();
    let file = chunker.chunk_file("bad.rs", Language::Rust, broken);
    assert!(
        file.chunks.is_empty(),
        "got chunks: {:?}",
        file.chunks.len()
    );
    assert_eq!(file.diagnostics.len(), 1);
}

#[test]
fn single_error_keeps_valid_symbols() {
    // Extra unmatched token after a valid function — small error island.
    let source = "fn good() {\n    let x = 1;\n}\n\n@@@\n\nfn also() {\n    let y = 2;\n}\n";
    let mut chunker = Chunker::new().unwrap();
    let file = chunker.chunk_file("partial.rs", Language::Rust, source);
    assert!(
        file.diagnostics.is_empty(),
        "small error should not fail the file: {:?}",
        file.diagnostics
    );
    assert!(file.chunks.iter().any(|c| c.record.symbol == "good"));
    assert!(file.chunks.iter().any(|c| c.record.symbol == "also"));
}

#[test]
fn chunking_is_deterministic() {
    let source = include_str!("fixtures/rust/sample.rs");
    let mut chunker = Chunker::new().unwrap();
    let a = chunker.chunk_file("sample.rs", Language::Rust, source);
    let b = chunker.chunk_file("sample.rs", Language::Rust, source);
    assert_eq!(a.chunks.len(), b.chunks.len());
    for (ca, cb) in a.chunks.iter().zip(b.chunks.iter()) {
        assert_eq!(ca.record, cb.record);
        assert_eq!(ca.body, cb.body);
        assert_eq!(ca.fragment, cb.fragment);
    }
}
