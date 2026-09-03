mod common;

use common::{CommitSpec, TempRepo};
use indexing::{IndexConfig, Indexer, NullProgress};
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

#[test]
fn index_all_fixture_produces_expected_symbols() {
    let h = Harness::new(&[CommitSpec::new("init")
        .file("a.rs", sample_fn("alpha"))
        .file("b.rs", sample_fn("beta"))]);
    let mut indexer = h.indexer(IndexConfig::default());
    let report = indexer.index_all(&mut NullProgress).unwrap();

    assert_eq!(report.files_indexed, 2);
    assert!(
        report.chunks_added >= 2,
        "chunks_added={}",
        report.chunks_added
    );
    assert_eq!(report.embeddings_computed, report.chunks_added);
    assert!(matches!(
        indexer.store().verify().unwrap(),
        Integrity::Ok { .. }
    ));

    let rows_a = indexer.store().rows_for_file("a.rs").unwrap();
    assert!(!rows_a.is_empty());
    let symbols: Vec<_> = rows_a
        .iter()
        .filter_map(|r| indexer.store().get(*r).unwrap())
        .map(|c| c.symbol)
        .collect();
    assert!(symbols.iter().any(|s| s == "alpha"), "got {symbols:?}");
}

#[test]
fn excluded_paths_never_opened_and_counted() {
    let repo = TempRepo::build(&[CommitSpec::new("init").file("ok.rs", sample_fn("ok"))]);
    // Create unreadable excluded sentinel under node_modules.
    let nm = repo.path().join("node_modules");
    std::fs::create_dir_all(&nm).unwrap();
    let sentinel = nm.join("secret.rs");
    std::fs::write(&sentinel, b"fn secret() {}").unwrap();
    let mut perms = std::fs::metadata(&sentinel).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o000);
    std::fs::set_permissions(&sentinel, perms).unwrap();

    // Unsupported extension
    std::fs::write(repo.path().join("notes.txt"), "hello").unwrap();
    // Commit so repo is clean for indexing walk (dirty policy default indexes worktree anyway)
    std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo.path())
        .status()
        .unwrap();
    // Don't commit — IndexWorktree indexes worktree. Restore readable after test.

    let store_dir = tempfile::tempdir().unwrap();
    let embedder = MockEmbedder::new(DIMS);
    let store = ChunkStore::create(store_dir.path(), DIMS, embedder.model_id()).unwrap();
    let mut indexer = Indexer::open(store, &embedder, repo.path(), IndexConfig::default()).unwrap();
    let report = indexer.index_all(&mut NullProgress).unwrap();

    assert!(report.files_excluded >= 1);
    assert!(report.files_unsupported >= 1);
    // Reaching here without I/O error means we never opened the 000 sentinel.
    let mut perms = std::fs::metadata(&sentinel).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&sentinel, perms).unwrap();
}

#[test]
fn counters_reconcile_after_index_all() {
    let h = Harness::new(&[CommitSpec::new("init")
        .file("a.rs", sample_fn("a"))
        .file("b.rs", sample_fn("b"))]);
    let mut indexer = h.indexer(IndexConfig::default());
    let report = indexer.index_all(&mut NullProgress).unwrap();
    report.assert_reconciles();
    assert_eq!(report.live_after, report.chunks_added);
}

#[test]
fn noop_reindex_embeds_nothing() {
    let h = Harness::new(&[CommitSpec::new("init")
        .file("a.rs", sample_fn("a"))
        .file("b.rs", sample_fn("b"))]);
    let mut indexer = h.indexer(IndexConfig::default());
    let first = indexer.index_all(&mut NullProgress).unwrap();
    let live = first.live_after;
    drop(indexer);

    let mut indexer = h.reopen(IndexConfig::default());
    let second = indexer.index_all(&mut NullProgress).unwrap();
    assert_eq!(second.embeddings_computed, 0);
    assert_eq!(second.chunks_added, 0);
    assert_eq!(second.chunks_removed, 0);
    assert_eq!(indexer.store().stats().unwrap().live, live);
}

#[test]
fn lexical_index_tracks_pipeline_lifecycle() {
    let h = Harness::new(&[CommitSpec::new("init")
        .file("a.rs", sample_fn("alpha_scarlet"))
        .file("b.rs", sample_fn("beta_cobalt"))]);
    let mut indexer = h.indexer(IndexConfig {
        compact_threshold: 0.20,
        ..IndexConfig::default()
    });
    indexer.index_all(&mut NullProgress).unwrap();

    let alpha_rows = indexer.store().rows_for_file("a.rs").unwrap();
    let alpha_hits = indexer.lexical().search("alpha_scarlet", 10).unwrap();
    assert_eq!(alpha_hits.first().map(|hit| hit.row), Some(alpha_rows[0]));

    h.repo
        .apply_commit(&CommitSpec::new("rename").rename("a.rs", "renamed.rs"));
    indexer.update(&mut NullProgress).unwrap();
    let renamed_rows = indexer.store().rows_for_file("renamed.rs").unwrap();
    assert_eq!(renamed_rows, alpha_rows);
    let renamed_hits = indexer.lexical().search("renamed.rs", 10).unwrap();
    assert_eq!(renamed_hits[0].row, alpha_rows[0]);

    h.repo
        .apply_commit(&CommitSpec::new("delete").delete("b.rs"));
    let delete_report = indexer.update(&mut NullProgress).unwrap();
    let compaction = delete_report
        .compacted
        .as_ref()
        .expect("deleting one of two rows should compact at this threshold");
    assert_eq!(compaction.row_mapping.len(), 1);
    assert_eq!(compaction.row_mapping[0].0.get(), 1);
    assert_eq!(compaction.row_mapping[0].1.get(), 0);
    assert!(
        indexer
            .lexical()
            .search("beta_cobalt", 10)
            .unwrap()
            .is_empty()
    );
    let returned_rows = indexer
        .lexical()
        .search("alpha_scarlet", 10)
        .unwrap()
        .into_iter()
        .map(|result| result.row)
        .collect::<Vec<_>>();
    assert!(
        indexer
            .store()
            .get_many(&returned_rows)
            .unwrap()
            .into_iter()
            .all(|record| record.is_some())
    );
}
