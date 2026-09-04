use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use daemon::{Daemon, DaemonConfig};
use inference::OnnxEmbedder;
use retrieval::FusionConfig;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("daemon=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    let mut store = None;
    let mut repo = None;
    let mut model = None;
    let mut socket = None;
    let mut idle_secs: u64 = 15 * 60;
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
            "--model" => {
                i += 1;
                model = Some(PathBuf::from(&args[i]));
            }
            "--socket" => {
                i += 1;
                socket = Some(PathBuf::from(&args[i]));
            }
            "--idle-secs" => {
                i += 1;
                idle_secs = args[i].parse().unwrap_or(idle_secs);
            }
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let store_dir = store.expect("--store required");
    let repo_dir = repo.expect("--repo required");
    let model_dir = model.unwrap_or_else(|| PathBuf::from("."));
    let socket_path = match socket {
        Some(p) => p,
        None => daemon::paths::socket_path_for_store(&store_dir).expect("socket path"),
    };

    let embedder = match OnnxEmbedder::load(&model_dir, 32) {
        Ok(embedder) => Arc::new(embedder) as Arc<dyn inference::Embedder>,
        Err(error) => {
            eprintln!("load CUDA embedder failed: {error}");
            std::process::exit(1);
        }
    };

    let config = DaemonConfig {
        store_dir,
        model_dir,
        repo_dir,
        socket_path,
        idle_timeout: Duration::from_secs(idle_secs),
        max_concurrent_searches: 4,
        fusion: FusionConfig::default(),
    };

    let daemon = match Daemon::bind(config, embedder).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("bind failed: {e:?}");
            std::process::exit(1);
        }
    };
    if let Err(e) = daemon.serve().await {
        eprintln!("serve failed: {e:?}");
        std::process::exit(1);
    }
}
