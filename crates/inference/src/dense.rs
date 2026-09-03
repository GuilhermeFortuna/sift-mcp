use half::f16;

use crate::InferError;

/// Whole-matrix exact scorer implemented by hardware-specific backends.
pub trait DenseScorer: Send {
    fn prepare(&mut self, matrix: &[f16], rows: u64, dims: u32) -> Result<(), InferError>;
    fn append(&mut self, rows: &[f16]) -> Result<(), InferError>;
    fn score_all(&mut self, query: &[f16]) -> Result<Vec<f32>, InferError>;
    fn resident_bytes(&self) -> u64;
    fn uploaded_bytes(&self) -> u64;
}

#[cfg(feature = "cuda")]
mod cuda {
    use std::ffi::c_void;
    use std::sync::Arc;

    use cudarc::cublas::{CudaBlas, result, sys};
    use cudarc::driver::{CudaContext, CudaSlice, CudaStream, DevicePtr, DevicePtrMut};

    use super::*;

    pub struct CudaDenseScorer {
        stream: Arc<CudaStream>,
        blas: CudaBlas,
        matrix: Option<CudaSlice<f16>>,
        rows: u64,
        dims: u32,
        uploaded_bytes: u64,
    }

    impl CudaDenseScorer {
        pub fn new() -> Result<Self, InferError> {
            let context = CudaContext::new(0).map_err(|error| InferError::GpuUnavailable {
                detail: error.to_string(),
            })?;
            let stream = context.default_stream();
            let blas = CudaBlas::new(stream.clone())
                .map_err(|error| InferError::Runtime(format!("create cuBLAS handle: {error}")))?;
            Ok(Self {
                stream,
                blas,
                matrix: None,
                rows: 0,
                dims: 0,
                uploaded_bytes: 0,
            })
        }

        fn runtime(context: &str, error: impl std::fmt::Display) -> InferError {
            InferError::Runtime(format!("{context}: {error}"))
        }
    }

    impl DenseScorer for CudaDenseScorer {
        fn prepare(&mut self, matrix: &[f16], rows: u64, dims: u32) -> Result<(), InferError> {
            if matrix.len() != rows as usize * dims as usize {
                return Err(InferError::Runtime(format!(
                    "dense matrix shape mismatch: {} values for {rows}x{dims}",
                    matrix.len()
                )));
            }
            self.matrix = Some(
                self.stream
                    .clone_htod(matrix)
                    .map_err(|error| Self::runtime("upload dense matrix", error))?,
            );
            self.rows = rows;
            self.dims = dims;
            self.uploaded_bytes = std::mem::size_of_val(matrix) as u64;
            Ok(())
        }

        fn append(&mut self, rows: &[f16]) -> Result<(), InferError> {
            if self.dims == 0 || !rows.len().is_multiple_of(self.dims as usize) {
                return Err(InferError::Runtime(format!(
                    "appended dense values {} do not match width {}",
                    rows.len(),
                    self.dims
                )));
            }
            if rows.is_empty() {
                return Ok(());
            }
            let old = self
                .matrix
                .take()
                .ok_or_else(|| InferError::Runtime("dense scorer is not prepared".into()))?;
            let old_len = self.rows as usize * self.dims as usize;
            let mut grown = self
                .stream
                .alloc_zeros::<f16>(old_len + rows.len())
                .map_err(|error| Self::runtime("grow dense matrix", error))?;
            {
                let mut old_destination = grown.slice_mut(..old_len);
                self.stream
                    .memcpy_dtod(&old, &mut old_destination)
                    .map_err(|error| Self::runtime("copy prepared dense rows", error))?;
            }
            {
                let mut appended_destination = grown.slice_mut(old_len..);
                self.stream
                    .memcpy_htod(rows, &mut appended_destination)
                    .map_err(|error| Self::runtime("upload appended dense rows", error))?;
            }
            self.matrix = Some(grown);
            self.rows += (rows.len() / self.dims as usize) as u64;
            self.uploaded_bytes += std::mem::size_of_val(rows) as u64;
            Ok(())
        }

        fn score_all(&mut self, query: &[f16]) -> Result<Vec<f32>, InferError> {
            if query.len() != self.dims as usize {
                return Err(InferError::Runtime(format!(
                    "dense query width mismatch: expected {}, got {}",
                    self.dims,
                    query.len()
                )));
            }
            if self.rows == 0 {
                return Ok(Vec::new());
            }
            let matrix = self
                .matrix
                .as_ref()
                .ok_or_else(|| InferError::Runtime("dense scorer is not prepared".into()))?;
            let query_device = self
                .stream
                .clone_htod(query)
                .map_err(|error| Self::runtime("upload dense query", error))?;
            let mut scores_device = self
                .stream
                .alloc_zeros::<f32>(self.rows as usize)
                .map_err(|error| Self::runtime("allocate dense scores", error))?;
            let alpha = 1.0_f32;
            let beta = 0.0_f32;
            {
                let (matrix_ptr, _matrix_guard) = matrix.device_ptr(&self.stream);
                let (query_ptr, _query_guard) = query_device.device_ptr(&self.stream);
                let (scores_ptr, _scores_guard) = scores_device.device_ptr_mut(&self.stream);
                unsafe {
                    result::gemm_ex(
                        *self.blas.handle(),
                        sys::cublasOperation_t::CUBLAS_OP_T,
                        sys::cublasOperation_t::CUBLAS_OP_N,
                        self.rows as i32,
                        1,
                        self.dims as i32,
                        (&alpha as *const f32).cast::<c_void>(),
                        matrix_ptr as *const c_void,
                        sys::cudaDataType_t::CUDA_R_16F,
                        self.dims as i32,
                        query_ptr as *const c_void,
                        sys::cudaDataType_t::CUDA_R_16F,
                        self.dims as i32,
                        (&beta as *const f32).cast::<c_void>(),
                        scores_ptr as *mut c_void,
                        sys::cudaDataType_t::CUDA_R_32F,
                        self.rows as i32,
                        sys::cublasComputeType_t::CUBLAS_COMPUTE_32F,
                        sys::cublasGemmAlgo_t::CUBLAS_GEMM_DEFAULT,
                    )
                }
                .map_err(|error| Self::runtime("score dense matrix", error))?;
            }
            self.stream
                .clone_dtoh(&scores_device)
                .map_err(|error| Self::runtime("download dense scores", error))
        }

        fn resident_bytes(&self) -> u64 {
            self.rows * self.dims as u64 * std::mem::size_of::<f16>() as u64
        }

        fn uploaded_bytes(&self) -> u64 {
            self.uploaded_bytes
        }
    }
}

#[cfg(feature = "cuda")]
pub use cuda::CudaDenseScorer;
