use std::ops::Range;

use tree_sitter::{Language as TsLanguage, Node, Parser, Query, QueryCursor, StreamingIterator, Tree};

use crate::error::{ChunkDiagnostic, ChunkError};
use crate::hash::content_hash;
use crate::Language;
use storage::ChunkRecord;

/// Character-count approximation of the model's 512-token context
/// (≈4 characters per token). Exact accounting waits for SIFT-005's tokenizer.
pub const OVERSIZE_CHAR_THRESHOLD: usize = 2048;

/// Minimum size for a synthetic `file_prelude` chunk.
pub const FILE_PRELUDE_MIN_CHARS: usize = 80;

/// Fail the file when error nodes are at least this fraction of all nodes.
pub const ERROR_NODE_RATIO_THRESHOLD: f64 = 0.5;

/// A chunk plus the body text to embed. The record is what SIFT-002 stores;
/// the body is what SIFT-005 embeds and is not persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub record: ChunkRecord,
    pub body: String,
    /// Some(n) when the parent symbol was split; n is 0-based fragment index.
    pub fragment: Option<u32>,
}

/// Everything one file produced, including why nothing was produced.
#[derive(Debug, Clone, Default)]
pub struct FileChunks {
    pub chunks: Vec<Chunk>,
    pub diagnostics: Vec<ChunkDiagnostic>,
}

struct LangConfig {
    language: TsLanguage,
    query: Query,
    container_kinds: &'static [&'static str],
    statement_kinds: &'static [&'static str],
}

/// One tree-sitter Parser and query set per Language.
pub struct Chunker {
    configs: [LangConfig; 7],
    parser: Parser,
}

#[derive(Debug, Clone)]
struct SymbolHit {
    kind: String,
    name: String,
    /// Byte range including attached documentation.
    range: Range<usize>,
    /// Byte range of the construct itself (no docs), for signature extraction.
    node_range: Range<usize>,
    start_line: u32,
    end_line: u32,
    doc_first_line: Option<String>,
    /// True when this symbol can contain other symbols (class, impl, mod, …).
    is_container: bool,
}

impl Chunker {
    pub fn new() -> Result<Self, ChunkError> {
        Ok(Self {
            configs: [
                rust_config()?,
                python_config()?,
                typescript_config()?,
                javascript_config()?,
                go_config()?,
                c_config()?,
                cpp_config()?,
            ],
            parser: Parser::new(),
        })
    }

