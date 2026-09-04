mod common;

use common::{CommitSpec, TempRepo};
use indexing::{DIRTY_COMMIT_SUFFIX, DirtyPolicy, IndexConfig, IndexError, Indexer, NullProgress};
use inference::{Embedder, MockEmbedder};
use storage::{ChunkStore, Integrity};

const DIMS: u32 = 8;

struct Harness {
    _store_dir: tempfile::TempDir,
    embedder: MockEmbedder,
    repo: TempRepo,
}

impl Harness {
    fn new(commits: &[CommitSpec]) -> Self {
        let embedder = MockEmbedder::new(DIMS).with_batch_limit(4);
        let repo = TempRepo::build(commits);
        let store_dir = tempfile::tempdir().unwrap();
        Self {
            _store_dir: store_dir,
            embedder,
            repo,
        }
    }

    fn store_path(&self) -> &std::path::Path {
        self._store_dir.path()
    }

    fn indexer(&self, config: IndexConfig) -> Indexer<'_> {
        let store = ChunkStore::create(self.store_path(), DIMS, self.embedder.model_id()).unwrap();
        Indexer::open(store, &self.embedder, self.repo.path(), config).unwrap()
    }

    fn reopen(&self, config: IndexConfig) -> Indexer<'_> {
        let store = ChunkStore::open(self.store_path()).unwrap();
        Indexer::open(store, &self.embedder, self.repo.path(), config).unwrap()
    }
}

fn sample_fn(name: &str) -> String {
    format!("pub fn {name}() {{\n    let x = 1;\n}}\n")
}

fn many_files_init(n: usize) -> CommitSpec {
    let mut spec = CommitSpec::new("init");
    for i in 0..n {
        let name = format!("f{i}");
        spec = spec.file(format!("f{i}.rs"), sample_fn(&name));
    }
    // Target file with one function we'll edit later.
    spec.file("target.rs", sample_fn("target_fn"))
}

#[test]
fn update_body_edit_reembeds_one_chunk() {
    let h = Harness::new(&[many_files_init(20)]);
    let mut indexer = h.indexer(IndexConfig::default());
    indexer.index_all(&mut NullProgress).unwrap();
    drop(indexer);

    h.repo.apply_commit(
        &CommitSpec::new("edit target")
            .file("target.rs", "pub fn target_fn() {\n    let x = 99;\n}\n"),
    );

    let mut indexer = h.reopen(IndexConfig::default());
    let report = indexer.update(&mut NullProgress).unwrap();
    assert_eq!(report.embeddings_computed, 1, "{report:?}");
    assert_eq!(report.chunks_added, 1);
    assert_eq!(report.chunks_removed, 1);
    assert_eq!(report.files_indexed, 1);
    assert!(matches!(
        indexer.store().verify().unwrap(),
        Integrity::Ok { .. }
    ));
}

#[test]
fn update_rename_reembeds_nothing() {
    let h = Harness::new(&[CommitSpec::new("init").file("old.rs", sample_fn("renamed"))]);
    let mut indexer = h.indexer(IndexConfig::default());
    indexer.index_all(&mut NullProgress).unwrap();
    let before = indexer.store().stats().unwrap().live;
    drop(indexer);

    h.repo
        .apply_commit(&CommitSpec::new("rename").rename("old.rs", "new.rs"));

    let mut indexer = h.reopen(IndexConfig::default());
    let report = indexer.update(&mut NullProgress).unwrap();
    assert_eq!(report.embeddings_computed, 0, "{report:?}");
    assert_eq!(report.chunks_added, 0);
    assert!(indexer.store().rows_for_file("old.rs").unwrap().is_empty());
    let rows = indexer.store().rows_for_file("new.rs").unwrap();
    assert!(!rows.is_empty());
    for row in rows {
        let rec = indexer.store().get(row).unwrap().unwrap();
        assert_eq!(rec.file, "new.rs");
    }
    assert_eq!(indexer.store().stats().unwrap().live, before);
}

