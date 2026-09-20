//! Git Smart HTTP edge paths and the durable CI-trigger outbox.
//!
//! Complements `git_http_protocol.rs`: where that suite drives the happy
//! push/clone paths with the real `git` client, this suite exercises the
//! handler error branches (unknown repository, repository row without
//! storage, no database, oversized bodies), the legacy and path-suffixed
//! routes, and the full push → `events` outbox → CI trigger delivery
//! pipeline against a scripted HTTP receiver — including lease reclaim of
//! a stale `delivering` row and the requeue-on-failure path.

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gitforge_common::{RepoId, UserId};

mod common;

use common::{free_port, run_git};

/// Environment for a spawned git-server instance.
struct TestServer {
    child: tokio::process::Child,
    http_port: u16,
    repo_id: RepoId,
    db_path: std::path::PathBuf,
    base: std::path::PathBuf,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// A CI trigger request captured by the scripted receiver.
#[derive(Debug, Clone)]
struct RecordedTrigger {
    authorization: Option<String>,
    body: serde_json::Value,
}

/// Spawn the real git-server binary with a seeded `testowner/proto`
/// repository plus any extra environment.
async fn spawn_seeded_server(extra_env: &[(&str, &str)]) -> TestServer {
    let unique = uuid::Uuid::new_v4();
    let base = std::env::temp_dir().join(format!("git-server-edges-{unique}"));
    let git_root = base.join("git");
    let db_path = base.join("gitforge.db");
    std::fs::create_dir_all(&git_root).expect("create git root");

    let (repo_id, db_url) = seed_database(&db_path, &git_root).await;

    let http_port = free_port();
    let ssh_port = free_port();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_git-server"));
    command
        .env("HTTP_PORT", http_port.to_string())
        .env("SSH_PORT", ssh_port.to_string())
        .env("GIT_ROOT", &git_root)
        .env("DATABASE_URL", &db_url)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let child = command.spawn().expect("spawn git-server binary");
    let child = wait_healthy(child, http_port).await;

    TestServer {
        child,
        http_port,
        repo_id,
        db_path,
        base,
    }
}

/// Spawn the real git-server binary without `DATABASE_URL`, which must
/// degrade to 503 on every git route instead of serving unverified repos.
async fn spawn_dbless_server() -> TestServer {
    let unique = uuid::Uuid::new_v4();
    let base = std::env::temp_dir().join(format!("git-server-dbless-{unique}"));
    let git_root = base.join("git");
    std::fs::create_dir_all(&git_root).expect("create git root");

    let http_port = free_port();
    let ssh_port = free_port();
    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_git-server"))
        .env("HTTP_PORT", http_port.to_string())
        .env("SSH_PORT", ssh_port.to_string())
        .env("GIT_ROOT", &git_root)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn git-server binary");
    let child = wait_healthy(child, http_port).await;

    TestServer {
        child,
        http_port,
        repo_id: RepoId::new(),
        db_path: base.join("none.db"),
        base,
    }
}

/// Seed a database with `testowner/proto` and create its bare repository
/// under the git root, exactly as the storage layout expects.
async fn seed_database(db_path: &std::path::Path, git_root: &std::path::Path) -> (RepoId, String) {
    // A bare path (no scheme) makes Pool::new create the file via ?mode=rwc.
    let pool = gitforge_db::Pool::new(&db_path.display().to_string())
        .await
        .expect("create sqlite pool");
    pool.migrate().await.expect("run migrations");
    let user = gitforge_db::models::User::new(
        "testowner".to_string(),
        "testowner@example.com".to_string(),
        "hash".to_string(),
    );
    let user_id: UserId = user.id;
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .expect("create user");
    let repository =
        gitforge_db::models::Repository::new("proto".to_string(), user_id, "git".to_string());
    let repo_id = repository.id;
    gitforge_db::queries::RepoQueries::create(&pool, &repository)
        .await
        .expect("create repository");
    drop(pool);

    let bare_repo = git_root.join(repo_id.to_string());
    run_git(
        &[
            "init",
            "--bare",
            "--initial-branch=main",
            bare_repo.to_str().unwrap(),
        ],
        git_root,
        &[],
    );
    (repo_id, db_path.display().to_string())
}

async fn wait_healthy(mut child: tokio::process::Child, http_port: u16) -> tokio::process::Child {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let health_url = format!("http://127.0.0.1:{http_port}/health");
    loop {
        if tokio::time::Instant::now() >= deadline {
            let _ = child.start_kill();
            panic!("git-server did not become healthy at {health_url}");
        }
        if let Ok(response) = client.get(&health_url).send().await {
            if response.status().is_success() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    child
}

/// A scripted CI-trigger receiver: records every request (auth header +
/// parsed JSON body) and answers with a fixed status. Built on the same
/// axum stack the server itself uses, so HTTP framing is never the
/// variable under test.
async fn spawn_trigger_receiver(
    status: u16,
) -> (
    String,
    Arc<Mutex<Vec<RecordedTrigger>>>,
    tokio::task::JoinHandle<()>,
) {
    use axum::{extract::State, http::HeaderMap, response::IntoResponse, routing::post};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind trigger receiver");
    let port = listener.local_addr().unwrap().port();
    let recorded: Arc<Mutex<Vec<RecordedTrigger>>> = Arc::new(Mutex::new(Vec::new()));

    #[derive(Clone)]
    struct ReceiverState {
        status: u16,
        recorded: Arc<Mutex<Vec<RecordedTrigger>>>,
    }
    let state = ReceiverState {
        status,
        recorded: recorded.clone(),
    };

    async fn handle(
        State(state): State<ReceiverState>,
        headers: HeaderMap,
        body: String,
    ) -> impl IntoResponse {
        let authorization = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok().map(str::to_string));
        let parsed = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
        state.recorded.lock().unwrap().push(RecordedTrigger {
            authorization,
            body: parsed,
        });
        (
            axum::http::StatusCode::from_u16(state.status).unwrap(),
            String::new(),
        )
    }

    let app = axum::Router::new()
        .route("/internal/ci/trigger", post(handle))
        .with_state(state);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (
        format!("http://127.0.0.1:{port}/internal/ci/trigger"),
        recorded,
        handle,
    )
}

/// A commit pushed through the real `git` client; returns its sha.
async fn push_a_commit(server: &TestServer, name: &str) -> String {
    let origin_url = format!("http://127.0.0.1:{}/testowner/proto", server.http_port);
    let work = server.base.join(format!("work-{name}"));
    std::fs::create_dir_all(&work).expect("create work dir");
    run_git(&["init", "--initial-branch=main"], &work, &[]);
    run_git(&["config", "user.email", "dev@example.com"], &work, &[]);
    run_git(&["config", "user.name", "Edge Test"], &work, &[]);
    std::fs::write(work.join(format!("{name}.txt")), format!("{name}\n")).expect("write file");
    run_git(&["add", "."], &work, &[]);
    run_git(&["commit", "-m", name], &work, &[]);
    // The server delivers the CI trigger inline before answering the push,
    // so bound the push: a receiver stall must fail the test, not hang it.
    let push = tokio::task::spawn_blocking({
        let origin_url = origin_url.clone();
        let push_work = work.clone();
        move || run_git(&["push", &origin_url, "main"], &push_work, &[])
    });
    tokio::time::timeout(Duration::from_secs(60), push)
        .await
        .expect("git push must finish within 60s")
        .expect("push task join");
    let head = run_git(&["rev-parse", "HEAD"], &work, &[]);
    String::from_utf8_lossy(&head.stdout).trim().to_string()
}

async fn event_rows(db_path: &std::path::Path) -> Vec<(String, String, i64)> {
    let pool = gitforge_db::Pool::new(&db_path.display().to_string())
        .await
        .expect("open db for assertions");
    let rows: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT id, event_type, delivery_attempts FROM events")
            .fetch_all(pool.pool())
            .await
            .expect("query events");
    rows
}

/// A legacy route, a path-suffixed route, and both Smart HTTP ref
/// advertisements are served; malformed packs are rejected with 500.
#[tokio::test]
async fn test_routes_advertisements_and_malformed_packs() {
    let mut server = spawn_seeded_server(&[]).await;
    let client = reqwest::Client::new();
    let root = format!("http://127.0.0.1:{}", server.http_port);

    // Legacy explicit upload-pack route still serves the advertisement.
    let response = client
        .get(format!("{root}/git-upload-pack/testowner/proto"))
        .send()
        .await
        .expect("legacy upload-pack");
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-type"],
        "application/x-git-upload-pack-advertisement"
    );
    assert!(!response.bytes().await.unwrap().is_empty());

    // The path-suffixed variant delegates to the same handler.
    let response = client
        .get(format!("{root}/git-upload-pack/testowner/proto/extra/path"))
        .send()
        .await
        .expect("path upload-pack");
    assert_eq!(response.status(), 200);

    // Standard info/refs advertises both services.
    let response = client
        .get(format!(
            "{root}/testowner/proto/info/refs?service=git-receive-pack"
        ))
        .send()
        .await
        .expect("receive-pack advertisement");
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-type"],
        "application/x-git-receive-pack-advertisement"
    );