    pub fn chunk_file(&mut self, rel_path: &str, language: Language, source: &str) -> FileChunks {
        let mut out = FileChunks::default();
        let cfg = &self.configs[language as usize];

        if self.parser.set_language(&cfg.language).is_err() {
            out.diagnostics.push(ChunkDiagnostic {
                file: rel_path.to_string(),
                message: format!("failed to set language for {rel_path}"),
            });
            return out;
        }

        let Some(tree) = self.parser.parse(source, None) else {
            out.diagnostics.push(ChunkDiagnostic {
                file: rel_path.to_string(),
                message: format!("parse returned no tree for {rel_path}"),
            });
            return out;
        };

        if error_ratio(&tree) >= ERROR_NODE_RATIO_THRESHOLD {
            out.diagnostics.push(ChunkDiagnostic {
                file: rel_path.to_string(),
                message: format!("parse failed for {rel_path}: too many error nodes"),
            });
            return out;
        }

        let hits = collect_symbols(&tree, source, cfg);
        let ordered = order_with_containers(hits);

        // Track covered ranges for prelude computation.
        let mut covered: Vec<Range<usize>> = Vec::new();

        for hit in &ordered {
            let member_ranges: Vec<Range<usize>> = ordered
                .iter()
                .filter(|m| m.range != hit.range && range_contains(&hit.range, &m.range))
                .map(|m| m.range.clone())
                .collect();

            let body = if hit.is_container && !member_ranges.is_empty() {
                body_minus_members(source, &hit.range, &member_ranges)
            } else {
                source[hit.range.clone()].to_string()
            };

            if body.trim().is_empty() {
                continue;
            }

            covered.push(hit.range.clone());

            let signature = first_signature_line(source, &hit.node_range);
            let qualified = hit.name.clone();
            let split = body.chars().count() > OVERSIZE_CHAR_THRESHOLD;
            let fragments = split_if_oversize(source, cfg, &tree, hit, &body, &signature);
            for (frag_idx, frag_body) in fragments.into_iter().enumerate() {
                let fragment = if split { Some(frag_idx as u32) } else { None };
                let hash = content_hash(language, &qualified, &frag_body);
                out.chunks.push(Chunk {
                    record: ChunkRecord {
                        repository: String::new(),
                        file: rel_path.replace('\\', "/"),
                        language: language.as_str().to_string(),
                        symbol: qualified.clone(),
                        symbol_type: hit.kind.clone(),
                        signature: signature.clone(),
                        doc_first_line: hit.doc_first_line.clone(),
                        line_start: hit.start_line,
                        line_end: hit.end_line,
                        content_hash: hash,
                    },
                    body: frag_body,
                    fragment,
                });
            }
        }

        // File prelude: uncovered top-level text above the minimum size.
        if let Some(prelude) = file_prelude(source, &covered) {
            let hash = content_hash(language, "<file_prelude>", &prelude);
            let (ps, pe) = prelude_span(source, &covered).unwrap_or((0, source.len()));
            let (start_line, end_line) = line_span(source, ps, pe);
            out.chunks.insert(
                0,
                Chunk {
                    record: ChunkRecord {
                        repository: String::new(),
                        file: rel_path.replace('\\', "/"),
                        language: language.as_str().to_string(),
                        symbol: "<file_prelude>".to_string(),
                        symbol_type: "file_prelude".to_string(),
                        signature: String::new(),
                        doc_first_line: None,
                        line_start: start_line,
                        line_end: end_line,
                        content_hash: hash,
                    },
                    body: prelude,
                    fragment: None,
                },
            );
        }

        out
    }
}

impl Default for Chunker {
    fn default() -> Self {
        Self::new().expect("Chunker::new")
    }
}

fn error_ratio(tree: &Tree) -> f64 {
    let root = tree.root_node();
    let mut total = 0u64;
    let mut errors = 0u64;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        total += 1;
        if node.is_error() || node.is_missing() {
            errors += 1;
        }
        let mut c = node.walk();
        for child in node.children(&mut c) {
            stack.push(child);
        }
    }
    if total == 0 {
        0.0
    } else {
        errors as f64 / total as f64
    }
}

fn collect_symbols(tree: &Tree, source: &str, cfg: &LangConfig) -> Vec<SymbolHit> {
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&cfg.query, tree.root_node(), source.as_bytes());
    let mut hits = Vec::new();

    while let Some(m) = matches.next() {
        let mut name = None;
        let mut recv = None;
        let mut def_node: Option<Node> = None;
        for cap in m.captures {
            let capture_name = cfg.query.capture_names()[cap.index as usize];
            match capture_name {
                "name" => {
                    name = Some(source[cap.node.byte_range()].to_string());
                }
                "recv" => {
                    recv = Some(source[cap.node.byte_range()].to_string());
                }
                "def" => {
                    def_node = Some(cap.node);
                }
                _ => {}
            }
        }
        let Some(node) = def_node else {
            continue;
        };
        let Some(mut name) = name else {
            continue;
        };
        if let Some(recv) = recv {
            name = format!("{recv}::{name}");
        }
        let kind = map_kind(node.kind());
        let is_container = cfg.container_kinds.contains(&node.kind());
        let (doc_start, doc_first) = attached_doc(source, node);
        let start_byte = doc_start.unwrap_or(node.start_byte());
        let end_byte = node.end_byte();
        let start_line = byte_to_line(source, start_byte);
        let end_line = byte_to_line(source, end_byte.saturating_sub(1).max(start_byte));
        hits.push(SymbolHit {
            kind,
            name,
            range: start_byte..end_byte,
            node_range: node.start_byte()..node.end_byte(),
            start_line,
            end_line,
            doc_first_line: doc_first,
            is_container,
        });
    }

    // Qualify names by enclosing containers.
    qualify_names(&mut hits);
    hits
}

