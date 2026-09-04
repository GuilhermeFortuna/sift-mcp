//! End-to-end repository indexing: walk, parse, embed, store.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crossbeam_channel::bounded;
use inference::{Embedder, Role};
use rayon::prelude::*;
use retrieval::{LexicalDoc, LexicalIndex};
use serde::{Deserialize, Serialize};
use storage::{ChunkRecord, ChunkStore, CompactionReport, ContentHash, Integrity, RowId};

use crate::chunker::Chunker;
use crate::error::IndexError;
use crate::exclusions::{Exclusions, HEAD_SNIFF_BYTES, MAX_FILE_BYTES, SkipReason};
use crate::git::{FileChange, RepoGit};
use crate::language::Language;

/// Marker suffix when the worktree was dirty at index time.
pub const DIRTY_COMMIT_SUFFIX: &str = "+dirty";

pub struct IndexConfig {
    pub embed_batch: usize,
    /// 0 = available parallelism.
    pub parse_threads: usize,
    pub compact_threshold: f64,
    pub dirty_worktree: DirtyPolicy,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            embed_batch: 32,
            parse_threads: 0,
            compact_threshold: 0.20,
            dirty_worktree: DirtyPolicy::IndexWorktree,
        }
    }
}

/// What to do when the worktree differs from HEAD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyPolicy {
    IndexWorktree,
    IndexCommitOnly,
    Refuse,
}

/// Reported for every run, full or incremental.
#[derive(Debug, Clone)]
pub struct IndexReport {
    pub commit: String,
    pub files_seen: u64,
    pub files_indexed: u64,
    pub files_excluded: u64,
    pub files_unsupported: u64,
    pub files_unparsed: u64,
    pub chunks_added: u64,
    pub chunks_reused: u64,
    pub chunks_removed: u64,
    pub embeddings_computed: u64,
    pub chunks_truncated: u64,
    pub parse_millis: u64,
    pub embed_millis: u64,
    pub store_millis: u64,
    pub wall_millis: u64,
    pub compacted: Option<CompactionReport>,
    pub live_before: u64,
    pub live_after: u64,
}

impl IndexReport {
    /// `live_after == live_before + chunks_added - chunks_removed`
    pub fn assert_reconciles(&self) {
        assert_eq!(
            self.live_after,
            self.live_before + self.chunks_added - self.chunks_removed,
            "counter reconciliation failed: live_before={} added={} removed={} live_after={}",
            self.live_before,
            self.chunks_added,
            self.chunks_removed,
            self.live_after
        );
    }
}

pub trait Progress {
    fn phase(&mut self, phase: Phase, done: u64, total: Option<u64>);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Phase {
    Walking,
    Parsing,
    Embedding,
    Storing,
    Compacting,
}

/// Progress sink that discards all updates.
pub struct NullProgress;

impl Progress for NullProgress {
    fn phase(&mut self, _phase: Phase, _done: u64, _total: Option<u64>) {}
}

pub struct Indexer<'a> {
    store: ChunkStore,
    lexical: LexicalIndex,
    embedder: &'a dyn Embedder,
    repo: PathBuf,
    git: RepoGit,
    exclusions: Exclusions,
    config: IndexConfig,
    /// Test hook: fail after this many successful store batches.
    interrupt_after_batches: Option<u64>,
    batches_committed: u64,
    publication: PublicationJournal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum PublicationOp {
    Add { row: u64, doc: LexicalDoc },
    Remove { rows: Vec<u64> },
    UpdateFile { rows: Vec<u64> },
    Renumber { mapping: Vec<(u64, u64)> },
}

struct PublicationJournal {
    path: PathBuf,
}

impl PublicationJournal {
    fn new(store: &ChunkStore) -> Self {
        Self {
            path: store.dir().join("index.publication"),
        }
    }

