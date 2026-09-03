/// blake3 over the normalized symbol body. Excludes the file path by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash(pub(crate) [u8; 32]);

impl ContentHash {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Hash normalized body bytes with blake3.
    pub fn of(body: &[u8]) -> Self {
        Self(*blake3::hash(body).as_bytes())
    }
}

/// Position of a chunk's embedding in the matrix. Assigned only by the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowId(pub(crate) u64);

impl RowId {
    pub(crate) fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn from_u64(id: u64) -> Self {
        Self::new(id)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

/// A chunk as it is stored. Mirrors the record shape in the design document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRecord {
    pub repository: String,
    pub file: String, // repository-relative, forward slashes on all platforms
    pub language: String,
    pub symbol: String,
    pub symbol_type: String,
    pub signature: String,
    pub doc_first_line: Option<String>,
    pub line_start: u32, // 1-based, inclusive
    pub line_end: u32,   // 1-based, inclusive
    pub content_hash: ContentHash,
}
