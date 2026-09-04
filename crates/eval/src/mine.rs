//! Mine retrieval labels from git history and documentation.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use gix::bstr::ByteSlice;
use indexing::{Chunker, Language};
use regex::Regex;
use similar::{ChangeTag, TextDiff};
use storage::ChunkStore;

use crate::corpus::require_mined_revision;
use crate::error::EvalError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LabelSource {
    CommitSubject,
    Docstring,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub query: String,
    /// Expected answers as (file, qualified symbol), matching ChunkRecord.
    pub expected: Vec<(String, String)>,
    pub source: LabelSource,
    /// Commit hash, or file:symbol for docstrings.
    pub provenance: String,
}

/// Why a commit was rejected. Counted and reported per rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    Merge,
    TooManySymbols { count: usize },
    SubjectTooShort { words: usize },
    MaintenancePattern { pattern: &'static str },
    NoSymbolsTouched,
    SymbolsNotIndexed { missing: usize },
}

impl RejectReason {
    pub fn rule_name(&self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::TooManySymbols { .. } => "too_many_symbols",
            Self::SubjectTooShort { .. } => "subject_too_short",
            Self::MaintenancePattern { .. } => "maintenance_pattern",
            Self::NoSymbolsTouched => "no_symbols_touched",
            Self::SymbolsNotIndexed { .. } => "symbols_not_indexed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MiningConfig {
    pub max_symbols_per_commit: usize,
    pub min_subject_words: usize,
    pub maintenance_patterns: Vec<String>,
    pub max_commits: Option<usize>,
    /// When true, refuse unless HEAD equals the pinned mined corpus revision.
    pub enforce_pinned_revision: bool,
}

impl Default for MiningConfig {
    fn default() -> Self {
        Self {
            max_symbols_per_commit: 3,
            min_subject_words: 4,
            maintenance_patterns: vec![
                "wip".into(),
                "fixup".into(),
                "typo".into(),
                "bump".into(),
                "lint".into(),
            ],
            max_commits: None,
            enforce_pinned_revision: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MiningReport {
    pub commits_examined: u64,
    pub labels_accepted: u64,
    pub rejected: BTreeMap<String, u64>,
}

impl MiningReport {
    fn new() -> Self {
        Self {
            commits_examined: 0,
            labels_accepted: 0,
            rejected: BTreeMap::new(),
        }
    }

    fn reject(&mut self, reason: &RejectReason) {
        *self
            .rejected
            .entry(reason.rule_name().to_string())
            .or_insert(0) += 1;
    }

    pub fn reconciles(&self) -> bool {
        let rejected_sum: u64 = self.rejected.values().sum();
        self.labels_accepted + rejected_sum == self.commits_examined
    }
}

pub fn mine_commits(
    repo_path: &Path,
    store: &ChunkStore,
    config: &MiningConfig,
) -> Result<(Vec<Label>, MiningReport), EvalError> {
    if config.enforce_pinned_revision {
        require_mined_revision(repo_path)?;
    }

    let repo = gix::open(repo_path).map_err(|e| EvalError::Git(e.to_string()))?;
    let tip = repo
        .head_id()
        .map_err(|e| EvalError::Git(e.to_string()))?;

    let indexed = indexed_symbol_set(store)?;
    let mut chunker = Chunker::new().map_err(|e| EvalError::Index(e.to_string()))?;
    let maintenance = compile_maintenance(&config.maintenance_patterns)?;

    let mut report = MiningReport::new();
    let mut labels = Vec::new();

    let walk = repo
        .rev_walk([tip])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            Default::default(),
        ))
        .all()
        .map_err(|e| EvalError::Git(e.to_string()))?;

    for info in walk {
        let info = info.map_err(|e| EvalError::Git(e.to_string()))?;
        if let Some(max) = config.max_commits {
            if report.commits_examined >= max as u64 {
                break;
            }
        }
        report.commits_examined += 1;

        let commit = repo
            .find_object(info.id)
            .map_err(|e| EvalError::Git(e.to_string()))?
            .peel_to_commit()
            .map_err(|e| EvalError::Git(e.to_string()))?;

        let parent_count = commit.parent_ids().count();
        if parent_count > 1 {
            report.reject(&RejectReason::Merge);
            continue;
        }

        let subject = commit_subject(&commit)?;
        if let Some(pattern) = match_maintenance(&subject, &maintenance) {
            report.reject(&RejectReason::MaintenancePattern { pattern });
            continue;
        }

        let words = subject.split_whitespace().count();
        if words < config.min_subject_words {
            report.reject(&RejectReason::SubjectTooShort { words });
            continue;
        }

        let parent = if parent_count == 1 {
            Some(
                commit
                    .parent_ids()
                    .next()
                    .unwrap()
                    .object()
                    .map_err(|e| EvalError::Git(e.to_string()))?
                    .peel_to_commit()
                    .map_err(|e| EvalError::Git(e.to_string()))?,
            )
        } else {
            None
        };

        let touched = touched_symbols(&repo, &commit, parent.as_ref(), &mut chunker)?;
        if touched.is_empty() {
            report.reject(&RejectReason::NoSymbolsTouched);
            continue;
        }
        if touched.len() > config.max_symbols_per_commit {
            report.reject(&RejectReason::TooManySymbols {
                count: touched.len(),
            });
            continue;
        }

        let missing = touched
            .iter()
            .filter(|pair| !indexed.contains(*pair))
            .count();
        if missing > 0 {
            report.reject(&RejectReason::SymbolsNotIndexed { missing });
            continue;
        }

        labels.push(Label {
            query: subject,
            expected: touched,
            source: LabelSource::CommitSubject,
            provenance: commit.id().to_string(),
        });
        report.labels_accepted += 1;
    }

    labels.sort_by(|a, b| a.provenance.cmp(&b.provenance));
    Ok((labels, report))
}

/// Docstring labels. The held-out set the index must be built without.
pub fn mine_docstrings(
    _repo: &Path,
    _store: &ChunkStore,
) -> Result<(Vec<Label>, MiningReport), EvalError> {
    todo!("mine_docstrings")
}

fn compile_maintenance(patterns: &[String]) -> Result<Vec<(String, Regex)>, EvalError> {
    let mut out = Vec::new();
    for p in patterns {
        let re = Regex::new(&format!(r"(?i)\b{}\b", regex::escape(p)))
            .map_err(|e| EvalError::message(e.to_string()))?;
        out.push((p.clone(), re));
    }
    Ok(out)
}

fn match_maintenance(subject: &str, patterns: &[(String, Regex)]) -> Option<&'static str> {
    // Map known patterns to static strs used in RejectReason.
    const KNOWN: &[&str] = &["wip", "fixup", "typo", "bump", "lint"];
    for (name, re) in patterns {
        if re.is_match(subject) {
            for k in KNOWN {
                if name.eq_ignore_ascii_case(k) {
                    return Some(*k);
                }
            }
            return Some("custom");
        }
    }
    None
}

fn commit_subject(commit: &gix::Commit<'_>) -> Result<String, EvalError> {
    let raw = commit
        .message_raw()
        .map_err(|e| EvalError::Git(e.to_string()))?;
    let text = raw
        .to_str()
        .map_err(|e| EvalError::Git(format!("commit message utf8: {e}")))?;
    Ok(text.lines().next().unwrap_or("").trim().to_string())
}

fn indexed_symbol_set(store: &ChunkStore) -> Result<BTreeSet<(String, String)>, EvalError> {
    let mut set = BTreeSet::new();
    for row in store
        .live_rows()
        .map_err(|e| EvalError::Store(e.to_string()))?
    {
        if let Some(rec) = store
            .get(row)
            .map_err(|e| EvalError::Store(e.to_string()))?
        {
            if rec.symbol == "<file_prelude>" {
                continue;
            }
            set.insert((rec.file, rec.symbol));
        }
    }
    Ok(set)
}

fn touched_symbols(
    repo: &gix::Repository,
    commit: &gix::Commit<'_>,
    parent: Option<&gix::Commit<'_>>,
    chunker: &mut Chunker,
) -> Result<Vec<(String, String)>, EvalError> {
    let new_tree = commit
        .tree()
        .map_err(|e| EvalError::Git(e.to_string()))?;
    let old_tree = match parent {
        Some(p) => Some(p.tree().map_err(|e| EvalError::Git(e.to_string()))?),
        None => None,
    };

    let paths = changed_paths(repo, old_tree.as_ref(), &new_tree)?;
    let mut touched: BTreeSet<(String, String)> = BTreeSet::new();

    for path in paths {
        let Some(lang) = Language::from_path(Path::new(&path)) else {
            continue;
        };
        let new_text = match blob_at_tree(repo, &new_tree, &path)? {
            Some(t) => t,
            None => continue, // deleted
        };
        let old_text = match &old_tree {
            Some(tree) => blob_at_tree(repo, tree, &path)?.unwrap_or_default(),
            None => String::new(),
        };

        let changed_lines = if old_text.is_empty() {
            (1..=line_count(&new_text)).collect::<BTreeSet<_>>()
        } else {
            new_side_changed_lines(&old_text, &new_text)
        };
        if changed_lines.is_empty() {
            continue;
        }

        let chunks = chunker.chunk_file(&path, lang, &new_text);
        for chunk in chunks.chunks {
            if chunk.record.symbol == "<file_prelude>" {
                continue;
            }
            let start = chunk.record.line_start;
            let end = chunk.record.line_end;
            if changed_lines.iter().any(|l| (start..=end).contains(l)) {
                touched.insert((chunk.record.file, chunk.record.symbol));
            }
        }
    }

    Ok(touched.into_iter().collect())
}

fn changed_paths(
    repo: &gix::Repository,
    old_tree: Option<&gix::Tree<'_>>,
    new_tree: &gix::Tree<'_>,
) -> Result<Vec<String>, EvalError> {
    use gix::diff::Options as DiffOptions;
    use gix::object::tree::diff::ChangeDetached;

    let mut opts = DiffOptions::default();
    opts.track_path();

    let changes = repo
        .diff_tree_to_tree(old_tree, Some(new_tree), Some(opts))
        .map_err(|e| EvalError::Git(e.to_string()))?;

    let mut paths = Vec::new();
    for change in changes {
        let path = match change {
            ChangeDetached::Addition { location, .. }
            | ChangeDetached::Deletion { location, .. }
            | ChangeDetached::Modification { location, .. } => location_to_string(&location),
            ChangeDetached::Rewrite { location, .. } => location_to_string(&location),
        };
        if let Some(p) = path {
            paths.push(p);
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn location_to_string(location: &gix::bstr::BString) -> Option<String> {
    let s = location.to_str().ok()?;
    if s.is_empty() {
        None
    } else {
        Some(s.replace('\\', "/"))
    }
}

fn blob_at_tree(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    path: &str,
) -> Result<Option<String>, EvalError> {
    let entry = match tree.lookup_entry_by_path(path) {
        Ok(Some(e)) => e,
        Ok(None) => return Ok(None),
        Err(e) => return Err(EvalError::Git(e.to_string())),
    };
    if !entry.mode().is_blob() && !entry.mode().is_link() {
        return Ok(None);
    }
    let obj = repo
        .find_object(entry.oid())
        .map_err(|e| EvalError::Git(e.to_string()))?;
    let data = obj.data.clone();
    match String::from_utf8(data) {
        Ok(s) => Ok(Some(s)),
        Err(_) => Ok(None),
    }
}

fn line_count(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }
    text.lines().count().max(1) as u32
}

fn new_side_changed_lines(old: &str, new: &str) -> BTreeSet<u32> {
    let mut set = BTreeSet::new();
    let mut new_lineno = 0u32;
    let diff = TextDiff::from_lines(old, new);
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                new_lineno += 1;
            }
            ChangeTag::Insert => {
                new_lineno += 1;
                set.insert(new_lineno);
            }
            ChangeTag::Delete => {}
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexing::{IndexConfig, Indexer, NullProgress};
    use inference::{Embedder, MockEmbedder};
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    const DIMS: u32 = 8;

    fn git(cwd: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn git_output(cwd: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git");
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn write(cwd: &Path, rel: &str, contents: &str) {
        let full = cwd.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, contents).unwrap();
        git(cwd, &["add", "--", rel]);
    }

    fn commit(cwd: &Path, message: &str) -> String {
        git(cwd, &["commit", "-m", message]);
        git_output(cwd, &["rev-parse", "HEAD"])
    }

    fn fn_src(name: &str) -> String {
        format!("pub fn {name}() {{\n    let x = 1;\n}}\n")
    }

    fn multi_fns(names: &[&str]) -> String {
        names
            .iter()
            .map(|n| fn_src(n))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Fixture history covering every filter rule and three accepted commits.
    fn build_mining_fixture() -> TempDir {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        git(p, &["init", "-b", "main"]);
        git(p, &["config", "user.email", "test@example.com"]);
        git(p, &["config", "user.name", "Test"]);
        git(p, &["config", "commit.gpgsign", "false"]);

        write(p, "lib.rs", "// root\n");
        commit(p, "Initialize the empty library crate");

        write(p, "lib.rs", &fn_src("alpha"));
        commit(p, "Add the alpha helper function");

        write(p, "lib.rs", &(fn_src("alpha") + &fn_src("beta")));
        commit(p, "Improve beta calculation logic path");

        write(
            p,
            "lib.rs",
            &(fn_src("alpha") + &fn_src("beta") + &fn_src("gamma")),
        );
        commit(p, "Introduce gamma for clarity sake");

        git(p, &["checkout", "-b", "side"]);
        write(p, "side.rs", &fn_src("side_only"));
        commit(p, "wip add side helper symbol here");
        git(p, &["checkout", "main"]);
        git(
            p,
            &["merge", "--no-ff", "-m", "Merge branch side into main", "side"],
        );

        write(p, "many.rs", &multi_fns(&["s1", "s2", "s3", "s4", "s5"]));
        commit(p, "Add five unrelated helper symbols together");

        write(p, "short.rs", &fn_src("shorty"));
        commit(p, "Add shorty");

        write(p, "wip.rs", &fn_src("wip_fn"));
        commit(p, "wip unfinished helper for later");
        write(p, "fixup.rs", &fn_src("fixup_fn"));
        commit(p, "fixup the previous helper change");
        write(p, "typo.rs", &fn_src("typo_fn"));
        commit(p, "typo in a comment only really");
        write(p, "bump.rs", &fn_src("bump_fn"));
        commit(p, "bump dependency versions across workspace");
        write(p, "lint.rs", &fn_src("lint_fn"));
        commit(p, "lint cleanup across the helper module");

        write(p, "NOTES.md", "just notes\n");
        commit(p, "Document the repository overview notes");

        dir
    }

    fn index_repo(repo: &Path) -> (TempDir, ChunkStore) {
        let store_dir = TempDir::new().unwrap();
        let embedder = MockEmbedder::new(DIMS);
        let store = ChunkStore::create(store_dir.path(), DIMS, embedder.model_id()).unwrap();
        let mut indexer = Indexer::open(store, &embedder, repo, IndexConfig::default()).unwrap();
        indexer.index_all(&mut NullProgress).unwrap();
        let store = ChunkStore::open(store_dir.path()).unwrap();
        (store_dir, store)
    }

    fn test_config() -> MiningConfig {
        MiningConfig {
            enforce_pinned_revision: false,
            ..MiningConfig::default()
        }
    }

    #[test]
    fn mine_commits_accepts_three_and_attributes_each_rejection() {
        let repo = build_mining_fixture();
        let (_dir, store) = index_repo(repo.path());
        let (labels, report) = mine_commits(repo.path(), &store, &test_config()).unwrap();

        assert_eq!(
            labels.len(),
            3,
            "expected exactly three accepted labels, got {}: {:?}",
            labels.len(),
            labels.iter().map(|l| &l.query).collect::<Vec<_>>()
        );
        let queries: BTreeSet<_> = labels.iter().map(|l| l.query.as_str()).collect();
        assert!(queries.contains("Add the alpha helper function"));
        assert!(queries.contains("Improve beta calculation logic path"));
        assert!(queries.contains("Introduce gamma for clarity sake"));

        assert!(report.rejected.get("merge").copied().unwrap_or(0) >= 1);
        assert!(
            report
                .rejected
                .get("too_many_symbols")
                .copied()
                .unwrap_or(0)
                >= 1
        );
        assert!(
            report
                .rejected
                .get("subject_too_short")
                .copied()
                .unwrap_or(0)
                >= 1
        );
        assert!(
            report
                .rejected
                .get("maintenance_pattern")
                .copied()
                .unwrap_or(0)
                >= 5
        );
        assert!(
            report
                .rejected
                .get("no_symbols_touched")
                .copied()
                .unwrap_or(0)
                >= 1
        );
    }

    #[test]
    fn mining_report_reconciles_accepted_plus_rejected() {
        let repo = build_mining_fixture();
        let (_dir, store) = index_repo(repo.path());
        let (_labels, report) = mine_commits(repo.path(), &store, &test_config()).unwrap();
        let rejected_sum: u64 = report.rejected.values().sum();
        assert_eq!(
            report.labels_accepted + rejected_sum,
            report.commits_examined,
            "accepted {} + rejected {} != examined {}",
            report.labels_accepted,
            rejected_sum,
            report.commits_examined
        );
        assert!(report.reconciles());
    }
}
