//! End-to-end fused search: embed, retrieve, fuse, assemble.

use std::time::Instant;

use half::f16;
use inference::{Embedder, Role};
use storage::{ChunkStore, RowId};

use crate::dense::DenseIndex;
use crate::fusion::{FusionConfig, fuse};
use crate::lexical::LexicalIndex;
use crate::result::{SearchResult, preview_from_body};
use crate::RetrievalError;
use crate::ScoredRow;

/// Which retrievers ran and which failed. Degradation is data, not a log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchDiagnostics {
    pub lexical_ok: bool,
    pub dense_ok: bool,
    pub lexical_error: Option<String>,
    pub dense_error: Option<String>,
    pub stage_millis: StageTimings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageTimings {
    pub embed: u64,
    pub lexical: u64,
    pub dense: u64,
    pub fuse: u64,
    pub assemble: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub diagnostics: SearchDiagnostics,
}

pub struct Searcher<'a> {
    lexical: &'a LexicalIndex,
    dense: &'a DenseIndex,
    store: &'a ChunkStore,
    embedder: &'a dyn Embedder,
    /// When true, run lexical and dense concurrently (step 8+).
    concurrent: bool,
}

impl<'a> Searcher<'a> {
    pub fn new(
        lexical: &'a LexicalIndex,
        dense: &'a DenseIndex,
        store: &'a ChunkStore,
        embedder: &'a dyn Embedder,
    ) -> Self {
        Self {
            lexical,
            dense,
            store,
            embedder,
            concurrent: false,
        }
    }

    /// Enable concurrent retriever dispatch. Used after the sequential path works.
    pub fn with_concurrent(mut self, concurrent: bool) -> Self {
        self.concurrent = concurrent;
        self
    }

    pub fn search(
        &self,
        query: &str,
        top_k: usize,
        config: &FusionConfig,
    ) -> Result<SearchResponse, RetrievalError> {
        self.run(query, top_k, config, Role::Query, true)
    }

    /// find_similar_code: embeds `code` as a document, skips the lexical path.
    pub fn search_similar(
        &self,
        code: &str,
        top_k: usize,
        config: &FusionConfig,
    ) -> Result<SearchResponse, RetrievalError> {
        self.run(code, top_k, config, Role::Document, false)
    }

    fn run(
        &self,
        text: &str,
        top_k: usize,
        config: &FusionConfig,
        role: Role,
        run_lexical: bool,
    ) -> Result<SearchResponse, RetrievalError> {
        let wall = Instant::now();

        let embed_started = Instant::now();
        let embeddings = self.embedder.embed(&[text], role)?;
        let embed_millis = embed_started.elapsed().as_millis() as u64;
        let query_vec = embeddings
            .into_iter()
            .next()
            .map(|e| e.vector)
            .unwrap_or_default();

        let (lexical_result, dense_result, lexical_millis, dense_millis) = if self.concurrent
            && run_lexical
        {
            dispatch_concurrent(
                self.lexical,
                self.dense,
                text,
                &query_vec,
                self.embedder.model_id(),
                config,
            )
        } else {
            dispatch_sequential(
                self.lexical,
                self.dense,
                text,
                &query_vec,
                self.embedder.model_id(),
                config,
                run_lexical,
            )
        };

        let mut lexical_ok = true;
        let mut dense_ok = true;
        let mut lexical_error = None;
        let mut dense_error = None;
        let mut lexical_rows = Vec::new();
        let mut dense_rows = Vec::new();

        match lexical_result {
            Ok(rows) => lexical_rows = rows,
            Err(error) => {
                lexical_ok = false;
                lexical_error = Some(error.to_string());
            }
        }
        match dense_result {
            Ok(rows) => dense_rows = rows,
            Err(error) => {
                dense_ok = false;
                dense_error = Some(error.to_string());
            }
        }

        if !lexical_ok && !dense_ok {
            return Err(RetrievalError::BothRetrieversFailed {
                lexical: lexical_error.unwrap_or_else(|| "unknown".into()),
                dense: dense_error.unwrap_or_else(|| "unknown".into()),
            });
        }

        let fuse_started = Instant::now();
        let fused = fuse(&lexical_rows, &dense_rows, config);
        let fuse_millis = fuse_started.elapsed().as_millis() as u64;

        let assemble_started = Instant::now();
        let results = assemble(self.store, self.lexical, &fused, top_k)?;
        let assemble_millis = assemble_started.elapsed().as_millis() as u64;

        let total = wall.elapsed().as_millis() as u64;
        Ok(SearchResponse {
            results,
            diagnostics: SearchDiagnostics {
                lexical_ok,
                dense_ok,
                lexical_error,
                dense_error,
                stage_millis: StageTimings {
                    embed: embed_millis,
                    lexical: lexical_millis,
                    dense: dense_millis,
                    fuse: fuse_millis,
                    assemble: assemble_millis,
                    total,
                },
            },
        })
    }
}

