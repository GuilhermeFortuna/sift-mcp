use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

#[tokio::test]
async fn an_arbitrary_csrf_token_is_not_authorization() {
    let response = console::api::router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/repositories")
                .header("host", "127.0.0.1:7331")
                .header("origin", "http://127.0.0.1:7331")
                .header("content-type", "application/json")
                .header("X-Sift-CSRF", "forged")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn cross_origin_reads_are_rejected() {
    let response = console::api::router()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .header("host", "127.0.0.1:7331")
                .header("origin", "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
