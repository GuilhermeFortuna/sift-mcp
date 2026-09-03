//! Lexical retrieval backed by Tantivy.

use std::path::Path;

use tantivy::collector::{Collector, SegmentCollector, TopDocs, TopNComputer};
use tantivy::query::{BooleanQuery, BoostQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    FAST, Field, INDEXED, IndexRecordOption, STORED, Schema, TantivyDocument, TextFieldIndexing,
    TextOptions, Value,
};
use tantivy::tokenizer::TextAnalyzer;
use tantivy::tokenizer::{TokenStream, Tokenizer};
use tantivy::{
    DocAddress, Index, IndexReader, IndexWriter, Score, SegmentOrdinal, SegmentReader, Term,
};

use storage::RowId;

use crate::error::RetrievalError;
use crate::tokenize::CodeTokenizer;

/// BM25 boost applied to symbol-name matches.
pub const SYMBOL_BOOST: f32 = 4.0;
/// BM25 boost applied to signature matches.
pub const SIGNATURE_BOOST: f32 = 3.0;
/// BM25 boost applied to documentation matches.
pub const DOC_BOOST: f32 = 2.0;
/// BM25 boost applied to body matches.
pub const BODY_BOOST: f32 = 1.0;
/// BM25 boost applied to file-path matches.
pub const FILE_BOOST: f32 = 0.5;

const INDEX_DIRECTORY: &str = "lexical";
const TOKENIZER_NAME: &str = "code";
const WRITER_MEMORY_BYTES: usize = 50_000_000;

pub struct LexicalDoc {
    pub symbol: String,
    pub signature: String,
    pub doc_first_line: Option<String>,
    pub file: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
/// A row ranked by raw, non-negative BM25 score.
///
/// Scores are unnormalized and are meaningful for ordering within one query;
/// they are not comparable across different queries.
pub struct ScoredRow {
    pub row: RowId,
    pub score: f32,
}

#[derive(Clone, Copy)]
struct Fields {
    row: Field,
    symbol: Field,
    signature: Field,
    doc: Field,
    file: Field,
    body: Field,
}

pub struct LexicalIndex {
    writer: IndexWriter,
    reader: IndexReader,
    fields: Fields,
}

struct RowTopCollector {
    limit: usize,
}

struct RowTopSegmentCollector {
    top: TopNComputer<Score, (u64, DocAddress)>,
    rows: tantivy::fastfield::Column<u64>,
    segment_ord: SegmentOrdinal,
}

impl Collector for RowTopCollector {
    type Fruit = Vec<(Score, (u64, DocAddress))>;
    type Child = RowTopSegmentCollector;

    fn for_segment(
        &self,
        segment_local_id: SegmentOrdinal,
        segment: &SegmentReader,
    ) -> tantivy::Result<Self::Child> {
        Ok(RowTopSegmentCollector {
            top: TopNComputer::new(self.limit),
            rows: segment.fast_fields().u64("row")?,
            segment_ord: segment_local_id,
        })
    }

    fn requires_scoring(&self) -> bool {
        true
    }

    fn merge_fruits(
        &self,
        segment_fruits: Vec<<Self::Child as SegmentCollector>::Fruit>,
    ) -> tantivy::Result<Self::Fruit> {
        let mut top: TopNComputer<Score, (u64, DocAddress)> = TopNComputer::new(self.limit);
        for fruit in segment_fruits {
            for (score, row_and_address) in fruit {
                top.push(score, row_and_address);
            }
        }
        Ok(top
            .into_sorted_vec()
            .into_iter()
            .map(|document| (document.feature, document.doc))
            .collect())
    }
}

impl SegmentCollector for RowTopSegmentCollector {
    type Fruit = Vec<(Score, (u64, DocAddress))>;

    fn collect(&mut self, doc: tantivy::DocId, score: Score) {
        self.top.push(
            score,
            (
                self.rows.values.get_val(doc),
                DocAddress {
                    segment_ord: self.segment_ord,
                    doc_id: doc,
                },
            ),
        );
    }

