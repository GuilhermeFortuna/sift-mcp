//! Paired console overhead measurement (collection off vs on).
//!
//! Requires the `resident` feature. Driven by `scripts/measure-console-overhead.sh`.

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use daemon::measure::{
    ConsoleMeasureArgs, ConsoleRunReport, aggregate_console_summary, nearest_rank_percentile,
    pair_order, parse_console_measure_args,
};

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_console_measure_args(&raw) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            eprintln!(
                "usage: measure_console_overhead --repo PATH --store PATH --model PATH --daemon PATH --console PATH --runs N --output DIR"
            );
            std::process::exit(2);
        }
    };
    if let Err(e) = run(args) {
        eprintln!("measurement failed: {e}");
        std::process::exit(1);
    }
}

fn run(args: ConsoleMeasureArgs) -> Result<(), String> {
    std::fs::create_dir_all(&args.output).map_err(|e| e.to_string())?;

    let queries = [
        "fn main",
        "error handling",
        "struct Config",
        "TODO",
        "async fn",
    ];

    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let mut reports = Vec::new();

    rt.block_on(async {
        // Connect or spawn daemon
        let mut daemon_client = daemon::DaemonClient::connect_or_spawn(
            &args.store,
            &args.repo,
            &args.model,
            Duration::from_secs(30),
            &args.daemon,
        )
        .await
        .map_err(|e| format!("daemon connect: {e:?}"))?;

        // Warmup daemon directly
        for q in queries {
            let _ = daemon_client
                .request(daemon::Request::Search {
                    query: (*q).into(),
                    top_k: 5,
                })
                .await;
        }

        for run_index in 0..args.runs {
            let modes = pair_order(run_index);
            for collection in modes {
                let report = measure_one_pair_element(
                    &args,
                    collection,
                    run_index,
                    &queries,
                    &mut daemon_client,
                )
                .await?;

                let path = args.output.join(format!(
                    "run-{}-collect-{}.json",
                    run_index,
                    if collection { "on" } else { "off" }
                ));
                let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
                std::fs::write(&path, json).map_err(|e| e.to_string())?;
                reports.push(report);
            }
        }

        let summary = aggregate_console_summary(reports);
        let summary_path = args.output.join("summary.json");
        std::fs::write(
            &summary_path,
            serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        println!("wrote {}", summary_path.display());
        Ok(())
    })
}

async fn measure_one_pair_element(
    args: &ConsoleMeasureArgs,
    collection: bool,
    run_index: u32,
    queries: &[&str],
    daemon_client: &mut daemon::DaemonClient,
) -> Result<ConsoleRunReport, String> {
    let port = 7450 + (run_index % 20) as u16 + if collection { 100 } else { 0 };
    let listen_addr = format!("127.0.0.1:{port}");
    let temp_db_dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let db_path = temp_db_dir.path().join("console.sqlite3");

    // Spawn console instance with --collect on/off
    let mut console_child = Command::new(&args.console)
        .arg("--listen")
        .arg(&listen_addr)
        .arg("--database")
        .arg(&db_path)
        .arg("--collect")
        .arg(if collection { "on" } else { "off" })
        .spawn()
        .map_err(|e| format!("failed to spawn console binary: {e}"))?;

    let console_pid = console_child.id();

    // Guard to ensure process cleanup
    struct ProcessGuard<'a>(&'a mut Child);
    impl Drop for ProcessGuard<'_> {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _guard = ProcessGuard(&mut console_child);

    // Wait for console to be ready
    let mut ready = false;
    let mut session_body = String::new();
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Ok((200, body)) =
            http_request(&listen_addr, "GET", "/api/v1/session", None, None).await
        {
            session_body = body;
            ready = true;
            break;
        }
    }
    if !ready {
        return Err(format!("console on {listen_addr} failed to start in time"));
    }

    // Get CSRF token
    let session_res: serde_json::Value =
        serde_json::from_str(&session_body).map_err(|e| e.to_string())?;
    let csrf_token = session_res["csrf_token"]
        .as_str()
        .ok_or("missing csrf_token")?
        .to_string();

    // Register repository
    let reg_body = serde_json::json!({
        "name": "measure-repo",
        "repo_path": args.repo,
        "store_path": args.store,
        "model_path": args.model,
        "daemon_path": args.daemon,
    })
    .to_string();

    let (status, reg_resp) = http_request(
        &listen_addr,
        "POST",
        "/api/v1/repositories",
        Some(&csrf_token),
        Some(&reg_body),
    )
    .await?;

    if status != 201 {
        return Err(format!(
            "registration failed with status {status}: {reg_resp}"
        ));
    }

    let reg_val: serde_json::Value = serde_json::from_str(&reg_resp).map_err(|e| e.to_string())?;
    let repo_id = reg_val["id"].as_str().ok_or("missing repo id")?.to_string();
    let search_path = format!("/api/v1/repositories/{repo_id}/search");

    // Warmup through console
    for q in queries {
        let payload = serde_json::json!({ "query": q, "top_k": 5 }).to_string();
        let _ = http_request(
            &listen_addr,
            "POST",
            &search_path,
            Some(&csrf_token),
            Some(&payload),
        )
        .await;
    }

    // Timed search requests through console
    let mut latencies = Vec::new();
    for q in queries.iter().cycle().take(20) {
        let payload = serde_json::json!({ "query": q, "top_k": 5 }).to_string();
        let start = Instant::now();
        let (status, resp_str) = http_request(
            &listen_addr,
            "POST",
            &search_path,
            Some(&csrf_token),
            Some(&payload),
        )
        .await?;
        if status != 200 {
            return Err(format!("search failed with status {status}: {resp_str}"));
        }
        latencies.push(start.elapsed().as_micros() as u64);
    }

    let mut sorted = latencies.clone();
    sorted.sort_unstable();
    let p50 = nearest_rank_percentile(&sorted, 50).unwrap_or(0);
    let p95 = nearest_rank_percentile(&sorted, 95).unwrap_or(0);

    // Read daemon resources
    let status = daemon_client
        .request(daemon::Request::Status)
        .await
        .map_err(|e| format!("status: {e:?}"))?;
    let (device_id, device_used, process_used) = match status {
        daemon::Response::Status(s) => (
            s.resources.device_id,
            s.resources.device_used_bytes,
            s.resources.process_used_bytes,
        ),
        _ => (None, None, None),
    };

    let daemon_rss = read_daemon_rss(&args.store);
    let console_rss = read_pid_rss(console_pid);

    Ok(ConsoleRunReport {
        collection,
        run_index,
        latencies_micros: latencies.clone(),
        p50_micros: p50,
        p95_micros: p95,
        daemon_rss_bytes: daemon_rss,
        console_rss_bytes: console_rss,
        device_id,
        device_used_bytes: device_used,
        process_used_bytes: process_used,
        sample_count: latencies.len(),
    })
}

