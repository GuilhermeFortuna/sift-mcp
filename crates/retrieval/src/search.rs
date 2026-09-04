//! End-to-end fused search: embed, retrieve, fuse, assemble.

use std::time::Instant;

use half::f16;
use inference::{Embedder, Role};
use storage::{ChunkStore, RowId};

use crate::RetrievalError;
use crate::ScoredRow;
use crate::dense::DenseIndex;
use crate::fusion::{FusionConfig, fuse};
use crate::lexical::LexicalIndex;
use crate::result::{
    SearchDiagnostics, SearchResponse, SearchResult, StageTimings, preview_from_body,
};

pub struct Searcher<'a> {
    lexical: &'a LexicalIndex,
    dense: &'a DenseIndex,
    store: &'a ChunkStore,
    embedder: &'a dyn Embedder,
    /// When true, run lexical and dense concurrently.
    concurrent: bool,
    /// Test-only artificial delay applied inside each retriever path.
    #[cfg(test)]
    retriever_delay: Option<std::time::Duration>,
    #[cfg(test)]
    force_lexical_error: Option<&'static str>,
    #[cfg(test)]
    force_dense_error: Option<&'static str>,
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
            concurrent: true,
            #[cfg(test)]
            retriever_delay: None,
            #[cfg(test)]
            force_lexical_error: None,
            #[cfg(test)]
            force_dense_error: None,
        }
    }

    /// Enable or disable concurrent retriever dispatch.
    pub fn with_concurrent(mut self, concurrent: bool) -> Self {
        self.concurrent = concurrent;
        self
    }

    #[cfg(test)]
    fn with_retriever_delay(mut self, delay: std::time::Duration) -> Self {
        self.retriever_delay = Some(delay);
        self
    }

    #[cfg(test)]
    fn with_forced_lexical_error(mut self, message: &'static str) -> Self {
        self.force_lexical_error = Some(message);
        self
    }

    #[cfg(test)]
    fn with_forced_dense_error(mut self, message: &'static str) -> Self {
        self.force_dense_error = Some(message);
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

        let delay = {
            #[cfg(test)]
            {
                self.retriever_delay
            }
            #[cfg(not(test))]
            {
                None
            }
        };
        let force_lexical = {
            #[cfg(test)]
            {
                self.force_lexical_error
            }
            #[cfg(not(test))]
            {
                None
            }
        };
        let force_dense = {
            #[cfg(test)]
            {
                self.force_dense_error
            }
            #[cfg(not(test))]
            {
                None
            }
        };

        let hooks = DispatchHooks {
            delay,
            force_lexical,
            force_dense,
        };
        let request = DispatchRequest {
            lexical: self.lexical,
            dense: self.dense,
            text,
            query_vec: &query_vec,
            model_id: self.embedder.model_id(),
            config,
            run_lexical,
            hooks,
        };

        let outcome = if self.concurrent && run_lexical {
            dispatch_concurrent(request)
        } else {
            dispatch_sequential(request)
        };
        let lexical_result = outcome.lexical;
        let dense_result = outcome.dense;
        let lexical_millis = outcome.lexical_millis;
        let dense_millis = outcome.dense_millis;

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

fn maybe_delay(delay: Option<std::time::Duration>) {
    if let Some(delay) = delay {
        std::thread::sleep(delay);
    }
}

fn maybe_force_lexical_error(forced: Option<&str>) -> Result<(), RetrievalError> {
    if let Some(message) = forced {
        return Err(RetrievalError::Tantivy(message.to_owned()));
    }
    Ok(())
}

fn maybe_force_dense_error(forced: Option<&str>) -> Result<(), RetrievalError> {
    if let Some(message) = forced {
        return Err(RetrievalError::Dense(message.to_owned()));
    }
    Ok(())
}

struct DispatchRequest<'a> {
    lexical: &'a LexicalIndex,
    dense: &'a DenseIndex,
    text: &'a str,
    query_vec: &'a [f16],
    model_id: &'a str,
    config: &'a FusionConfig,
    run_lexical: bool,
    hooks: DispatchHooks<'a>,
}

