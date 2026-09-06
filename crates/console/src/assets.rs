use crate::api::types::ApiError;
use axum::{
    body::Body,
    http::{Request, header},
    response::{IntoResponse, Response},
};
use std::path::{Component, Path, PathBuf};
#[derive(Clone)]
pub struct Assets {
    root: PathBuf,
}
impl Assets {
    pub fn open(path: &Path) -> Result<Self, std::io::Error> {
        let root = path.canonicalize()?;
        if !root.is_dir() || !root.join("index.html").is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Frontend assets are missing; build ui and supply --assets.",
            ));
        }
        Ok(Self { root })
    }
    pub async fn serve(&self, request: Request<Body>) -> Response {
        if request.method() != http::Method::GET && request.method() != http::Method::HEAD {
            return ApiError::missing().into_response();
        }
        let Some(path) = decode(request.uri().path()) else {
            return ApiError::invalid("Invalid asset path.").into_response();
        };
        if path == "/api" || path.starts_with("/api/") {
            return ApiError::missing().into_response();
        }
        let relative = Path::new(path.trim_start_matches('/'));
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
            || path.contains('\\')
            || path.contains('\0')
        {
            return ApiError::invalid("Asset path traversal is forbidden.").into_response();
        }
        let mut target = self.root.join(relative);
        if path == "/" {
            target = self.root.join("index.html");
        }
        if !target.exists()
            && relative.extension().is_none()
            && request.headers().get(header::ACCEPT).is_some_and(|v| {
                v.to_str()
                    .is_ok_and(|v| v.split(',').any(|x| x.trim().starts_with("text/html")))
            })
        {
            target = self.root.join("index.html");
        }
        let Ok(canonical) = tokio::fs::canonicalize(target).await else {
            return ApiError::missing().into_response();
        };
        if !canonical.starts_with(&self.root) {
            return ApiError::new(
                "forbidden",
                "Assets must remain inside the configured asset directory.",
                false,
            )
            .into_response();
        }
        let Ok(bytes) = tokio::fs::read(&canonical).await else {
            return ApiError::missing().into_response();
        };
        let mime = match canonical.extension().and_then(|s| s.to_str()) {
            Some("html") => "text/html; charset=utf-8",
            Some("js" | "mjs") => "text/javascript; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("json") => "application/json",
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            Some("ico") => "image/x-icon",
            Some("woff2") => "font/woff2",
            _ => "application/octet-stream",
        };
        let length = bytes.len();
        let mut response = if request.method() == http::Method::HEAD {
            Body::empty().into_response()
        } else {
            Body::from(bytes).into_response()
        };
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, mime.parse().unwrap());
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, length.into());
        response
    }
}
fn decode(path: &str) -> Option<String> {
    let b = path.as_bytes();
    let mut result = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hi = (*b.get(i + 1)? as char).to_digit(16)?;
            let lo = (*b.get(i + 2)? as char).to_digit(16)?;
            result.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            result.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(result).ok()
}
