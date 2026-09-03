#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "requires CUDA hardware and ONNX Runtime"]
    fn gpu_inference_is_only_meaningful_with_hardware() {
        assert!(
            cfg!(feature = "cuda"),
            "GPU tests are only meaningful with the cuda feature and hardware"
        );
    }
}
