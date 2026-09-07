use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use console::assets::Assets;
use std::process::Command;

#[tokio::test]
async fn packaged_assets_serve_direct_search_route_and_prevent_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let ui_root = dir.path().join("ui");
    std::fs::create_dir(&ui_root).unwrap();
    std::fs::write(
        ui_root.join("index.html"),
        "<!doctype html><html><body><div id=\"root\">Search Lab Shell</div></body></html>",
    )
    .unwrap();
    std::fs::write(dir.path().join("secret.txt"), "CONFIDENTIAL_HOST_DATA").unwrap();

    let assets = Assets::open(&ui_root).unwrap();

    // Direct /search route navigation with text/html accept header
    let res = assets
        .serve(
            Request::builder()
                .uri("/search")
                .header("accept", "text/html")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body_bytes = to_bytes(res.into_body(), 10_000).await.unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert!(body_str.contains("Search Lab Shell"));

    // Traversal attempts must be rejected
    for traversal_path in [
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/..%2fsecret.txt",
        "/%2e%2e%2fsecret.txt",
    ] {
        let res = assets
            .serve(
                Request::builder()
                    .uri(traversal_path)
                    .header("accept", "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert!(
            res.status() == StatusCode::BAD_REQUEST
                || res.status() == StatusCode::NOT_FOUND
                || res.status() == StatusCode::FORBIDDEN,
            "path {traversal_path} must be rejected, got {}",
            res.status()
        );
    }
}

#[test]
fn missing_assets_returns_actionable_error_referencing_assets_flag() {
    let empty_dir = tempfile::tempdir().unwrap();
    let err = Assets::open(empty_dir.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("--assets"),
        "error message must advise user on --assets flag: {msg}"
    );
    assert!(
        msg.contains("Frontend assets are missing"),
        "error message must describe missing assets: {msg}"
    );
}

#[test]
fn console_binary_runs_under_sanitized_path_without_node_or_python() {
    let target_bin = env!("CARGO_BIN_EXE_sift-console");

    // Construct a sanitized PATH directory containing only standard minimal utilities
    let temp_bin_dir = tempfile::tempdir().unwrap();
    // Intentionally no node, pnpm, python, python3 in this sanitized PATH
    let output = Command::new(target_bin)
        .arg("--help")
        .env("PATH", temp_bin_dir.path())
        .output()
        .expect("sift-console binary should execute without node or python");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("sift-console"));
}
