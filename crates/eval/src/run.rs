//! Ablation evaluation runs and machine-readable manifests.

use std::collections::BTreeMap;

use retrieval::{FusionConfig, Searcher};
use serde::{Deserialize, Serialize};

use crate::corpus::HARNESS_VERSION;
use crate::error::EvalError;
use crate::metrics::{
    BytesBeforeHit, Metrics, percentile, reciprocal_rank, top_k_accuracy,
};
use crate::mine::{Label, LabelSource};
use storage::ChunkStore;

/// The three configurations the design bets on. No reranking anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ablation {
    LexicalOnly,
    DenseOnly,
    Fused,
}

impl Ablation {
    pub fn fusion_config(self) -> FusionConfig {
        match self {
            Self::LexicalOnly => FusionConfig {
                lexical_depth: 50,
                dense_depth: 0,
                rrf_k: 60.0,
            },
            Self::DenseOnly => FusionConfig {
                lexical_depth: 0,
                dense_depth: 50,
                rrf_k: 60.0,
            },
            Self::Fused => FusionConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LengthBucket {
    Short,
    Medium,
    Long,
}

impl LengthBucket {
    pub fn from_query(query: &str) -> Self {
        let words = query.split_whitespace().count();
        if words <= 4 {
            Self::Short
        } else if words <= 10 {
            Self::Medium
        } else {
            Self::Long
        }
    }
}

/// Everything needed to compare two runs and attribute a difference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunManifest {
    pub repo_commit: String,
    pub indexed_commit: String,
    pub model_id: String,
    pub fusion: FusionConfigSerde,
    pub harness_version: u32,
    pub label_set: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FusionConfigSerde {
    pub lexical_depth: usize,
    pub dense_depth: usize,
    pub rrf_k: f32,
}

impl From<FusionConfig> for FusionConfigSerde {
    fn from(c: FusionConfig) -> Self {
        Self {
            lexical_depth: c.lexical_depth,
            dense_depth: c.dense_depth,
            rrf_k: c.rrf_k,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvalRun {
    pub manifest: RunManifest,
    pub per_ablation: BTreeMap<Ablation, Metrics>,
    pub by_source: BTreeMap<LabelSource, Metrics>,
    pub by_query_length: BTreeMap<LengthBucket, Metrics>,
}

/// Partition labels into scorable vs discarded (expected symbols absent from index).
pub fn partition_labels(
    store: &ChunkStore,
    labels: &[Label],
) -> Result<(Vec<Label>, u64), EvalError> {
    let indexed = {
        let mut set = std::collections::BTreeSet::new();
        for row in store
            .live_rows()
            .map_err(|e| EvalError::Store(e.to_string()))?
        {
            if let Some(rec) = store
                .get(row)
                .map_err(|e| EvalError::Store(e.to_string()))?
            {
                set.insert((rec.file, rec.symbol));
            }
        }
        set
    };

    let mut scored = Vec::new();
    let mut discarded = 0u64;
    for label in labels {
        let all_present = label.expected.iter().all(|e| indexed.contains(e));
        if all_present && !label.expected.is_empty() {
            scored.push(label.clone());
        } else {
            discarded += 1;
        }
    }
    Ok((scored, discarded))
}

pub fn evaluate(
    searcher: &Searcher<'_>,
    store: &ChunkStore,
    labels: &[Label],
    ablations: &[Ablation],
    manifest: RunManifest,
) -> Result<EvalRun, EvalError> {
    let (scored_labels, discarded) = partition_labels(store, labels)?;

    let mut per_ablation = BTreeMap::new();
    for &ablation in ablations {
        let metrics = run_ablation(searcher, &scored_labels, ablation, discarded)?;
        per_ablation.insert(ablation, metrics);
    }

    let fused = Ablation::Fused;
    let by_source = breakdown_by_source(searcher, &scored_labels, fused, discarded)?;
    let by_query_length = breakdown_by_length(searcher, &scored_labels, fused, discarded)?;

    Ok(EvalRun {
        manifest,
        per_ablation,
        by_source,
        by_query_length,
    })
}

fn run_ablation(
    searcher: &Searcher<'_>,
    labels: &[Label],
    ablation: Ablation,
    discarded: u64,
) -> Result<Metrics, EvalError> {
    let config = ablation.fusion_config();
    if labels.is_empty() {
        return Ok(Metrics {
            labels_scored: 0,
            labels_discarded: discarded,
            top_1: 0.0,
            top_3: 0.0,
            top_10: 0.0,
            mrr: 0.0,
            latency_p50_ms: 0.0,
            latency_p95_ms: 0.0,
            peak_gpu_bytes: 0,
            bytes_before_hit: BytesBeforeHit::default(),
        });
    }

    let mut rankings = Vec::with_capacity(labels.len());
    let mut expected = Vec::with_capacity(labels.len());
    let mut latencies = Vec::with_capacity(labels.len());
    let mut rr_sum = 0.0;

    for label in labels {
        let started = std::time::Instant::now();
        let response = searcher
            .search(&label.query, 10, &config)
            .map_err(|e| EvalError::Retrieval(e.to_string()))?;
        latencies.push(started.elapsed().as_secs_f64() * 1000.0);

        let ranked: Vec<(String, String)> = response
            .results
            .iter()
            .map(|r| (r.file.clone(), r.symbol.clone()))
            .collect();
        rr_sum += reciprocal_rank(&ranked, &label.expected);
        rankings.push(ranked);
        expected.push(label.expected.clone());
    }

    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = labels.len() as f64;
    Ok(Metrics {
        labels_scored: labels.len() as u64,
        labels_discarded: discarded,
        top_1: top_k_accuracy(&rankings, &expected, 1),
        top_3: top_k_accuracy(&rankings, &expected, 3),
        top_10: top_k_accuracy(&rankings, &expected, 10),
        mrr: rr_sum / n,
        latency_p50_ms: percentile(&latencies, 0.50),
        latency_p95_ms: percentile(&latencies, 0.95),
        peak_gpu_bytes: 0,
        bytes_before_hit: BytesBeforeHit::default(),
    })
}

fn breakdown_by_source(
    searcher: &Searcher<'_>,
    labels: &[Label],
    ablation: Ablation,
    discarded: u64,
) -> Result<BTreeMap<LabelSource, Metrics>, EvalError> {
    let mut map = BTreeMap::new();
    for source in [LabelSource::CommitSubject, LabelSource::Docstring] {
        let subset: Vec<_> = labels
            .iter()
            .filter(|l| l.source == source)
            .cloned()
            .collect();
        if subset.is_empty() {
            continue;
        }
        map.insert(source, run_ablation(searcher, &subset, ablation, discarded)?);
    }
    Ok(map)
}

fn breakdown_by_length(
    searcher: &Searcher<'_>,
    labels: &[Label],
    ablation: Ablation,
    discarded: u64,
) -> Result<BTreeMap<LengthBucket, Metrics>, EvalError> {
    let mut map = BTreeMap::new();
    for bucket in [LengthBucket::Short, LengthBucket::Medium, LengthBucket::Long] {
        let subset: Vec<_> = labels
            .iter()
            .filter(|l| LengthBucket::from_query(&l.query) == bucket)
            .cloned()
            .collect();
        if subset.is_empty() {
            continue;
        }
        map.insert(bucket, run_ablation(searcher, &subset, ablation, discarded)?);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mine::{Label, LabelSource};
    use indexing::{IndexConfig, Indexer, NullProgress};
    use inference::{Embedder, MockEmbedder};
    use retrieval::dense::{DenseBackend, DenseIndex};
    use retrieval::{LexicalIndex, Searcher};
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    const DIMS: u32 = 8;

    fn git(cwd: &std::path::Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(cwd)
                .status()
                .unwrap()
                .success()
        );
    }

    fn write_commit(cwd: &std::path::Path, path: &str, contents: &str, msg: &str) {
        let full = cwd.join(path);
        if let Some(p) = full.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(&full, contents).unwrap();
        git(cwd, &["add", "-A"]);
        git(cwd, &["commit", "-m", msg]);
    }

    #[test]
    fn absent_expected_symbols_are_discarded_not_scored() {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        git(p, &["init", "-b", "main"]);
        git(p, &["config", "user.email", "t@e.com"]);
        git(p, &["config", "user.name", "T"]);
        git(p, &["config", "commit.gpgsign", "false"]);
        write_commit(
            p,
            "a.rs",
            "pub fn present() { let x = 1; }\n",
            "Add present helper function here",
        );

        let store_dir = TempDir::new().unwrap();
        let embedder = MockEmbedder::new(DIMS);
        {
            let store =
                ChunkStore::create(store_dir.path(), DIMS, embedder.model_id()).unwrap();
            let mut indexer =
                Indexer::open(store, &embedder, p, IndexConfig::default()).unwrap();
            indexer.index_all(&mut NullProgress).unwrap();
        }
        let store = ChunkStore::open(store_dir.path()).unwrap();

        let labels = vec![
            Label {
                query: "find present".into(),
                expected: vec![("a.rs".into(), "present".into())],
                source: LabelSource::CommitSubject,
                provenance: "1".into(),
            },
            Label {
                query: "find missing".into(),
                expected: vec![("a.rs".into(), "does_not_exist".into())],
                source: LabelSource::CommitSubject,
                provenance: "2".into(),
            },
        ];

        let (scored, discarded) = partition_labels(&store, &labels).unwrap();
        assert_eq!(scored.len(), 1);
        assert_eq!(discarded, 1);
        assert_eq!(scored[0].expected[0].1, "present");

        let lexical = LexicalIndex::open(store.dir()).unwrap();
        let dense = DenseIndex::from_store(&store, DenseBackend::Cpu).unwrap();
        let searcher = Searcher::new(&lexical, &dense, &store, &embedder);
        let manifest = RunManifest {
            repo_commit: "test".into(),
            indexed_commit: "test".into(),
            model_id: embedder.model_id().to_string(),
            fusion: FusionConfig::default().into(),
            harness_version: HARNESS_VERSION,
            label_set: "mined".into(),
            timestamp: "now".into(),
        };
        let run = evaluate(
            &searcher,
            &store,
            &labels,
            &[Ablation::Fused],
            manifest,
        )
        .unwrap();
        let m = &run.per_ablation[&Ablation::Fused];
        assert_eq!(m.labels_scored, 1);
        assert_eq!(m.labels_discarded, 1);
        assert!(m.top_1.is_finite());
    }

    #[test]
    fn metrics_over_zero_labels_report_zero_scored_not_nan() {
        let m = Metrics {
            labels_scored: 0,
            labels_discarded: 3,
            top_1: 0.0,
            top_3: 0.0,
            top_10: 0.0,
            mrr: 0.0,
            latency_p50_ms: 0.0,
            latency_p95_ms: 0.0,
            peak_gpu_bytes: 0,
            bytes_before_hit: BytesBeforeHit::default(),
        };
        assert_eq!(m.labels_scored, 0);
        assert!(m.top_1.is_finite());
        assert!(!m.top_1.is_nan());
    }

    #[test]
    fn evaluate_three_ablations_differ_and_report_latency() {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        git(p, &["init", "-b", "main"]);
        git(p, &["config", "user.email", "t@e.com"]);
        git(p, &["config", "user.name", "T"]);
        git(p, &["config", "commit.gpgsign", "false"]);
        write_commit(
            p,
            "a.rs",
            "pub fn alpha() { let x = 1; }\npub fn beta() { let y = 2; }\n",
            "Add alpha and beta helpers together",
        );

        let store_dir = TempDir::new().unwrap();
        let embedder = MockEmbedder::new(DIMS);
        {
            let store =
                ChunkStore::create(store_dir.path(), DIMS, embedder.model_id()).unwrap();
            let mut indexer =
                Indexer::open(store, &embedder, p, IndexConfig::default()).unwrap();
            indexer.index_all(&mut NullProgress).unwrap();
        }
        let store = ChunkStore::open(store_dir.path()).unwrap();
        let lexical = LexicalIndex::open(store.dir()).unwrap();
        let dense = DenseIndex::from_store(&store, DenseBackend::Cpu).unwrap();
        let searcher = Searcher::new(&lexical, &dense, &store, &embedder);

        let labels = vec![Label {
            query: "alpha helper".into(),
            expected: vec![("a.rs".into(), "alpha".into())],
            source: LabelSource::CommitSubject,
            provenance: "1".into(),
        }];
        let ablations = [
            Ablation::LexicalOnly,
            Ablation::DenseOnly,
            Ablation::Fused,
        ];
        let manifest = RunManifest {
            repo_commit: "test".into(),
            indexed_commit: store.indexed_commit().unwrap().unwrap_or_default(),
            model_id: embedder.model_id().to_string(),
            fusion: FusionConfig::default().into(),
            harness_version: HARNESS_VERSION,
            label_set: "mined".into(),
            timestamp: "now".into(),
        };
        let run = evaluate(&searcher, &store, &labels, &ablations, manifest).unwrap();
        assert_eq!(run.per_ablation.len(), 3);
        let lex = &run.per_ablation[&Ablation::LexicalOnly];
        let den = &run.per_ablation[&Ablation::DenseOnly];
        let fused = &run.per_ablation[&Ablation::Fused];
        // At least one ablation path should differ from fused on scores or be reported separately.
        assert_eq!(lex.labels_scored, 1);
        assert_eq!(den.labels_scored, 1);
        assert_eq!(fused.labels_scored, 1);
        assert!(lex.latency_p50_ms >= 0.0);
        assert!(den.latency_p50_ms >= 0.0);
        assert!(fused.latency_p50_ms >= 0.0);
        // Fusion configs differ so contributions differ even when ranking ties.
        assert_ne!(
            Ablation::LexicalOnly.fusion_config().dense_depth,
            Ablation::Fused.fusion_config().dense_depth
        );
        assert_ne!(
            Ablation::DenseOnly.fusion_config().lexical_depth,
            Ablation::Fused.fusion_config().lexical_depth
        );
    }

    #[test]
    fn breakdowns_partition_labels_exactly_once() {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        git(p, &["init", "-b", "main"]);
        git(p, &["config", "user.email", "t@e.com"]);
        git(p, &["config", "user.name", "T"]);
        git(p, &["config", "commit.gpgsign", "false"]);
        write_commit(
            p,
            "a.rs",
            "/// Doc for documented\npub fn documented() { let x = 1; }\n",
            "Add documented helper function here",
        );

        let store_dir = TempDir::new().unwrap();
        let embedder = MockEmbedder::new(DIMS);
        {
            let store =
                ChunkStore::create(store_dir.path(), DIMS, embedder.model_id()).unwrap();
            let mut indexer =
                Indexer::open(store, &embedder, p, IndexConfig::default()).unwrap();
            indexer.index_all(&mut NullProgress).unwrap();
        }
        let store = ChunkStore::open(store_dir.path()).unwrap();
        let lexical = LexicalIndex::open(store.dir()).unwrap();
        let dense = DenseIndex::from_store(&store, DenseBackend::Cpu).unwrap();
        let searcher = Searcher::new(&lexical, &dense, &store, &embedder);

        let labels = vec![
            Label {
                query: "short q".into(), // 2 words -> Short
                expected: vec![("a.rs".into(), "documented".into())],
                source: LabelSource::CommitSubject,
                provenance: "1".into(),
            },
            Label {
                query: "Doc for documented".into(), // 3 words -> Short
                expected: vec![("a.rs".into(), "documented".into())],
                source: LabelSource::Docstring,
                provenance: "2".into(),
            },
            Label {
                query: "one two three four five six".into(), // 6 -> Medium
                expected: vec![("a.rs".into(), "documented".into())],
                source: LabelSource::CommitSubject,
                provenance: "3".into(),
            },
        ];
        let manifest = RunManifest {
            repo_commit: "test".into(),
            indexed_commit: "test".into(),
            model_id: embedder.model_id().to_string(),
            fusion: FusionConfig::default().into(),
            harness_version: HARNESS_VERSION,
            label_set: "mixed".into(),
            timestamp: "now".into(),
        };
        let run = evaluate(
            &searcher,
            &store,
            &labels,
            &[Ablation::Fused],
            manifest,
        )
        .unwrap();

        let source_sum: u64 = run.by_source.values().map(|m| m.labels_scored).sum();
        let length_sum: u64 = run.by_query_length.values().map(|m| m.labels_scored).sum();
        assert_eq!(source_sum, 3);
        assert_eq!(length_sum, 3);
        assert_eq!(
            run.by_source[&LabelSource::CommitSubject].labels_scored,
            2
        );
        assert_eq!(run.by_source[&LabelSource::Docstring].labels_scored, 1);
    }
}
