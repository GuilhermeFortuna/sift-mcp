use indexing::{Chunker, Language};

fn snapshot_lines(rel: &str, language: Language, source: &str) -> String {
    let mut chunker = Chunker::new().expect("chunker");
    let file = chunker.chunk_file(rel, language, source);
    assert!(
        file.diagnostics.is_empty(),
        "diagnostics: {:?}",
        file.diagnostics
    );
    file.chunks
        .iter()
        .map(|c| {
            format!(
                "{} {} {}:{}",
                c.record.symbol, c.record.symbol_type, c.record.line_start, c.record.line_end
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_line_ranges(rel: &str, language: Language, source: &str) {
    let mut chunker = Chunker::new().expect("chunker");
    let file = chunker.chunk_file(rel, language, source);
    let lines: Vec<&str> = source.lines().collect();
    for c in &file.chunks {
        if c.record.symbol_type == "file_prelude" {
            continue;
        }
        let start = c.record.line_start as usize;
        let end = c.record.line_end as usize;
        assert!(start >= 1 && start <= lines.len(), "bad start {}", start);
        assert!(end >= start && end <= lines.len(), "bad end {}", end);
        let first = lines[start - 1].trim();
        // First line should be the declaration or a documentation comment.
        assert!(
            !first.is_empty(),
            "empty first line for {}",
            c.record.symbol
        );
        let last = lines[end - 1].trim();
        assert!(
            !last.is_empty() || end > start,
            "empty last line for {}",
            c.record.symbol
        );
    }
}

#[test]
fn rust_snapshot() {
    let source = include_str!("fixtures/rust/sample.rs");
    insta::assert_snapshot!(snapshot_lines("sample.rs", Language::Rust, source));
}

#[test]
fn rust_line_ranges() {
    let source = include_str!("fixtures/rust/sample.rs");
    assert_line_ranges("sample.rs", Language::Rust, source);
}

#[test]
fn python_snapshot() {
    let source = include_str!("fixtures/python/sample.py");
    insta::assert_snapshot!(snapshot_lines("sample.py", Language::Python, source));
}

#[test]
fn python_line_ranges() {
    let source = include_str!("fixtures/python/sample.py");
    assert_line_ranges("sample.py", Language::Python, source);
}

#[test]
fn typescript_snapshot() {
    let source = include_str!("fixtures/typescript/sample.ts");
    insta::assert_snapshot!(snapshot_lines("sample.ts", Language::TypeScript, source));
}

#[test]
fn typescript_line_ranges() {
    let source = include_str!("fixtures/typescript/sample.ts");
    assert_line_ranges("sample.ts", Language::TypeScript, source);
}

#[test]
fn javascript_snapshot() {
    let source = include_str!("fixtures/javascript/sample.js");
    insta::assert_snapshot!(snapshot_lines("sample.js", Language::JavaScript, source));
}

#[test]
fn javascript_line_ranges() {
    let source = include_str!("fixtures/javascript/sample.js");
    assert_line_ranges("sample.js", Language::JavaScript, source);
}

#[test]
fn go_snapshot() {
    let source = include_str!("fixtures/go/sample.go");
    insta::assert_snapshot!(snapshot_lines("sample.go", Language::Go, source));
}

#[test]
fn go_line_ranges() {
    let source = include_str!("fixtures/go/sample.go");
    assert_line_ranges("sample.go", Language::Go, source);
}

#[test]
fn c_snapshot() {
    let source = include_str!("fixtures/c/sample.c");
    insta::assert_snapshot!(snapshot_lines("sample.c", Language::C, source));
}

#[test]
fn c_line_ranges() {
    let source = include_str!("fixtures/c/sample.c");
    assert_line_ranges("sample.c", Language::C, source);
}

#[test]
fn cpp_snapshot() {
    let source = include_str!("fixtures/cpp/sample.cpp");
    insta::assert_snapshot!(snapshot_lines("sample.cpp", Language::Cpp, source));
}

#[test]
fn cpp_line_ranges() {
    let source = include_str!("fixtures/cpp/sample.cpp");
    assert_line_ranges("sample.cpp", Language::Cpp, source);
}
