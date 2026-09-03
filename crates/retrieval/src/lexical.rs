//! Lexical retrieval backed by Tantivy.

use std::path::Path;

use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, BoostQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    FAST, Field, INDEXED, IndexRecordOption, STORED, Schema, TantivyDocument, TextFieldIndexing,
    TextOptions, Value,
};
use tantivy::tokenizer::TextAnalyzer;
use tantivy::tokenizer::{TokenStream, Tokenizer};
use tantivy::{Index, IndexReader, IndexWriter, Term};

use storage::RowId;

use crate::error::RetrievalError;
use crate::tokenize::CodeTokenizer;

pub const SYMBOL_BOOST: f32 = 4.0;
pub const SIGNATURE_BOOST: f32 = 3.0;
pub const DOC_BOOST: f32 = 2.0;
pub const BODY_BOOST: f32 = 1.0;
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

    pub fn commit(&mut self) -> Result<(), RetrievalError> {
        self.writer.commit().map_err(tantivy_error)?;
        self.reader.reload().map_err(tantivy_error)?;
        Ok(())
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<ScoredRow>, RetrievalError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(parsed) = self.build_query(query) else {
            return Ok(Vec::new());
        };
        let searcher = self.reader.searcher();
        let top_docs = searcher
            .search(&parsed, &TopDocs::with_limit(limit))
            .map_err(tantivy_error)?;
        let mut rows = Vec::with_capacity(top_docs.len());
        for (score, address) in top_docs {
            let document = searcher
                .doc::<TantivyDocument>(address)
                .map_err(tantivy_error)?;
            let Some(row) = document
                .get_first(self.fields.row)
                .and_then(|value| value.as_u64())
            else {
                continue;
            };
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

    pub fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs() as u64
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

#[cfg(test)]
mod tests {
    use half::f16;
    use storage::{ChunkRecord, ChunkStore, ContentHash};
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
}