struct DispatchHooks<'a> {
    delay: Option<std::time::Duration>,
    force_lexical: Option<&'a str>,
    force_dense: Option<&'a str>,
}

struct DispatchOutcome {
    lexical: Result<Vec<ScoredRow>, RetrievalError>,
    dense: Result<Vec<ScoredRow>, RetrievalError>,
    lexical_millis: u64,
    dense_millis: u64,
}

fn dispatch_sequential(request: DispatchRequest<'_>) -> DispatchOutcome {
    let lexical_pair = if request.run_lexical {
        let started = Instant::now();
        let result = (|| {
            maybe_force_lexical_error(request.hooks.force_lexical)?;
            maybe_delay(request.hooks.delay);
            request
                .lexical
                .search(request.text, request.config.lexical_depth)
        })();
        (result, started.elapsed().as_millis() as u64)
    } else {
        (Ok(Vec::new()), 0)
    };

    let dense_started = Instant::now();
    let dense_result = (|| {
        maybe_force_dense_error(request.hooks.force_dense)?;
        maybe_delay(request.hooks.delay);
        request.dense.search(
            request.query_vec,
            request.model_id,
            request.config.dense_depth,
        )
    })();
    DispatchOutcome {
        lexical: lexical_pair.0,
        dense: dense_result,
        lexical_millis: lexical_pair.1,
        dense_millis: dense_started.elapsed().as_millis() as u64,
    }
}