#[test]
fn update_delete_tombstones_file_chunks() {
    let h = Harness::new(&[CommitSpec::new("init")
        .file("keep.rs", sample_fn("keep"))
        .file("gone.rs", sample_fn("gone"))]);
    // Keep the same indexer across the commit so we never rely on WAL reopen timing.
    let mut indexer = h.indexer(IndexConfig {
        compact_threshold: 1.0, // isolate tombstone counting from compaction
        ..IndexConfig::default()
    });
    indexer.index_all(&mut NullProgress).unwrap();
    let gone_count = indexer.store().rows_for_file("gone.rs").unwrap().len() as u64;
    assert!(gone_count >= 1);
    let dead_before = indexer.store().stats().unwrap().dead;
    let base = indexer
        .store()
        .indexed_commit()
        .unwrap()
        .expect("indexed commit");

    h.repo
        .apply_commit(&CommitSpec::new("delete").delete("gone.rs"));

    let changes = indexing::RepoGit::open(h.repo.path())
        .unwrap()
        .changes_since(&base)
        .unwrap();
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, indexing::FileChange::Deleted(p) if p == "gone.rs")),
        "changes={changes:?}"
    );

    let report = indexer.update(&mut NullProgress).unwrap();
    assert_eq!(report.chunks_removed, gone_count, "{report:?}");
    assert_eq!(
        indexer.store().stats().unwrap().dead,
        dead_before + gone_count
    );
    assert!(indexer.store().rows_for_file("gone.rs").unwrap().is_empty());
}

#[test]
fn full_index_tombstones_files_missing_from_the_walk() {
    let h = Harness::new(&[CommitSpec::new("init")
        .file("keep.rs", sample_fn("keep"))
        .file("gone.rs", sample_fn("gone"))]);
    let mut indexer = h.indexer(IndexConfig {
        compact_threshold: 1.0,
        ..IndexConfig::default()
    });
    indexer.index_all(&mut NullProgress).unwrap();
    let gone = indexer.store().rows_for_file("gone.rs").unwrap();
    assert!(!gone.is_empty());

    std::fs::remove_file(h.repo.path().join("gone.rs")).unwrap();
    let report = indexer.index_all(&mut NullProgress).unwrap();

    assert_eq!(report.chunks_removed, gone.len() as u64, "{report:?}");
    assert!(indexer.store().rows_for_file("gone.rs").unwrap().is_empty());
    assert_eq!(indexer.store().stats().unwrap().live, 1);
}

#[test]
fn update_reorder_reembeds_nothing() {
    // Trailing sentinel keeps neither function at EOF so tree-sitter ranges
    // (and therefore content hashes) stay stable across reorder.
    let order_ab = "\
pub fn aaa() {
    let x = 1;
}

pub fn bbb() {
    let y = 2;
}

// end
";
    let order_ba = "\
pub fn bbb() {
    let y = 2;
}

pub fn aaa() {
    let x = 1;
}

// end
";

    let h = Harness::new(&[CommitSpec::new("init").file("ab.rs", order_ab)]);
    let mut indexer = h.indexer(IndexConfig {
        compact_threshold: 1.0,
        ..IndexConfig::default()
    });
    let first = indexer.index_all(&mut NullProgress).unwrap();
    let live = first.live_after;

    h.repo
        .apply_commit(&CommitSpec::new("reorder").file("ab.rs", order_ba));

    let report = indexer.update(&mut NullProgress).unwrap();
    assert_eq!(report.embeddings_computed, 0, "{report:?}");
    assert_eq!(indexer.store().stats().unwrap().live, live);
}

#[test]
fn indexed_commit_advances_only_after_success() {
    let h = Harness::new(&[
        CommitSpec::new("init").file("a.rs", sample_fn("a")),
        CommitSpec::new("second").file("b.rs", sample_fn("b")),
    ]);
    // Index only first commit state by resetting? Simpler: index current (both files),
    // then add third and interrupt update.
    let mut indexer = h.indexer(IndexConfig::default());
    let r = indexer.index_all(&mut NullProgress).unwrap();
    let base = r.commit.clone();
    assert_eq!(base, h.repo.head());
    drop(indexer);

    h.repo
        .apply_commit(&CommitSpec::new("third").file("c.rs", sample_fn("c")));
    let new_head = h.repo.head();

    let mut indexer = h.reopen(IndexConfig {
        embed_batch: 1,
        ..IndexConfig::default()
    });
    indexer.set_interrupt_after_batches(Some(1));
    let err = indexer.update(&mut NullProgress).unwrap_err();
    assert!(matches!(err, IndexError::Interrupted(_)), "{err:?}");
    assert_eq!(
        indexer.store().indexed_commit().unwrap().as_deref(),
        Some(base.as_str())
    );
    drop(indexer);

    let mut indexer = h.reopen(IndexConfig::default());
    let report = indexer.update(&mut NullProgress).unwrap();
    assert_eq!(report.commit, new_head);
    assert_eq!(
        indexer.store().indexed_commit().unwrap().as_deref(),
        Some(new_head.as_str())
    );
}

