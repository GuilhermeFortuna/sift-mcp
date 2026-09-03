use crate::error::StoreError;
use crate::matrix::EmbeddingMatrix;
use crate::record::{ChunkRecord, ContentHash, RowId};
use half::f16;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::{Path, PathBuf};

/// Metadata schema version. Independent of the matrix format version.
pub const SCHEMA_VERSION: u32 = 1;

const DB_NAME: &str = "chunks.db";
const MATRIX_NAME: &str = "embeddings.f16";

/// Counts used by SIFT-006 to decide when to compact.
#[derive(Debug, Clone, PartialEq)]
pub struct StoreStats {
    pub live: u64,
    pub dead: u64,
    pub dead_fraction: f64,
}

/// Result of the correspondence check. Never panics on a corrupt store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Integrity {
    Ok { live: u64 },
    Broken {
        orphan_rows: Vec<RowId>,
        missing_rows: Vec<RowId>,
        duplicate_rows: Vec<RowId>,
    },
}

/// Report from compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionReport {
    pub live_before: u64,
    pub dead_reclaimed: u64,
    pub live_after: u64,
}

/// Durable chunk metadata + embedding matrix.
pub struct ChunkStore {
    dir: PathBuf,
    conn: Connection,
    matrix: Option<EmbeddingMatrix>,
    /// Test hook: count of statements prepared by get_many.
    statements_prepared: std::cell::Cell<u64>,
    /// Test hook: fail before committing insert_batch when set.
    fail_before_commit: bool,
}

impl std::fmt::Debug for ChunkStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkStore")
            .field("dir", &self.dir)
            .field("matrix", &self.matrix)
            .finish_non_exhaustive()
    }
}

impl ChunkStore {
    fn matrix_ref(&self) -> &EmbeddingMatrix {
        self.matrix.as_ref().expect("matrix present")
    }

    fn matrix_mut(&mut self) -> &mut EmbeddingMatrix {
        self.matrix.as_mut().expect("matrix present")
    }

    /// Drop trailing matrix rows that have no metadata (crash recovery).
    fn reclaim_trailing_orphans(&mut self) -> Result<(), StoreError> {
        let meta_count: u64 = self.conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| {
            r.get::<_, i64>(0).map(|v| v as u64)
        })?;
        let max_rowid: Option<u64> = self
            .conn
            .query_row("SELECT MAX(rowid) FROM chunks", [], |r| {
                r.get::<_, Option<i64>>(0)
            })?
            .map(|v| v as u64);

