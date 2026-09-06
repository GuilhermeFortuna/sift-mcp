use axum::{
    Json, Router,
    body::Body,
    http::{HeaderValue, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;

pub const LISTEN_HOST: &str = "127.0.0.1:7331";
const ORIGIN: &str = "http://127.0.0.1:7331";
const CSRF: &str = "X-Sift-CSRF";
#[derive(Serialize)]
struct Error {
    code: &'static str,
    message: &'static str,
    retryable: bool,
}
pub fn router() -> Router {
    Router::new()
        .route(
            "/api/v1/health",
            get(|| async { Json(serde_json::json!({"status":"ok"})) }),
        )
        .layer(axum::extract::DefaultBodyLimit::max(1_048_576))
        .layer(middleware::from_fn(security))
}
async fn security(request: Request<Body>, next: Next) -> Response {
    if request
        .headers()
        .get(header::HOST)
        .is_none_or(|v| v.as_bytes() != LISTEN_HOST.as_bytes())
    {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_host",
            "request Host is not the configured loopback host",
        );
    }
    if !matches!(*request.method(), http::Method::GET | http::Method::HEAD)
        && (request
            .headers()
            .get(header::ORIGIN)
            .is_none_or(|v| v.as_bytes() != ORIGIN.as_bytes())
            || request.headers().get(CSRF).is_none())
    {
        return error(
            StatusCode::FORBIDDEN,
            "cross_origin",
            "mutations require the same Origin and a CSRF token",
        );
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
fn error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    let mut response = (
        status,
        Json(Error {
            code,
            message,
            retryable: false,
        }),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;
    #[tokio::test]
    async fn rejects_cross_origin_mutation() {
        let response = router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/health")
                    .header(header::HOST, LISTEN_HOST)
                    .header(header::ORIGIN, "https://evil.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    #[tokio::test]
    async fn accepts_loopback_health_without_mutation_headers() {
        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/health")
                    .header(header::HOST, LISTEN_HOST)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