fn read_pid_rss(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

fn read_daemon_rss(store: &std::path::Path) -> Option<u64> {
    let pid_file = store.join("daemon.pid");
    let content = std::fs::read_to_string(pid_file).ok()?;
    let pid = content.trim().parse::<u32>().ok()?;
    read_pid_rss(pid)
}

async fn http_request(
    addr: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> Result<(u16, String), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let mut stream = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("connect {addr}: {e}"))?;

    let body_str = body.unwrap_or("");
    let body_len = body_str.len();

    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {body_len}\r\n"
    );
    if let Some(t) = token {
        req.push_str(&format!("x-sift-csrf: {t}\r\n"));
    }
    if !body_str.is_empty() {
        req.push_str("Content-Type: application/json\r\n");
    }
    req.push_str("\r\n");
    req.push_str(body_str);

    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;

    let mut resp = Vec::new();
    stream
        .read_to_end(&mut resp)
        .await
        .map_err(|e| format!("read: {e}"))?;

    let resp_str = String::from_utf8_lossy(&resp);
    let mut parts = resp_str.splitn(2, "\r\n\r\n");
    let header_part = parts.next().unwrap_or("");
    let body_part = parts.next().unwrap_or("").to_string();

    let status = header_part
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);

    Ok((status, body_part))
}
