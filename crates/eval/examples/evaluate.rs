//! Evaluate retrieval over a store.
//!
//! ```text
//! cargo run --release -p eval --features cuda --example evaluate -- <store-path> --model <model-dir> [--ablations] [--set mined|docstring|handwritten] [--repo <path>]
//! ```

use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process;

use eval::{
    Ablation, HARNESS_VERSION, MiningConfig, RunManifest, default_handwritten_path, evaluate,
    load_handwritten, mine_commits, mine_docstrings,
};
use indexing::RepoGit;
use inference::{Embedder, OnnxEmbedder};
use retrieval::dense::{DenseBackend, DenseIndex};
use retrieval::{FusionConfig, LexicalIndex, Searcher};
use storage::ChunkStore;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let ablations_flag = args.iter().any(|a| a == "--ablations");
    let mut set_name = "mined".to_string();
    let mut repo: Option<PathBuf> = None;
    let mut store_path: Option<PathBuf> = None;
    let mut model_dir: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--ablations" => {}
            "--set" => {
                i += 1;
                set_name = args.get(i).cloned().ok_or("missing value for --set")?;
            }
            "--repo" => {
                i += 1;
                repo = Some(PathBuf::from(
                    args.get(i).ok_or("missing value for --repo")?,
                ));
            }
            "--model" => {
                i += 1;
                model_dir = Some(PathBuf::from(
                    args.get(i).ok_or("missing value for --model")?,
                ));
            }
            flag if flag.starts_with('-') => {
                return Err(format!("unknown flag {flag}").into());
            }
            other => {
                if store_path.is_none() {
                    store_path = Some(PathBuf::from(other));
                }
            }
        }
        i += 1;
    }

    let store_path = store_path
        .ok_or("usage: evaluate <store-path> --model <model-dir> [--ablations] [--set …]")?;
    let model_dir = model_dir.ok_or("evaluate requires --model <model-dir>")?;
    let store = ChunkStore::open(&store_path)?;
    let embedder = OnnxEmbedder::load(&model_dir, 32)?;
    store.require_model(embedder.model_id())?;
    let lexical = LexicalIndex::open(store.dir())?;
    let dense = DenseIndex::from_store(&store, DenseBackend::Cuda)?;
    let searcher = Searcher::new(&lexical, &dense, &store, &embedder);

    let mined_repo = (set_name == "mined").then(|| {
        repo.clone()
            .unwrap_or_else(|| eval::expand_home(eval::MINED_CORPUS_DEFAULT_PATH))
    });
    let labels = match set_name.as_str() {
        "handwritten" => load_handwritten(&default_handwritten_path())?,
        "docstring" => {
            let repo_path = repo.as_ref().ok_or("--set docstring requires --repo")?;
            mine_docstrings(repo_path, &store)?.0
        }
        "mined" => {
            let repo_path = mined_repo.as_ref().expect("mined repo path is resolved");
            mine_commits(
                repo_path,
                &store,
                &MiningConfig {
                    enforce_pinned_revision: true,
                    ..MiningConfig::default()
                },
            )?
            .0
        }
        other => return Err(format!("unknown --set {other}").into()),
    };

    let ablations = if ablations_flag {
        vec![Ablation::LexicalOnly, Ablation::DenseOnly, Ablation::Fused]
    } else {
        vec![Ablation::Fused]
    };

    let repo_commit = manifest_repo(&set_name, repo.as_deref())
        .as_ref()
        .and_then(|r| RepoGit::open(r).ok()?.head_commit().ok())
        .unwrap_or_default();

    let manifest = RunManifest {
        repo_commit,
        indexed_commit: store.indexed_commit()?.unwrap_or_default(),
        model_id: embedder.model_id().to_string(),
        fusion: FusionConfig::default().into(),
        harness_version: HARNESS_VERSION,
        label_set: set_name.clone(),
        timestamp: chrono_like_now(),
    };

    let run = evaluate(&searcher, &store, &labels, &ablations, manifest)?;

    let mut out = serde_json::Map::new();
    out.insert("manifest".into(), serde_json::to_value(&run.manifest)?);
    let mut abl = serde_json::Map::new();
    for (k, m) in &run.per_ablation {
        abl.insert(
            format!("{k:?}"),
            serde_json::json!({
                "labels_scored": m.labels_scored,
                "labels_discarded": m.labels_discarded,
                "top_1": m.top_1,
                "top_3": m.top_3,
                "top_10": m.top_10,
                "mrr": m.mrr,
                "latency_p50_ms": m.latency_p50_ms,
                "latency_p95_ms": m.latency_p95_ms,
                "peak_gpu_bytes": m.peak_gpu_bytes,
            }),
        );
    }
    out.insert("per_ablation".into(), serde_json::Value::Object(abl));
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

fn manifest_repo(set_name: &str, repo: Option<&Path>) -> Option<PathBuf> {
    match set_name {
        "mined" => Some(
            repo.map(Path::to_path_buf)
                .unwrap_or_else(|| eval::expand_home(eval::MINED_CORPUS_DEFAULT_PATH)),
        ),
        _ => repo.map(Path::to_path_buf),
    }
}

#[cfg(test)]
mod tests {
    use super::manifest_repo;

    #[test]
    fn default_mined_manifest_uses_the_default_corpus_path() {
        assert_eq!(
            manifest_repo("mined", None),
            Some(eval::expand_home(eval::MINED_CORPUS_DEFAULT_PATH))
        );
    }
}
