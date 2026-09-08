//! Per-user SSH public key registry routes.
//!
//! Keys registered here are the only ones the git transport's public-key
//! authentication accepts, making SSH strictly stronger than the
//! unauthenticated Smart HTTP transport. Registration validates the key by
//! parsing it and stores the OpenSSH fingerprint, which the transport
//! resolves against at authentication time.

use crate::middleware::AuthenticatedUser;
use axum::{
    extract::{Extension, Path},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, post},
    Json, Router,
};
use gitforge_common::SshKeyId;
use gitforge_db::{queries::SshKeyQueries, Pool};
use serde::{Deserialize, Serialize};
use ssh_key::{HashAlg, PublicKey as SshPublicKey};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct CreateSshKeyRequest {
    pub name: String,
    pub public_key: String,
}

#[derive(Debug, Serialize)]
pub struct SshKeyResponse {
    pub id: String,
    pub name: String,
    pub fingerprint: String,
    pub created_at: String,
}

impl SshKeyResponse {
    fn from_model(key: &gitforge_db::models::SshKey) -> Self {
        Self {
            id: key.id.to_string(),
            name: key.name.clone(),
            fingerprint: key.fingerprint.clone(),
            created_at: key.created_at.to_rfc3339(),
        }
    }
}

/// Maximum accepted length for the label and the key line. Public keys are
/// a few hundred bytes; anything larger is a mistake or abuse.
const MAX_KEY_NAME_LEN: usize = 100;
const MAX_KEY_MATERIAL_LEN: usize = 8 * 1024;

pub fn ssh_key_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/ssh-keys", post(create_key).get(list_keys))
        .route("/ssh-keys/{id}", delete(delete_key))
}

fn error_response(status: StatusCode, code: &str, message: String) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({
            "error": code,
            "message": message,
        })),
    )
        .into_response()
}

/// Parse an OpenSSH public key line and return its normalized form plus the
/// OpenSSH fingerprint used as the authentication identity.
fn parse_public_key(raw: &str) -> Result<(String, String), String> {
    let key = SshPublicKey::from_openssh(raw)
        .map_err(|error| format!("not a valid OpenSSH public key: {error}"))?;
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    let normalized = key
        .to_openssh()
        .map_err(|error| format!("failed to encode public key: {error}"))?;
    Ok((normalized, fingerprint))
}

async fn create_key(
    user: AuthenticatedUser,
    Extension(pool): Extension<Arc<Pool>>,
    Json(request): Json<CreateSshKeyRequest>,
) -> impl IntoResponse {
    let name = request.name.trim().to_string();
    if name.is_empty() || name.len() > MAX_KEY_NAME_LEN {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            format!("key name must be 1..={MAX_KEY_NAME_LEN} characters"),
        );
    }
    let material = request.public_key.trim();
    if material.is_empty() || material.len() > MAX_KEY_MATERIAL_LEN {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_public_key",
            format!("public key must be 1..={MAX_KEY_MATERIAL_LEN} bytes"),
        );
    }

    let (public_key, fingerprint) = match parse_public_key(material) {
        Ok(parsed) => parsed,
        Err(message) => {
            return error_response(StatusCode::BAD_REQUEST, "invalid_public_key", message);
        }
    };

    // One public key maps to exactly one account, so a duplicate
    // fingerprint is a conflict no matter who registered it first.
    match SshKeyQueries::find_by_fingerprint(&pool, &fingerprint).await {
        Ok(Some(_)) => {
            return error_response(
                StatusCode::CONFLICT,
                "duplicate_key",
                format!("this public key is already registered ({fingerprint})"),
            );
        }
        Ok(None) => {}
        Err(error) => {
            tracing::error!(%error, "ssh key fingerprint lookup failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "failed to register ssh key".to_string(),
            );
        }
    }

    let key = gitforge_db::models::SshKey::new(user.claims.user_id, name, fingerprint, public_key);
    if let Err(error) = SshKeyQueries::create(&pool, &key).await {
        tracing::error!(%error, "ssh key registration failed");
        return match error.kind {
            gitforge_common::ErrorKind::InvalidInput => {
                error_response(StatusCode::CONFLICT, "duplicate_key", error.message)
            }
            _ => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "failed to register ssh key".to_string(),
            ),
        };
    }

    tracing::info!(
        user_id = %user.claims.user_id,
        fingerprint = %key.fingerprint,
        "ssh key registered"
    );
    (StatusCode::CREATED, Json(SshKeyResponse::from_model(&key))).into_response()
}