fn qualify_names(hits: &mut [SymbolHit]) {
    let snapshot: Vec<(Range<usize>, String, bool)> = hits
        .iter()
        .map(|h| (h.range.clone(), h.name.clone(), h.is_container))
        .collect();

    for hit in hits.iter_mut() {
        let mut parent_refs: Vec<(usize, String)> = snapshot
            .iter()
            .filter(|(r, _, is_c)| {
                *is_c && r.start <= hit.range.start && hit.range.end <= r.end && *r != hit.range
            })
            .map(|(r, name, _)| (r.start, name.clone()))
            .collect();
        parent_refs.sort_by_key(|(start, _)| *start);
        let parent_simple: Vec<String> = parent_refs
            .into_iter()
            .map(|(_, n)| n.rsplit("::").next().unwrap_or(&n).to_string())
            .collect();
        if !parent_simple.is_empty() {
            let simple = hit.name.rsplit("::").next().unwrap_or(&hit.name).to_string();
            hit.name = format!("{}::{}", parent_simple.join("::"), simple);
        }
    }
}

fn order_with_containers(mut hits: Vec<SymbolHit>) -> Vec<SymbolHit> {
    hits.sort_by_key(|h| (h.range.start, h.range.end));
    hits
}

fn range_contains(outer: &Range<usize>, inner: &Range<usize>) -> bool {
    outer.start <= inner.start && inner.end <= outer.end && outer != inner
}

fn body_minus_members(source: &str, outer: &Range<usize>, members: &[Range<usize>]) -> String {
    let mut members = members.to_vec();
    members.sort_by_key(|r| r.start);
    let mut out = String::new();
    let mut cursor = outer.start;
    for m in members {
        if m.start > cursor && m.start <= outer.end {
            out.push_str(&source[cursor..m.start]);
        }
        cursor = cursor.max(m.end);
    }
    if cursor < outer.end {
        out.push_str(&source[cursor..outer.end]);
    }
    out
}

fn first_signature_line(source: &str, node_range: &Range<usize>) -> String {
    let text = &source[node_range.clone()];
    text.lines()
        .map(str::trim_end)
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn split_if_oversize(
    source: &str,
    cfg: &LangConfig,
    tree: &Tree,
    hit: &SymbolHit,
    body: &str,
    signature: &str,
) -> Vec<String> {
    if body.chars().count() <= OVERSIZE_CHAR_THRESHOLD {
        return vec![body.to_string()];
    }

    // Find statement boundary byte offsets within the node.
    let node = tree
        .root_node()
        .descendant_for_byte_range(hit.node_range.start, hit.node_range.end)
        .unwrap_or(tree.root_node());

    let mut stmt_ends: Vec<usize> = Vec::new();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if cfg.statement_kinds.contains(&n.kind())
            && n.start_byte() >= hit.node_range.start
            && n.end_byte() <= hit.node_range.end
        {
            stmt_ends.push(n.end_byte());
        }
        let mut c = n.walk();
        for child in n.children(&mut c) {
            stack.push(child);
        }
    }
    stmt_ends.sort_unstable();
    stmt_ends.dedup();

    if stmt_ends.is_empty() {
        // Fallback: hard split on newlines near the threshold.
        return hard_split(body, signature);
    }

    // Map body (which may be container-minus-members) is hard to align; for
    // oversize we split the raw node span including docs.
    let full = &source[hit.range.clone()];
    let base = hit.range.start;
    let mut fragments = Vec::new();
    let mut start = 0usize;
    let mut last_cut = 0usize;
    for end_abs in stmt_ends {
        let end = end_abs.saturating_sub(base);
        if end <= start || end > full.len() {
            continue;
        }
        let candidate = &full[start..end];
        if candidate.chars().count() >= OVERSIZE_CHAR_THRESHOLD / 2
            && candidate.chars().count() <= OVERSIZE_CHAR_THRESHOLD
        {
            fragments.push(prefix_sig(signature, candidate));
            start = end;
            last_cut = end;
        } else if candidate.chars().count() > OVERSIZE_CHAR_THRESHOLD {
            if last_cut > start {
                fragments.push(prefix_sig(signature, &full[start..last_cut]));
                start = last_cut;
            }
            // Force progress.
            fragments.push(prefix_sig(signature, &full[start..end]));
            start = end;
            last_cut = end;
        } else {
            last_cut = end;
        }
    }
    if start < full.len() {
        let rest = full[start..].trim();
        if !rest.is_empty() {
            if let Some(last) = fragments.last_mut() {
                if last.chars().count() + rest.chars().count() <= OVERSIZE_CHAR_THRESHOLD {
                    // merge into previous without double signature
                    let without = strip_sig_prefix(last, signature);
                    *last = prefix_sig(signature, &format!("{without}{rest}"));
                } else {
                    fragments.push(prefix_sig(signature, &full[start..]));
                }
            } else {
                fragments.push(prefix_sig(signature, &full[start..]));
            }
        }
    }
    if fragments.is_empty() {
        return hard_split(body, signature);
    }
    fragments
}

