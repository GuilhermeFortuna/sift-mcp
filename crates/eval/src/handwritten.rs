//! Hand-written sanity-check label set. Never merge into mined metrics.

use std::path::Path;

use serde::Deserialize;

use crate::error::EvalError;
use crate::mine::{Label, LabelSource};

#[derive(Debug, Deserialize)]
struct HandwrittenEntry {
    query: String,
    expected: Vec<(String, String)>,
}

/// Load the committed hand-written sanity set.
pub fn load_handwritten(path: &Path) -> Result<Vec<Label>, EvalError> {
    let text = std::fs::read_to_string(path)?;
    let entries: Vec<HandwrittenEntry> = serde_json::from_str(&text)?;
    Ok(entries
        .into_iter()
        .enumerate()
        .map(|(i, e)| Label {
            query: e.query,
            expected: e.expected,
            source: LabelSource::CommitSubject,
            provenance: format!("handwritten:{i}"),
        })
        .collect())
}

/// Default path relative to the eval crate manifest.
pub fn default_handwritten_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("data/handwritten.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mine::{MiningConfig, mine_commits};
    use indexing::{IndexConfig, Indexer, NullProgress};
    use inference::{Embedder, MockEmbedder};
    use storage::ChunkStore;
    use tempfile::TempDir;

    #[test]
    fn handwritten_loads_only_from_its_file_and_not_from_mining() {
        let path = default_handwritten_path();
        let handwritten = load_handwritten(&path).unwrap();
        assert!(
            handwritten.len() >= 25,
            "expected ~30 sanity questions, got {}",
            handwritten.len()
        );
        assert!(handwritten.iter().all(|l| l.provenance.starts_with("handwritten:")));

        // Mining a tiny fixture must not produce handwritten provenance.
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(p)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "t@e.com"])
            .current_dir(p)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "T"])
            .current_dir(p)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "commit.gpgsign", "false"])
            .current_dir(p)
            .status()
            .unwrap();
        std::fs::write(p.join("a.rs"), "pub fn alpha() { let x = 1; }\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "a.rs"])
            .current_dir(p)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "Add the alpha helper function"])
            .current_dir(p)
            .status()
            .unwrap();

        let store_dir = TempDir::new().unwrap();
        let embedder = MockEmbedder::new(8);
        {
            let store =
                ChunkStore::create(store_dir.path(), 8, embedder.model_id()).unwrap();
            let mut indexer =
                Indexer::open(store, &embedder, p, IndexConfig::default()).unwrap();
            indexer.index_all(&mut NullProgress).unwrap();
        }
        let store = ChunkStore::open(store_dir.path()).unwrap();
        let (mined, _) = mine_commits(
            p,
            &store,
            &MiningConfig {
                enforce_pinned_revision: false,
                ..MiningConfig::default()
            },
        )
        .unwrap();
        assert!(
            mined.iter().all(|l| !l.provenance.starts_with("handwritten:")),
            "mined set must never include handwritten labels"
        );
    }
}