#[test]
fn dirty_policy_index_worktree_marks_commit() {
    let h = Harness::new(&[CommitSpec::new("init").file("a.rs", sample_fn("a"))]);
    h.repo.write_uncommitted("a.rs", &sample_fn("a_dirty"));

    let mut indexer = h.indexer(IndexConfig {
        dirty_worktree: DirtyPolicy::IndexWorktree,
        ..IndexConfig::default()
    });
    let report = indexer.index_all(&mut NullProgress).unwrap();
    assert!(
        report.commit.ends_with(DIRTY_COMMIT_SUFFIX),
        "commit={}",
        report.commit
    );

    // Another dirty edit then update should reconcile.
    h.repo.write_uncommitted("a.rs", &sample_fn("a_dirtiera"));
    let report2 = indexer.update(&mut NullProgress).unwrap();
    assert!(report2.commit.ends_with(DIRTY_COMMIT_SUFFIX));
}

#[test]
fn first_update_reconciles_dirty_paths_after_clean_index() {
    let h = Harness::new(&[CommitSpec::new("init").file("a.rs", sample_fn("original"))]);
    let mut indexer = h.indexer(IndexConfig::default());
    indexer.index_all(&mut NullProgress).unwrap();
    drop(indexer);

    std::fs::write(h.repo.path().join("a.rs"), sample_fn("working_tree_change")).unwrap();
    let mut indexer = h.reopen(IndexConfig::default());
    let report = indexer.update(&mut NullProgress).unwrap();
    assert_eq!(report.embeddings_computed, 1, "{report:?}");
    assert!(indexer.store().rows_for_file("a.rs").is_ok());
}

#[test]
fn dirty_deletion_tombstones_the_old_file_rows() {
    let h = Harness::new(&[CommitSpec::new("init").file("gone.rs", sample_fn("gone"))]);
    let mut indexer = h.indexer(IndexConfig {
        compact_threshold: 1.0,
        ..IndexConfig::default()
    });
    indexer.index_all(&mut NullProgress).unwrap();
    let gone = indexer.store().rows_for_file("gone.rs").unwrap();
    assert!(!gone.is_empty());

    std::fs::remove_file(h.repo.path().join("gone.rs")).unwrap();
    let report = indexer.update(&mut NullProgress).unwrap();

    assert_eq!(report.chunks_removed, gone.len() as u64, "{report:?}");
    assert!(indexer.store().rows_for_file("gone.rs").unwrap().is_empty());
}

#[test]
fn duplicate_normalized_chunks_keep_both_locations_but_embed_once() {
    let h = Harness::new(&[CommitSpec::new("init")
        .file("a.rs", sample_fn("same"))
        .file("b.rs", sample_fn("same"))]);
    let mut indexer = h.indexer(IndexConfig::default());
    let report = indexer.index_all(&mut NullProgress).unwrap();
    assert_eq!(report.chunks_added, 2, "{report:?}");
    assert_eq!(report.embeddings_computed, 1, "{report:?}");
    assert_eq!(indexer.store().rows_for_file("a.rs").unwrap().len(), 1);
    assert_eq!(indexer.store().rows_for_file("b.rs").unwrap().len(), 1);
    report.assert_reconciles();
}

#[test]
fn dirty_policy_refuse_errors() {
    let h = Harness::new(&[CommitSpec::new("init").file("a.rs", sample_fn("a"))]);
    h.repo.write_uncommitted("a.rs", &sample_fn("dirty"));
    let mut indexer = h.indexer(IndexConfig {
        dirty_worktree: DirtyPolicy::Refuse,
        ..IndexConfig::default()
    });
    let err = indexer.index_all(&mut NullProgress).unwrap_err();
    match err {
        IndexError::DirtyRefused(msg) => assert!(msg.contains("a.rs"), "{msg}"),
        other => panic!("expected DirtyRefused, got {other:?}"),
    }
}