fn hard_split(body: &str, signature: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for line in body.lines() {
        if !buf.is_empty() && buf.chars().count() + line.chars().count() + 1 > OVERSIZE_CHAR_THRESHOLD
        {
            out.push(prefix_sig(signature, &buf));
            buf.clear();
        }
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(line);
    }
    if !buf.is_empty() {
        out.push(prefix_sig(signature, &buf));
    }
    if out.is_empty() {
        out.push(prefix_sig(signature, body));
    }
    out
}

fn prefix_sig(signature: &str, body: &str) -> String {
    if body.starts_with(signature) {
        body.to_string()
    } else {
        format!("{signature}\n{body}")
    }
}

fn strip_sig_prefix(text: &str, signature: &str) -> String {
    text.strip_prefix(signature)
        .map(|s| s.strip_prefix('\n').unwrap_or(s).to_string())
        .unwrap_or_else(|| text.to_string())
}

fn file_prelude(source: &str, covered: &[Range<usize>]) -> Option<String> {
    let (start, end) = prelude_span(source, covered)?;
    let text = source[start..end].trim();
    if text.chars().count() >= FILE_PRELUDE_MIN_CHARS {
        Some(text.to_string())
    } else {
        None
    }
}

fn prelude_span(source: &str, covered: &[Range<usize>]) -> Option<(usize, usize)> {
    if covered.is_empty() {
        let t = source.trim();
        if t.is_empty() {
            return None;
        }
        return Some((0, source.len()));
    }
    let mut sorted = covered.to_vec();
    sorted.sort_by_key(|r| r.start);
    let first = sorted[0].start;
    if first == 0 {
        return None;
    }
    Some((0, first))
}

fn attached_doc(source: &str, node: Node) -> (Option<usize>, Option<String>) {
    // Walk preceding siblings / previous named nodes that are comments.
    let mut doc_start = None;
    let mut first_line = None;
    let mut prev = node.prev_named_sibling();
    // Also consider contiguous comment lines immediately above via byte scan.
    let before = &source[..node.start_byte()];
    let mut end = before.len();
    // Trim trailing whitespace after last comment.
    while end > 0 && matches!(before.as_bytes()[end - 1], b' ' | b'\t' | b'\n' | b'\r') {
        end -= 1;
    }
    let slice = &before[..end];
    // Collect trailing comment block.
    let lines: Vec<&str> = slice.lines().collect();
    let mut idx = lines.len();
    let mut comment_lines: Vec<&str> = Vec::new();
    while idx > 0 {
        let line = lines[idx - 1].trim();
        if line.is_empty() && comment_lines.is_empty() {
            idx -= 1;
            continue;
        }
        if is_doc_comment_line(line) {
            comment_lines.push(lines[idx - 1]);
            idx -= 1;
        } else {
            break;
        }
    }
    if !comment_lines.is_empty() {
        comment_lines.reverse();
        // Find byte offset of first comment line.
        let first = comment_lines[0];
        if let Some(pos) = slice.rfind(first) {
            // Prefer leftmost occurrence of the block — use line-based search.
            let mut search_from = 0;
            let mut found = None;
            for (i, l) in lines.iter().enumerate() {
                if i >= idx {
                    found = Some(search_from);
                    break;
                }
                search_from += l.len() + 1; // +1 newline approx
            }
            let start = found.unwrap_or(pos);
            // Better: compute exact offset of lines[idx].
            let mut off = 0;
            for (i, l) in lines.iter().enumerate() {
                if i == idx {
                    doc_start = Some(off);
                    break;
                }
                off += l.len();
                if off < before.len() {
                    // account for newline
                    off += 1;
                }
            }
            let _ = start;
            first_line = comment_lines
                .first()
                .map(|l| l.trim().to_string());
        }
    }

    // Fallback using tree sibling comments.
    if doc_start.is_none() {
        while let Some(p) = prev {
            if p.kind().contains("comment") {
                doc_start = Some(p.start_byte());
                let text = &source[p.byte_range()];
                first_line = text.lines().next().map(|l| l.trim().to_string());
                prev = p.prev_named_sibling();
                if prev.is_some_and(|n| n.kind().contains("comment")) {
                    continue;
                }
                break;
            }
            break;
        }
    }

    (doc_start, first_line)
}

