use crate::Integrity;
use crate::error::StoreError;
use crate::record::RowId;
use half::f16;
use memmap2::{Mmap, MmapMut, MmapOptions};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// On-disk matrix format version. Bumped only when the binary layout changes.
pub const MATRIX_FORMAT_VERSION: u32 = 1;

const MAGIC: &[u8; 8] = b"SIFTEMB\0";
const MODEL_ID_CAPACITY: usize = 256;
/// Fixed header size: magic(8) + ver(4) + dims(4) + rows(8) + model_len(4) + model(256) + pad
const HEADER_SIZE: usize = 288;
/// Grow the backing file in blocks of this many rows to amortize remaps.
const GROW_BLOCK_ROWS: u64 = 1024;

/// On-disk header. Written once; read and checked on every open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixHeader {
    pub magic: [u8; 8],
    pub format_version: u32,
    pub dims: u32,
    pub rows: u64,
    pub model_id: String,
}

impl MatrixHeader {
    fn encode(&self) -> Result<[u8; HEADER_SIZE], StoreError> {
        if self.model_id.len() > MODEL_ID_CAPACITY {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "model_id exceeds maximum length",
            )));
        }
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..8].copy_from_slice(&self.magic);
        buf[8..12].copy_from_slice(&self.format_version.to_le_bytes());
        buf[12..16].copy_from_slice(&self.dims.to_le_bytes());
        buf[16..24].copy_from_slice(&self.rows.to_le_bytes());
        let mid = self.model_id.as_bytes();
        buf[24..28].copy_from_slice(&(mid.len() as u32).to_le_bytes());
        buf[28..28 + mid.len()].copy_from_slice(mid);
        Ok(buf)
    }

    fn decode(bytes: &[u8]) -> Result<Self, StoreError> {
        if bytes.len() < HEADER_SIZE {
            return Err(StoreError::Corrupt(Integrity::Broken {
                orphan_rows: Vec::new(),
                missing_rows: Vec::new(),
                duplicate_rows: Vec::new(),
            }));
        }
        let mut magic = [0u8; 8];
        magic.copy_from_slice(&bytes[0..8]);
        if &magic != MAGIC {
            return Err(StoreError::Corrupt(Integrity::Broken {
                orphan_rows: Vec::new(),
                missing_rows: Vec::new(),
                duplicate_rows: Vec::new(),
            }));
        }
        let format_version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if format_version != MATRIX_FORMAT_VERSION {
            return Err(StoreError::SchemaVersion {
                expected: MATRIX_FORMAT_VERSION,
                got: format_version,
            });
        }
        let dims = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let rows = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let model_len = u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize;
        if model_len > MODEL_ID_CAPACITY {
            return Err(StoreError::Corrupt(Integrity::Broken {
                orphan_rows: Vec::new(),
                missing_rows: Vec::new(),
                duplicate_rows: Vec::new(),
            }));
        }
        let model_id = std::str::from_utf8(&bytes[28..28 + model_len])
            .map_err(|e| StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?
            .to_owned();
        Ok(Self {
            magic,
            format_version,
            dims,
            rows,
            model_id,
        })
    }
}

enum Map {
    Read(Mmap),
    Write(MmapMut),
}

impl Map {
    fn as_bytes(&self) -> &[u8] {
        match self {
            Map::Read(m) => m,
            Map::Write(m) => m,
        }
    }

    fn as_bytes_mut(&mut self) -> Result<&mut [u8], StoreError> {
        match self {
            Map::Write(m) => Ok(m),
            Map::Read(_) => Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "matrix opened read-only",
            ))),
        }
    }
}

/// Memory-mapped fp16 matrix, one fixed-width row per chunk.
pub struct EmbeddingMatrix {
    path: PathBuf,
    header: MatrixHeader,
    /// Capacity in rows currently mapped (may exceed header.rows).
    capacity_rows: u64,
    map: Map,
}

