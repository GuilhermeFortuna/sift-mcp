use std::path::PathBuf;
use std::process::ExitCode;

use mcp_client::tools::{descriptions, rendered};
use mcp_client::{SiftMcpConfig, SiftMcpServer};

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--print-tool-descriptions") {
        println!("DESCRIPTIONS_VERSION={}", mcp_client::tools::DESCRIPTIONS_VERSION);
        for d in descriptions() {
            println!("=== {} ===", d.name);
            print!("{}", rendered(d.name));
            println!();
        }
        return ExitCode::SUCCESS;
    }

    let mut store = None;
    let mut repo = None;
    let mut model = None;
    let mut binary = None;
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
            "--daemon-binary" => {
                i += 1;
                binary = Some(PathBuf::from(&args[i]));
            }
            "-h" | "--help" => {
                eprintln!(
                    "usage: mcp-client [--store DIR] [--repo DIR] [--model DIR] [--daemon-binary PATH]\n       mcp-client --print-tool-descriptions"
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown arg: {other}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }

    let store_dir = store.unwrap_or_else(|| PathBuf::from(".sift-store"));
    let repo_dir = repo.unwrap_or_else(|| PathBuf::from("."));
    let model_dir = model.unwrap_or_else(|| PathBuf::from("."));
    let daemon_binary = binary.unwrap_or_else(|| PathBuf::from("sift-daemon"));

    // Stdout is the MCP protocol — keep logs on stderr only.
    let server = SiftMcpServer::with_config(SiftMcpConfig {
        store_dir,
        repo_dir,
        model_dir,
        daemon_binary,
        connect_deadline: std::time::Duration::from_secs(60),
        allow_spawn: true,
        socket_path: None,
    });
    if let Err(e) = server.serve_stdio().await {
        eprintln!("mcp-client serve error: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