fn dispatch_concurrent(request: DispatchRequest<'_>) -> DispatchOutcome {
    let handle = request.lexical.search_handle();
    let text_owned = request.text.to_owned();
    let query_owned = request.query_vec.to_vec();
    let model_owned = request.model_id.to_owned();
    let lexical_depth = request.config.lexical_depth;
    let dense_depth = request.config.dense_depth;
    let delay = request.hooks.delay;
    let force_lexical = request.hooks.force_lexical;
    let force_dense = request.hooks.force_dense;
    let dense = request.dense;

    let mut lexical_out = None;
    let mut dense_out = None;
    let mut lexical_millis = 0;
    let mut dense_millis = 0;

    std::thread::scope(|scope| {
        let lexical_thread = scope.spawn(|| {
            let started = Instant::now();
            let result = (|| {
                maybe_force_lexical_error(force_lexical)?;
                maybe_delay(delay);
                handle.search(&text_owned, lexical_depth)
            })();
            (result, started.elapsed().as_millis() as u64)
        });
        let dense_thread = scope.spawn(|| {
            let started = Instant::now();
            let result = (|| {
                maybe_force_dense_error(force_dense)?;
                maybe_delay(delay);
                dense.search(&query_owned, &model_owned, dense_depth)
            })();
            (result, started.elapsed().as_millis() as u64)
        });
        let (lex_result, lex_ms) = lexical_thread.join().expect("lexical thread");
        let (den_result, den_ms) = dense_thread.join().expect("dense thread");
        lexical_out = Some(lex_result);
        dense_out = Some(den_result);
        lexical_millis = lex_ms;
        dense_millis = den_ms;
    });

    DispatchOutcome {
        lexical: lexical_out.expect("lexical result"),
        dense: dense_out.expect("dense result"),
        lexical_millis,
        dense_millis,
    }
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
        let body = bodies
            .get(index)
            .and_then(|b| b.as_ref())
            .map(String::as_str)
            .unwrap_or("");
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

    #[test]
    fn retrievers_run_concurrently_when_enabled() {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};

        let fixture = build_fixture();
        let delay = Duration::from_millis(40);
        let lex_times = Mutex::new(None);
        let den_times = Mutex::new(None);

        // Instrument via the same concurrent join used by Searcher: overlapping
        // sleeps prove the paths are not serialized.
        let handle = fixture.lexical.search_handle();
        let dense = &fixture.dense;
        let query_vec = fixture.embedder.query_matching("clamp_decoder_timestamps");
        let model_id = fixture.embedder.model_id().to_owned();

        let wall = Instant::now();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let enter = Instant::now();
                std::thread::sleep(delay);
                let _ = handle.search("clamp_decoder_timestamps", 10);
                let exit = Instant::now();
                *lex_times.lock().unwrap() = Some((enter, exit));
            });
            scope.spawn(|| {
                let enter = Instant::now();
                std::thread::sleep(delay);
                let _ = dense.search(&query_vec, &model_id, 10);
                let exit = Instant::now();
                *den_times.lock().unwrap() = Some((enter, exit));
            });
        });
        let wall_ms = wall.elapsed().as_millis() as u64;

        let (lex_enter, lex_exit) = lex_times.lock().unwrap().unwrap();
        let (den_enter, den_exit) = den_times.lock().unwrap().unwrap();
        let overlap = lex_enter < den_exit && den_enter < lex_exit;
        assert!(overlap, "retriever intervals must overlap");

        let searcher = Searcher::new(
            &fixture.lexical,
            &fixture.dense,
            &fixture.store,
            &fixture.embedder,
        )
        .with_retriever_delay(delay);
        let response = searcher
            .search("clamp_decoder_timestamps", 5, &FusionConfig::default())
            .unwrap();
        let stages = &response.diagnostics.stage_millis;
        assert!(
            stages.total < stages.lexical + stages.dense,
            "total {} should be less than lexical+dense {}+{}",
            stages.total,
            stages.lexical,
            stages.dense
        );
        // Wall of the instrumented pair should also beat serialized 2*delay.
        assert!(
            wall_ms < (delay.as_millis() as u64) * 2,
            "wall {wall_ms} should be under 2*delay"
        );
    }

    #[test]
    fn metadata_resolved_with_one_get_many_per_search() {
        let fixture = build_fixture();
        let _ = fixture.store.take_statements_prepared();

        let searcher = Searcher::new(
            &fixture.lexical,
            &fixture.dense,
            &fixture.store,
            &fixture.embedder,
        );
        let config = FusionConfig {
            lexical_depth: 50,
            dense_depth: 50,
            rrf_k: 60.0,
        };
        let _ = searcher
            .search("clamp_decoder_timestamps", 10, &config)
            .unwrap();
        let first = fixture.store.take_statements_prepared();
        assert!(first > 0, "expected get_many to prepare statements");

        let _ = searcher
            .search("clamp_decoder_timestamps", 3, &config)
            .unwrap();
        let second = fixture.store.take_statements_prepared();
        assert_eq!(
            first, second,
            "prepare count must be independent of candidate/result count (one get_many)"
        );

        // A direct get_many of varying sizes must match the same prepare budget.
        let _ = fixture.store.take_statements_prepared();
        let rows: Vec<_> = (0..4).map(storage::RowId::from_u64).collect();
        fixture.store.get_many(&rows).unwrap();
        let direct = fixture.store.take_statements_prepared();
        assert_eq!(first, direct);
    }

    #[test]
    fn degrades_when_one_retriever_fails() {
        let fixture = build_fixture();
        let config = FusionConfig::default();

        let dense_failed = Searcher::new(
            &fixture.lexical,
            &fixture.dense,
            &fixture.store,
            &fixture.embedder,
        )
        .with_forced_dense_error("dense exploded");
        let response = dense_failed
            .search("clamp_decoder_timestamps", 5, &config)
            .expect("degraded search must return Ok");
        assert!(response.diagnostics.lexical_ok);
        assert!(!response.diagnostics.dense_ok);
        assert!(
            response
                .diagnostics
                .dense_error
                .as_deref()
                .is_some_and(|e| e.contains("dense exploded"))
        );
        assert!(
            !response.results.is_empty(),
            "lexical-only results expected"
        );

        let lexical_failed = Searcher::new(
            &fixture.lexical,
            &fixture.dense,
            &fixture.store,
            &fixture.embedder,
        )
        .with_forced_lexical_error("lexical exploded");
        let response = lexical_failed
            .search("clamp_decoder_timestamps", 5, &config)
            .expect("degraded search must return Ok");
        assert!(!response.diagnostics.lexical_ok);
        assert!(response.diagnostics.dense_ok);
        assert!(
            response
                .diagnostics
                .lexical_error
                .as_deref()
                .is_some_and(|e| e.contains("lexical exploded"))
        );
        assert!(!response.results.is_empty(), "dense-only results expected");
    }

    #[test]
    fn both_retrievers_failing_returns_error() {
        let fixture = build_fixture();
        let searcher = Searcher::new(
            &fixture.lexical,
            &fixture.dense,
            &fixture.store,
            &fixture.embedder,
        )
        .with_forced_lexical_error("lexical down")
        .with_forced_dense_error("dense down");
        let err = searcher
            .search("clamp_decoder_timestamps", 5, &FusionConfig::default())
            .expect_err("both failing must be Err");
        match err {
            crate::RetrievalError::BothRetrieversFailed { lexical, dense } => {
                assert!(lexical.contains("lexical down"));
                assert!(dense.contains("dense down"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn exact_identifier_recovered_via_lexical_despite_poor_dense() {
        let dir = tempdir().unwrap();
        let embedder = MockEmbedder::new(DIMS);
        // Dense-friendly distractors use bodies near the query embedding space;
        // the defining chunk uses an unrelated body so dense ranks it poorly.
        let identifier = "parseHTTPResponse";
        let mut texts = vec![
            "return status from parsed headers".to_string(), // defining body (dissimilar)
        ];
        for i in 0..8 {
            texts.push(format!("{identifier} distractor body variant {i}"));
        }
        let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let embeddings = embedder.embed(&text_refs, Role::Document).unwrap();

        let mut store = ChunkStore::create(dir.path(), DIMS, embedder.model_id()).unwrap();
        let mut chunks = Vec::new();
        chunks.push((
            record(identifier, "src/http.rs", &texts[0]),
            embeddings[0].vector.clone(),
        ));
        for i in 1..texts.len() {
            let symbol = format!("distractor{i}");
            chunks.push((
                record(&symbol, &format!("src/{symbol}.rs"), &texts[i]),
                embeddings[i].vector.clone(),
            ));
        }
        let rows = store.insert_batch(&chunks).unwrap();

        let mut docs = vec![(
            rows[0],
            LexicalDoc {
                symbol: identifier.into(),
                signature: format!("fn {identifier}()"),
                doc_first_line: Some("Parse an HTTP response".into()),
                file: "src/http.rs".into(),
                body: texts[0].clone(),
            },
        )];
        for i in 1..texts.len() {
            let symbol = format!("distractor{i}");
            docs.push((
                rows[i],
                LexicalDoc {
                    symbol: symbol.clone(),
                    signature: format!("fn {symbol}()"),
                    doc_first_line: None,
                    file: format!("src/{symbol}.rs"),
                    body: texts[i].clone(),
                },
            ));
        }
        let mut lexical = LexicalIndex::open(dir.path()).unwrap();
        lexical.add_batch(&docs).unwrap();
        lexical.commit().unwrap();
        let dense = DenseIndex::from_store(&store, DenseBackend::Cpu).unwrap();

        // Confirm dense alone ranks the defining chunk poorly (outside top 5).
        let query_vec = embedder.embed(&[identifier], Role::Query).unwrap()[0]
            .vector
            .clone();
        let dense_only = dense.search(&query_vec, embedder.model_id(), 9).unwrap();
        let dense_rank = dense_only.iter().position(|r| r.row == rows[0]);
        assert!(
            dense_rank.is_none_or(|rank| rank >= 5),
            "dense should rank identifier poorly, got {dense_rank:?}"
        );

        let searcher = Searcher::new(&lexical, &dense, &store, &embedder);
        let response = searcher
            .search(identifier, 5, &FusionConfig::default())
            .unwrap();
        let found = response
            .results
            .iter()
            .take(5)
            .any(|r| r.symbol == identifier);
        assert!(
            found,
            "lexical contribution must surface {identifier} in top 5; got {:?}",
            response
                .results
                .iter()
                .map(|r| r.symbol.as_str())
                .collect::<Vec<_>>()
        );
        let hit = response
            .results
            .iter()
            .find(|r| r.symbol == identifier)
            .unwrap();
        assert!(hit.lexical_score.is_some());
    }

    struct RecordingEmbedder {
        inner: MockEmbedder,
        roles: std::sync::Mutex<Vec<Role>>,
    }

    impl RecordingEmbedder {
        fn new(dims: u32) -> Self {
            Self {
                inner: MockEmbedder::new(dims),
                roles: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl Embedder for RecordingEmbedder {
        fn model_id(&self) -> &str {
            self.inner.model_id()
        }

        fn dims(&self) -> u32 {
            self.inner.dims()
        }

        fn embed(
            &self,
            texts: &[&str],
            role: Role,
        ) -> Result<Vec<inference::Embedding>, inference::InferError> {
            self.roles.lock().unwrap().push(role);
            self.inner.embed(texts, role)
        }
    }

    #[test]
    fn search_similar_uses_document_role_and_ranks_snippet() {
        let dir = tempdir().unwrap();
        let embedder = RecordingEmbedder::new(DIMS);
        let texts = [
            "alpha packet parser body",
            "let mut t = pts; if t < last { t = last + 1; }",
            "gamma renderer body",
        ];
        let symbols = ["alpha", "normalize_timestamp", "gamma"];
        let embeddings = embedder.embed(&texts, Role::Document).unwrap();
        embedder.roles.lock().unwrap().clear();

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
                        doc_first_line: None,
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

        let searcher = Searcher::new(&lexical, &dense, &store, &embedder);
        let snippet = texts[1];
        let response = searcher
            .search_similar(snippet, 3, &FusionConfig::default())
            .unwrap();

        assert_eq!(response.results[0].symbol, "normalize_timestamp");
        assert!(response.diagnostics.lexical_ok);
        assert!(response.diagnostics.dense_ok);
        let roles = embedder.roles.lock().unwrap().clone();
        assert_eq!(roles, vec![Role::Document]);
        // Lexical path skipped: no lexical scores on similar search.
        assert!(response.results[0].lexical_score.is_none());
        assert!(response.results[0].dense_score.is_some());
    }

    #[test]
    fn results_never_carry_full_file_or_body_past_preview() {
        let dir = tempdir().unwrap();
        let embedder = MockEmbedder::new(DIMS);
        let large_body = format!("{}\n{}", "fn huge() {\n", "x".repeat(PREVIEW_MAX_BYTES * 4));
        let whole_file = format!(
            "// file header\n{large_body}\nfn other() {{}}\n{}",
            "y".repeat(2000)
        );
        assert!(whole_file.len() > PREVIEW_MAX_BYTES);

        let embeddings = embedder
            .embed(&[large_body.as_str()], Role::Document)
            .unwrap();
        let mut store = ChunkStore::create(dir.path(), DIMS, embedder.model_id()).unwrap();
        let rows = store
            .insert_batch(&[(
                record("huge", "src/huge.rs", &large_body),
                embeddings[0].vector.clone(),
            )])
            .unwrap();
        let mut lexical = LexicalIndex::open(dir.path()).unwrap();
        lexical
            .add_batch(&[(
                rows[0],
                LexicalDoc {
                    symbol: "huge".into(),
                    signature: "fn huge()".into(),
                    doc_first_line: None,
                    file: "src/huge.rs".into(),
                    body: large_body.clone(),
                },
            )])
            .unwrap();
        lexical.commit().unwrap();
        let dense = DenseIndex::from_store(&store, DenseBackend::Cpu).unwrap();

        let searcher = Searcher::new(&lexical, &dense, &store, &embedder);
        let response = searcher
            .search("huge", 5, &FusionConfig::default())
            .unwrap();
        assert!(!response.results.is_empty());
        for result in &response.results {
            assert!(
                result.preview.len() <= PREVIEW_MAX_BYTES,
                "preview {} exceeds bound",
                result.preview.len()
            );
            assert_ne!(result.preview, whole_file);
            assert!(
                result.preview.len() < large_body.len(),
                "preview must not be the full symbol body"
            );
            // Serialized JSON must not embed the full body either.
            let json = serde_json::to_string(result).unwrap();
            assert!(!json.contains(&large_body));
            assert!(json.len() < whole_file.len());
        }
    }
}