    fn harvest(self) -> Self::Fruit {
        self.top
            .into_sorted_vec()
            .into_iter()
            .map(|document| (document.feature, document.doc))
            .collect()
    }
}

impl LexicalIndex {
    pub fn open(dir: &Path) -> Result<Self, RetrievalError> {
        let index_dir = dir.join(INDEX_DIRECTORY);
        std::fs::create_dir_all(&index_dir)?;
        let index = if index_dir.join("meta.json").exists() {
            Index::open_in_dir(&index_dir).map_err(tantivy_error)?
        } else {
            Index::create_in_dir(&index_dir, build_schema()).map_err(tantivy_error)?
        };
        index.tokenizers().register(
            TOKENIZER_NAME,
            TextAnalyzer::builder(CodeTokenizer::new()).build(),
        );
        let fields = fields(&index.schema())?;
        let writer = index
            .writer_with_num_threads(1, WRITER_MEMORY_BYTES)
            .map_err(tantivy_error)?;
        let reader = index.reader().map_err(tantivy_error)?;
        Ok(Self {
            writer,
            reader,
            fields,
        })
    }

    pub fn add_batch(&mut self, docs: &[(RowId, LexicalDoc)]) -> Result<(), RetrievalError> {
        for (row, doc) in docs {
            self.writer
                .delete_term(Term::from_field_u64(self.fields.row, row.get()));
            self.writer
                .add_document(to_document(self.fields, *row, doc))
                .map_err(tantivy_error)?;
        }
        Ok(())
    }

    pub fn remove(&mut self, rows: &[RowId]) -> Result<(), RetrievalError> {
        for row in rows {
            self.writer
                .delete_term(Term::from_field_u64(self.fields.row, row.get()));
        }
        Ok(())
    }

    pub fn update_file_paths(&mut self, paths: &[(RowId, String)]) -> Result<(), RetrievalError> {
        for (row, path) in paths {
            let Some(old_document) = self.load_document(*row)? else {
                continue;
            };
            let Some(mut document) = lexical_doc(&old_document, self.fields) else {
                continue;
            };
            document.file = path.clone();
            self.writer
                .delete_term(Term::from_field_u64(self.fields.row, row.get()));
            self.writer
                .add_document(to_document(self.fields, *row, &document))
                .map_err(tantivy_error)?;
        }
        Ok(())
    }

    pub fn renumber(&mut self, mapping: &[(RowId, RowId)]) -> Result<(), RetrievalError> {
        for (old_row, new_row) in mapping {
            let Some(old_document) = self.load_document(*old_row)? else {
                continue;
            };
            let Some(document) = lexical_doc(&old_document, self.fields) else {
                continue;
            };
            self.writer
                .delete_term(Term::from_field_u64(self.fields.row, old_row.get()));
            self.writer
                .delete_term(Term::from_field_u64(self.fields.row, new_row.get()));
            self.writer
                .add_document(to_document(self.fields, *new_row, &document))
                .map_err(tantivy_error)?;
        }
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), RetrievalError> {
        self.writer.commit().map_err(tantivy_error)?;
        self.reader.reload().map_err(tantivy_error)?;
        Ok(())
    }

