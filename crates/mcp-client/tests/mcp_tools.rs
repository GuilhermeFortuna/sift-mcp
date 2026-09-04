//! MCP tools over the real stdio/duplex transport against a MockEmbedder daemon.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use daemon::DaemonClient;
use daemon::protocol::{DaemonError, Request, Response};
use daemon::server::{Daemon, DaemonConfig};
use indexing::{IndexConfig, Indexer, NullProgress};
use inference::{Embedder, MockEmbedder};
use mcp_client::{SiftMcpConfig, SiftMcpServer};
use retrieval::FusionConfig;
use rmcp::model::CallToolRequestParams;
use rmcp::{ClientHandler, ServiceExt};
use serde_json::json;
use storage::ChunkStore;
use tempfile::TempDir;

const DIMS: u32 = 8;

struct Harness {
    _runtime: TempDir,
    store_dir: TempDir,
    repo: TempDir,
    embedder: Arc<MockEmbedder>,
    socket: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let runtime = TempDir::new().unwrap();
        let mut perms = std::fs::metadata(runtime.path()).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(runtime.path(), perms).unwrap();
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", runtime.path());
        }
        let store_dir = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        init_git_repo(repo.path());
        write_rs(repo.path(), "src/lib.rs", "pub fn alpha() { let x = 1; }\n");
        git_commit(repo.path(), "init");

        let embedder = Arc::new(MockEmbedder::new(DIMS).with_batch_limit(4));
        let store = ChunkStore::create(store_dir.path(), DIMS, embedder.model_id()).unwrap();
        let mut indexer = Indexer::open(
            store,
            embedder.as_ref(),
            repo.path(),
            IndexConfig::default(),
        )
        .unwrap();
        indexer.index_all(&mut NullProgress).unwrap();
        let (store, lexical) = indexer.into_parts();
        drop(lexical);
        drop(store);

        let socket = daemon::paths::socket_path_for_store(store_dir.path()).unwrap();
        Self {
            _runtime: runtime,
            store_dir,
            repo,
            embedder,
            socket,
        }
    }

    fn config(&self) -> DaemonConfig {
        DaemonConfig {
            store_dir: self.store_dir.path().to_path_buf(),
            model_dir: PathBuf::from("."),
            repo_dir: self.repo.path().to_path_buf(),
            socket_path: self.socket.clone(),
            idle_timeout: Duration::from_secs(60),
            max_concurrent_searches: 4,
            fusion: FusionConfig::default(),
        }
    }

    async fn start_daemon(&self) {
        let d = Daemon::bind(
            self.config(),
            Arc::clone(&self.embedder) as Arc<dyn Embedder>,
        )
        .await
        .unwrap();
        tokio::spawn(async move {
            let _ = d.serve().await;
        });
        wait_ready(&self.socket).await;
    }

    fn mcp_server(&self) -> SiftMcpServer {
        SiftMcpServer::with_config(SiftMcpConfig {
            store_dir: self.store_dir.path().to_path_buf(),
            repo_dir: self.repo.path().to_path_buf(),
            model_dir: PathBuf::from("."),
            daemon_binary: PathBuf::from("sift-daemon"),
            connect_deadline: Duration::from_secs(10),
            allow_spawn: false,
        })
    }
}

fn init_git_repo(path: &Path) {
    std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "t@example.com"])
        .current_dir(path)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "t"])
        .current_dir(path)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "commit.gpgsign", "false"])
        .current_dir(path)
        .status()
        .unwrap();
}

fn write_rs(repo: &Path, rel: &str, body: &str) {
    let full = repo.join(rel);
    if let Some(p) = full.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(full, body).unwrap();
}

fn git_commit(repo: &Path, msg: &str) {
    std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", msg])
        .current_dir(repo)
        .status()
        .unwrap();
}

async fn wait_ready(socket: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(mut c) = DaemonClient::connect(socket).await {
            match c.request(Request::Status).await {
                Ok(Response::Status(s)) if !s.model_id.is_empty() && !s.indexing => return,
                Err(DaemonError::Starting) => {}
                _ => {}
            }
        }
        if Instant::now() > deadline {
            panic!("daemon not ready");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[derive(Clone, Default)]
struct TestClient;

impl ClientHandler for TestClient {}

async fn call_tool(
    server: SiftMcpServer,
    name: &str,
    arguments: serde_json::Value,
) -> rmcp::model::CallToolResult {
    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let _ = server
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await;
    });
    let client = TestClient.serve(client_transport).await.unwrap();
    let mut params = CallToolRequestParams::new(name.to_owned());
    params.arguments = arguments.as_object().cloned();
    let result = client.call_tool(params).await.unwrap();
    let _ = client.cancel().await;
    result
}

