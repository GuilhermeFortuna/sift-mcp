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
        // Explicit per-test socket — avoid XDG_RUNTIME_DIR (races under parallel tests).
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

        let socket = runtime.path().join("daemon.sock");
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
            socket_path: Some(self.socket.clone()),
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

fn tool_error_text(result: &rmcp::model::CallToolResult) -> String {
    use rmcp::model::ContentBlock;
    assert_eq!(result.is_error, Some(true), "{result:?}");
    match &result.content[0] {
        ContentBlock::Text(t) => t.text.clone(),
        other => panic!("expected text, got {other:?}"),
    }
}

#[tokio::test]
async fn search_code_preview_never_exceeds_bound_on_large_files() {
    let h = Harness::new();
    // One symbol whose body exceeds the preview bound (single chunk).
    let filler = "x".repeat(retrieval::PREVIEW_MAX_BYTES + 200);
    let big = format!("pub fn huge() {{\n    let s = \"{filler}\";\n}}\n");
    assert!(big.len() > retrieval::PREVIEW_MAX_BYTES);
    write_rs(h.repo.path(), "src/huge.rs", &big);
    git_commit(h.repo.path(), "huge");
    h.start_daemon().await;
    let indexed = call_tool(
        h.mcp_server(),
        "index_repository",
        json!({ "path": h.repo.path().to_string_lossy(), "full": false }),
    )
    .await;
    let _ = tool_text(&indexed);

    let search = call_tool(
        h.mcp_server(),
        "search_code",
        json!({ "query": "huge", "top_k": 5 }),
    )
    .await;
    let text = tool_text(&search);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let results = value["results"].as_array().unwrap();
    assert!(!results.is_empty(), "{text}");
    for r in results {
        let preview = r["preview"].as_str().unwrap();
        assert!(
            preview.len() <= retrieval::PREVIEW_MAX_BYTES,
            "preview len {} exceeds {}",
            preview.len(),
            retrieval::PREVIEW_MAX_BYTES
        );
        assert!(
            preview.len() < big.len(),
            "search must not return whole file"
        );
    }

    let sym = call_tool(
        h.mcp_server(),
        "get_symbol",
        json!({ "file": "src/huge.rs", "symbol": "huge" }),
    )
    .await;
    let sym_text = tool_text(&sym);
    let sym_val: serde_json::Value = serde_json::from_str(&sym_text).unwrap();
    let body = sym_val["body"].as_str().unwrap();
    assert!(body.len() > retrieval::PREVIEW_MAX_BYTES);
}

#[tokio::test]
async fn get_symbol_absent_is_actionable() {
    let h = Harness::new();
    h.start_daemon().await;

    let absent = call_tool(
        h.mcp_server(),
        "get_symbol",
        json!({ "file": "src/lib.rs", "symbol": "no_such_symbol" }),
    )
    .await;
    let absent_msg = tool_error_text(&absent);
    assert!(absent_msg.contains("not found") || absent_msg.contains("Symbol not found"));
    assert!(absent_msg.contains("src/lib.rs"));
    assert!(absent_msg.contains("no_such_symbol"));
}

#[tokio::test]
async fn cold_start_spawns_daemon_and_searches() {
    let h = Harness::new();
    let runtime = TempDir::new().unwrap();
    let mut perms = std::fs::metadata(runtime.path()).unwrap().permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(runtime.path(), perms).unwrap();
    // SAFETY: spawn path derives the socket from XDG_RUNTIME_DIR; isolate this test.
    unsafe {
        std::env::set_var("XDG_RUNTIME_DIR", runtime.path());
    }

    let bin = std::env::var_os("CARGO_BIN_EXE_sift_daemon_test")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/sift-daemon-test")
        });
    if !bin.exists() {
        let status = std::process::Command::new("cargo")
            .args(["build", "-p", "daemon", "--bin", "sift-daemon-test"])
            .status()
            .unwrap();
        assert!(status.success());
    }
    let bin = if bin.exists() {
        bin
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/sift-daemon-test")
    };
    assert!(bin.exists(), "sift-daemon missing at {}", bin.display());

    let server = SiftMcpServer::with_config(SiftMcpConfig {
        store_dir: h.store_dir.path().to_path_buf(),
        repo_dir: h.repo.path().to_path_buf(),
        model_dir: PathBuf::from("."),
        daemon_binary: bin,
        connect_deadline: Duration::from_secs(30),
        allow_spawn: true,
        socket_path: None,
    });
    let result = call_tool(
        server,
        "search_code",
        json!({ "query": "alpha", "top_k": 5 }),
    )
    .await;
    let text = tool_text(&result);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        value["results"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "{text}"
    );
}

#[tokio::test]
async fn advertised_tools_are_exactly_four_without_resources_or_prompts() {
    let h = Harness::new();
    h.start_daemon().await;
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
    let client = TestClient.serve(client_transport).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    let mut sorted: Vec<_> = tools.iter().map(|t| t.name.to_string()).collect();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![
            "find_similar_code".to_string(),
            "get_symbol".to_string(),
            "index_repository".to_string(),
            "search_code".to_string(),
        ]
    );
    for t in &tools {
        let desc = t.description.as_ref().map(|d| d.as_ref()).unwrap_or("");
        assert!(desc.contains("Prefer"), "missing prefer_over in {}", t.name);
        assert!(desc.contains("Examples:"), "missing examples in {}", t.name);
    }
    let info = client
        .peer_info()
        .expect("server peer info after handshake");
    assert!(info.capabilities.tools.is_some());
    assert!(info.capabilities.resources.is_none());
    assert!(info.capabilities.prompts.is_none());
    let _ = client.cancel().await;
}

#[tokio::test]
async fn unreachable_daemon_names_cause() {
    let h = Harness::new();
    // Do not start daemon; spawn disabled.
    let result = call_tool(
        h.mcp_server(),
        "search_code",
        json!({ "query": "alpha", "top_k": 5 }),
    )
    .await;
    let msg = tool_error_text(&result);
    assert!(
        msg.contains("unreachable")
            || msg.contains("spawning is disabled")
            || msg.contains("Daemon error")
            || msg.contains("timed out")
            || msg.contains("connect"),
        "{msg}"
    );
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