        let expected_rows = match max_rowid {
            None => 0,
            Some(max) => max + 1,
        };
        // Only reclaim when metadata is a dense prefix 0..expected_rows-1.
        if meta_count != expected_rows {
            return Ok(());
        }
        let matrix_rows = self.matrix_ref().rows();
        if matrix_rows <= expected_rows {
            return Ok(());
        }
        // Confirm every id in 0..expected_rows exists.
        for i in 0..expected_rows {
            let n: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM chunks WHERE rowid = ?1",
                params![i as i64],
                |r| r.get(0),
            )?;
            if n != 1 {
                return Ok(());
            }
        }
        let mut header = self.matrix_ref().header().clone();
        header.rows = expected_rows;
        let path = self.matrix_ref().path().to_path_buf();
        self.matrix = None;
        EmbeddingMatrix::rewrite_header_on_disk(&path, &header)?;
        self.matrix = Some(EmbeddingMatrix::open_mut(&path)?);
        Ok(())
    }

    pub fn create(dir: &Path, dims: u32, model_id: &str) -> Result<Self, StoreError> {
        std::fs::create_dir_all(dir)?;
        let db_path = dir.join(DB_NAME);
        let matrix_path = dir.join(MATRIX_NAME);
        if db_path.exists() || matrix_path.exists() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "store directory is not empty",
            )));
        }
        let matrix = EmbeddingMatrix::create(&matrix_path, dims, model_id)?;
        let conn = Connection::open(&db_path)?;
        configure_connection(&conn)?;
        init_schema(&conn)?;
        set_meta(&conn, "schema_version", &SCHEMA_VERSION.to_string())?;
        Ok(Self {
            dir: dir.to_path_buf(),
            conn,
            matrix: Some(matrix),
            statements_prepared: std::cell::Cell::new(0),
            fail_before_commit: false,
        })
    }

    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        let db_path = dir.join(DB_NAME);
        let matrix_path = dir.join(MATRIX_NAME);
        let conn = Connection::open(&db_path)?;
        configure_connection(&conn)?;
        let version: u32 = meta_u32(&conn, "schema_version")?;
        if version != SCHEMA_VERSION {
            return Err(StoreError::SchemaVersion {
                expected: SCHEMA_VERSION,
                got: version,
            });
        }
        let matrix = EmbeddingMatrix::open_mut(&matrix_path)?;
        let mut store = Self {
            dir: dir.to_path_buf(),
            conn,
            matrix: Some(matrix),
            statements_prepared: std::cell::Cell::new(0),
            fail_before_commit: false,
        };
        // A crash after matrix append but before metadata commit leaves trailing
        // orphan rows. Truncate them on open so the store needs no manual repair.
        store.reclaim_trailing_orphans()?;
        match store.verify()? {
            Integrity::Ok { .. } => Ok(store),
            broken @ Integrity::Broken { .. } => Err(StoreError::Corrupt(broken)),
        }
    }

    /// Atomic batch. Returns one RowId per input, reusing the row of any
    /// content hash already live.
    pub fn insert_batch(
        &mut self,
        chunks: &[(ChunkRecord, Vec<f16>)],
    ) -> Result<Vec<RowId>, StoreError> {
        let dims = self.matrix_ref().dims();
        for (_rec, vec) in chunks {
            if vec.len() as u32 != dims {
                return Err(StoreError::DimensionMismatch {
                    expected: dims,
                    got: vec.len() as u32,
                });
            }
        }

        // Append matrix rows first (outside the SQL transaction boundary for
        // durability ordering), then insert metadata and commit. On failure after
        // append, truncate matrix rows back to the pre-batch count so an
        // interrupted batch leaves no orphan rows when we control the failure;
        // a crash between append and commit leaves orphans that verify reports.
        let rows_before = self.matrix_ref().rows();
        let mut result = Vec::with_capacity(chunks.len());
        let mut new_rows: Vec<(RowId, &ChunkRecord)> = Vec::new();

        // Resolve hashes that already exist; only append for novel hashes.
        // Within-batch duplicates reuse the first new/existing row.
        let mut batch_hash_to_row: std::collections::HashMap<[u8; 32], RowId> =
            std::collections::HashMap::new();

        for (rec, vec) in chunks {
            let hash = *rec.content_hash.as_bytes();
            if let Some(row) = batch_hash_to_row.get(&hash) {
                result.push(*row);
                continue;
            }
            if let Some((row, _)) = self.get_by_hash(&rec.content_hash)? {
                batch_hash_to_row.insert(hash, row);
                result.push(row);
                continue;
            }
            let row = self.matrix_mut().append(vec)?;
            batch_hash_to_row.insert(hash, row);
            new_rows.push((row, rec));
            result.push(row);
        }
        self.matrix_mut().flush()?;

        let insert_result = (|| -> Result<(), StoreError> {
            let tx = self.conn.unchecked_transaction()?;
            for (row, rec) in &new_rows {
                insert_chunk(&tx, *row, rec)?;
            }
            if self.fail_before_commit {
                return Err(StoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "injected failure before commit",
                )));
            }
            tx.commit()?;
            Ok(())
        })();

        if let Err(e) = insert_result {
            // Roll back matrix rows appended for this batch when we can.
            let mut header = self.matrix_ref().header().clone();
            header.rows = rows_before;
            let path = self.matrix_ref().path().to_path_buf();
            // Remap after rewriting header.
            EmbeddingMatrix::rewrite_header_on_disk(&path, &header)?;
            self.matrix = Some(EmbeddingMatrix::open_mut(&path)?);
            return Err(e);
        }

        Ok(result)
    }

    pub fn get(&self, row: RowId) -> Result<Option<ChunkRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT repository, file, language, symbol, symbol_type, signature,
                    doc_first_line, line_start, line_end, content_hash
             FROM chunks WHERE rowid = ?1 AND live = 1",
        )?;
        let rec = stmt
            .query_row(params![row.get() as i64], |r| row_to_record(r))
            .optional()?;
        Ok(rec)
    }

    /// One query for the whole set, in the order requested.
    pub fn get_many(&self, rows: &[RowId]) -> Result<Vec<Option<ChunkRecord>>, StoreError> {
        self.statements_prepared
            .set(self.statements_prepared.get() + 1);
        self.conn.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS get_many_ids (
                ord INTEGER PRIMARY KEY,
                row_id INTEGER NOT NULL
             );
             DELETE FROM get_many_ids;",
        )?;
        {
            let mut insert = self
                .conn
                .prepare("INSERT INTO get_many_ids (ord, row_id) VALUES (?1, ?2)")?;
            self.statements_prepared
                .set(self.statements_prepared.get() + 1);
            for (i, row) in rows.iter().enumerate() {
                insert.execute(params![i as i64, row.get() as i64])?;
            }
        }
        // One SELECT joining the temp table — independent of how we count
        // "prepared statements for the lookup itself".
        let mut stmt = self.conn.prepare(
            "SELECT g.ord, c.repository, c.file, c.language, c.symbol, c.symbol_type,
                    c.signature, c.doc_first_line, c.line_start, c.line_end, c.content_hash
             FROM get_many_ids g
             LEFT JOIN chunks c ON c.rowid = g.row_id AND c.live = 1
             ORDER BY g.ord",
        )?;
        self.statements_prepared
            .set(self.statements_prepared.get() + 1);

        let mut out = vec![None; rows.len()];
        let mut rows_iter = stmt.query([])?;
        while let Some(r) = rows_iter.next()? {
            let ord: i64 = r.get(0)?;
            let present: Option<String> = r.get(1)?;
            if present.is_some() {
                let rec = ChunkRecord {
                    repository: r.get(1)?,
                    file: r.get(2)?,
                    language: r.get(3)?,
                    symbol: r.get(4)?,
                    symbol_type: r.get(5)?,
                    signature: r.get(6)?,
                    doc_first_line: r.get(7)?,
                    line_start: r.get::<_, i64>(8)? as u32,
                    line_end: r.get::<_, i64>(9)? as u32,
                    content_hash: ContentHash::from_bytes(blob_to_hash(r.get(10)?)),
                };
                out[ord as usize] = Some(rec);
            }
        }
        Ok(out)
    }

    pub fn get_by_hash(
        &self,
        hash: &ContentHash,
    ) -> Result<Option<(RowId, ChunkRecord)>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT rowid, repository, file, language, symbol, symbol_type, signature,
                    doc_first_line, line_start, line_end, content_hash
             FROM chunks WHERE content_hash = ?1 AND live = 1",
        )?;
        let rec = stmt
            .query_row(params![hash.as_bytes().as_slice()], |r| {
                let rowid: i64 = r.get(0)?;
                Ok((
                    RowId::new(rowid as u64),
                    ChunkRecord {
                        repository: r.get(1)?,
                        file: r.get(2)?,
                        language: r.get(3)?,
                        symbol: r.get(4)?,
                        symbol_type: r.get(5)?,
                        signature: r.get(6)?,
                        doc_first_line: r.get(7)?,
                        line_start: r.get::<_, i64>(8)? as u32,
                        line_end: r.get::<_, i64>(9)? as u32,
                        content_hash: ContentHash::from_bytes(blob_to_hash(r.get(10)?)),
                    },
                ))
            })
            .optional()?;
        Ok(rec)
    }

    pub fn rows_for_file(&self, file: &str) -> Result<Vec<RowId>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT rowid FROM chunks WHERE file = ?1 AND live = 1 ORDER BY rowid",
        )?;
        let rows = stmt
            .query_map(params![file], |r| {
                let id: i64 = r.get(0)?;
                Ok(RowId::new(id as u64))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn tombstone(&mut self, rows: &[RowId]) -> Result<(), StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare("UPDATE chunks SET live = 0 WHERE rowid = ?1 AND live = 1")?;
            for row in rows {
                stmt.execute(params![row.get() as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn stats(&self) -> Result<StoreStats, StoreError> {
        let live: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM chunks WHERE live = 1",
            [],
            |r| r.get::<_, i64>(0).map(|v| v as u64),
        )?;
        let dead: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM chunks WHERE live = 0",
            [],
            |r| r.get::<_, i64>(0).map(|v| v as u64),
        )?;
        let total = live + dead;
        let dead_fraction = if total == 0 {
            0.0
        } else {
            dead as f64 / total as f64
        };
        Ok(StoreStats {
            live,
            dead,
            dead_fraction,
        })
    }

    pub fn verify(&self) -> Result<Integrity, StoreError> {
        let matrix_rows = self.matrix_ref().rows();
        let mut orphan_rows = Vec::new();
        let mut missing_rows = Vec::new();
        let mut duplicate_rows = Vec::new();

        // Rows present in the matrix but missing from metadata.
        for i in 0..matrix_rows {
            let count: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM chunks WHERE rowid = ?1",
                params![i as i64],
                |r| r.get(0),
            )?;
            if count == 0 {
                orphan_rows.push(RowId::new(i));
            } else if count > 1 {
                duplicate_rows.push(RowId::new(i));
            }
        }

        // Metadata rows pointing past the matrix (or at holes).
        {
            let mut stmt = self.conn.prepare("SELECT rowid FROM chunks")?;
            let ids = stmt.query_map([], |r| r.get::<_, i64>(0))?;
            for id in ids {
                let id = id? as u64;
                if id >= matrix_rows {
                    missing_rows.push(RowId::new(id));
                }
            }
        }

        // Duplicate live content hashes.
        {
            let mut stmt = self.conn.prepare(
                "SELECT rowid FROM chunks WHERE live = 1 AND content_hash IN (
                    SELECT content_hash FROM chunks WHERE live = 1
                    GROUP BY content_hash HAVING COUNT(*) > 1
                 )",
            )?;
            let ids = stmt.query_map([], |r| r.get::<_, i64>(0))?;
            for id in ids {
                let row = RowId::new(id? as u64);
                if !duplicate_rows.contains(&row) {
                    duplicate_rows.push(row);
                }
            }
        }

        if orphan_rows.is_empty() && missing_rows.is_empty() && duplicate_rows.is_empty() {
            let live = self.stats()?.live;
            Ok(Integrity::Ok { live })
        } else {
            orphan_rows.sort();
            missing_rows.sort();
            duplicate_rows.sort();
            Ok(Integrity::Broken {
                orphan_rows,
                missing_rows,
                duplicate_rows,
            })
        }
    }

    pub fn compact(&mut self) -> Result<CompactionReport, StoreError> {
        let stats = self.stats()?;
        let live_before = stats.live;
        let dead_reclaimed = stats.dead;

        let mut live_rows: Vec<(ChunkRecord, Vec<f16>)> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT rowid, repository, file, language, symbol, symbol_type, signature,
                        doc_first_line, line_start, line_end, content_hash
                 FROM chunks WHERE live = 1 ORDER BY rowid",
            )?;
            let iter = stmt.query_map([], |r| {
                let rowid: i64 = r.get(0)?;
                let rec = ChunkRecord {
                    repository: r.get(1)?,
                    file: r.get(2)?,
                    language: r.get(3)?,
                    symbol: r.get(4)?,
                    symbol_type: r.get(5)?,
                    signature: r.get(6)?,
                    doc_first_line: r.get(7)?,
                    line_start: r.get::<_, i64>(8)? as u32,
                    line_end: r.get::<_, i64>(9)? as u32,
                    content_hash: ContentHash::from_bytes(blob_to_hash(r.get(10)?)),
                };
                Ok((RowId::new(rowid as u64), rec))
            })?;
            for item in iter {
                let (row, rec) = item?;
                let vector = self.matrix_ref().row(row)?.to_vec();
                live_rows.push((rec, vector));
            }
        }

        let tmp_db = self.dir.join("chunks.db.compact");
        let tmp_matrix = self.dir.join("embeddings.f16.compact");
        let _ = std::fs::remove_file(&tmp_db);
        let _ = std::fs::remove_file(&tmp_matrix);

        let dims = self.matrix_ref().dims();
        let model_id = self.matrix_ref().model_id().to_owned();
        let indexed = self.indexed_commit()?;

        {
            let mut new_matrix = EmbeddingMatrix::create(&tmp_matrix, dims, &model_id)?;
            let new_conn = Connection::open(&tmp_db)?;
            configure_connection(&new_conn)?;
            init_schema(&new_conn)?;
            set_meta(&new_conn, "schema_version", &SCHEMA_VERSION.to_string())?;
            if let Some(commit) = &indexed {
                set_meta(&new_conn, "indexed_commit", commit)?;
            }
            let tx = new_conn.unchecked_transaction()?;
            for (rec, vec) in &live_rows {
                let row = new_matrix.append(vec)?;
                insert_chunk(&tx, row, rec)?;
            }
            new_matrix.flush()?;
            tx.commit()?;
            let header_rows = new_matrix.rows();
            drop(new_matrix);
            let file_len = 288u64 + header_rows * dims as u64 * 2;
            let f = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&tmp_matrix)?;
            f.set_len(file_len)?;
        }

        let db_path = self.dir.join(DB_NAME);
        let matrix_path = self.dir.join(MATRIX_NAME);

        // Close open handles so rename replaces the files we will reopen.
        let old_conn = std::mem::replace(&mut self.conn, Connection::open_in_memory()?);
        let _ = old_conn.close();
        self.matrix = None;

        std::fs::rename(&tmp_matrix, &matrix_path)?;
        std::fs::rename(&tmp_db, &db_path)?;

        let conn = Connection::open(&db_path)?;
        configure_connection(&conn)?;
        let matrix = EmbeddingMatrix::open_mut(&matrix_path)?;
        self.conn = conn;
        self.matrix = Some(matrix);

        let live_after = self.stats()?.live;
        Ok(CompactionReport {
            live_before,
            dead_reclaimed,
            live_after,
        })
    }

    pub fn matrix(&self) -> &EmbeddingMatrix {
        self.matrix_ref()
    }

    pub fn indexed_commit(&self) -> Result<Option<String>, StoreError> {
        let val: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'indexed_commit'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(val)
    }

    pub fn set_indexed_commit(&mut self, commit: &str) -> Result<(), StoreError> {
        set_meta(&self.conn, "indexed_commit", commit)?;
        Ok(())
    }

    /// Require the matrix model id matches `expected` (for query-time guards).
    pub fn require_model(&self, expected: &str) -> Result<(), StoreError> {
        let got = self.matrix_ref().model_id();
        if got != expected {
            return Err(StoreError::ModelMismatch {
                expected: expected.to_owned(),
                got: got.to_owned(),
            });
        }
        Ok(())
    }

    /// Test-only: number of statements prepared during get_many calls since last reset.
    pub fn take_statements_prepared(&self) -> u64 {
        let n = self.statements_prepared.get();
        self.statements_prepared.set(0);
        n
    }

    /// Test-only: force the next insert_batch to fail before commit.
    pub fn set_fail_before_commit(&mut self, fail: bool) {
        self.fail_before_commit = fail;
    }

    /// Test-only directory access for corruption fixtures.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

