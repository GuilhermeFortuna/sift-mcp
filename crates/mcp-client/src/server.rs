//! MCP stdio server: thin pass-through to the resident daemon.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use daemon::DaemonClient;
use daemon::protocol::{DaemonError, IndexMode, Request, Response};
use futures::StreamExt;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, NumberOrString, PaginatedRequestParams,
    ProgressNotificationParam, ProgressToken, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use tokio::sync::Mutex;

use crate::params::{
    self, FindSimilarCodeParams, GetSymbolParams, IndexRepositoryParams, SearchCodeParams,
};
use crate::tools;

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("{0}")]
    Message(String),
}

/// Configuration for connecting to / spawning the daemon.
#[derive(Clone)]
pub struct SiftMcpConfig {
    pub store_dir: PathBuf,
    pub repo_dir: PathBuf,
    pub model_dir: PathBuf,
    pub daemon_binary: PathBuf,
    pub connect_deadline: Duration,
    /// When false, never spawn; only connect to an existing socket.
    pub allow_spawn: bool,
}

pub struct SiftMcpServer {
    config: SiftMcpConfig,
    client: Arc<Mutex<Option<DaemonClient>>>,
    tool_router: ToolRouter<Self>,
}

impl Clone for SiftMcpServer {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            client: Arc::clone(&self.client),
            tool_router: self.tool_router.clone(),
        }
    }
}

impl SiftMcpServer {
    pub fn new(store_dir: PathBuf) -> Self {
        Self::with_config(SiftMcpConfig {
            store_dir,
            repo_dir: PathBuf::from("."),
            model_dir: PathBuf::from("."),
            daemon_binary: PathBuf::from("sift-daemon"),
            connect_deadline: Duration::from_secs(60),
            allow_spawn: true,
        })
    }

    pub fn with_config(config: SiftMcpConfig) -> Self {
        Self {
            config,
            client: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
        }
    }

    /// Serves the MCP stdio transport until the stream closes.
    pub async fn serve_stdio(self) -> Result<(), ServeError> {
        let service = self
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| ServeError::Message(e.to_string()))?;
        service
            .waiting()
            .await
            .map_err(|e| ServeError::Message(e.to_string()))?;
        Ok(())
    }

    async fn ensure_client(&self) -> Result<(), DaemonError> {
        let mut guard = self.client.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        let client = if self.config.allow_spawn {
            DaemonClient::connect_or_spawn(
                &self.config.store_dir,
                &self.config.repo_dir,
                &self.config.model_dir,
                self.config.connect_deadline,
                &self.config.daemon_binary,
            )
            .await?
        } else {
            let socket = daemon::paths::socket_path_for_store(&self.config.store_dir)?;
            let deadline = std::time::Instant::now() + self.config.connect_deadline;
            loop {
                match DaemonClient::connect(&socket).await {
                    Ok(c) => break c,
                    Err(DaemonError::Starting) => {}
                    Err(e) if std::time::Instant::now() >= deadline => return Err(e),
                    Err(_) if std::time::Instant::now() >= deadline => {
                        return Err(DaemonError::Internal {
                            detail: format!(
                                "daemon unreachable at {} and spawning is disabled; start sift-daemon or enable spawn",
                                socket.display()
                            ),
                        });
                    }
                    Err(_) => {}
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        *guard = Some(client);
        Ok(())
    }

    async fn with_client_request(&self, req: Request) -> Result<Response, DaemonError> {
        self.ensure_client().await?;
        let deadline = std::time::Instant::now() + self.config.connect_deadline;
        loop {
            let result = {
                let mut guard = self.client.lock().await;
                let client = guard.as_mut().expect("client after ensure");
                client.request(req.clone()).await
            };
            match result {
                Err(DaemonError::Starting) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
                other => return other,
            }
        }
    }
}

fn to_tool_error(err: DaemonError) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(format_daemon_error(&err))])
}

fn format_daemon_error(err: &DaemonError) -> String {
    match err {
        DaemonError::Starting => "Daemon is still starting (loading models). Retry shortly.".into(),
        DaemonError::IndexInProgress => {
            "An index is already in progress. Retry shortly after it completes.".into()
        }
        DaemonError::SymbolNotFound { file, symbol } => {
            format!(
                "Symbol not found: `{symbol}` in `{file}`. Check the qualified name (e.g. Type::method) and that the file is indexed; call index_repository if the repo changed."
            )
        }
        DaemonError::SymbolAmbiguous {
            file,
            symbol,
            candidates,
        } => {
            format!(
                "Symbol `{symbol}` in `{file}` is ambiguous. Candidates: {}. Pass the qualified name from the list.",
                candidates.join(", ")
            )
        }
        DaemonError::StoreStale { reason } => {
            format!(
                "Store is stale ({reason}). Call index_repository to rebuild against the current repository."
            )
        }
        DaemonError::GpuUnavailable { detail } => {
            format!(
                "GPU unavailable: {detail}. The daemon needs a working CUDA device, or use a CPU/mock build for tests."
            )
        }
        DaemonError::ProtocolVersion { daemon, client } => {
            format!(
                "Protocol version mismatch (daemon={daemon}, client={client}). Upgrade sift-daemon and mcp-client together."
            )
        }
        DaemonError::RequestTooLarge { bytes, limit } => {
            format!("Request too large ({bytes} bytes; limit {limit}). Shrink the payload.")
        }
        DaemonError::Malformed { detail } => {
            format!("Malformed daemon response: {detail}.")
        }
        DaemonError::Internal { detail } => {
            format!(
                "Daemon error: {detail}. Ensure sift-daemon can start (check --store/--repo/--model) and the socket is reachable."
            )
        }
    }
}

