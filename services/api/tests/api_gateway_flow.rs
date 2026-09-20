//! End-to-end API gateway flow test.
//!
//! This test spawns the real `api` binary against a temporary SQLite
//! database and drives it like a genuine client: health, login with a
//! seeded bcrypt-hashed account, an authenticated repository create/list
//! round trip, and the per-user SSH key registry endpoints (register,
//! duplicate rejection, list, delete). No layer is mocked — the JWT
//! middleware, extractors, and SQL all run in the real service, and the
//! service is stopped with SIGTERM so `cargo llvm-cov` counts the
//! startup and shutdown path.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use gitforge_common::UserId;

mod common;

/// A real ed25519 public key, generated with ssh-keygen for these tests.
const ED25519_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMQwksn9LBYwU9NUSXPjXnIzjPLYeLvmrJ967iPNRE6k api-gateway-test";

/// Environment for a spawned api service.
struct ApiService {
    child: tokio::process::Child,
    port: u16,
    db_path: PathBuf,
}

impl Drop for ApiService {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// Prepare the database with one account, then spawn the real api binary.
async fn spawn_api() -> ApiService {
    let unique = uuid::Uuid::new_v4();
    let base = std::env::temp_dir().join(format!("gitforge-api-{unique}"));
    let artifacts = base.join("artifacts");
    let db_path = base.join("gitforge.db");
    std::fs::create_dir_all(&artifacts).expect("create layout");

    let pool = gitforge_db::Pool::new(&db_path.display().to_string())
        .await
        .expect("create sqlite pool");
    pool.migrate().await.expect("run migrations");
    let password_hash =
        gitforge_common::password::hash_password("harness-passphrase").expect("hash password");
    let user = gitforge_db::models::User::new(
        "gatewayowner".to_string(),
        "gatewayowner@example.com".to_string(),
        password_hash,
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .expect("create user");
    drop(pool);

    let port = free_port();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_api"))
        .env("PORT", port.to_string())
        .env("DATABASE_URL", format!("sqlite:{}", db_path.display()))
        .env("JWT_SECRET", "harness-jwt-secret")
        .env("GITFORGE_ARTIFACT_ROOT", &artifacts)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn api binary");

    // Wait for the gateway to come up.
    let health = format!("http://127.0.0.1:{port}/health");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if tokio::time::Instant::now() >= deadline {
            let _ = child.start_kill();
            panic!("api service did not become healthy at {health}");
        }
        if let Ok(response) = reqwest::get(&health).await {
            if let Ok(body) = response.text().await {
                if body.contains("healthy") {
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    ApiService {
        child,
        port,
        db_path,
    }
}

#[tokio::test]
async fn test_login_and_authenticated_repository_and_ssh_key_flow() {
    let mut service = spawn_api().await;
    let base = format!("http://127.0.0.1:{}", service.port);
    let client = reqwest::Client::new();

    // ─── Protected routes reject anonymous callers ──────────────────────
    let status = client
        .get(format!("{base}/api/repos"))
        .send()
        .await
        .expect("anonymous repos request")
        .status();
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);

    // ─── Login with the seeded account, and reject a bad password ───────
    let login = |password: &str| {
        client
            .post(format!("{base}/auth/login"))
            .json(&serde_json::json!({
                "username": "gatewayowner",
                "password": password,
            }))
    };
    let bad = login("wrong-passphrase")
        .send()
        .await
        .expect("bad login request");
    assert_eq!(bad.status(), reqwest::StatusCode::UNAUTHORIZED);

    let good = login("harness-passphrase")
        .send()
        .await
        .expect("login request");
    assert_eq!(good.status(), reqwest::StatusCode::OK);
    let login_body: serde_json::Value = good.json().await.expect("parse login response");
    let token = login_body["token"].as_str().expect("token in response");
    assert_eq!(login_body["token_type"], "Bearer");

    // ─── Repository create + list round trip over the real API ──────────
    let created = client
        .post(format!("{base}/api/repos"))
        .header("authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({"name": "harness-repo"}))
        .send()
        .await
        .expect("create repo request");
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    let repo_body: serde_json::Value = created.json().await.expect("parse repo response");
    let repo_name = repo_body["name"].as_str().expect("repo name");
    assert_eq!(repo_name, "harness-repo");

    let listed = client
        .get(format!("{base}/api/repos"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .expect("list repos request");
    assert_eq!(listed.status(), reqwest::StatusCode::OK);
    let listed_body: serde_json::Value = listed.json().await.expect("parse repos");
    let names: Vec<&str> = listed_body
        .as_array()
        .expect("repos array")
        .iter()
        .filter_map(|repo| repo["name"].as_str())
        .collect();
    assert!(
        names.contains(&"harness-repo"),
        "created repository must be listed: {names:?}"
    );

    // ─── SSH key registry: register, duplicate 409, list, delete ────────
    let register = |key: &str| {
        client
            .post(format!("{base}/api/ssh-keys"))
            .header("authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({"name": "laptop", "public_key": key}))
    };
    let key = register(ED25519_KEY)
        .send()
        .await
        .expect("register ssh key");
    assert_eq!(key.status(), reqwest::StatusCode::CREATED);
    let key_body: serde_json::Value = key.json().await.expect("parse ssh key response");
    let key_id = key_body["id"].as_str().expect("key id").to_string();
    assert!(key_body["fingerprint"]
        .as_str()
        .expect("fingerprint")
        .starts_with("SHA256:"));

    let duplicate = register(ED25519_KEY)
        .send()
        .await
        .expect("duplicate ssh key");
    assert_eq!(duplicate.status(), reqwest::StatusCode::CONFLICT);

    let keys = client
        .get(format!("{base}/api/ssh-keys"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .expect("list ssh keys");
    assert_eq!(keys.status(), reqwest::StatusCode::OK);
    let keys_body: serde_json::Value = keys.json().await.expect("parse keys");
    assert_eq!(keys_body.as_array().expect("keys array").len(), 1);

    let deleted = client
        .delete(format!("{base}/api/ssh-keys/{key_id}"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .expect("delete ssh key");
    assert_eq!(deleted.status(), reqwest::StatusCode::NO_CONTENT);

    // The registry rows lived in the service's own database.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let user_id: UserId = gitforge_db::queries::UserQueries::get_by_username(&pool, "gatewayowner")
        .await
        .expect("lookup user")
        .expect("user exists")
        .id;
    let remaining = gitforge_db::queries::SshKeyQueries::list_by_user(&pool, user_id)
        .await
        .expect("list keys from database");
    assert!(remaining.is_empty(), "deleted key must be gone");

    common::shutdown_gracefully(&mut service.child).await;
}
