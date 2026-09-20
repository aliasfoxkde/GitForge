//! Integration tests for the per-user SSH public key registry.
//!
//! These keys are the only ones the git transport's public-key
//! authentication accepts, so the registry contract matters: keys must
//! parse, fingerprints are globally unique, listing is private, and
//! deletion is owner-scoped.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use gitforge_api::{ApiAuth, ApiServer};
use gitforge_db::{
    models::{SshKey, User},
    queries::{SshKeyQueries, UserQueries},
    Pool,
};
use serde_json::{json, Value};
use tower::ServiceExt;

// A real ed25519 public key generated with ssh-keygen for these tests, plus
// the same key blob under a different comment (fingerprints must match).
const ED25519_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMQwksn9LBYwU9NUSXPjXnIzjPLYeLvmrJ967iPNRE6k gitforge-api-test";
const ED25519_KEY_OTHER_COMMENT: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMQwksn9LBYwU9NUSXPjXnIzjPLYeLvmrJ967iPNRE6k laptop";
const SECOND_ED25519_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAINwIi84PHFF0wrOuz4fcMloW6va+3erK+MaxOyg5XzPr second-key";

struct Fixture {
    app: Router,
    pool: Pool,
    user_token: String,
    user_id: gitforge_common::UserId,
    other_token: String,
    other_id: gitforge_common::UserId,
}

async fn seed() -> Fixture {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let user = User::new(
        "key-user".to_string(),
        "key-user@example.com".to_string(),
        "hash".to_string(),
    );
    let other = User::new(
        "key-other".to_string(),
        "key-other@example.com".to_string(),
        "hash".to_string(),
    );
    for u in [&user, &other] {
        UserQueries::create(&pool, u).await.unwrap();
    }
    let auth = ApiAuth::new("test-secret");
    let app = ApiServer::new("test-secret", pool.clone()).into_router();
    Fixture {
        app,
        pool,
        user_token: auth
            .generate_token(user.id, &user.username, "developer")
            .unwrap(),
        user_id: user.id,
        other_token: auth
            .generate_token(other.id, &other.username, "developer")
            .unwrap(),
        other_id: other.id,
    }
}

async fn send(
    app: Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json");
    let payload = body.map(|v| v.to_string()).unwrap_or_default();
    let response = app
        .oneshot(builder.body(Body::from(payload)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn register_key_parses_fingerprints_and_persists() {
    let f = seed().await;

    let (status, body) = send(
        f.app.clone(),
        "POST",
        "/api/ssh-keys",
        &f.user_token,
        Some(json!({"name": "laptop", "public_key": ED25519_KEY})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["name"], "laptop");
    let fingerprint = body["fingerprint"].as_str().unwrap();
    assert!(fingerprint.starts_with("SHA256:"));
    assert_eq!(fingerprint.len(), "SHA256:".len() + 43);

    // The fingerprint in the response is the one stored as auth identity.
    let stored = SshKeyQueries::list_by_user(&f.pool, f.user_id)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].fingerprint, fingerprint);
}

#[tokio::test]
async fn register_key_rejects_bad_names_and_material() {
    let f = seed().await;

    for (label, payload) in [
        (
            "empty-name",
            json!({"name": "   ", "public_key": ED25519_KEY}),
        ),
        (
            "long-name",
            json!({"name": "k".repeat(101), "public_key": ED25519_KEY}),
        ),
        ("empty-key", json!({"name": "laptop", "public_key": "   "})),
        (
            "oversized-key",
            json!({"name": "laptop", "public_key": format!("{ED25519_KEY} {}", "x".repeat(9 * 1024))}),
        ),
        (
            "garbage-key",
            json!({"name": "laptop", "public_key": "not a key"}),
        ),
        (
            "truncated-key",
            json!({"name": "laptop", "public_key": "ssh-ed25519 notbase64"}),
        ),
    ] {
        let (status, body) = send(
            f.app.clone(),
            "POST",
            "/api/ssh-keys",
            &f.user_token,
            Some(payload),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label}: {body}");
        let code = body["error"].as_str().unwrap();
        assert!(
            code == "invalid_name" || code == "invalid_public_key",
            "{label}: {code}"
        );
    }
    assert!(SshKeyQueries::list_by_user(&f.pool, f.user_id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn fingerprints_are_globally_unique_regardless_of_comment() {
    let f = seed().await;

    let first = json!({"name": "laptop", "public_key": ED25519_KEY});
    let (status, _) = send(
        f.app.clone(),
        "POST",
        "/api/ssh-keys",
        &f.user_token,
        Some(first),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // The same key with a different comment carries the same fingerprint,
    // so re-registering it — by anyone — is a conflict, not a duplicate row.
    let same_key_other_comment =
        json!({"name": "desktop", "public_key": ED25519_KEY_OTHER_COMMENT});
    let (status, body) = send(
        f.app.clone(),
        "POST",
        "/api/ssh-keys",
        &f.user_token,
        Some(same_key_other_comment),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "duplicate_key");

    let (status, body) = send(
        f.app.clone(),
        "POST",
        "/api/ssh-keys",
        &f.other_token,
        Some(json!({"name": "stolen", "public_key": ED25519_KEY})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "duplicate_key");

    // A genuinely different key registers fine for the second account.
    let (status, _) = send(
        f.app.clone(),
        "POST",
        "/api/ssh-keys",
        &f.other_token,
        Some(json!({"name": "second", "public_key": SECOND_ED25519_KEY})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn listing_is_private_and_deletion_is_owner_scoped() {
    let f = seed().await;
    let key = SshKey::new(
        f.user_id,
        "laptop".to_string(),
        "SHA256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa0".to_string(),
        ED25519_KEY.to_string(),
    );
    SshKeyQueries::create(&f.pool, &key).await.unwrap();
    let other_key = SshKey::new(
        f.other_id,
        "second".to_string(),
        "SHA256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        SECOND_ED25519_KEY.to_string(),
    );
    SshKeyQueries::create(&f.pool, &other_key).await.unwrap();

    // Listings contain only the caller's own keys.
    let (status, body) = send(f.app.clone(), "GET", "/api/ssh-keys", &f.user_token, None).await;
    assert_eq!(status, StatusCode::OK);
    let mine = body.as_array().unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["name"], "laptop");
    assert_eq!(mine[0]["id"], key.id.to_string());

    let (status, body) = send(f.app.clone(), "GET", "/api/ssh-keys", &f.other_token, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);

    // Deleting another account's key id is a not-found, not a leak.
    let (status, _) = send(
        f.app.clone(),
        "DELETE",
        &format!("/api/ssh-keys/{}", other_key.id),
        &f.user_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = send(
        f.app.clone(),
        "DELETE",
        "/api/ssh-keys/not-a-uuid",
        &f.user_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_id");

    let (status, _) = send(
        f.app.clone(),
        "DELETE",
        &format!("/api/ssh-keys/{}", key.id),
        &f.user_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(SshKeyQueries::list_by_user(&f.pool, f.user_id)
        .await
        .unwrap()
        .is_empty());

    // Deleting the same key again is a not-found.
    let (status, _) = send(
        f.app.clone(),
        "DELETE",
        &format!("/api/ssh-keys/{}", key.id),
        &f.user_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