fn tool_text(result: &rmcp::model::CallToolResult) -> String {
    use rmcp::model::ContentBlock;
    assert_eq!(result.is_error, Some(false), "{result:?}");
    match &result.content[0] {
        ContentBlock::Text(t) => t.text.clone(),
        other => panic!("expected text, got {other:?}"),
    }
}

#[tokio::test]
async fn search_code_stdio_snapshot() {
    let h = Harness::new();
    h.start_daemon().await;
    let result = call_tool(
        h.mcp_server(),
        "search_code",
        json!({ "query": "alpha", "top_k": 5 }),
    )
    .await;
    let text = tool_text(&result);
    // Stabilize non-deterministic timing fields before snapshot.
    let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
    if let Some(obj) = value
        .get_mut("diagnostics")
        .and_then(|d| d.get_mut("stage_millis"))
        .and_then(|s| s.as_object_mut())
    {
        for (_k, v) in obj.iter_mut() {
            *v = json!(0);
        }
    }
    insta::assert_json_snapshot!(value);
}

#[tokio::test]
async fn find_similar_code_stdio_snapshot() {
    let h = Harness::new();
    h.start_daemon().await;
    let result = call_tool(
        h.mcp_server(),
        "find_similar_code",
        json!({ "code": "pub fn alpha() { let x = 1; }", "top_k": 5 }),
    )
    .await;
    let text = tool_text(&result);
    let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
    if let Some(obj) = value
        .get_mut("diagnostics")
        .and_then(|d| d.get_mut("stage_millis"))
        .and_then(|s| s.as_object_mut())
    {
        for (_k, v) in obj.iter_mut() {
            *v = json!(0);
        }
    }
    insta::assert_json_snapshot!(value);
}

#[tokio::test]
async fn get_symbol_stdio_snapshot() {
    let h = Harness::new();
    h.start_daemon().await;
    let result = call_tool(
        h.mcp_server(),
        "get_symbol",
        json!({ "file": "src/lib.rs", "symbol": "alpha" }),
    )
    .await;
    let text = tool_text(&result);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    insta::assert_json_snapshot!(value);
}

#[tokio::test]
async fn index_repository_streams_progress_then_summary() {
    let h = Harness::new();
    h.start_daemon().await;

    // Force index work so progress frames are produced.
    write_rs(
        h.repo.path(),
        "src/beta.rs",
        "pub fn beta() { let y = 2; }\n",
    );
    git_commit(h.repo.path(), "add beta");

    let server = h.mcp_server();
    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let _ = server
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await;
    });

    let progress = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let client_handler = ProgressClient {
        progress: Arc::clone(&progress),
    };
    let client = client_handler.serve(client_transport).await.unwrap();
    let mut params = CallToolRequestParams::new("index_repository");
    let token =
        rmcp::model::ProgressToken(rmcp::model::NumberOrString::String("test-index".into()));
    let mut meta = rmcp::model::RequestMetaObject::new();
    meta.set_progress_token(token);
    params.meta = Some(meta);
    params.arguments = json!({
        "path": h.repo.path().to_string_lossy(),
        "full": false
    })
    .as_object()
    .cloned();
    let result = client.call_tool(params).await.unwrap();
    let _ = client.cancel().await;
    let text = tool_text(&result);
    let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
    if let Some(obj) = value.as_object_mut() {
        for key in [
            "commit",
            "parse_millis",
            "embed_millis",
            "store_millis",
            "wall_millis",
        ] {
            if let Some(v) = obj.get_mut(key) {
                if v.is_string() {
                    *v = json!("<redacted>");
                } else {
                    *v = json!(0);
                }
            }
        }
    }
    let msgs = progress.lock().unwrap().clone();
    assert!(
        !msgs.is_empty(),
        "expected MCP progress notifications before summary, got none; result={text}"
    );
    insta::assert_json_snapshot!(value);
}

#[derive(Clone)]
struct ProgressClient {
    progress: Arc<std::sync::Mutex<Vec<String>>>,
}

impl ClientHandler for ProgressClient {
    async fn on_progress(
        &self,
        params: rmcp::model::ProgressNotificationParam,
        _context: rmcp::service::NotificationContext<rmcp::RoleClient>,
    ) {
        let msg = params.message.unwrap_or_default();
        self.progress.lock().unwrap().push(msg);
    }
}