impl std::fmt::Debug for EmbeddingMatrix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddingMatrix")
            .field("path", &self.path)
            .field("header", &self.header)
            .field("capacity_rows", &self.capacity_rows)
            .finish_non_exhaustive()
    }
}

#[allow(dead_code)] // helpers used by ChunkStore in later steps
impl EmbeddingMatrix {
    pub fn create(path: &Path, dims: u32, model_id: &str) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let header = MatrixHeader {
            magic: *MAGIC,
            format_version: MATRIX_FORMAT_VERSION,
            dims,
            rows: 0,
            model_id: model_id.to_owned(),
        };
        let capacity_rows = GROW_BLOCK_ROWS;
        let file_len = HEADER_SIZE as u64 + capacity_rows * dims as u64 * 2;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len(file_len)?;
        {
            let encoded = header.encode()?;
            file.write_all(&encoded)?;
            file.flush()?;
        }
        let map = unsafe { MmapOptions::new().map_mut(&file)? };
        Ok(Self {
            path: path.to_path_buf(),
            header,
            capacity_rows,
            map: Map::Write(map),
        })
    }

    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let file = File::open(path)?;
        let map = unsafe { MmapOptions::new().map(&file)? };
        let header = MatrixHeader::decode(&map)?;
        let capacity_rows = if header.dims == 0 {
            0
        } else {
            let data_bytes = map.len().saturating_sub(HEADER_SIZE);
            (data_bytes / (header.dims as usize * 2)) as u64
        };
        Ok(Self {
            path: path.to_path_buf(),
            header,
            capacity_rows,
            map: Map::Read(map),
        })
    }

    /// Open for mutation (append). Used by ChunkStore.
    pub(crate) fn open_mut(path: &Path) -> Result<Self, StoreError> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let map = unsafe { MmapOptions::new().map_mut(&file)? };
        let header = MatrixHeader::decode(&map)?;
        let capacity_rows = if header.dims == 0 {
            0
        } else {
            let data_bytes = map.len().saturating_sub(HEADER_SIZE);
            (data_bytes / (header.dims as usize * 2)) as u64
        };
        Ok(Self {
            path: path.to_path_buf(),
            header,
            capacity_rows,
            map: Map::Write(map),
        })
    }

    pub fn append(&mut self, vector: &[f16]) -> Result<RowId, StoreError> {
        let dims = self.header.dims as usize;
        if vector.len() != dims {
            return Err(StoreError::DimensionMismatch {
                expected: self.header.dims,
                got: vector.len() as u32,
            });
        }
        if self.header.rows >= self.capacity_rows {
            self.grow()?;
        }
        let row = self.header.rows;
        let offset = HEADER_SIZE + (row as usize) * dims * 2;
        let bytes = self.map.as_bytes_mut()?;
        let dst = &mut bytes[offset..offset + dims * 2];
        let src = f16_slice_as_bytes(vector);
        dst.copy_from_slice(src);
        self.header.rows += 1;
        self.write_header()?;
        Ok(RowId::new(row))
    }

    pub fn row(&self, row: RowId) -> Result<&[f16], StoreError> {
        if row.0 >= self.header.rows {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("row {} out of range (rows={})", row.0, self.header.rows),
            )));
        }
        let dims = self.header.dims as usize;
        let offset = HEADER_SIZE + (row.0 as usize) * dims * 2;
        let bytes = &self.map.as_bytes()[offset..offset + dims * 2];
        Ok(bytes_as_f16_slice(bytes))
    }

    /// Whole matrix as a contiguous slice, including dead rows.
    pub fn as_slice(&self) -> &[f16] {
        let dims = self.header.dims as usize;
        let n = (self.header.rows as usize) * dims;
        if n == 0 {
            return &[];
        }
        let bytes = &self.map.as_bytes()[HEADER_SIZE..HEADER_SIZE + n * 2];
        bytes_as_f16_slice(bytes)
    }

    pub fn dims(&self) -> u32 {
        self.header.dims
    }

    pub fn model_id(&self) -> &str {
        &self.header.model_id
    }

    pub fn rows(&self) -> u64 {
        self.header.rows
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn header(&self) -> &MatrixHeader {
        &self.header
    }

    /// Truncate logical row count (used during compaction swap setup).
    pub(crate) fn set_rows_for_test_corruption(&mut self, rows: u64) -> Result<(), StoreError> {
        self.header.rows = rows;
        self.write_header()
    }

    fn grow(&mut self) -> Result<(), StoreError> {
        let new_capacity = self.capacity_rows + GROW_BLOCK_ROWS;
        let file_len = HEADER_SIZE as u64 + new_capacity * self.header.dims as u64 * 2;
        // Drop map before resizing.
        let path = self.path.clone();
        // Replace with a temporary empty read map pattern: drop write map.
        self.map = Map::Read(unsafe {
            // Re-open briefly after drop — construct placeholder by remapping.
            let f = File::open(&path)?;
            MmapOptions::new().len(HEADER_SIZE).map(&f)?
        });
        {
            let file = OpenOptions::new().read(true).write(true).open(&path)?;
            file.set_len(file_len)?;
            let map = unsafe { MmapOptions::new().map_mut(&file)? };
            self.map = Map::Write(map);
        }
        self.capacity_rows = new_capacity;
        Ok(())
    }

    fn write_header(&mut self) -> Result<(), StoreError> {
        let encoded = self.header.encode()?;
        let bytes = self.map.as_bytes_mut()?;
        bytes[..HEADER_SIZE].copy_from_slice(&encoded);
        if let Map::Write(m) = &self.map {
            m.flush()?;
        }
        Ok(())
    }

    /// Persist header rows field by reopening file (for create path after drop).
    pub(crate) fn flush(&mut self) -> Result<(), StoreError> {
        self.write_header()?;
        if let Map::Write(m) = &self.map {
            m.flush()?;
        }
        Ok(())
    }

    /// Test helper: rewrite the on-disk header row count without a live map.
    pub fn rewrite_header_for_test(
        path: &Path,
        dims: u32,
        model_id: &str,
        rows: u64,
    ) -> Result<(), StoreError> {
        let header = MatrixHeader {
            magic: *MAGIC,
            format_version: MATRIX_FORMAT_VERSION,
            dims,
            rows,
            model_id: model_id.to_owned(),
        };
        Self::rewrite_header_on_disk(path, &header)
    }

    /// Rewrite file to exactly `rows` of capacity (compaction helper).
    pub(crate) fn create_compacted(
        path: &Path,
        dims: u32,
        model_id: &str,
        vectors: &[&[f16]],
    ) -> Result<Self, StoreError> {
        let mut matrix = Self::create(path, dims, model_id)?;
        for v in vectors {
            matrix.append(v)?;
        }
        let used = HEADER_SIZE as u64 + matrix.header.rows * dims as u64 * 2;
        matrix.flush()?;
        matrix.map = Map::Read(unsafe {
            let f = File::open(path)?;
            MmapOptions::new().len(HEADER_SIZE).map(&f)?
        });
        {
            let file = OpenOptions::new().read(true).write(true).open(path)?;
            file.set_len(used)?;
            let map = unsafe { MmapOptions::new().map_mut(&file)? };
            matrix.map = Map::Write(map);
        }
        matrix.capacity_rows = matrix.header.rows;
        Ok(matrix)
    }

    /// Sync header.rows to the file without holding the map (corruption helpers).
    pub(crate) fn rewrite_header_on_disk(
        path: &Path,
        header: &MatrixHeader,
    ) -> Result<(), StoreError> {
        let mut file = OpenOptions::new().write(true).open(path)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header.encode()?)?;
        file.flush()?;
        Ok(())
    }
}

fn f16_slice_as_bytes(v: &[f16]) -> &[u8] {
    // f16 is transparent over u16; safe to view as bytes.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2) }
}

fn bytes_as_f16_slice(bytes: &[u8]) -> &[f16] {
    assert!(bytes.len().is_multiple_of(2));
    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f16, bytes.len() / 2) }
}