fn is_doc_comment_line(line: &str) -> bool {
    line.starts_with("///")
        || line.starts_with("//!")
        || line.starts_with("/**")
        || line.starts_with("/*!")
        || line.starts_with("*")
        || line.starts_with("*/")
        || line.starts_with("/*")
        || line.starts_with("//")
        || line.starts_with("#")
}

fn map_kind(kind: &str) -> String {
    match kind {
        "function_item" | "function_definition" | "function_declaration" | "method_definition"
        | "method_declaration" | "function" => "function".to_string(),
        "struct_item" | "struct_specifier" => "struct".to_string(),
        "type_declaration" | "type_spec" => "type".to_string(),
        "class_definition" | "class_declaration" | "class_specifier" => "class".to_string(),
        "impl_item" => "impl".to_string(),
        "mod_item" | "module" | "internal_module" => "module".to_string(),
        "enum_item" | "enum_specifier" => "enum".to_string(),
        "trait_item" | "interface_declaration" => "trait".to_string(),
        other => other.to_string(),
    }
}

fn byte_to_line(source: &str, byte: usize) -> u32 {
    let byte = byte.min(source.len());
    source[..byte].bytes().filter(|b| *b == b'\n').count() as u32 + 1
}

fn line_span(source: &str, start: usize, end: usize) -> (u32, u32) {
    let start_line = byte_to_line(source, start);
    let end_line = if end == 0 {
        start_line
    } else {
        byte_to_line(source, end.saturating_sub(1).max(start))
    };
    (start_line, end_line)
}

fn rust_config() -> Result<LangConfig, ChunkError> {
    let language: TsLanguage = tree_sitter_rust::LANGUAGE.into();
    let query = Query::new(
        &language,
        r#"
        (function_item name: (identifier) @name) @def
        (struct_item name: (type_identifier) @name) @def
        (enum_item name: (type_identifier) @name) @def
        (impl_item type: (type_identifier) @name) @def
        (mod_item name: (identifier) @name) @def
        (trait_item name: (type_identifier) @name) @def
        "#,
    )
    .map_err(|e| ChunkError::Parser(e.to_string()))?;
    Ok(LangConfig {
        language,
        query,
        container_kinds: &["impl_item", "mod_item", "struct_item", "trait_item", "enum_item"],
        statement_kinds: &[
            "let_declaration",
            "expression_statement",
            "return_expression",
            "if_expression",
            "match_expression",
            "for_expression",
            "while_expression",
            "loop_expression",
        ],
    })
}

fn python_config() -> Result<LangConfig, ChunkError> {
    let language: TsLanguage = tree_sitter_python::LANGUAGE.into();
    let query = Query::new(
        &language,
        r#"
        (function_definition name: (identifier) @name) @def
        (class_definition name: (identifier) @name) @def
        "#,
    )
    .map_err(|e| ChunkError::Parser(e.to_string()))?;
    Ok(LangConfig {
        language,
        query,
        container_kinds: &["class_definition"],
        statement_kinds: &[
            "expression_statement",
            "return_statement",
            "if_statement",
            "for_statement",
            "while_statement",
            "with_statement",
            "assignment",
        ],
    })
}

