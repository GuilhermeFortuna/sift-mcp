mod support;

use console::collector::ObserverPool;
use daemon::EventCursor;

#[tokio::test]
async fn repeated_observations_reuse_one_observer_connection() {
    let socket = std::env::current_dir()
        .unwrap()
        .join("target")
        .join(format!("observer-pool-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let mock = support::MockDaemon::bind(
        socket.clone(),
        daemon::Response::Observation(support::observation("mock", 0)),
    )
    .await;
    let mut pool = ObserverPool::new();

    pool.observe("repo", &socket, None).await.unwrap();
    pool.observe(
        "repo",
        &socket,
        Some(EventCursor {
            instance_id: "mock".into(),
            sequence: 0,
        }),
    )
    .await
    .unwrap();

    let requests = mock.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|request| matches!(request, daemon::Request::Hello { .. }))
            .count(),
        1
    );
}