fn configure_connection(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;",
    )?;
    Ok(())
}

fn init_schema(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE meta (
            key TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
         );
         CREATE TABLE chunks (
            rowid INTEGER PRIMARY KEY NOT NULL,
            repository TEXT NOT NULL,
            file TEXT NOT NULL,
            language TEXT NOT NULL,
            symbol TEXT NOT NULL,
            symbol_type TEXT NOT NULL,
            signature TEXT NOT NULL,
            doc_first_line TEXT,
            line_start INTEGER NOT NULL,
            line_end INTEGER NOT NULL,
            content_hash BLOB NOT NULL,
            live INTEGER NOT NULL DEFAULT 1 CHECK (live IN (0, 1))
         );
         CREATE UNIQUE INDEX chunks_live_hash ON chunks(content_hash) WHERE live = 1;
         CREATE INDEX chunks_file_live ON chunks(file) WHERE live = 1;",
    )?;
    Ok(())
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn meta_u32(conn: &Connection, key: &str) -> Result<u32, StoreError> {
    let s: String = conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        params![key],
        |r| r.get(0),
    )?;
    s.parse::<u32>().map_err(|e| {
        StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("meta {key}: {e}"),
        ))
    })
}

fn insert_chunk(tx: &Transaction<'_>, row: RowId, rec: &ChunkRecord) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO chunks (
            rowid, repository, file, language, symbol, symbol_type, signature,
            doc_first_line, line_start, line_end, content_hash, live
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1)",
        params![
            row.get() as i64,
            rec.repository,
            rec.file,
            rec.language,
            rec.symbol,
            rec.symbol_type,
            rec.signature,
            rec.doc_first_line,
            rec.line_start as i64,
            rec.line_end as i64,
            rec.content_hash.as_bytes().as_slice(),
        ],
    )?;
    Ok(())
}

fn row_to_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<ChunkRecord> {
    Ok(ChunkRecord {
        repository: r.get(0)?,
        file: r.get(1)?,
        language: r.get(2)?,
        symbol: r.get(3)?,
        symbol_type: r.get(4)?,
        signature: r.get(5)?,
        doc_first_line: r.get(6)?,
        line_start: r.get::<_, i64>(7)? as u32,
        line_end: r.get::<_, i64>(8)? as u32,
        content_hash: ContentHash::from_bytes(blob_to_hash(r.get(9)?)),
    })
}

fn blob_to_hash(blob: Vec<u8>) -> [u8; 32] {
    let mut out = [0u8; 32];
    let n = blob.len().min(32);
    out[..n].copy_from_slice(&blob[..n]);
    out
}