fn typescript_config() -> Result<LangConfig, ChunkError> {
    let language: TsLanguage = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    let query = Query::new(
        &language,
        r#"
        (function_declaration name: (identifier) @name) @def
        (class_declaration name: (type_identifier) @name) @def
        (method_definition name: (property_identifier) @name) @def
        "#,
    )
    .map_err(|e| ChunkError::Parser(e.to_string()))?;
    Ok(LangConfig {
        language,
        query,
        container_kinds: &["class_declaration"],
        statement_kinds: &[
            "expression_statement",
            "return_statement",
            "if_statement",
            "for_statement",
            "while_statement",
            "lexical_declaration",
            "variable_declaration",
        ],
    })
}

fn javascript_config() -> Result<LangConfig, ChunkError> {
    let language: TsLanguage = tree_sitter_javascript::LANGUAGE.into();
    let query = Query::new(
        &language,
        r#"
        (function_declaration name: (identifier) @name) @def
        (class_declaration name: (identifier) @name) @def
        (method_definition name: (property_identifier) @name) @def
        "#,
    )
    .map_err(|e| ChunkError::Parser(e.to_string()))?;
    Ok(LangConfig {
        language,
        query,
        container_kinds: &["class_declaration"],
        statement_kinds: &[
            "expression_statement",
            "return_statement",
            "if_statement",
            "for_statement",
            "while_statement",
            "lexical_declaration",
            "variable_declaration",
        ],
    })
}

fn go_config() -> Result<LangConfig, ChunkError> {
    let language: TsLanguage = tree_sitter_go::LANGUAGE.into();
    let query = Query::new(
        &language,
        r#"
        (function_declaration name: (identifier) @name) @def
        (method_declaration
          receiver: (parameter_list
            (parameter_declaration
              type: [
                (type_identifier) @recv
                (pointer_type (type_identifier) @recv)
              ]))
          name: (field_identifier) @name) @def
        (type_declaration (type_spec name: (type_identifier) @name) @def)
        "#,
    )
    .map_err(|e| ChunkError::Parser(e.to_string()))?;
    Ok(LangConfig {
        language,
        query,
        container_kinds: &["type_declaration", "type_spec"],
        statement_kinds: &[
            "expression_statement",
            "return_statement",
            "if_statement",
            "for_statement",
            "short_var_declaration",
            "assignment_statement",
        ],
    })
}

fn c_config() -> Result<LangConfig, ChunkError> {
    let language: TsLanguage = tree_sitter_c::LANGUAGE.into();
    let query = Query::new(
        &language,
        r#"
        (function_definition
          declarator: (function_declarator declarator: (identifier) @name)) @def
        (struct_specifier
          name: (type_identifier) @name
          body: (field_declaration_list)) @def
        "#,
    )
    .map_err(|e| ChunkError::Parser(e.to_string()))?;
    Ok(LangConfig {
        language,
        query,
        container_kinds: &["struct_specifier"],
        statement_kinds: &[
            "expression_statement",
            "return_statement",
            "if_statement",
            "for_statement",
            "while_statement",
            "declaration",
        ],
    })
}

fn cpp_config() -> Result<LangConfig, ChunkError> {
    let language: TsLanguage = tree_sitter_cpp::LANGUAGE.into();
    let query = Query::new(
        &language,
        r#"
        (function_definition
          declarator: (function_declarator declarator: [
            (identifier) @name
            (field_identifier) @name
          ])) @def
        (class_specifier
          name: (type_identifier) @name
          body: (field_declaration_list)) @def
        (struct_specifier
          name: (type_identifier) @name
          body: (field_declaration_list)) @def
        "#,
    )
    .map_err(|e| ChunkError::Parser(e.to_string()))?;
    Ok(LangConfig {
        language,
        query,
        container_kinds: &["class_specifier", "struct_specifier"],
        statement_kinds: &[
            "expression_statement",
            "return_statement",
            "if_statement",
            "for_statement",
            "while_statement",
            "declaration",
        ],
    })
}