    fn prepare(&self, ops: &[PublicationOp]) -> Result<(), IndexError> {
        if self.path.exists() {
            return Err(IndexError::Other(
                "publication journal already contains an unfinished operation".into(),
            ));
        }
        let tmp = self.path.with_extension("publication.tmp");
        let bytes = serde_json::to_vec(ops).map_err(|e| IndexError::Other(e.to_string()))?;
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, &self.path)?;
        if let Some(parent) = self.path.parent()
            && let Ok(dir) = fs::File::open(parent)
        {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    fn clear(&self) -> Result<(), IndexError> {
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }

    fn recover(&self, store: &ChunkStore, lexical: &mut LexicalIndex) -> Result<(), IndexError> {
        if !self.path.exists() {
            return Ok(());
        }
        let bytes = fs::read(&self.path)?;
        let ops: Vec<PublicationOp> = serde_json::from_slice(&bytes)
            .map_err(|e| IndexError::Other(format!("publication journal: {e}")))?;
        for op in ops {
            match op {
                PublicationOp::Add { row, doc } => {
                    let row = RowId::from_u64(row);
                    if store.get(row)?.is_some() {
                        lexical.add_batch(&[(row, doc)])?;
                    } else {
                        lexical.remove(&[row])?;
                    }
                }
                PublicationOp::Remove { rows } => {
                    let mut removed = Vec::with_capacity(rows.len());
                    for row in rows {
                        let row = RowId::from_u64(row);
                        if store.get(row)?.is_none() {
                            removed.push(row);
                        }
                    }
                    let rows = removed;
                    lexical.remove(&rows)?;
                }
                PublicationOp::UpdateFile { rows } => {
                    let mut paths = Vec::with_capacity(rows.len());
                    for row in rows {
                        let row = RowId::from_u64(row);
                        if let Some(record) = store.get(row)? {
                            paths.push((row, record.file));
                        }
                    }
                    lexical.update_file_paths(&paths)?;
                }
                PublicationOp::Renumber { mapping } => {
                    let expected_rows = mapping
                        .iter()
                        .map(|(_, new)| new.saturating_add(1))
                        .max()
                        .unwrap_or(0);
                    if store.matrix().rows() == expected_rows
                        && store.stats()?.live == expected_rows
                    {
                        lexical.renumber(
                            &mapping
                                .into_iter()
                                .map(|(old, new)| (RowId::from_u64(old), RowId::from_u64(new)))
                                .collect::<Vec<_>>(),
                        )?;
                    }
                }
            }
        }
        lexical.commit()?;
        self.clear()
    }
}

enum ParsedFile {
    Ok {
        rel: String,
        chunks: Vec<(ChunkRecord, String)>,
    },
    Unparsed,
    SkipIdentical {
        reused: u64,
    },
}

struct WorkItem {
    rel: String,
    language: Language,
    source: String,
}

impl<'a> Indexer<'a> {
    pub fn open(
        store: ChunkStore,
        embedder: &'a dyn Embedder,
        repo: &Path,
        config: IndexConfig,
    ) -> Result<Self, IndexError> {
        store.require_model(embedder.model_id())?;
        let live = store.stats()?.live;
        let mut lexical = LexicalIndex::open(store.dir())?;
        let publication = PublicationJournal::new(&store);
        publication.recover(&store, &mut lexical)?;
        let indexed = lexical.num_docs();
        if live != indexed {
            return Err(IndexError::LexicalOutOfSync {
                store_live: live,
                indexed,
            });
        }
        let git = RepoGit::open(repo)?;
        let exclusions = Exclusions::for_repository(repo)?;
        Ok(Self {
            store,
            lexical,
            embedder,
            repo: repo.to_path_buf(),
            git,
            exclusions,
            config,
            interrupt_after_batches: None,
            batches_committed: 0,
            publication,
        })
    }

    pub fn store(&self) -> &ChunkStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut ChunkStore {
        &mut self.store
    }

    pub fn lexical(&self) -> &LexicalIndex {
        &self.lexical
    }

    pub fn into_store(self) -> ChunkStore {
        self.store
    }

    pub fn into_parts(self) -> (ChunkStore, LexicalIndex) {
        (self.store, self.lexical)
    }

