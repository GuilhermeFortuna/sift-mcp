pub use crate::registry::{Registration, RegistrationInput};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}
impl ApiError {
    pub fn new(code: &str, message: &str, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
        }
    }
    pub fn invalid(message: &str) -> Self {
        Self::new("invalid_request", message, false)
    }
    pub fn missing() -> Self {
        Self::new(
            "not_found",
            "The requested registration or job does not exist.",
            false,
        )
    }
    pub fn database() -> Self {
        Self::new(
            "history_recording_error",
            "Console metadata could not be recorded. Daemon retrieval remains available.",
            true,
        )
    }
    pub fn timeout() -> Self {
        Self::new(
            "timeout",
            "The response deadline elapsed. The disconnected operation may still be running.",
            true,
        )
    }
    pub fn status(&self) -> StatusCode {
        match self.code.as_str() {
            "not_found" | "symbol_not_found" => StatusCode::NOT_FOUND,
            "invalid_host" | "invalid_request" | "invalid_registration" => StatusCode::BAD_REQUEST,
            "cross_origin" | "invalid_csrf" | "forbidden" => StatusCode::FORBIDDEN,
            "unsupported_media_type" => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "request_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
            "duplicate_store" | "index_in_progress" | "symbol_ambiguous" | "store_stale" => {
                StatusCode::CONFLICT
            }
            "timeout" => StatusCode::GATEWAY_TIMEOUT,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut r = (self.status(), Json(self)).into_response();
        r.headers_mut()
            .insert("cache-control", "no-store".parse().unwrap());
        r
    }
}
impl From<daemon::DaemonError> for ApiError {
    fn from(e: daemon::DaemonError) -> Self {
        use daemon::DaemonError::*;
        let (code, message, retryable) = match e {
            ProtocolVersion { .. } => (
                "protocol_incompatible",
                "The configured daemon uses an incompatible protocol. It was not replaced.",
                false,
            ),
            Starting => ("starting", "The daemon is starting.", true),
            IndexInProgress => (
                "index_in_progress",
                "An indexing operation is already running.",
                true,
            ),
            SymbolNotFound { .. } => (
                "symbol_not_found",
                "The symbol was not found in the current index.",
                false,
            ),
            SymbolAmbiguous { .. } => (
                "symbol_ambiguous",
                "The symbol is ambiguous; specify its qualified name.",
                false,
            ),
            StoreStale { .. } => (
                "store_stale",
                "The index is stale; explicitly index the registered repository.",
                false,
            ),
            GpuUnavailable { .. } => ("gpu_unavailable", "Daemon inference is unavailable.", true),
            RequestTooLarge { .. } => (
                "request_too_large",
                "The request exceeds the daemon frame limit.",
                false,
            ),
            Malformed { .. } => (
                "connection_lost",
                "The daemon response was lost or invalid. The operation may still be running.",
                true,
            ),
            Internal { .. } => (
                "daemon_unavailable",
                "Could not connect to or start the configured daemon.",
                true,
            ),
            ObserverForbidden { .. } => (
                "forbidden",
                "This operation is not allowed on an observer connection.",
                false,
            ),
        };
        Self::new(code, message, retryable)
    }
}
impl From<crate::db::DbError> for ApiError {
    fn from(e: crate::db::DbError) -> Self {
        use crate::registry::RegistryError;
        match e {
            crate::db::DbError::Registry(RegistryError::Unknown(_)) => Self::missing(),
            crate::db::DbError::Registry(RegistryError::DuplicateStore(_)) => Self::new(
                "duplicate_store",
                "A registration already uses this canonical store location.",
                false,
            ),
            crate::db::DbError::Registry(e) => {
                Self::new("invalid_registration", &e.to_string(), false)
            }
            _ => Self::database(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}
pub type ActivityPage = Page<crate::history::RequestEvent>;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Succeeded,
    Failed,
    Interrupted,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexJob {
    pub id: String,
    pub repository_id: String,
    pub state: JobState,
    pub progress: Option<daemon::IndexPhase>,
    pub done: u64,
    pub total: Option<u64>,
    pub report: Option<daemon::IndexReportWire>,
    pub error_code: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchInput {
    pub query: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimilarInput {
    pub code: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolInput {
    pub file: String,
    pub symbol: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexInput {
    pub mode: IndexMode,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndexMode {
    Update,
    Full,
}
fn default_top_k() -> usize {
    5
}
