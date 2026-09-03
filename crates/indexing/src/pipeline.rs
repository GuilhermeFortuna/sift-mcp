//! End-to-end repository indexing: walk, parse, embed, store.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crossbeam_channel::bounded;
use inference::{Embedder, Role};
use rayon::prelude::*;
use retrieval::{LexicalDoc, LexicalIndex};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        let lexical = LexicalIndex::open(store.dir())?;
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
        let work = self.collect_all_files(&mut report)?;

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

        if dirty
            && matches!(self.config.dirty_worktree, DirtyPolicy::IndexWorktree)
            && base.ends_with(DIRTY_COMMIT_SUFFIX)
        {
            // Prior index included dirty files — fully reconcile those paths.
            for path in self.git.dirty_paths()? {
                if let Some(item) = self.work_item_for_path(&path, &mut report)? {
                    work.push(item);
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
            let compact_report = self.store.compact()?;
            report.store_millis += t.elapsed().as_millis() as u64;
            self.lexical.renumber(&compact_report.row_mapping)?;
            self.lexical.commit()?;
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
                    }
                }
                FileChange::Deleted(path) => {
                    let rows = self.store.rows_for_file(path)?;
                    let n = rows.len() as u64;
                    if n > 0 {
                        self.store.tombstone(&rows)?;
                        self.lexical.remove(&rows)?;
                        self.lexical.commit()?;
                        report.chunks_removed += n;
                        report.files_indexed += 1;
                    }
                }
                FileChange::Renamed { from, to } => {
                    let rows = self.store.rows_for_file(from)?;
                    let n = self.store.rekey_file(from, to)?;
                    if n > 0 {
                        let paths = rows
                            .iter()
                            .map(|row| (*row, to.clone()))
                            .collect::<Vec<_>>();
                        self.lexical.update_file_paths(&paths)?;
                        self.lexical.commit()?;
                        // Re-parse destination so content changes bundled with a
                        // rename are reconciled; pure renames skip via hash set.
                        if let Some(item) = self.work_item_for_path(to, report)? {
                            work.push(item);
                        } else {
                            report.files_indexed += 1;
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

        let source = match self.read_source(rel)? {
            Some(s) => s,
            None => return Ok(None),
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

    fn collect_all_files(&self, report: &mut IndexReport) -> Result<Vec<WorkItem>, IndexError> {
        let mut work = Vec::new();
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
                match self.read_source(&rel)? {
                    Some(source) => work.push(WorkItem {
                        rel,
                        language,
                        source,
                    }),
                    None => {
                        report.files_excluded += 1;
                    }
                }
            }
        }
        Ok(work)
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

        let mut existing: HashMap<String, HashMap<ContentHash, RowId>> = HashMap::new();
        for item in &work {
            let rows = self.store.rows_for_file(&item.rel)?;
            let mut map = HashMap::new();
            for row in rows {
                if let Some(rec) = self.store.get(row)? {
                    map.insert(rec.content_hash, row);
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

                        let new_hashes: HashSet<ContentHash> =
                            file.chunks.iter().map(|c| c.record.content_hash).collect();
                        let old_hashes: HashSet<ContentHash> = existing_for_parse
                            .get(&item.rel)
                            .map(|m| m.keys().copied().collect())
                            .unwrap_or_default();

                        if new_hashes == old_hashes {
                            let _ = tx.send(ParsedFile::SkipIdentical {
                                reused: new_hashes.len() as u64,
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
            let mut pending_rels: Vec<String> = Vec::new();
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
                        let old = existing.get(&rel).cloned().unwrap_or_default();
                        let new_hashes: HashSet<ContentHash> =
                            chunks.iter().map(|(r, _)| r.content_hash).collect();

                        // Tombstone hashes that vanished from this file.
                        let to_tombstone: Vec<RowId> = old
                            .iter()
                            .filter(|(h, _)| !new_hashes.contains(h))
                            .map(|(_, id)| *id)
                            .collect();
                        if !to_tombstone.is_empty() {
                            let t = Instant::now();
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
                            report.store_millis += t.elapsed().as_millis() as u64;
                            report.chunks_removed += to_tombstone.len() as u64;
                        }

                        for (rec, body) in chunks {
                            if old.contains_key(&rec.content_hash) {
                                report.chunks_reused += 1;
                                continue;
                            }
                            pending.push((rec, body));
                            pending_rels.push(rel.clone());
                            if pending.len() >= self.config.embed_batch {
                                if let Err(e) = self.flush_batch(&mut pending, report, progress) {
                                    consume_err = Some(e);
                                    break;
                                }
                                pending_rels.clear();
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
        let texts: Vec<&str> = pending.iter().map(|(_, b)| b.as_str()).collect();
        let t_embed = Instant::now();
        let embeddings = self.embedder.embed(&texts, Role::Document)?;
        report.embed_millis += t_embed.elapsed().as_millis() as u64;

        assert_eq!(embeddings.len(), pending.len());
        let lexical_docs: Vec<_> = pending
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
        let mut batch = Vec::with_capacity(pending.len());
        for ((rec, _), emb) in pending.iter().zip(embeddings) {
            if emb.truncated {
                report.chunks_truncated += 1;
            }
            batch.push((rec.clone(), emb.vector));
        }

        progress.phase(Phase::Storing, report.chunks_added, None);
        let t_store = Instant::now();
        let ids = self.store.insert_batch(&batch)?;
        report.store_millis += t_store.elapsed().as_millis() as u64;

        self.lexical.add_batch(
            &ids.into_iter()
                .zip(lexical_docs.into_iter().map(|(_, doc)| doc))
                .collect::<Vec<_>>(),
        )?;
        self.lexical.commit()?;
        pending.clear();

        let novel = batch.len() as u64;
        // Count as added only hashes that were not already live — insert_batch reuses.
        // We only put novel hashes in pending, so all are added.
        report.chunks_added += novel;
        report.embeddings_computed += novel;
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
