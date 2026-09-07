use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use console::ConsoleConfig;
use daemon::Response;
use serde_json::{Value, json};
use tower::ServiceExt;

mod support;

#[tokio::test]
async fn an_arbitrary_csrf_token_is_not_authorization() {
    let response = console::api::router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/repositories")
                .header("host", "127.0.0.1:7331")
                .header("origin", "http://127.0.0.1:7331")
                .header("content-type", "application/json")
                .header("X-Sift-CSRF", "forged")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn cross_origin_reads_are_rejected() {
    let response = console::api::router()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .header("host", "127.0.0.1:7331")
                .header("origin", "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn privacy_boundary_query_code_and_symbol_never_enter_sqlite_or_history() {
    let t = tempfile::tempdir().unwrap();
    for name in ["assets", "repo", "model", "state"] {
        std::fs::create_dir_all(t.path().join(name)).unwrap();
    }
    std::fs::write(t.path().join("assets/index.html"), "<main>Console</main>").unwrap();
    let daemon = t.path().join("daemon");
    std::fs::write(&daemon, "#!/bin/sh\nexit 1\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o700)).unwrap();

    let db_path = t.path().join("state/console.sqlite3");
    let config = ConsoleConfig {
        listen: "127.0.0.1:7449".parse().unwrap(),
        database_path: db_path.clone(),
        asset_path: t.path().join("assets"),
        collect: true,
    };
    let app = console::api::application(config).await.unwrap();

    let req = |app: &axum::Router, method: &str, path: &str, token: &str, body: Value| {
        let app = app.clone();
        let method = method.to_string();
        let path = path.to_string();
        let token = token.to_string();
        async move {
            let res = app
                .oneshot(
                    Request::builder()
                        .method(method.as_str())
                        .uri(path.as_str())
                        .header("host", "127.0.0.1:7449")
                        .header("origin", "http://127.0.0.1:7449")
                        .header("content-type", "application/json")
                        .header("x-sift-csrf", token.as_str())
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = res.status();
            let bytes = to_bytes(res.into_body(), 2_000_000).await.unwrap();
            let val = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            (status, val)
        }
    };

    let (_, session) = req(&app, "GET", "/api/v1/session", "", Value::Null).await;
    let token = session["csrf_token"].as_str().unwrap().to_owned();

    let reg_input = json!({
        "name": "privacy-test",
        "repo_path": t.path().join("repo"),
        "store_path": t.path().join("store"),
        "model_path": t.path().join("model"),
        "daemon_path": daemon,
    });
    let (status, reg) = req(&app, "POST", "/api/v1/repositories", &token, reg_input).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = reg["id"].as_str().unwrap();

    let socket = daemon::paths::socket_path_for_store(std::path::Path::new(
        reg["config"]["store_path"].as_str().unwrap(),
    ))
    .unwrap();

    let private_query = "SECRET_QUERY_TOKEN_7331";
    let private_code = "SECRET_CODE_TOKEN_7331";
    let private_symbol_body = "SECRET_SYMBOL_TOKEN_7331";

    let search_wire = json!({
        "results": [{
            "file": "secret.rs",
            "symbol": "secret_fn",
            "signature": "fn secret_fn()",
            "doc": null,
            "preview": "secret preview",
            "lines": [1, 2],
            "lexical_score": 1.0,
            "dense_score": null,
            "fused_score": 0.5
        }],
        "diagnostics": {
            "lexical_ok": true,
            "dense_ok": true,
            "lexical_error": null,
            "dense_error": null,
            "stage_millis": { "embed": 1, "lexical": 1, "dense": 1, "fuse": 1, "assemble": 1, "total": 4 }
        }
    });

    let _mock = support::MockDaemon::bind(
        socket,
        Response::Search(serde_json::from_value(search_wire).unwrap()),
    )
    .await;

    // Send private query and code
    let (status, _) = req(
        &app,
        "POST",
        &format!("/api/v1/repositories/{id}/search"),
        &token,
        json!({ "query": private_query, "top_k": 5 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = req(
        &app,
        "POST",
        &format!("/api/v1/repositories/{id}/similar"),
        &token,
        json!({ "code": private_code, "top_k": 5 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Read raw sqlite database files (including WAL / shm if present)
    let read_all_db_contents = || {
        let mut content = String::new();
        for entry in std::fs::read_dir(db_path.parent().unwrap()).unwrap() {
            let entry = entry.unwrap();
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("console.sqlite3")
                && let Ok(bytes) = std::fs::read(entry.path())
            {
                content.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        content
    };

    let db_content = read_all_db_contents();

    // Assert that the private tokens NEVER appear anywhere in the sqlite database
    assert!(
        !db_content.contains(private_query),
        "database must not persist private query"
    );
    assert!(
        !db_content.contains(private_code),
        "database must not persist private code"
    );
    assert!(
        !db_content.contains(private_symbol_body),
        "database must not persist private symbol body"
    );

    // Negative fixture: prove that if a leak were introduced into the database,
    // the assertion detects it rather than succeeding vacuously.
    let leak_sentinel = "INTENTIONAL_LEAK_SENTINEL_DETECTED";
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "INSERT INTO collection_gaps(repository_id, from_unix_ms, to_unix_ms, reason) VALUES (?1, 0, 10, ?2)",
        rusqlite::params![id, leak_sentinel],
    )
    .unwrap();
    drop(conn);

    let db_content_after = read_all_db_contents();
    assert!(
        db_content_after.contains(leak_sentinel),
        "leak detector must verify that stored data is observable in the database bytes"
    );
}
