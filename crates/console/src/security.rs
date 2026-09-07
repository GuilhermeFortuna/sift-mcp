use crate::api::types::ApiError;
use axum::{
    body::{Body, to_bytes},
    extract::State,
    http::{HeaderValue, Method, Request, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
#[derive(Clone)]
pub struct Security {
    pub host: String,
    pub origin: String,
    pub csrf: String,
}
impl Security {
    pub fn new(host: String) -> Self {
        Self {
            origin: format!("http://{host}"),
            host,
            csrf: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
        }
    }
}
pub async fn guard(
    State(config): State<Security>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let error = if request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        != Some(config.host.as_str())
    {
        Some(ApiError::new(
            "invalid_host",
            "Host must match the configured loopback address.",
            false,
        ))
    } else if request
        .headers()
        .get(header::ORIGIN)
        .is_some_and(|v| v.as_bytes() != config.origin.as_bytes())
        || request
            .headers()
            .get("sec-fetch-site")
            .is_some_and(|v| v == "cross-site" || v == "same-site")
    {
        Some(ApiError::new(
            "cross_origin",
            "Requests must come from the console origin.",
            false,
        ))
    } else if !matches!(*request.method(), Method::GET | Method::HEAD) {
        if request
            .headers()
            .get(header::ORIGIN)
            .is_none_or(|v| v.as_bytes() != config.origin.as_bytes())
        {
            Some(ApiError::new(
                "cross_origin",
                "Mutations require the console Origin.",
                false,
            ))
        } else if request
            .headers()
            .get("x-sift-csrf")
            .is_none_or(|v| v.as_bytes() != config.csrf.as_bytes())
        {
            Some(ApiError::new(
                "invalid_csrf",
                "Refresh the console session before making this request.",
                false,
            ))
        } else if request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_none_or(|v| v.split(';').next().unwrap_or("").trim() != "application/json")
        {
            Some(ApiError::new(
                "unsupported_media_type",
                "Mutations require application/json.",
                false,
            ))
        } else {
            None
        }
    } else {
        None
    };
    let mut response = if let Some(e) = error {
        e.into_response()
    } else {
        if !matches!(*request.method(), Method::GET | Method::HEAD) {
            let (parts, body) = request.into_parts();
            match to_bytes(body, daemon::MAX_REQUEST_BYTES).await {
                Ok(bytes) => request = Request::from_parts(parts, Body::from(bytes)),
                Err(_) => {
                    return headers(
                        ApiError::new(
                            "request_too_large",
                            "JSON requests are limited to 1 MiB.",
                            false,
                        )
                        .into_response(),
                    );
                }
            }
        }
        next.run(request).await
    };
    if response.status() == axum::http::StatusCode::UNPROCESSABLE_ENTITY {
        response = ApiError::invalid("The JSON request does not match the endpoint contract.")
            .into_response();
    }
    if response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|v| v.as_bytes().starts_with(b"application/json"))
    {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    headers(response)
}
fn headers(mut r: Response) -> Response {
    r.headers_mut().insert("content-security-policy",HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"));
    r.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    r.headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    r
}
