mod support;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use console::ConsoleConfig;
use daemon::{IndexReportWire, Response};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("host", "127.0.0.1:7442")
                .header("origin", "http://127.0.0.1:7442")
                .header("content-type", "application/json")
                .header("x-sift-csrf", token)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_000_000).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn setup() -> (tempfile::TempDir, Router, String, Value) {
    let t = tempfile::tempdir().unwrap();
    for name in ["assets", "repo", "model"] {
        std::fs::create_dir(t.path().join(name)).unwrap();
    }
    std::fs::write(t.path().join("assets/index.html"), "<main>Console</main>").unwrap();
    let daemon = t.path().join("daemon");
    std::fs::write(&daemon, "#!/bin/sh\nexit 1\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = ConsoleConfig {
        listen: "127.0.0.1:7442".parse().unwrap(),
        database_path: t.path().join("state/console.sqlite3"),
        asset_path: t.path().join("assets"),
    };
    let app = console::api::application(config).await.unwrap();
    let (_, session) = request(&app, "GET", "/api/v1/session", "", Value::Null).await;
    let token = session["csrf_token"].as_str().unwrap().to_owned();
    let input = json!({"name":"first","repo_path":t.path().join("repo"),"store_path":t.path().join("store"),"model_path":t.path().join("model"),"daemon_path":daemon});
    let (status, registration) = request(&app, "POST", "/api/v1/repositories", &token, input).await;
    assert_eq!(status, StatusCode::CREATED, "{registration}");
    (t, app, token, registration)
}
fn report() -> IndexReportWire {
    serde_json::from_value(json!({"commit":"abc","files_seen":1,"files_indexed":1,"files_excluded":0,"files_unsupported":0,"files_unparsed":0,"chunks_added":2,"chunks_reused":0,"chunks_removed":0,"embeddings_computed":2,"chunks_truncated":0,"parse_millis":1,"embed_millis":2,"store_millis":1,"wall_millis":4,"live_before":0,"live_after":2})).unwrap()
}
#[tokio::test]
async fn indexing_survives_http_disconnect_and_refuses_duplicate_and_edits() {
    let (_t, app, token, r) = setup().await;
    let id = r["id"].as_str().unwrap();
    let socket = daemon::paths::socket_path_for_store(std::path::Path::new(
        r["config"]["store_path"].as_str().unwrap(),
    ))
    .unwrap();
    let mock = support::MockDaemon::bind(socket, Response::IndexDone(report())).await;
    let (status, job) = request(
        &app,
        "POST",
        &format!("/api/v1/repositories/{id}/index"),
        &token,
        json!({"mode":"update"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{job}");
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        mock.index_started.notified(),
    )
    .await
    .unwrap();
    for (method, suffix, body) in [
        ("POST", "/index", json!({"mode":"full"})),
        ("PATCH", "", r["config"].clone()),
        ("DELETE", "", json!({})),
    ] {
        assert_eq!(
            request(
                &app,
                method,
                &format!("/api/v1/repositories/{id}{suffix}"),
                &token,
                body
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
    }
    // The initial HTTP response has already been dropped. The service still owns the index stream.
    mock.finish_index.notify_one();
    let path = format!("/api/v1/jobs/{}", job["id"].as_str().unwrap());
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let (_, j) = request(&app, "GET", &path, &token, Value::Null).await;
            if j["state"] != "running" {
                assert_eq!(j["state"], "succeeded", "{j}");
                assert_eq!(j["report"], serde_json::to_value(report()).unwrap());
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn search_and_similar_preserve_daemon_records_and_reject_operation_paths() {
    let (_t, app, token, r) = setup().await;
    let id = r["id"].as_str().unwrap();
    let socket = daemon::paths::socket_path_for_store(std::path::Path::new(
        r["config"]["store_path"].as_str().unwrap(),
    ))
    .unwrap();
    let wire = json!({"results":[{"file":"b.rs","symbol":"b","signature":"fn b()","doc":null,"preview":"PRIVATE CODE","lines":[2,3],"lexical_score":null,"dense_score":0.5,"fused_score":0.125},{"file":"a.rs","symbol":"a","signature":"fn a()","doc":null,"preview":"a","lines":[1,1],"lexical_score":1.0,"dense_score":null,"fused_score":0.0625}],"diagnostics":{"lexical_ok":true,"dense_ok":true,"lexical_error":null,"dense_error":null,"stage_millis":{"embed":1,"lexical":2,"dense":3,"fuse":4,"assemble":5,"total":15}}});
    let response = serde_json::from_value(json!({"Search":wire})).unwrap();
    let mock = support::MockDaemon::bind(socket, response).await;
    for (action, body) in [
        ("search", json!({"query":"PRIVATE QUERY","top_k":2})),
        ("similar", json!({"code":"PRIVATE CODE","top_k":2})),
    ] {
        let (status, out) = request(
            &app,
            "POST",
            &format!("/api/v1/repositories/{id}/{action}"),
            &token,
            body,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{out}");
        assert_eq!(out, wire);
    }
    let before = mock.requests.lock().unwrap().len();
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/api/v1/repositories/{id}/index"),
            &token,
            json!({"mode":"full","repo_dir":"/arbitrary"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(mock.requests.lock().unwrap().len(), before);
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/repositories/missing/search",
            &token,
            json!({"query":"x"})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn protocol_mismatch_never_replaces_daemon() {
    let (t, app, token, r) = setup().await;
    let id = r["id"].as_str().unwrap();
    let sentinel = t.path().join("spawned");
    std::fs::write(
        t.path().join("daemon"),
        format!("#!/bin/sh\ntouch '{}'\n", sentinel.display()),
    )
    .unwrap();
    let socket = daemon::paths::socket_path_for_store(std::path::Path::new(
        r["config"]["store_path"].as_str().unwrap(),
    ))
    .unwrap();
    let _mock = support::MockDaemon::bind(
        socket,
        Response::Error(daemon::DaemonError::ProtocolVersion {
            daemon: 1,
            client: 2,
        }),
    )
    .await;
    let (status, error) = request(
        &app,
        "POST",
        &format!("/api/v1/repositories/{id}/start"),
        &token,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["code"], "protocol_incompatible");
    assert!(!sentinel.exists());
}