#[test]
fn compaction_triggers_above_threshold() {
    // One file with enough chunks that tombstoning most crosses 0.20.
    // Simpler: index several files, delete most, update with low threshold.
    let mut init = CommitSpec::new("init");
    for i in 0..10 {
        init = init.file(format!("f{i}.rs"), sample_fn(&format!("f{i}")));
    }
    let h = Harness::new(&[init]);
    let mut indexer = h.indexer(IndexConfig {
        compact_threshold: 0.20,
        ..IndexConfig::default()
    });
    let first = indexer.index_all(&mut NullProgress).unwrap();
    let live = first.live_after;
    drop(indexer);

    let mut del = CommitSpec::new("delete many");
    for i in 0..8 {
        del = del.delete(format!("f{i}.rs"));
    }
    h.repo.apply_commit(&del);

    let mut indexer = h.reopen(IndexConfig {
        compact_threshold: 0.20,
        ..IndexConfig::default()
    });
    let report = indexer.update(&mut NullProgress).unwrap();
    assert!(report.compacted.is_some(), "{report:?}");
    let compact = report.compacted.unwrap();
    assert_eq!(compact.live_after, live - report.chunks_removed);
    // After compact, live count unchanged from post-delete live.
    assert_eq!(indexer.store().stats().unwrap().live, compact.live_after);
    assert!(matches!(
        indexer.store().verify().unwrap(),
        Integrity::Ok { .. }
    ));
}

#[test]
fn parse_failure_leaves_rows_untouched() {
    let h = Harness::new(&[CommitSpec::new("init").file("a.rs", sample_fn("ok"))]);
    let mut indexer = h.indexer(IndexConfig::default());
    indexer.index_all(&mut NullProgress).unwrap();
    let rows_before = indexer.store().rows_for_file("a.rs").unwrap();
    let live = indexer.store().stats().unwrap().live;
    drop(indexer);

    // Introduce severe syntax errors.
    h.repo
        .apply_commit(&CommitSpec::new("break").file("a.rs", "fn {{{{\n!!!!!!\n{{{{{{\n"));

    let mut indexer = h.reopen(IndexConfig::default());
    let report = indexer.update(&mut NullProgress).unwrap();
    assert!(report.files_unparsed >= 1, "{report:?}");
    assert_eq!(indexer.store().stats().unwrap().live, live);
    assert_eq!(
        indexer.store().rows_for_file("a.rs").unwrap().len(),
        rows_before.len()
    );
}

#[test]
fn interrupt_and_resume_completes() {
    let mut init = CommitSpec::new("init");
    for i in 0..12 {
        init = init.file(format!("f{i}.rs"), sample_fn(&format!("fn{i}")));
    }
    let h = Harness::new(&[init]);

    let mut indexer = h.indexer(IndexConfig {
        embed_batch: 2,
        ..IndexConfig::default()
    });
    indexer.set_interrupt_after_batches(Some(3));
    let err = indexer.index_all(&mut NullProgress).unwrap_err();
    assert!(matches!(err, IndexError::Interrupted(_)));
    assert!(matches!(
        indexer.store().verify().unwrap(),
        Integrity::Ok { .. }
    ));
    let partial_live = indexer.store().stats().unwrap().live;
    assert!(partial_live > 0);
    drop(indexer);

    let mut indexer = h.reopen(IndexConfig {
        embed_batch: 2,
        ..IndexConfig::default()
    });
    let report = indexer.index_all(&mut NullProgress).unwrap();
    assert!(matches!(
        indexer.store().verify().unwrap(),
        Integrity::Ok { .. }
    ));
    // Final live should equal a clean full index.
    let final_live = report.live_after;
    drop(indexer);

    let store_dir2 = tempfile::tempdir().unwrap();
    let embedder = MockEmbedder::new(DIMS).with_batch_limit(4);
    let store = ChunkStore::create(store_dir2.path(), DIMS, embedder.model_id()).unwrap();
    let mut clean = Indexer::open(store, &embedder, h.repo.path(), IndexConfig::default()).unwrap();
    let clean_report = clean.index_all(&mut NullProgress).unwrap();
    assert_eq!(final_live, clean_report.live_after);
}