    /// Test-only: interrupt after `n` successful `insert_batch` calls.
    pub fn set_interrupt_after_batches(&mut self, n: Option<u64>) {
        self.interrupt_after_batches = n;
        self.batches_committed = 0;
    }

    pub fn index_all(&mut self, progress: &mut dyn Progress) -> Result<IndexReport, IndexError> {
        let wall = Instant::now();
        let live_before = self.store.stats()?.live;
        let mut report = empty_report(live_before);

        let dirty = self.git.is_dirty()?;
        self.apply_dirty_policy(dirty)?;

        progress.phase(Phase::Walking, 0, None);
        let (work, present_files) = self.collect_all_files(&mut report)?;

        self.reconcile_absent_files(&present_files, &mut report)?;

        self.process_work_items(work, &mut report, progress)?;

        let commit = self.record_commit(dirty)?;
        report.commit = commit;
        self.maybe_compact(&mut report, progress)?;
        report.live_after = self.store.stats()?.live;
        report.wall_millis = wall.elapsed().as_millis() as u64;
        report.assert_reconciles();
        Ok(report)
    }

    pub fn update(&mut self, progress: &mut dyn Progress) -> Result<IndexReport, IndexError> {
        let wall = Instant::now();
        let live_before = self.store.stats()?.live;
        let mut report = empty_report(live_before);

        let dirty = self.git.is_dirty()?;
        self.apply_dirty_policy(dirty)?;

        let Some(base) = self.store.indexed_commit()? else {
            return self.index_all(progress);
        };

        let base_clean = base
            .strip_suffix(DIRTY_COMMIT_SUFFIX)
            .unwrap_or(base.as_str());

        let mut work = Vec::new();

        if dirty && matches!(self.config.dirty_worktree, DirtyPolicy::IndexWorktree) {
            // Always reconcile dirty paths. This is required when a clean
            // indexed commit is followed by an uncommitted edit.
            for path in self.git.dirty_paths()? {
                if let Some(item) = self.work_item_for_path(&path, &mut report)? {
                    work.push(item);
                } else {
                    self.remove_file(&path, &mut report)?;
                }
            }
        }

        match self.git.changes_since(base_clean) {
            Ok(changes) => {
                self.apply_file_changes(&changes, &mut work, &mut report)?;
            }
            Err(_) if base.ends_with(DIRTY_COMMIT_SUFFIX) => {
                // Dirty base may not resolve as a rev; rely on dirty-path reconcile.
            }
            Err(e) => return Err(e),
        }

        // Deduplicate work items by path (last wins).
        {
            let mut seen = HashSet::new();
            work.retain(|w| seen.insert(w.rel.clone()));
        }

        self.process_work_items(work, &mut report, progress)?;

        let commit = self.record_commit(dirty)?;
        report.commit = commit;
        self.maybe_compact(&mut report, progress)?;
        report.live_after = self.store.stats()?.live;
        report.wall_millis = wall.elapsed().as_millis() as u64;
        report.assert_reconciles();
        Ok(report)
    }

    fn apply_dirty_policy(&self, dirty: bool) -> Result<(), IndexError> {
        if !dirty {
            return Ok(());
        }
        match self.config.dirty_worktree {
            DirtyPolicy::Refuse => {
                let paths = self.git.dirty_paths().unwrap_or_default();
                Err(IndexError::DirtyRefused(paths.join(", ")))
            }
            DirtyPolicy::IndexWorktree | DirtyPolicy::IndexCommitOnly => Ok(()),
        }
    }

    fn record_commit(&mut self, dirty: bool) -> Result<String, IndexError> {
        let mut commit = self.git.head_commit()?;
        if dirty && matches!(self.config.dirty_worktree, DirtyPolicy::IndexWorktree) {
            commit.push_str(DIRTY_COMMIT_SUFFIX);
        }
        self.store.set_indexed_commit(&commit)?;
        Ok(commit)
    }