    /// Search with permissive lexical disjunction semantics.
    ///
    /// Results use Tantivy's raw BM25 score, including the field boosts above.
    /// Scores are non-negative and unnormalized, so callers must not compare
    /// scores across queries.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<ScoredRow>, RetrievalError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(parsed) = self.build_query(query) else {
            return Ok(Vec::new());
        };
        let searcher = self.reader.searcher();
        let top_docs = searcher
            .search(&parsed, &RowTopCollector { limit })
            .map_err(tantivy_error)?;
        let mut rows = Vec::with_capacity(top_docs.len());
        for (score, (row, _address)) in top_docs {
            rows.push(ScoredRow {
                row: RowId::from_u64(row),
                score,
            });
        }
        Ok(rows)
    }

    fn build_query(&self, query: &str) -> Option<Box<dyn Query>> {
        let mut tokenizer = CodeTokenizer::new();
        let mut stream = tokenizer.token_stream(query);
        let mut terms = Vec::new();
        stream.process(&mut |token| {
            if !terms.iter().any(|term: &String| term == &token.text) {
                terms.push(token.text.clone());
            }
        });
        if terms.is_empty() {
            return None;
        }

        let fields = [
            (self.fields.symbol, SYMBOL_BOOST),
            (self.fields.signature, SIGNATURE_BOOST),
            (self.fields.doc, DOC_BOOST),
            (self.fields.body, BODY_BOOST),
            (self.fields.file, FILE_BOOST),
        ];
        let clauses = terms
            .iter()
            .flat_map(|term| {
                fields.iter().map(move |(field, boost)| {
                    let term_query = TermQuery::new(
                        tantivy::Term::from_field_text(*field, term),
                        IndexRecordOption::WithFreqsAndPositions,
                    );
                    (
                        Occur::Should,
                        Box::new(BoostQuery::new(Box::new(term_query), *boost)) as Box<dyn Query>,
                    )
                })
            })
            .collect();
        Some(Box::new(BooleanQuery::new(clauses)))
    }

    fn load_document(&self, row: RowId) -> Result<Option<TantivyDocument>, RetrievalError> {
        let query = TermQuery::new(
            Term::from_field_u64(self.fields.row, row.get()),
            IndexRecordOption::Basic,
        );
        let searcher = self.reader.searcher();
        let mut documents = searcher
            .search(&query, &TopDocs::with_limit(1))
            .map_err(tantivy_error)?;
        let Some((_, address)) = documents.pop() else {
            return Ok(None);
        };
        searcher
            .doc::<TantivyDocument>(address)
            .map(Some)
            .map_err(tantivy_error)
    }

    pub fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs()
    }
}

fn tantivy_error(error: impl std::fmt::Display) -> RetrievalError {
    RetrievalError::Tantivy(error.to_string())
}

fn build_schema() -> Schema {
    let mut builder = Schema::builder();
    builder.add_u64_field("row", INDEXED | STORED | FAST);
    let text = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(TOKENIZER_NAME)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
        .set_stored();
    builder.add_text_field("symbol", text.clone());
    builder.add_text_field("signature", text.clone());
    builder.add_text_field("doc", text.clone());
    builder.add_text_field("file", text.clone());
    builder.add_text_field("body", text);
    builder.build()
}

fn fields(schema: &Schema) -> Result<Fields, RetrievalError> {
    let get = |name: &str| {
        schema
            .get_field(name)
            .map_err(|error| RetrievalError::Tantivy(error.to_string()))
    };
    Ok(Fields {
        row: get("row")?,
        symbol: get("symbol")?,
        signature: get("signature")?,
        doc: get("doc")?,
        file: get("file")?,
        body: get("body")?,
    })
}

fn to_document(fields: Fields, row: RowId, doc: &LexicalDoc) -> TantivyDocument {
    let mut document = TantivyDocument::default();
    document.add_u64(fields.row, row.get());
    document.add_text(fields.symbol, &doc.symbol);
    document.add_text(fields.signature, &doc.signature);
    if let Some(doc_first_line) = &doc.doc_first_line {
        document.add_text(fields.doc, doc_first_line);
    }
    document.add_text(fields.file, &doc.file);
    document.add_text(fields.body, &doc.body);
    document
}

