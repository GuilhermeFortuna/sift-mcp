use std::cmp::Ordering;
use std::collections::BinaryHeap;

use half::f16;
use storage::{ChunkStore, RowId};
use storage::EmbeddingMatrix;

use crate::{RetrievalError, ScoredRow};

pub enum DenseBackend {
    Cpu,
}

pub struct DenseIndex<'a> {
    matrix: &'a EmbeddingMatrix,
    live: LiveMask,
    backend: DenseBackend,
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    row: RowId,
    score: f32,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.row == other.row && self.score.to_bits() == other.score.to_bits()
    }
}

impl Eq for Candidate {}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then_with(|| self.row.cmp(&other.row))
    }
}

impl<'a> DenseIndex<'a> {
    pub fn from_store(
        store: &'a ChunkStore,
        backend: DenseBackend,
    ) -> Result<Self, RetrievalError> {
        let live = LiveMask::from_store(store)?;
        Self::prepare(store.matrix(), &live, backend)
    }

    pub fn prepare(
        matrix: &'a EmbeddingMatrix,
        live: &LiveMask,
        backend: DenseBackend,
    ) -> Result<Self, RetrievalError> {
        if matrix.rows() as usize != live.len() {
            return Err(RetrievalError::Dense(format!(
                "live mask has {} rows but matrix has {}",
                live.len(),
                matrix.rows()
            )));
        }
        Ok(Self {
            matrix,
            live: live.clone(),
            backend,
        })
    }

    pub fn search(
        &self,
        query: &[f16],
        model_id: &str,
        limit: usize,
    ) -> Result<Vec<ScoredRow>, RetrievalError> {
        if query.len() != self.matrix.dims() as usize {
            return Err(RetrievalError::DimensionMismatch {
                expected: self.matrix.dims(),
                got: query.len() as u32,
            });
        }
        if model_id != self.matrix.model_id() {
            return Err(RetrievalError::ModelMismatch {
                expected: self.matrix.model_id().to_owned(),
                got: model_id.to_owned(),
            });
        }
        if limit == 0 {
            return Ok(Vec::new());
        }

        match self.backend {
            DenseBackend::Cpu => self.search_cpu(query, limit),
        }
    }

    fn search_cpu(&self, query: &[f16], limit: usize) -> Result<Vec<ScoredRow>, RetrievalError> {
        let dims = self.matrix.dims() as usize;
        let mut top = BinaryHeap::with_capacity(limit.saturating_add(1));
        for (row, vector) in self.matrix.as_slice().chunks_exact(dims).enumerate() {
            let row = RowId::from_u64(row as u64);
            if !self.live.is_live(row) {
                continue;
            }
            let score = vector
                .iter()
                .zip(query)
                .map(|(value, query_value)| value.to_f32() * query_value.to_f32())
                .sum::<f32>();
            if !score.is_finite() {
                return Err(RetrievalError::Dense(format!(
                    "non-finite score for row {}",
                    row.get()
                )));
            }
            let candidate = Candidate { row, score };
            if top.len() < limit {
                top.push(candidate);
            } else if top.peek().is_some_and(|worst| candidate < *worst) {
                top.pop();
                top.push(candidate);
            }
        }

        let mut results = top
            .into_iter()
            .map(|candidate| ScoredRow {
                row: candidate.row,
                score: candidate.score,
            })
            .collect::<Vec<_>>();
        results.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.row.cmp(&right.row))
        });
        Ok(results)
    }

    pub fn resident_bytes(&self) -> u64 {
        (self.matrix.as_slice().len() * std::mem::size_of::<f16>()) as u64
    }
}

/// Snapshot of live matrix rows used to exclude tombstones without metadata
/// lookups in the query path.
#[derive(Debug, Clone)]
pub struct LiveMask {
    rows: Vec<bool>,
}

impl LiveMask {
    pub fn from_store(store: &ChunkStore) -> Result<Self, RetrievalError> {
        let mut rows = vec![false; store.matrix().rows() as usize];
        for row in store.live_rows()? {
            if let Some(slot) = rows.get_mut(row.get() as usize) {
                *slot = true;
            }
        }
        Ok(Self { rows })
    }

    pub fn is_live(&self, row: RowId) -> bool {
        self.rows.get(row.get() as usize).copied().unwrap_or(false)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn from_bits(rows: Vec<bool>) -> Self {
        Self { rows }
    }
}

#[cfg(test)]
mod tests {
    use half::f16;
    use inference::{Embedder, MockEmbedder, Role};
    use storage::{ChunkRecord, ChunkStore, ContentHash, EmbeddingMatrix, RowId};
    use tempfile::TempDir;