async fn list_keys(
    user: AuthenticatedUser,
    Extension(pool): Extension<Arc<Pool>>,
) -> impl IntoResponse {
    match SshKeyQueries::list_by_user(&pool, user.claims.user_id).await {
        Ok(keys) => {
            let response: Vec<SshKeyResponse> =
                keys.iter().map(SshKeyResponse::from_model).collect();
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(error) => {
            tracing::error!(%error, "ssh key listing failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "failed to list ssh keys".to_string(),
            )
        }
    }
}

async fn delete_key(
    user: AuthenticatedUser,
    Extension(pool): Extension<Arc<Pool>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let key_id = match Uuid::parse_str(&id) {
        Ok(uuid) => SshKeyId::from(uuid),
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_id",
                "Invalid ssh key ID format".to_string(),
            );
        }
    };

    match SshKeyQueries::delete_owned(&pool, key_id, user.claims.user_id).await {
        Ok(true) => (StatusCode::NO_CONTENT).into_response(),
        Ok(false) => error_response(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such ssh key registered to this account".to_string(),
        ),
        Err(error) => {
            tracing::error!(%error, "ssh key deletion failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "failed to delete ssh key".to_string(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitforge_common::UserId;

    // A real ed25519 public key, generated with ssh-keygen for these tests.
    const ED25519_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMQwksn9LBYwU9NUSXPjXnIzjPLYeLvmrJ967iPNRE6k gitforge-api-test";
    // Same key blob as ED25519_KEY with a different comment.
    const ED25519_KEY_OTHER_COMMENT: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMQwksn9LBYwU9NUSXPjXnIzjPLYeLvmrJ967iPNRE6k laptop";

    #[test]
    fn test_parse_public_key_accepts_valid_ed25519() {
        let (normalized, fingerprint) = parse_public_key(ED25519_KEY).unwrap();
        assert!(normalized.starts_with("ssh-ed25519 "));
        assert!(fingerprint.starts_with("SHA256:"));
        // The fingerprint is exactly OpenSSH's format: SHA256: + unpadded
        // standard base64 (43 characters for a 32-byte digest).
        let digest = fingerprint.strip_prefix("SHA256:").unwrap();
        assert_eq!(digest.len(), 43);
        assert!(!digest.ends_with('='));
    }

    #[test]
    fn test_parse_public_key_is_comment_insensitive() {
        // Comments may differ between the registered line and the line a
        // client presents; the fingerprint must be identical anyway.
        let (_, base) = parse_public_key(ED25519_KEY).unwrap();
        let (_, other) = parse_public_key(ED25519_KEY_OTHER_COMMENT).unwrap();
        assert_eq!(base, other);
    }

    #[test]
    fn test_parse_public_key_rejects_garbage() {
        assert!(parse_public_key("not a key").is_err());
        assert!(parse_public_key("ssh-ed25519 notbase64").is_err());
        assert!(parse_public_key("").is_err());
    }

    #[test]
    fn test_ssh_key_response_serialization() {
        let key = gitforge_db::models::SshKey::new(
            UserId::new(),
            "laptop".to_string(),
            "SHA256:abc".to_string(),
            ED25519_KEY.to_string(),
        );
        let json = serde_json::to_string(&SshKeyResponse::from_model(&key)).unwrap();
        assert!(json.contains("\"name\":\"laptop\""));
        assert!(json.contains("\"fingerprint\":\"SHA256:abc\""));
    }
}