    fn maybe_compact(
        &mut self,
        report: &mut IndexReport,
        progress: &mut dyn Progress,
    ) -> Result<(), IndexError> {
        let stats = self.store.stats()?;
        if stats.dead_fraction > self.config.compact_threshold {
            progress.phase(Phase::Compacting, 0, None);
            let t = Instant::now();
            let mapping = self
                .store
                .live_rows()?
                .into_iter()
                .enumerate()
                .map(|(new_row, old_row)| (old_row, RowId::from_u64(new_row as u64)))
                .collect::<Vec<_>>();
            self.publication.prepare(&[PublicationOp::Renumber {
                mapping: mapping
                    .iter()
                    .map(|(old, new)| (old.get(), new.get()))
                    .collect(),
            }])?;
            let compact_report = self.store.compact()?;
            report.store_millis += t.elapsed().as_millis() as u64;
            if compact_report.row_mapping != mapping {
                return Err(IndexError::Other(
                    "compaction row mapping changed after publication prepare".into(),
                ));
            }
            self.lexical.renumber(&compact_report.row_mapping)?;
            self.lexical.commit()?;
            self.publication.clear()?;
            report.compacted = Some(compact_report);
        }
        Ok(())
    }

    fn apply_file_changes(
        &mut self,
        changes: &[FileChange],
        work: &mut Vec<WorkItem>,
        report: &mut IndexReport,
    ) -> Result<(), IndexError> {
        for change in changes {
            match change {
                FileChange::Added(path) | FileChange::Modified(path) => {
                    if let Some(item) = self.work_item_for_path(path, report)? {
                        work.push(item);
                    } else {
                        self.remove_file(path, report)?;
                    }
                }
                FileChange::Deleted(path) => {
                    self.remove_file(path, report)?;
                }
                FileChange::Renamed { from, to } => {
                    let rows = self.store.rows_for_file(from)?;
                    if !rows.is_empty() {
                        self.publication.prepare(&[PublicationOp::UpdateFile {
                            rows: rows.iter().map(|row| row.get()).collect(),
                        }])?;
                    }
                    let n = self.store.rekey_file(from, to)?;
                    if n > 0 {
                        let paths = rows
                            .iter()
                            .map(|row| (*row, to.clone()))
                            .collect::<Vec<_>>();
                        self.lexical.update_file_paths(&paths)?;
                        self.lexical.commit()?;
                        self.publication.clear()?;
                        // Re-parse destination so content changes bundled with a
                        // rename are reconciled; pure renames skip via hash set.
                        if let Some(item) = self.work_item_for_path(to, report)? {
                            work.push(item);
                        } else {
                            self.remove_file(to, report)?;
                        }
                    } else if let Some(item) = self.work_item_for_path(to, report)? {
                        work.push(item);
                    }
                }
            }
        }
        Ok(())
    }

    fn work_item_for_path(
        &self,
        rel: &str,
        report: &mut IndexReport,
    ) -> Result<Option<WorkItem>, IndexError> {
        let path = self.repo.join(rel);
        if self.exclusions.check_path(&path).is_some() {
            report.files_excluded += 1;
            return Ok(None);
        }
        let Some(language) = Language::from_path(Path::new(rel)) else {
            report.files_unsupported += 1;
            report.files_excluded += 1;
            return Ok(None);
        };

        let source = match self.read_source(rel) {
            Ok(Some(s)) => s,
            Ok(None) | Err(IndexError::Io(_)) => return Ok(None),
            Err(e) => return Err(e),
        };

        Ok(Some(WorkItem {
            rel: rel.replace('\\', "/"),
            language,
            source,
        }))
    }

