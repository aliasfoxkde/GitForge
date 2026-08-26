//! Administrative user-management routes.

use crate::middleware::AuthenticatedUser;
use axum::{
    extract::{Extension, Path},
    http::StatusCode,
    response::IntoResponse,
    routing::patch,
    Json, Router,
};
use gitforge_common::UserId;
use gitforge_db::{queries::UserQueries, Pool};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct UpdateRoleRequest {
    pub role: String,
}

#[derive(Debug, Serialize)]
pub struct UpdateRoleResponse {
    pub user_id: String,
    pub role: String,
}

pub fn user_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/users/{id}/role", patch(update_role))
}

fn valid_role(role: &str) -> bool {
    matches!(role, "admin" | "maintainer" | "developer" | "read_only")
}

async fn update_role(
    user: AuthenticatedUser,
    Extension(pool): Extension<Arc<Pool>>,
    Path(id): Path<String>,
    Json(request): Json<UpdateRoleRequest>,
) -> impl IntoResponse {
    if user.claims.role != "admin" {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "forbidden",
                "message": "Only administrators may manage user roles"
            })),
        )
            .into_response();
    }
    if !valid_role(&request.role) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_role",
                "message": "Role must be admin, maintainer, developer, or read_only"
            })),
        )
            .into_response();
    }
    let user_id = match Uuid::parse_str(&id) {
        Ok(uuid) => UserId::from(uuid),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_id",
                    "message": "Invalid user ID format"
                })),
            )
                .into_response();
        }
    };

    let current_role = match UserQueries::get_role(&pool, user_id).await {
        Ok(Some(role)) => role,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "not_found",
                    "message": "User not found"
                })),
            )
                .into_response();
        }
        Err(error) => {
            tracing::error!(%error, %user_id, "failed to load target user role");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "database_error"})),
            )
                .into_response();
        }
    };

    if current_role == "admin" && request.role != "admin" {
        match UserQueries::count_role(&pool, "admin").await {
            Ok(count) if count <= 1 => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "last_admin",
                        "message": "The last administrator cannot be demoted"
                    })),
                )
                    .into_response();
            }
            Ok(_) => {}
            Err(error) => {
                tracing::error!(%error, "failed to count administrators");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "database_error"})),
                )
                    .into_response();
            }
        }
    }

    match UserQueries::set_role(&pool, user_id, &request.role).await {
        Ok(true) => (
            StatusCode::OK,
            Json(UpdateRoleResponse {
                user_id: user_id.to_string(),
                role: request.role,
            }),
        )
            .into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "not_found",
                "message": "User not found"
            })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(%error, %user_id, "failed to persist user role");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "database_error"})),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_role_accepts_supported_roles_only() {
        for role in ["admin", "maintainer", "developer", "read_only"] {
            assert!(valid_role(role));
        }
        for role in ["user", "root", "", "ADMIN"] {
            assert!(!valid_role(role));
        }
    }
}
