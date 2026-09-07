use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use console::assets::Assets;
#[tokio::test]
async fn production_assets_and_spa_fallback_do_not_expose_api_or_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ui");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("index.html"), "<main>Console</main>").unwrap();
    std::fs::write(root.join("app.js"), "export default 1;").unwrap();
    std::fs::write(dir.path().join("secret"), "PRIVATE").unwrap();
    std::os::unix::fs::symlink(dir.path().join("secret"), root.join("escape.txt")).unwrap();
    let assets = Assets::open(&root).unwrap();
    for path in ["/", "/repositories/example"] {
        let r = assets
            .serve(
                Request::builder()
                    .uri(path)
                    .header("accept", "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(
            String::from_utf8(to_bytes(r.into_body(), 1000).await.unwrap().to_vec())
                .unwrap()
                .contains("Console")
        );
    }
    for (path, status) in [
        ("/api/missing", StatusCode::NOT_FOUND),
        ("/api", StatusCode::NOT_FOUND),
        ("/missing.js", StatusCode::NOT_FOUND),
        ("/%2e%2e/secret", StatusCode::BAD_REQUEST),
        ("/escape.txt", StatusCode::FORBIDDEN),
    ] {
        let r = assets
            .serve(
                Request::builder()
                    .uri(path)
                    .header("accept", "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(r.status(), status, "{path}");
        assert!(
            !String::from_utf8(to_bytes(r.into_body(), 2000).await.unwrap().to_vec())
                .unwrap()
                .contains("PRIVATE")
        );
    }
    let js = assets
        .serve(
            Request::builder()
                .uri("/app.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(
        js.headers()["content-type"],
        "text/javascript; charset=utf-8"
    );
}
#[test]
fn missing_build_has_an_actionable_startup_error() {
    let t = tempfile::tempdir().unwrap();
    assert!(
        Assets::open(t.path())
            .err()
            .unwrap()
            .to_string()
            .contains("--assets")
    );
}