    fn read_source(&self, rel: &str) -> Result<Option<String>, IndexError> {
        if matches!(self.config.dirty_worktree, DirtyPolicy::IndexCommitOnly) {
            let out = Command::new("git")
                .args(["show", &format!("HEAD:{rel}")])
                .current_dir(&self.repo)
                .output()
                .map_err(|e| IndexError::Git(e.to_string()))?;
            if !out.status.success() {
                return Ok(None);
            }
            let bytes = out.stdout;
            if bytes.len() as u64 > MAX_FILE_BYTES {
                return Ok(None);
            }
            let head_len = HEAD_SNIFF_BYTES.min(bytes.len());
            if self.exclusions.check_head(&bytes[..head_len]).is_some() {
                return Ok(None);
            }
            return Ok(std::str::from_utf8(&bytes).ok().map(str::to_string));
        }

        let path = self.repo.join(rel);
        if !path.is_file() {
            return Ok(None);
        }
        let meta = fs::metadata(&path)?;
        if self.exclusions.check_size(meta.len()).is_some() || meta.len() > MAX_FILE_BYTES {
            return Ok(None);
        }
        let bytes = fs::read(&path)?;
        let head_len = HEAD_SNIFF_BYTES.min(bytes.len());
        if self.exclusions.check_head(&bytes[..head_len]).is_some() {
            return Ok(None);
        }
        Ok(std::str::from_utf8(&bytes).ok().map(str::to_string))
    }