fn dispatch_sequential(
    lexical: &LexicalIndex,
    dense: &DenseIndex,
    text: &str,
    query_vec: &[f16],
    model_id: &str,
    config: &FusionConfig,
    run_lexical: bool,
) -> (
    Result<Vec<ScoredRow>, RetrievalError>,
    Result<Vec<ScoredRow>, RetrievalError>,
    u64,
    u64,
) {
    let lexical_result = if run_lexical {
        let started = Instant::now();
        let result = lexical.search(text, config.lexical_depth);
        let millis = started.elapsed().as_millis() as u64;
        (result, millis)
    } else {
        (Ok(Vec::new()), 0)
    };

    let dense_started = Instant::now();
    let dense_result = dense.search(query_vec, model_id, config.dense_depth);
    let dense_millis = dense_started.elapsed().as_millis() as u64;

    (
        lexical_result.0,
        dense_result,
        lexical_result.1,
        dense_millis,
    )
}

fn dispatch_concurrent(
    lexical: &LexicalIndex,
    dense: &DenseIndex,
    text: &str,
    query_vec: &[f16],
    model_id: &str,
    config: &FusionConfig,
) -> (
    Result<Vec<ScoredRow>, RetrievalError>,
    Result<Vec<ScoredRow>, RetrievalError>,
    u64,
    u64,
) {
    // Placeholder for step 8 — currently sequential so concurrency tests fail.
    dispatch_sequential(lexical, dense, text, query_vec, model_id, config, true)
}

