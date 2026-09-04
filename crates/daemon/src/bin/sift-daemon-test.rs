//! CPU-only daemon fixture used by integration tests.
//!
//! The production `sift-daemon` binary is CUDA-only. This separate target
//! keeps the protocol and cold-start tests runnable on CPU-only CI without
//! making a CPU embedder an accidental production fallback.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use daemon::{Daemon, DaemonConfig};
use inference::MockEmbedder;
use retrieval::FusionConfig;

#[tokio::main]
async fn main() {
    let mut store = None;
    let mut repo = None;
    let mut socket = None;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--store" => {
                i += 1;
                store = Some(PathBuf::from(&args[i]));
            }
            "--repo" => {
                i += 1;
                repo = Some(PathBuf::from(&args[i]));
            }
            "--socket" => {
                i += 1;
                socket = Some(PathBuf::from(&args[i]));
            }
            "--model" => i += 1,
            "--idle-secs" => i += 1,
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let store_dir = store.expect("--store required");
    let repo_dir = repo.expect("--repo required");
    let socket_path = socket
        .unwrap_or_else(|| daemon::paths::socket_path_for_store(&store_dir).expect("socket path"));
    let embedder = Arc::new(MockEmbedder::new(8)) as Arc<dyn inference::Embedder>;
    let config = DaemonConfig {
        store_dir,
        model_dir: PathBuf::from("."),
        repo_dir,
        socket_path,
        idle_timeout: Duration::from_secs(15 * 60),
        max_concurrent_searches: 4,
        fusion: FusionConfig::default(),
    };
    let daemon = Daemon::bind(config, embedder).await.unwrap_or_else(|e| {
        eprintln!("bind failed: {e:?}");
        std::process::exit(1);
    });
    if let Err(e) = daemon.serve().await {
        eprintln!("serve failed: {e:?}");
        std::process::exit(1);
    }
}
