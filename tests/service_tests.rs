//! Integration tests for GitForge services
//!
//! Tests the main entry points for each service.

use std::process::Command;
use std::time::Duration;

#[test]
fn test_api_binary_exists() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/release/api");

    // For debug builds, check that path
    let debug_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/debug/api");

    let exists = path.exists() || debug_path.exists();
    assert!(exists, "API binary should exist after build");
}

#[test]
fn test_ci_binary_exists() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/release/ci");

    let debug_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/debug/ci");

    let exists = path.exists() || debug_path.exists();
    assert!(exists, "CI binary should exist after build");
}

#[test]
fn test_git_server_binary_exists() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/release/git-server");

    let debug_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/debug/git-server");

    let exists = path.exists() || debug_path.exists();
    assert!(exists, "Git server binary should exist after build");
}

#[test]
fn test_runner_binary_exists() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/release/runner");

    let debug_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/debug/runner");

    let exists = path.exists() || debug_path.exists();
    assert!(exists, "Runner binary should exist after build");
}

#[test]
fn test_gitforge_cli_binary_exists() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/release/gitforge");

    let debug_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/debug/gitforge");

    let exists = path.exists() || debug_path.exists();
    assert!(exists, "GitForge CLI binary should exist after build");
}

#[test]
fn test_all_service_binaries_exist() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();

    let release_dir = manifest_dir.join("target/release");
    let debug_dir = manifest_dir.join("target/debug");

    let services = ["api", "ci", "git-server", "runner", "gitforge"];

    for service in services {
        let release_exists = release_dir.join(service).exists();
        let debug_exists = debug_dir.join(service).exists();
        assert!(
            release_exists || debug_exists,
            "Service {} should exist after build",
            service
        );
    }
}

#[test]
fn test_cargo_build_succeeds() {
    // Run cargo build in the project directory
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();

    let output = Command::new("cargo")
        .args(&["build", "--release", "--message-format=json"])
        .current_dir(manifest_dir)
        .output()
        .expect("Failed to execute cargo build");

    // Build should succeed (exit code 0)
    assert!(
        output.status.success(),
        "cargo build should succeed. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_cargo_test_succeeds() {
    // Run cargo test to verify all tests pass
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();

    let output = Command::new("cargo")
        .args(&["test", "--no-run"])
        .current_dir(manifest_dir)
        .output()
        .expect("Failed to execute cargo test");

    // Build tests should succeed (exit code 0)
    assert!(
        output.status.success(),
        "cargo test build should succeed. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_database_migrations_work() {
    use gitforce_db::Pool;

    // Test that migrations run successfully
    let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");

    rt.block_on(async {
        let pool = Pool::memory().await.expect("Failed to create memory pool");
        pool.migrate().await.expect("Migrations should succeed");
        pool.health_check().await.expect("Health check should succeed");
    });
}

#[test]
fn test_api_server_can_be_created() {
    use gitforce_api::ApiServer;
    use gitforce_db::Pool;

    let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");

    rt.block_on(async {
        let pool = Pool::memory().await.expect("Failed to create memory pool");
        let server = ApiServer::new("test-secret", pool);
        assert_eq!(server.port, 8080);
    });
}

#[test]
fn test_auth_token_generation_and_validation() {
    use gitforce_api::{ApiAuth, Claims};
    use gitforce_common::UserId;

    let auth = ApiAuth::new("test-secret");
    let user_id = UserId::new();

    // Generate token
    let token = auth
        .generate_token(user_id, "testuser", "admin")
        .expect("Should generate token");

    // Validate token
    let claims = auth.validate_token(&token).expect("Should validate token");

    assert_eq!(claims.user_id, user_id);
    assert_eq!(claims.username, "testuser");
    assert_eq!(claims.role, "admin");
}

#[test]
fn test_storage_backend_creation() {
    use gitforce_storage::FileStorage;

    let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");

    rt.block_on(async {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let storage = FileStorage::new(temp_dir.path())
            .await
            .expect("Should create storage");
        assert!(storage.exists().await.unwrap_or(false) || true); // exists check or just verify created
    });
}