fn assemble(
    store: &ChunkStore,
    lexical: &LexicalIndex,
    fused: &[crate::fusion::FusedRow],
    top_k: usize,
) -> Result<Vec<SearchResult>, RetrievalError> {
    let selected: Vec<_> = fused.iter().take(top_k).collect();
    let row_ids: Vec<RowId> = selected.iter().map(|row| row.row).collect();
    let records = store.get_many(&row_ids)?;
    let bodies = lexical.bodies(&row_ids)?;

    let mut results = Vec::with_capacity(selected.len());
    for (index, fused_row) in selected.iter().enumerate() {
        let Some(record) = records.get(index).and_then(|r| r.as_ref()) else {
            continue;
        };
        let body = bodies.get(index).and_then(|b| b.as_ref()).map(String::as_str).unwrap_or("");
        results.push(SearchResult {
            file: record.file.clone(),
            symbol: record.symbol.clone(),
            signature: record.signature.clone(),
            doc: record.doc_first_line.clone(),
            preview: preview_from_body(body),
            lines: [record.line_start, record.line_end],
            lexical_score: fused_row.lexical.score,
            dense_score: fused_row.dense.score,
            fused_score: fused_row.fused_score,
        });
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use inference::{Embedder, MockEmbedder, Role};
    use storage::{ChunkRecord, ChunkStore, ContentHash};
    use tempfile::tempdir;

    use super::Searcher;
    use crate::dense::{DenseBackend, DenseIndex};
    use crate::fusion::FusionConfig;
    use crate::lexical::{LexicalDoc, LexicalIndex};
    use crate::result::PREVIEW_MAX_BYTES;

    const DIMS: u32 = 64;

    struct Fixture {
        _dir: tempfile::TempDir,
        store: ChunkStore,
        lexical: LexicalIndex,
        dense: DenseIndex,
        embedder: MockEmbedder,
    }

    fn record(symbol: &str, file: &str, body_hash: &str) -> ChunkRecord {
        ChunkRecord {
            repository: "fixture".into(),
            file: file.into(),
            language: "rust".into(),
            symbol: symbol.into(),
            symbol_type: "function".into(),
            signature: format!("fn {symbol}()"),
            doc_first_line: Some(format!("docs for {symbol}")),
            line_start: 10,
            line_end: 40,
            content_hash: ContentHash::of(body_hash.as_bytes()),
        }
    }

    fn build_fixture() -> Fixture {
        let dir = tempdir().unwrap();
        let embedder = MockEmbedder::new(DIMS);
        let texts = [
            "parse an incoming packet frame",
            "clamp_decoder_timestamps to monotonic order unique_marker_xyz",
            "render the final animation frame",
            "flush pending disk writes",
        ];
        let symbols = [
            "parse_packet",
            "clamp_decoder_timestamps",
            "render_frame",
            "flush_writes",
        ];
        let embeddings = embedder.embed(&texts, Role::Document).unwrap();

        let mut store = ChunkStore::create(dir.path(), DIMS, embedder.model_id()).unwrap();
        let chunks: Vec<_> = texts
            .iter()
            .zip(embeddings)
            .enumerate()
            .map(|(i, (text, embedding))| {
                (
                    record(symbols[i], &format!("src/{}.rs", symbols[i]), text),
                    embedding.vector,
                )
            })
            .collect();
        let rows = store.insert_batch(&chunks).unwrap();

        let docs: Vec<_> = texts
            .iter()
            .enumerate()
            .map(|(i, text)| {
                (
                    rows[i],
                    LexicalDoc {
                        symbol: symbols[i].into(),
                        signature: format!("fn {}()", symbols[i]),
                        doc_first_line: Some(format!("docs for {}", symbols[i])),
                        file: format!("src/{}.rs", symbols[i]),
                        body: (*text).into(),
                    },
                )
            })
            .collect();
        let mut lexical = LexicalIndex::open(dir.path()).unwrap();
        lexical.add_batch(&docs).unwrap();
        lexical.commit().unwrap();

        let dense = DenseIndex::from_store(&store, DenseBackend::Cpu).unwrap();
        Fixture {
            _dir: dir,
            store,
            lexical,
            dense,
            embedder,
        }
    }

    #[test]
    fn search_returns_known_answer_at_rank_one_with_fields() {
        let fixture = build_fixture();
        let searcher = Searcher::new(
            &fixture.lexical,
            &fixture.dense,
            &fixture.store,
            &fixture.embedder,
        );
        let config = FusionConfig::default();
        let response = searcher
            .search("clamp_decoder_timestamps", 5, &config)
            .unwrap();

        assert!(response.diagnostics.lexical_ok);
        assert!(response.diagnostics.dense_ok);
        assert!(!response.results.is_empty());
        let top = &response.results[0];
        assert_eq!(top.symbol, "clamp_decoder_timestamps");
        assert_eq!(top.file, "src/clamp_decoder_timestamps.rs");
        assert!(!top.signature.is_empty());
        assert!(top.doc.is_some());
        assert_eq!(top.lines, [10, 40]);
        assert!(!top.preview.is_empty());
        assert!(top.preview.len() <= PREVIEW_MAX_BYTES);
        assert!(top.fused_score > 0.0);
    }
}