    use super::{DenseBackend, DenseIndex, LiveMask};
    use crate::dense_reference::reference_search;

    fn record(index: u64) -> ChunkRecord {
        ChunkRecord {
            repository: "repo".into(),
            file: format!("src/{index}.rs"),
            language: "rust".into(),
            symbol: format!("symbol_{index}"),
            symbol_type: "function".into(),
            signature: format!("fn symbol_{index}()"),
            doc_first_line: None,
            line_start: 1,
            line_end: 2,
            content_hash: ContentHash::of(&index.to_le_bytes()),
        }
    }

    #[test]
    fn live_mask_tracks_every_matrix_position() {
        let dir = TempDir::new().unwrap();
        let mut store = ChunkStore::create(dir.path(), 4, "model").unwrap();
        let chunks = (0..100)
            .map(|index| (record(index), vec![f16::from_f32(index as f32); 4]))
            .collect::<Vec<_>>();
        let rows = store.insert_batch(&chunks).unwrap();
        let tombstoned = rows.iter().copied().step_by(10).collect::<Vec<_>>();
        store.tombstone(&tombstoned).unwrap();

        let mask = LiveMask::from_store(&store).unwrap();

        assert_eq!(mask.len(), store.matrix().rows() as usize);
        for row in rows {
            assert_eq!(
                mask.is_live(row),
                !tombstoned.contains(&row),
                "wrong liveness for row {}",
                row.get()
            );
        }
        assert!(!mask.is_live(RowId::from_u64(100)));
    }

    fn random_value(state: &mut u64) -> f32 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        ((*state >> 40) as f32 / (1_u32 << 24) as f32) * 2.0 - 1.0
    }

    fn normalized_vector(state: &mut u64, dims: usize) -> Vec<f32> {
        let mut vector = (0..dims)
            .map(|_| random_value(state))
            .collect::<Vec<_>>();
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        for value in &mut vector {
            *value /= norm;
        }
        vector
    }

    #[test]
    fn cpu_backend_matches_reference_for_randomized_queries() {
        const ROWS: usize = 1_000;
        const DIMS: usize = 64;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("matrix.f16");
        let mut matrix = EmbeddingMatrix::create(&path, DIMS as u32, "model").unwrap();
        let mut state = 0x5eed_cafe_f00d_u64;
        let mut matrix_f32 = Vec::with_capacity(ROWS * DIMS);
        for _ in 0..ROWS {
            let row = normalized_vector(&mut state, DIMS)
                .into_iter()
                .map(f16::from_f32)
                .collect::<Vec<_>>();
            matrix_f32.extend(row.iter().map(|value| value.to_f32()));
            matrix.append(&row).unwrap();
        }
        let live = LiveMask::from_bits((0..ROWS).map(|row| row % 19 != 0).collect());
        let index = DenseIndex::prepare(&matrix, &live, DenseBackend::Cpu).unwrap();

        for _ in 0..50 {
            let query_f32 = normalized_vector(&mut state, DIMS);
            let query = query_f32
                .into_iter()
                .map(f16::from_f32)
                .collect::<Vec<_>>();
            let reference_query = query.iter().map(|value| value.to_f32()).collect::<Vec<_>>();
            let expected = reference_search(&matrix_f32, &live, DIMS, &reference_query, 25);
            let actual = index.search(&query, "model", 25).unwrap();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn mock_query_returns_matching_chunk_at_rank_one() {
        const DIMS: u32 = 64;
        let dir = TempDir::new().unwrap();
        let embedder = MockEmbedder::new(DIMS);
        let texts = [
            "parse an incoming packet",
            "clamp timestamps that move backwards",
            "render the final frame",
            "flush pending writes",
        ];
        let embeddings = embedder.embed(&texts, Role::Document).unwrap();
        let mut store = ChunkStore::create(dir.path(), DIMS, embedder.model_id()).unwrap();
        let chunks = texts
            .iter()
            .zip(embeddings)
            .enumerate()
            .map(|(index, (_text, embedding))| (record(index as u64), embedding.vector))
            .collect::<Vec<_>>();
        let rows = store.insert_batch(&chunks).unwrap();
        let index = DenseIndex::from_store(&store, DenseBackend::Cpu).unwrap();

        let query = embedder.query_matching(texts[1]);
        let results = index.search(&query, embedder.model_id(), 3).unwrap();

        assert_eq!(results[0].row, rows[1]);
    }
}
