#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "requires CUDA hardware and ONNX Runtime"]
    fn gpu_inference_is_only_meaningful_with_hardware() {
        panic!("GPU inference tests are only meaningful with CUDA hardware and ONNX Runtime");
    }
}
