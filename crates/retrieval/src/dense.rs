use storage::{ChunkStore, RowId};

use crate::RetrievalError;

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
}

#[cfg(test)]
mod tests {
    use half::f16;
    use storage::{ChunkRecord, ChunkStore, ContentHash, RowId};
    use tempfile::TempDir;

    use super::LiveMask;

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
}