    let response = client
        .get(format!("{root}/testowner/proto/info/refs"))
        .send()
        .await
        .expect("default advertisement");
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-type"],
        "application/x-git-upload-pack-advertisement"
    );

    // Malformed pack bodies reach the real upload/receive-pack children and
    // come back as 500s rather than panics.
    for path in [
        "/testowner/proto/git-upload-pack",
        "/testowner/proto/git-receive-pack",
        "/git-receive-pack/testowner/proto/extra/path",
    ] {
        let response = client
            .post(format!("{root}{path}"))
            .body(b"this is not pkt-line data".to_vec())
            .send()
            .await
            .expect("malformed pack post");
        assert_eq!(
            response.status(),
            500,
            "malformed pack on {path} must be a 500"
        );
    }

    common::shutdown_gracefully(&mut server.child).await;
}

#[tokio::test]
async fn test_unknown_and_unbacked_repositories_are_not_served() {
    let mut server = spawn_seeded_server(&[]).await;
    let client = reqwest::Client::new();
    let root = format!("http://127.0.0.1:{}", server.http_port);

    // A repository row that does not exist at all.
    for path in [
        "/testowner/does-not-exist/info/refs?service=git-receive-pack",
        "/testowner/does-not-exist/info/refs",
    ] {
        let response = client
            .get(format!("{root}{path}"))
            .send()
            .await
            .expect("unknown repo info/refs");
        assert_eq!(response.status(), 404, "{path}");
    }
    for path in [
        "/testowner/does-not-exist/git-upload-pack",
        "/testowner/does-not-exist/git-receive-pack",
    ] {
        let response = client
            .post(format!("{root}{path}"))
            .body(b"0000".to_vec())
            .send()
            .await
            .expect("unknown repo post");
        assert_eq!(response.status(), 404, "{path}");
    }

    // A repository row exists in the database but no bare repository was
    // provisioned in storage: the storage check must 404, not 500.
    let pool = gitforge_db::Pool::new(&server.db_path.display().to_string())
        .await
        .expect("open db");
    let user = gitforge_db::models::User::new(
        "ghostowner".to_string(),
        "ghost@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .expect("create ghost user");
    let ghost =
        gitforge_db::models::Repository::new("ghost".to_string(), user.id, "git".to_string());
    gitforge_db::queries::RepoQueries::create(&pool, &ghost)
        .await
        .expect("create ghost repo");
    drop(pool);

    let response = client
        .get(format!("{root}/ghostowner/ghost/info/refs"))
        .send()
        .await
        .expect("ghost repo info/refs");
    assert_eq!(response.status(), 404, "storage-less row must be 404");

    common::shutdown_gracefully(&mut server.child).await;
}

/// Without `DATABASE_URL` every git route degrades to 503 while the
/// health endpoint keeps serving.
#[tokio::test]
async fn test_dbless_server_rejects_git_routes() {
    let mut server = spawn_dbless_server().await;
    let client = reqwest::Client::new();
    let root = format!("http://127.0.0.1:{}", server.http_port);

    let response = client
        .get(format!("{root}/git-upload-pack/testowner/proto"))
        .send()
        .await
        .expect("dbless legacy upload-pack");
    assert_eq!(response.status(), 503);

    let response = client
        .get(format!(
            "{root}/testowner/proto/info/refs?service=git-receive-pack"
        ))
        .send()
        .await
        .expect("dbless info/refs");
    assert_eq!(response.status(), 503);

    let response = client
        .post(format!("{root}/testowner/proto/git-upload-pack"))
        .body(b"0000".to_vec())
        .send()
        .await
        .expect("dbless upload-pack post");
    assert_eq!(response.status(), 503);

    let response = client
        .post(format!("{root}/testowner/proto/git-receive-pack"))
        .body(b"0000".to_vec())
        .send()
        .await
        .expect("dbless receive-pack post");
    assert_eq!(response.status(), 503);

    let response = client
        .get(format!("{root}/health"))
        .send()
        .await
        .expect("dbless health");
    assert_eq!(response.status(), 200);

    common::shutdown_gracefully(&mut server.child).await;
}

/// Bodies over `GITFORGE_MAX_GIT_BODY_BYTES` are rejected explicitly
/// (413 on receive-pack, 400 on upload-pack) instead of silently
/// truncated.
#[tokio::test]
async fn test_oversized_git_bodies_are_rejected() {
    let mut server = spawn_seeded_server(&[("GITFORGE_MAX_GIT_BODY_BYTES", "1024")]).await;
    let client = reqwest::Client::new();
    let root = format!("http://127.0.0.1:{}", server.http_port);
    let oversized = vec![b'x'; 4096];

    let response = client
        .post(format!("{root}/testowner/proto/git-receive-pack"))
        .body(oversized.clone())
        .send()
        .await
        .expect("oversized receive-pack");
    assert_eq!(response.status(), 413);

    let response = client
        .post(format!("{root}/testowner/proto/git-upload-pack"))
        .body(oversized)
        .send()
        .await
        .expect("oversized upload-pack");
    assert_eq!(response.status(), 400);

    common::shutdown_gracefully(&mut server.child).await;
}

/// An accepted push writes a durable outbox event and the server delivers
/// it to the configured CI trigger with the bearer token and the pushed
/// ref/hash payload — including reclaiming a stale `delivering` lease
/// left behind by a crashed predecessor.
#[tokio::test]
async fn test_push_delivers_ci_trigger_and_reclaims_stale_lease() {
    let (trigger_url, recorded, receiver_handle) = spawn_trigger_receiver(200).await;
    let mut server = spawn_seeded_server(&[
        ("GITFORGE_CI_TRIGGER_URL", trigger_url.as_str()),
        ("GITFORGE_CI_TRIGGER_TOKEN", "ci-secret-token"),
    ])
    .await;

    // A predecessor crashed after claiming delivery: the row sits in
    // `delivering` with an expired lease. The delivery loop must reclaim
    // and deliver it alongside the new push's event.
    let stale_payload = serde_json::json!({
        "repo_id": server.repo_id.to_string(),
        "ref_name": "refs/heads/stale",
        "old_hash": "0".repeat(40),
        "new_hash": "a".repeat(40),
    });
    let pool = gitforge_db::Pool::new(&server.db_path.display().to_string())
        .await
        .expect("open db for stale row");
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created_at, delivery_token, delivery_until, delivery_attempts)
         VALUES (?, 'ci.trigger.delivering', ?, ?, ?, ?, 1)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(stale_payload.to_string())
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(uuid::Uuid::new_v4().to_string())
    .bind((chrono::Utc::now() - chrono::Duration::seconds(300)).to_rfc3339())
    .execute(pool.pool())
    .await
    .expect("insert stale delivering row");
    drop(pool);

    let pushed_sha = push_a_commit(&server, "ci-trigger").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);

    // Both the reclaimed stale event and the push event must end delivered.
    let delivered = loop {
        let rows = event_rows(&server.db_path).await;
        let delivered_count = rows
            .iter()
            .filter(|(_, event_type, _)| event_type == "ci.trigger.delivered")
            .count();
        if delivered_count >= 2 {
            break rows;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected 2 delivered triggers, events are {rows:?}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    assert!(
        delivered.iter().all(|(_, _, attempts)| *attempts >= 1),
        "delivery must have been attempted through the receiver: {delivered:?}"
    );

    let requests = recorded.lock().unwrap().clone();
    assert!(
        requests.len() >= 2,
        "receiver saw {} requests, expected at least 2",
        requests.len()
    );
    for request in &requests {
        assert_eq!(
            request.authorization.as_deref(),
            Some("Bearer ci-secret-token"),
            "trigger must carry the configured bearer token"
        );
    }
    let push_trigger = requests
        .iter()
        .find(|request| request.body["ref_name"] == "refs/heads/main")
        .expect("push event delivered to the trigger endpoint");
    assert_eq!(push_trigger.body["new_hash"], pushed_sha);
    assert_eq!(
        push_trigger.body["old_hash"],
        "0".repeat(40),
        "branch creation pushes advertise the zero hash as the old value"
    );
    assert_eq!(push_trigger.body["repo_id"], server.repo_id.to_string());

    receiver_handle.abort();
    common::shutdown_gracefully(&mut server.child).await;
}

/// A failing CI trigger keeps the push successful but returns the event
/// to the pending outbox with an incremented attempt counter, so the
/// delivery loop retries instead of losing the trigger.
#[tokio::test]
async fn test_failing_ci_trigger_requeues_the_event() {
    let (trigger_url, recorded, receiver_handle) = spawn_trigger_receiver(500).await;
    let mut server = spawn_seeded_server(&[
        ("GITFORGE_CI_TRIGGER_URL", trigger_url.as_str()),
        ("GITFORGE_CI_TRIGGER_TOKEN", "ci-secret-token"),
    ])
    .await;

    let pushed_sha = push_a_commit(&server, "ci-failure").await;
    assert_eq!(
        pushed_sha.len(),
        40,
        "push must succeed despite the CI outage"
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let rows = event_rows(&server.db_path).await;
        let requeued = rows
            .iter()
            .any(|(_, event_type, attempts)| event_type == "ci.trigger.pending" && *attempts >= 1);
        if requeued {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "event was never requeued after trigger failure; events are {rows:?}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    assert!(
        !recorded.lock().unwrap().is_empty(),
        "the receiver must have been called before the failure"
    );
    assert!(
        !rows_contain_delivered(&server.db_path).await,
        "a failed trigger must never be marked delivered"
    );

    receiver_handle.abort();
    common::shutdown_gracefully(&mut server.child).await;
}

async fn rows_contain_delivered(db_path: &std::path::Path) -> bool {
    event_rows(db_path)
        .await
        .iter()
        .any(|(_, event_type, _)| event_type == "ci.trigger.delivered")
}