    fn collect_all_files(
        &self,
        report: &mut IndexReport,
    ) -> Result<(Vec<WorkItem>, HashSet<String>), IndexError> {
        let mut work = Vec::new();
        let mut present_files = HashSet::new();
        let mut stack = vec![self.repo.clone()];
        while let Some(dir) = stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                if path.file_name().is_some_and(|n| n == ".git") {
                    continue;
                }
                if path.is_dir() {
                    if self.exclusions.check_path(&path).is_some() {
                        // Count the directory skip once; do not open children.
                        report.files_excluded += 1;
                        continue;
                    }
                    stack.push(path);
                    continue;
                }
                if !path.is_file() {
                    continue;
                }
                report.files_seen += 1;
                if let Some(reason) = self.exclusions.check_path(&path) {
                    report.files_excluded += 1;
                    if matches!(reason, SkipReason::UnsupportedLanguage) {
                        report.files_unsupported += 1;
                    }
                    continue;
                }
                let Some(language) = Language::from_path(&path) else {
                    report.files_unsupported += 1;
                    report.files_excluded += 1;
                    continue;
                };
                let rel = path
                    .strip_prefix(&self.repo)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                match self.read_source(&rel) {
                    Ok(Some(source)) => {
                        present_files.insert(rel.clone());
                        work.push(WorkItem {
                            rel,
                            language,
                            source,
                        });
                    }
                    Ok(None) | Err(IndexError::Io(_)) => {
                        report.files_excluded += 1;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok((work, present_files))
    }

    fn reconcile_absent_files(
        &mut self,
        present_files: &HashSet<String>,
        report: &mut IndexReport,
    ) -> Result<(), IndexError> {
        for file in self.store.live_files()? {
            if !present_files.contains(&file) {
                self.remove_file(&file, report)?;
            }
        }
        Ok(())
    }

    fn remove_file(&mut self, file: &str, report: &mut IndexReport) -> Result<(), IndexError> {
        let rows = self.store.rows_for_file(file)?;
        let n = rows.len() as u64;
        if n == 0 {
            return Ok(());
        }
        self.publication.prepare(&[PublicationOp::Remove {
            rows: rows.iter().map(|row| row.get()).collect(),
        }])?;
        self.store.tombstone(&rows)?;
        self.lexical.remove(&rows)?;
        self.lexical.commit()?;
        self.publication.clear()?;
        report.chunks_removed += n;
        report.files_indexed += 1;
        Ok(())
    }

    fn process_work_items(
        &mut self,
        work: Vec<WorkItem>,
        report: &mut IndexReport,
        progress: &mut dyn Progress,
    ) -> Result<(), IndexError> {
        if work.is_empty() {
            return Ok(());
        }

        let threads = if self.config.parse_threads == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        } else {
            self.config.parse_threads
        };

        let mut existing: HashMap<String, HashMap<ContentHash, Vec<RowId>>> = HashMap::new();
        for item in &work {
            let rows = self.store.rows_for_file(&item.rel)?;
            let mut map: HashMap<ContentHash, Vec<RowId>> = HashMap::new();
            for row in rows {
                if let Some(rec) = self.store.get(row)? {
                    map.entry(rec.content_hash).or_default().push(row);
                }
            }
            existing.insert(item.rel.clone(), map);
        }

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .map_err(|e| IndexError::Other(e.to_string()))?;

        let (tx, rx) = bounded::<ParsedFile>(threads.saturating_mul(4).max(8));
        let parse_millis_atomic = AtomicU64::new(0);
        let total = work.len() as u64;
        progress.phase(Phase::Parsing, 0, Some(total));

        let existing_for_parse = existing.clone();
        let mut consume_err: Option<IndexError> = None;
        std::thread::scope(|scope| {
            scope.spawn(|| {
                pool.install(|| {
                    work.into_par_iter().for_each_with(tx, |tx, item| {
                        let t0 = Instant::now();
                        let mut chunker = match Chunker::new() {
                            Ok(c) => c,
                            Err(_) => {
                                let _ = tx.send(ParsedFile::Unparsed);
                                return;
                            }
                        };
                        let file = chunker.chunk_file(&item.rel, item.language, &item.source);
                        let millis = t0.elapsed().as_millis() as u64;
                        parse_millis_atomic.fetch_add(millis, Ordering::Relaxed);

                        if !file.diagnostics.is_empty() && file.chunks.is_empty() {
                            let _ = tx.send(ParsedFile::Unparsed);
                            return;
                        }

                        let mut new_counts: HashMap<ContentHash, usize> = HashMap::new();
                        for chunk in &file.chunks {
                            *new_counts.entry(chunk.record.content_hash).or_default() += 1;
                        }
                        let old_counts: HashMap<ContentHash, usize> = existing_for_parse
                            .get(&item.rel)
                            .map(|m| m.iter().map(|(hash, rows)| (*hash, rows.len())).collect())
                            .unwrap_or_default();

                        if new_counts == old_counts {
                            let _ = tx.send(ParsedFile::SkipIdentical {
                                reused: file.chunks.len() as u64,
                            });
                            return;
                        }

                        let chunks: Vec<_> = file
                            .chunks
                            .into_iter()
                            .map(|c| (c.record, c.body))
                            .collect();
                        let _ = tx.send(ParsedFile::Ok {
                            rel: item.rel,
                            chunks,
                        });
                    });
                });
            });

            let mut pending: Vec<(ChunkRecord, String)> = Vec::new();
            let mut done = 0u64;

            for parsed in rx {
                done += 1;
                progress.phase(Phase::Parsing, done, Some(total));
                match parsed {
                    ParsedFile::Unparsed => {
                        report.files_unparsed += 1;
                    }
                    ParsedFile::SkipIdentical { reused } => {
                        report.files_indexed += 1;
                        report.chunks_reused += reused;
                    }
                    ParsedFile::Ok { rel, chunks } => {
                        report.files_indexed += 1;
                        let mut old = existing.get(&rel).cloned().unwrap_or_default();
                        let mut new_counts: HashMap<ContentHash, usize> = HashMap::new();
                        for (record, _) in &chunks {
                            *new_counts.entry(record.content_hash).or_default() += 1;
                        }

                        // Tombstone occurrences that vanished from this file.
                        let mut to_tombstone = Vec::new();
                        for (hash, rows) in &old {
                            let keep = new_counts.get(hash).copied().unwrap_or(0);
                            to_tombstone.extend(rows.iter().skip(keep).copied());
                        }
                        if !to_tombstone.is_empty() {
                            let t = Instant::now();
                            if let Err(e) = self.publication.prepare(&[PublicationOp::Remove {
                                rows: to_tombstone.iter().map(|row| row.get()).collect(),
                            }]) {
                                consume_err = Some(e);
                                break;
                            }
                            if let Err(e) = self.store.tombstone(&to_tombstone) {
                                consume_err = Some(e.into());
                                break;
                            }
                            if let Err(e) = self
                                .lexical
                                .remove(&to_tombstone)
                                .and_then(|_| self.lexical.commit())
                            {
                                consume_err = Some(e.into());
                                break;
                            }
                            if let Err(e) = self.publication.clear() {
                                consume_err = Some(e);
                                break;
                            }
                            report.store_millis += t.elapsed().as_millis() as u64;
                            report.chunks_removed += to_tombstone.len() as u64;
                        }

                        for (rec, body) in chunks {
                            if old.get_mut(&rec.content_hash).and_then(Vec::pop).is_some() {
                                report.chunks_reused += 1;
                                continue;
                            }
                            pending.push((rec, body));
                            if pending.len() >= self.config.embed_batch
                                && let Err(e) = self.flush_batch(&mut pending, report, progress)
                            {
                                consume_err = Some(e);
                                break;
                            }
                        }
                        if consume_err.is_some() {
                            break;
                        }
                    }
                }
            }

            if consume_err.is_none()
                && !pending.is_empty()
                && let Err(e) = self.flush_batch(&mut pending, report, progress)
            {
                consume_err = Some(e);
            }
        });

        report.parse_millis += parse_millis_atomic.load(Ordering::Relaxed);
        match consume_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn flush_batch(
        &mut self,
        pending: &mut Vec<(ChunkRecord, String)>,
        report: &mut IndexReport,
        progress: &mut dyn Progress,
    ) -> Result<(), IndexError> {
        if pending.is_empty() {
            return Ok(());
        }
        progress.phase(Phase::Embedding, report.embeddings_computed, None);
        // A batch can contain identical normalized bodies from different
        // files. Compute one embedding per hash, but retain one store and
        // lexical row for every file occurrence so location metadata survives.
        let items = std::mem::take(pending);
        let mut seen = HashSet::new();
        let mut unique = Vec::with_capacity(items.len());
        for item in &items {
            if seen.insert(item.0.content_hash) {
                unique.push(item.clone());
            }
        }
        if unique.is_empty() {
            return Ok(());
        }
        let texts: Vec<&str> = unique.iter().map(|(_, b)| b.as_str()).collect();
        let t_embed = Instant::now();
        let embeddings = self.embedder.embed(&texts, Role::Document)?;
        report.embed_millis += t_embed.elapsed().as_millis() as u64;

        assert_eq!(embeddings.len(), unique.len());
        let mut embedding_by_hash = HashMap::with_capacity(unique.len());
        for (item, embedding) in unique.iter().zip(embeddings.iter()) {
            embedding_by_hash.insert(item.0.content_hash, embedding.clone());
        }

        let lexical_docs: Vec<_> = items
            .iter()
            .map(|(rec, body)| {
                (
                    rec.clone(),
                    LexicalDoc {
                        symbol: rec.symbol.clone(),
                        signature: rec.signature.clone(),
                        doc_first_line: rec.doc_first_line.clone(),
                        file: rec.file.clone(),
                        body: body.clone(),
                    },
                )
            })
            .collect();
        let mut batch = Vec::with_capacity(items.len());
        for (rec, _) in &items {
            let emb = embedding_by_hash
                .get(&rec.content_hash)
                .expect("embedding exists for every pending content hash");
            if emb.truncated {
                report.chunks_truncated += 1;
            }
            batch.push((rec.clone(), emb.vector.clone()));
        }

        let base_row = self.store.matrix().rows();
        let publication_ops = lexical_docs
            .iter()
            .enumerate()
            .map(|(offset, (_, doc))| PublicationOp::Add {
                row: base_row + offset as u64,
                doc: doc.clone(),
            })
            .collect::<Vec<_>>();
        self.publication.prepare(&publication_ops)?;

        progress.phase(Phase::Storing, report.chunks_added, None);
        let t_store = Instant::now();
        let ids = self.store.insert_batch(&batch)?;
        report.store_millis += t_store.elapsed().as_millis() as u64;

        let expected_ids = publication_ops
            .iter()
            .map(|op| match op {
                PublicationOp::Add { row, .. } => RowId::from_u64(*row),
                _ => unreachable!(),
            })
            .collect::<Vec<_>>();
        if ids != expected_ids {
            return Err(IndexError::Other(
                "store row allocation changed after publication prepare".into(),
            ));
        }

        self.lexical.add_batch(
            &ids.into_iter()
                .zip(lexical_docs.into_iter().map(|(_, doc)| doc))
                .collect::<Vec<_>>(),
        )?;
        self.lexical.commit()?;
        self.publication.clear()?;

        let novel = batch.len() as u64;
        report.chunks_added += novel;
        report.embeddings_computed += unique.len() as u64;
        self.batches_committed += 1;

        if let Some(limit) = self.interrupt_after_batches
            && self.batches_committed >= limit
        {
            return Err(IndexError::Interrupted(format!(
                "stopped after {limit} batches"
            )));
        }
        Ok(())
    }
}

fn empty_report(live_before: u64) -> IndexReport {
    IndexReport {
        commit: String::new(),
        files_seen: 0,
        files_indexed: 0,
        files_excluded: 0,
        files_unsupported: 0,
        files_unparsed: 0,
        chunks_added: 0,
        chunks_reused: 0,
        chunks_removed: 0,
        embeddings_computed: 0,
        chunks_truncated: 0,
        parse_millis: 0,
        embed_millis: 0,
        store_millis: 0,
        wall_millis: 0,
        compacted: None,
        live_before,
        live_after: live_before,
    }
}

/// Ensure verify is Ok after mutating runs.
pub fn require_verify_ok(store: &ChunkStore) -> Result<(), IndexError> {
    match store.verify()? {
        Integrity::Ok { .. } => Ok(()),
        other => Err(IndexError::Other(format!("verify failed: {other:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use half::f16;
    use retrieval::{LexicalDoc, LexicalIndex};
    use storage::{ChunkRecord, ChunkStore, ContentHash};
    use tempfile::tempdir;

    use super::{PublicationJournal, PublicationOp};

    fn record() -> ChunkRecord {
        ChunkRecord {
            repository: "test".into(),
            file: "src/lib.rs".into(),
            language: "rust".into(),
            symbol: "answer".into(),
            symbol_type: "function".into(),
            signature: "fn answer()".into(),
            doc_first_line: Some("Returns the answer".into()),
            line_start: 1,
            line_end: 3,
            content_hash: ContentHash::of(b"answer"),
        }
    }

    fn document() -> LexicalDoc {
        LexicalDoc {
            symbol: "answer".into(),
            signature: "fn answer()".into(),
            doc_first_line: Some("Returns the answer".into()),
            file: "src/lib.rs".into(),
            body: "fn answer() { 42 }".into(),
        }
    }

    #[test]
    fn publication_recovery_replays_a_store_committed_add() {
        let dir = tempdir().unwrap();
        let mut store = ChunkStore::create(dir.path(), 1, "model").unwrap();
        let row = store
            .insert_batch(&[(record(), vec![f16::from_f32(1.0)])])
            .unwrap()[0];
        let journal = PublicationJournal::new(&store);
        journal
            .prepare(&[PublicationOp::Add {
                row: row.get(),
                doc: document(),
            }])
            .unwrap();

        let mut lexical = LexicalIndex::open(dir.path()).unwrap();
        journal.recover(&store, &mut lexical).unwrap();

        assert_eq!(lexical.num_docs(), 1);
        assert!(!journal.path.exists());
    }

    #[test]
    fn publication_recovery_does_not_apply_remove_before_store_tombstone() {
        let dir = tempdir().unwrap();
        let mut store = ChunkStore::create(dir.path(), 1, "model").unwrap();
        let row = store
            .insert_batch(&[(record(), vec![f16::from_f32(1.0)])])
            .unwrap()[0];
        let mut lexical = LexicalIndex::open(dir.path()).unwrap();
        lexical.add_batch(&[(row, document())]).unwrap();
        lexical.commit().unwrap();

        let journal = PublicationJournal::new(&store);
        journal
            .prepare(&[PublicationOp::Remove {
                rows: vec![row.get()],
            }])
            .unwrap();
        journal.recover(&store, &mut lexical).unwrap();

        assert_eq!(lexical.num_docs(), 1);
    }
}