fn param_error_result(err: params::ParamError) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(err.0)])
}

#[tool_router(router = tool_router)]
impl SiftMcpServer {
    #[tool(description = "see descriptions.toml / list_tools for the versioned agent-facing text")]
    async fn search_code(
        &self,
        Parameters(p): Parameters<SearchCodeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if let Err(e) = params::validate(&p) {
            return Ok(param_error_result(e));
        }
        let result = self
            .with_client_request(Request::Search {
                query: p.query.clone(),
                top_k: p.top_k,
            })
            .await;
        match result {
            Ok(Response::Search(resp)) => {
                let json = serde_json::to_string_pretty(&resp)
                    .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
            }
            Ok(other) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "unexpected daemon response: {other:?}"
            ))])),
            Err(e) => Ok(to_tool_error(e)),
        }
    }

    #[tool(description = "see descriptions.toml / list_tools for the versioned agent-facing text")]
    async fn find_similar_code(
        &self,
        Parameters(p): Parameters<FindSimilarCodeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if let Err(e) = params::validate(&p) {
            return Ok(param_error_result(e));
        }
        let result = self
            .with_client_request(Request::SearchSimilar {
                code: p.code.clone(),
                top_k: p.top_k,
            })
            .await;
        match result {
            Ok(Response::Search(resp)) => {
                let json = serde_json::to_string_pretty(&resp)
                    .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
            }
            Ok(other) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "unexpected daemon response: {other:?}"
            ))])),
            Err(e) => Ok(to_tool_error(e)),
        }
    }

    #[tool(description = "see descriptions.toml / list_tools for the versioned agent-facing text")]
    async fn get_symbol(
        &self,
        Parameters(p): Parameters<GetSymbolParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if let Err(e) = params::validate(&p) {
            return Ok(param_error_result(e));
        }
        let result = self
            .with_client_request(Request::GetSymbol {
                file: p.file.clone(),
                symbol: p.symbol.clone(),
            })
            .await;
        match result {
            Ok(Response::Symbol {
                file,
                symbol,
                language,
                signature,
                lines,
                body,
            }) => {
                let payload = serde_json::json!({
                    "file": file,
                    "symbol": symbol,
                    "language": language,
                    "signature": signature,
                    "lines": lines,
                    "body": body,
                });
                let json = serde_json::to_string_pretty(&payload)
                    .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
            }
            Ok(other) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "unexpected daemon response: {other:?}"
            ))])),
            Err(e) => Ok(to_tool_error(e)),
        }
    }

    #[tool(description = "see descriptions.toml / list_tools for the versioned agent-facing text")]
    async fn index_repository(
        &self,
        Parameters(p): Parameters<IndexRepositoryParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if let Err(e) = params::validate(&p) {
            return Ok(param_error_result(e));
        }
        // Path selects the repo for spawn; wire Index only carries mode.
        // When already connected, Index uses the daemon's configured repo.
        let _path = PathBuf::from(&p.path);
        let mode = if p.full {
            IndexMode::Full
        } else {
            IndexMode::Update
        };

        if let Err(e) = self.ensure_client().await {
            return Ok(to_tool_error(e));
        }

        let mut guard = self.client.lock().await;
        let client = guard.as_mut().expect("client");
        let mut stream = match client.request_streaming(Request::Index { mode }).await {
            Ok(s) => s,
            Err(e) => return Ok(to_tool_error(e)),
        };

        let progress_token = ctx
            .meta
            .get_progress_token()
            .unwrap_or_else(|| ProgressToken(NumberOrString::String("index".into())));
        let mut last_report = None;
        while let Some(frame) = stream.next().await {
            match frame {
                Response::IndexProgress { phase, done, total } => {
                    let progress = done as f64;
                    let mut param =
                        ProgressNotificationParam::new(progress_token.clone(), progress)
                            .with_message(format!("{phase:?} {done}"));
                    if let Some(t) = total {
                        param = param.with_total(t as f64);
                    }
                    if let Err(e) = ctx.peer.notify_progress(param).await {
                        return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                            "failed to notify progress: {e}"
                        ))]));
                    }
                }
                Response::IndexDone(report) => {
                    last_report = Some(report);
                }
                Response::Error(e) => return Ok(to_tool_error(e)),
                other => {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                        "unexpected index frame: {other:?}"
                    ))]));
                }
            }
        }

        match last_report {
            Some(report) => {
                let json = serde_json::to_string_pretty(&report)
                    .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
            }
            None => Ok(CallToolResult::error(vec![ContentBlock::text(
                "index stream ended without IndexDone",
            )])),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SiftMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("sift", env!("CARGO_PKG_VERSION")))
            .with_instructions("Sift code intelligence: search, similar code, get symbol, index.")
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, rmcp::ErrorData> {
        let mut tools = self.tool_router.list_all();
        for tool in &mut tools {
            let name = tool.name.to_string();
            tool.description = Some(std::borrow::Cow::Owned(tools::rendered(&name)));
        }
        Ok(rmcp::model::ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        let mut tool = self.tool_router.get(name)?.clone();
        tool.description = Some(std::borrow::Cow::Owned(tools::rendered(name)));
        Some(tool)
    }
}

/// Expose for tests / scripts that need the store path helper.
pub fn socket_path_for_store(store_dir: &Path) -> Result<PathBuf, DaemonError> {
    daemon::paths::socket_path_for_store(store_dir)
}