fn lexical_doc(document: &TantivyDocument, fields: Fields) -> Option<LexicalDoc> {
    Some(LexicalDoc {
        symbol: document.get_first(fields.symbol)?.as_str()?.to_owned(),
        signature: document.get_first(fields.signature)?.as_str()?.to_owned(),
        doc_first_line: document
            .get_first(fields.doc)
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        file: document.get_first(fields.file)?.as_str()?.to_owned(),
        body: document.get_first(fields.body)?.as_str()?.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use half::f16;
    use storage::{ChunkRecord, ChunkStore, ContentHash, RowId};
    use tempfile::tempdir;

    use super::{LexicalDoc, LexicalIndex};

    fn record(seed: u8) -> ChunkRecord {
        let mut hash = [0u8; 32];
        hash[0] = seed;
        ChunkRecord {
            repository: "fixture".into(),
            file: format!("file{seed}.rs"),
            language: "rust".into(),
            symbol: format!("symbol{seed}"),
            symbol_type: "function".into(),
            signature: format!("fn symbol{seed}()"),
            doc_first_line: Some(format!("documentation {seed}")),
            line_start: 1,
            line_end: 2,
            content_hash: ContentHash::from_bytes(hash),
        }
    }

    fn document(seed: u8) -> LexicalDoc {
        LexicalDoc {
            symbol: format!("symbol{seed}"),
            signature: format!("fn symbol{seed}()"),
            doc_first_line: Some(format!("documentation {seed}")),
            file: format!("file{seed}.rs"),
            body: format!("body{seed} unique{seed}"),
        }
    }

    #[test]
    fn adds_documents_and_searches_by_row_id() {
        let dir = tempdir().unwrap();
        let mut store = ChunkStore::create(dir.path(), 1, "test").unwrap();
        let records: Vec<_> = (0..3).map(record).collect();
        let rows = store
            .insert_batch(
                &records
                    .iter()
                    .map(|record| (record.clone(), vec![f16::from_f32(0.0)]))
                    .collect::<Vec<_>>(),
            )
            .unwrap();

        let mut index = LexicalIndex::open(dir.path()).unwrap();
        index
            .add_batch(
                &rows
                    .iter()
                    .enumerate()
                    .map(|(seed, row)| (*row, document(seed as u8)))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        index.commit().unwrap();

        assert_eq!(index.num_docs(), 3);
        let results = index.search("unique1", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].row, rows[1]);
    }

    #[test]
    fn exact_identifier_spellings_rank_defining_chunk_first() {
        let dir = tempdir().unwrap();
        let mut store = ChunkStore::create(dir.path(), 1, "test").unwrap();
        let records: Vec<_> = (0..6).map(record).collect();
        let rows = store
            .insert_batch(
                &records
                    .iter()
                    .map(|record| (record.clone(), vec![f16::from_f32(0.0)]))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let mut docs = vec![LexicalDoc {
            symbol: "normalize_timestamp".into(),
            signature: "fn normalize_timestamp()".into(),
            doc_first_line: Some("Defines timestamp normalization".into()),
            file: "src/time.rs".into(),
            body: "return normalized value".into(),
        }];
        docs.extend((0..5).map(|seed| LexicalDoc {
            symbol: format!("caller{seed}"),
            signature: format!("fn caller{seed}()"),
            doc_first_line: None,
            file: format!("src/caller{seed}.rs"),
            body: "normalize value".into(),
        }));

        let mut index = LexicalIndex::open(dir.path()).unwrap();
        index
            .add_batch(&rows.iter().copied().zip(docs).collect::<Vec<_>>())
            .unwrap();
        index.commit().unwrap();

        for query in [
            "normalize_timestamp",
            "normalizeTimestamp",
            "normalize timestamp",
        ] {
            let results = index.search(query, 6).unwrap();
            assert_eq!(
                results.first().map(|result| result.row),
                Some(rows[0]),
                "{query}"
            );
        }
    }

    #[test]
    fn retrieves_punctuated_quoted_error_string() {
        let dir = tempdir().unwrap();
        let mut store = ChunkStore::create(dir.path(), 1, "test").unwrap();
        let records: Vec<_> = (0..3).map(record).collect();
        let rows = store
            .insert_batch(
                &records
                    .iter()
                    .map(|record| (record.clone(), vec![f16::from_f32(0.0)]))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let docs = vec![
            LexicalDoc {
                symbol: "network_error".into(),
                signature: "fn network_error()".into(),
                doc_first_line: Some("Handles connection reset by peer".into()),
                file: "src/network.rs".into(),
                body: "return connection reset by peer".into(),
            },
            LexicalDoc {
                symbol: "other".into(),
                signature: "fn other()".into(),
                doc_first_line: None,
                file: "src/other.rs".into(),
                body: "connection retry".into(),
            },
            document(2),
        ];
        let mut index = LexicalIndex::open(dir.path()).unwrap();
        index
            .add_batch(&rows.iter().copied().zip(docs).collect::<Vec<_>>())
            .unwrap();
        index.commit().unwrap();

        for query in ["connection reset by peer", "\"connection reset by peer\":"] {
            let results = index.search(query, 3).unwrap();
            assert_eq!(
                results.first().map(|result| result.row),
                Some(rows[0]),
                "{query}"
            );
        }
    }

    #[test]
    fn multi_word_queries_have_locked_order() {
        let dir = tempdir().unwrap();
        let mut store = ChunkStore::create(dir.path(), 1, "test").unwrap();
        let records: Vec<_> = (0..4).map(record).collect();
        let rows = store
            .insert_batch(
                &records
                    .iter()
                    .map(|record| (record.clone(), vec![f16::from_f32(0.0)]))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let docs = vec![
            LexicalDoc {
                symbol: "clamp_decoder_timestamps".into(),
                signature: "fn clamp_decoder_timestamps()".into(),
                doc_first_line: Some("Clamps regressing decoder timestamps".into()),
                file: "src/decoder.rs".into(),
                body: "if timestamp < previous { timestamp = previous + 1 }".into(),
            },
            LexicalDoc {
                symbol: "parse_decoder_timestamps".into(),
                signature: "fn parse_decoder_timestamps()".into(),
                doc_first_line: Some("Parses decoder timestamps".into()),
                file: "src/parser.rs".into(),
                body: "decode timestamp values".into(),
            },
            LexicalDoc {
                symbol: "enforce_monotonic_order".into(),
                signature: "fn enforce_monotonic_order()".into(),
                doc_first_line: Some("Enforces monotonic order".into()),
                file: "src/order.rs".into(),
                body: "sort values into monotonic order".into(),
            },
            document(3),
        ];
        let mut index = LexicalIndex::open(dir.path()).unwrap();
        index
            .add_batch(&rows.iter().copied().zip(docs).collect::<Vec<_>>())
            .unwrap();
        index.commit().unwrap();

        insta::assert_debug_snapshot!(
            "decoder_timestamps",
            index.search("decoder timestamps", 4).unwrap()
        );
        insta::assert_debug_snapshot!(
            "regressing_timestamps",
            index.search("regressing timestamps", 4).unwrap()
        );
        insta::assert_debug_snapshot!(
            "monotonic_order",
            index.search("monotonic order", 4).unwrap()
        );
    }

    #[test]
    fn handles_empty_queries_limits_and_raw_scores() {
        let dir = tempdir().unwrap();
        let mut store = ChunkStore::create(dir.path(), 1, "test").unwrap();
        let records: Vec<_> = (0..3).map(record).collect();
        let rows = store
            .insert_batch(
                &records
                    .iter()
                    .map(|record| (record.clone(), vec![f16::from_f32(0.0)]))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let docs = vec![
            LexicalDoc {
                symbol: "exact_symbol".into(),
                signature: "fn exact_symbol()".into(),
                doc_first_line: None,
                file: "src/exact.rs".into(),
                body: "other".into(),
            },
            LexicalDoc {
                symbol: "other1".into(),
                signature: "fn other1()".into(),
                doc_first_line: None,
                file: "src/other1.rs".into(),
                body: "common".into(),
            },
            LexicalDoc {
                symbol: "other2".into(),
                signature: "fn other2()".into(),
                doc_first_line: None,
                file: "src/other2.rs".into(),
                body: "common common".into(),
            },
        ];
        let mut index = LexicalIndex::open(dir.path()).unwrap();
        index
            .add_batch(&rows.iter().copied().zip(docs).collect::<Vec<_>>())
            .unwrap();
        index.commit().unwrap();

        assert!(index.search("!!! :::", 10).unwrap().is_empty());
        assert!(index.search("exact_symbol", 0).unwrap().is_empty());
        let results = index.search("common", 1).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].score > 0.0);
        let exact = index.search("exact_symbol", 10).unwrap();
        assert_eq!(exact.first().map(|result| result.row), Some(rows[0]));
        assert!(exact[0].score > 1.0);
        let common = index.search("common", 10).unwrap();
        assert!(common.windows(2).all(|pair| pair[0].score >= pair[1].score));
    }

    #[test]
    fn equal_scores_are_ordered_by_row_id_across_merge() {
        let dir = tempdir().unwrap();
        let mut store = ChunkStore::create(dir.path(), 1, "test").unwrap();
        let records: Vec<_> = (0..4).map(record).collect();
        let rows = store
            .insert_batch(
                &records
                    .iter()
                    .map(|record| (record.clone(), vec![f16::from_f32(0.0)]))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let mut index = LexicalIndex::open(dir.path()).unwrap();
        for row in rows.iter().rev() {
            index
                .add_batch(&[(
                    *row,
                    LexicalDoc {
                        symbol: "same".into(),
                        signature: "fn same()".into(),
                        doc_first_line: None,
                        file: "src/same.rs".into(),
                        body: "same".into(),
                    },
                )])
                .unwrap();
            index.commit().unwrap();
        }

        let expected = rows.clone();
        let before = index.search("same", 4).unwrap();
        assert_eq!(
            before.iter().map(|result| result.row).collect::<Vec<_>>(),
            expected
        );
        let repeated = index.search("same", 4).unwrap();
        assert_eq!(repeated, before);

        let segment_ids: Vec<_> = index
            .reader
            .searcher()
            .segment_readers()
            .iter()
            .map(|segment| segment.segment_id())
            .collect();
        index.writer.merge(&segment_ids).wait().unwrap();
        index.reader.reload().unwrap();
        let after = index.search("same", 4).unwrap();
        assert_eq!(after, before);
    }

    #[test]
    fn removes_updates_paths_and_renumbers_rows() {
        let dir = tempdir().unwrap();
        let mut store = ChunkStore::create(dir.path(), 1, "test").unwrap();
        let records: Vec<_> = (0..3).map(record).collect();
        let rows = store
            .insert_batch(
                &records
                    .iter()
                    .map(|record| (record.clone(), vec![f16::from_f32(0.0)]))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let mut index = LexicalIndex::open(dir.path()).unwrap();
        index
            .add_batch(
                &rows
                    .iter()
                    .enumerate()
                    .map(|(seed, row)| (*row, document(seed as u8)))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        index.commit().unwrap();

        index.remove(&[rows[1]]).unwrap();
        index.commit().unwrap();
        assert!(index.search("unique1", 10).unwrap().is_empty());

        index
            .update_file_paths(&[(rows[0], "renamed/path.rs".into())])
            .unwrap();
        index.commit().unwrap();
        assert_eq!(index.search("renamed path", 10).unwrap()[0].row, rows[0]);

        index
            .renumber(&[
                (rows[0], RowId::from_u64(100)),
                (rows[2], RowId::from_u64(102)),
            ])
            .unwrap();
        index.commit().unwrap();
        assert_eq!(index.search("unique0", 10).unwrap()[0].row.get(), 100);
        assert_eq!(index.search("unique2", 10).unwrap()[0].row.get(), 102);
    }
}
