//! OpenAPI Documentation for GitForge API
//!
//! Interactive API documentation available at /swagger-ui when server is running.

use axum::{response::IntoResponse, routing::get, Json, Router};

use std::io::Read;
use std::path::Path;

/// Load the OpenAPI 3.0 specification from the external JSON file next to
/// this crate.
///
/// The spec lives in `openapi.json` beside the source so it can be
/// independently linted, versioned, and served as a standalone file
/// (e.g. `/api-docs/openapi.json`). The Rust source merely loads and
/// deserializes it.
fn load_openapi_spec() -> serde_json::Value {
    // Try the file next to the crate source first (development)
    let spec_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
    if spec_path.exists() {
        if let Ok(mut file) = std::fs::File::open(&spec_path) {
            let mut contents = String::new();
            if file.read_to_string(&mut contents).is_ok() {
                if let Ok(value) = serde_json::from_str(&contents) {
                    return value;
                }
            }
        }
    }

    // Fallback: use the inline default spec so the binary never fails
    // to start due to a missing spec file.
    serde_json::json!({
        "openapi": "3.0.3",
        "info": {
            "title": "GitForge API",
            "version": "1.0.0",
            "description": "Self-hosted Git platform with CI/CD capabilities",
            "contact": {
                "name": "GitForge Team",
                "email": "support@gitforge.dev"
            },
            "license": {
                "name": "MIT",
                "url": "https://opensource.org/licenses/MIT"
            }
        },
        "servers": [
            {
                "url": "/",
                "description": "Local server"
            }
        ],
        "tags": [
            {"name": "health", "description": "Health check endpoints"},
            {"name": "auth", "description": "Authentication status and login"},
            {"name": "repos", "description": "Repository management"},
            {"name": "ci", "description": "CI/CD pipelines"},
            {"name": "users", "description": "Administrative user management"},
            {"name": "runners", "description": "Runner management"},
            {"name": "artifacts", "description": "Artifact management"},
            {"name": "review", "description": "AI code review runs and findings"}
        ],
        "paths": {},
        "components": {
            "schemas": {}
        }
    })
}

/// OpenAPI spec response
pub async fn openapi_spec() -> impl IntoResponse {
    Json(load_openapi_spec())
}

/// Get the Swagger UI HTML page
pub async fn swagger_ui() -> impl IntoResponse {
    let html = r#"<!DOCTYPE html>
<html>
<head>
    <title>GitForge API - Swagger UI</title>
    <link rel="stylesheet" type="text/css" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css" />
    <style>
        body { margin: 0; padding: 0; }
    </style>
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
    <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-standalone-preset.js"></script>
    <script>
        window.onload = function() {
            SwaggerUIBundle({
                url: "/api-docs/openapi.json",
                dom_id: '#swagger-ui',
                presets: [
                    SwaggerUIBundle.presets.apis,
                    SwaggerUIBundle.SwaggerUIStandalonePreset
                ],
                layout: "StandaloneLayout"
            });
        };
    </script>
</body>
</html>"#;
    axum::response::Html(html)
}

/// Create the API docs router with OpenAPI spec and Swagger UI
pub fn api_docs_routes() -> Router {
    Router::new()
        .route("/api-docs/openapi.json", get(openapi_spec))
        .route("/swagger-ui", get(swagger_ui))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openapi_spec_contains_required_fields() {
        let spec = load_openapi_spec();
        assert!(spec.get("openapi").is_some());
        assert!(spec.get("info").is_some());
        assert!(spec.get("paths").is_some());
        assert!(spec.get("components").is_some());
    }

    #[test]
    fn test_openapi_version() {
        let spec = load_openapi_spec();
        assert_eq!(spec.get("openapi").unwrap(), "3.0.3");
    }

    #[test]
    fn test_api_info() {
        let spec = load_openapi_spec();
        let info = spec.get("info").unwrap();
        assert_eq!(info.get("title").unwrap(), "GitForge API");
        assert_eq!(info.get("version").unwrap(), "1.0.0");
    }

    #[test]
    fn test_api_paths_exist() {
        let spec = load_openapi_spec();
        let paths = spec.get("paths").unwrap().as_object().unwrap();
        assert!(paths.contains_key("/auth/login"));
        assert!(paths.contains_key("/auth/refresh"));
        assert!(paths.contains_key("/auth/logout"));
        assert!(paths.contains_key("/auth/status"));
        assert!(paths.contains_key("/health"));
        assert!(paths.contains_key("/repos"));
        assert!(paths.contains_key("/repos/{owner}/{repo}/policy"));
        assert!(paths.contains_key("/repos/{owner}/{repo}/commits/{sha}/status"));
        assert!(paths.contains_key("/users/{id}/role"));
        assert!(paths.contains_key("/pipelines"));
        assert!(paths.contains_key("/runners"));
        assert!(paths.contains_key("/artifacts"));
        assert!(paths.contains_key("/review-runs"));
        assert!(paths.contains_key("/review-runs/{id}"));
        assert!(paths.contains_key("/review-runs/{id}/findings"));
    }

    #[test]
    fn test_openapi_auth_routes_are_public_root_routes() {
        let spec = load_openapi_spec();
        let paths = spec.get("paths").unwrap().as_object().unwrap();
        let login = paths.get("/auth/login").unwrap().as_object().unwrap();
        assert!(login.get("post").is_some());
        assert!(login.get("post").unwrap().get("security").is_none());
        let status = paths.get("/auth/status").unwrap().as_object().unwrap();
        assert!(status.get("get").is_some());
        assert!(status.get("get").unwrap().get("security").is_none());
        assert!(!paths.contains_key("/api/auth/login"));
    }

    #[test]
    fn test_api_components_schemas() {
        let spec = load_openapi_spec();
        let components = spec.get("components").unwrap();
        let schemas = components.get("schemas").unwrap().as_object().unwrap();
        assert!(schemas.contains_key("HealthResponse"));
        assert!(schemas.contains_key("RepoResponse"));
        assert!(schemas.contains_key("PipelineRunResponse"));
        assert!(schemas.contains_key("JobResponse"));
        assert!(schemas.contains_key("RunnerResponse"));
        assert!(schemas.contains_key("ArtifactResponse"));
    }

    #[tokio::test]
    async fn test_openapi_spec_endpoint() {
        let response = openapi_spec().await.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_swagger_ui_returns_html() {
        let response = swagger_ui().await.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains("swagger-ui"));
        assert!(body_str.contains("GitForge API"));
    }

    #[test]
    fn test_api_docs_routes_creation() {
        let _routes = api_docs_routes();
        // Routes should be created successfully
    }

    #[test]
    fn test_openapi_spec_servers() {
        let spec = load_openapi_spec();
        let servers = spec.get("servers").unwrap().as_array().unwrap();
        assert!(!servers.is_empty());
        assert_eq!(servers[0].get("url").unwrap(), "/");
    }

    #[test]
    fn test_openapi_spec_tags() {
        let spec = load_openapi_spec();
        let tags = spec.get("tags").unwrap().as_array().unwrap();
        assert!(tags.len() >= 5);
    }

    #[test]
    fn test_openapi_paths_have_operations() {
        let spec = load_openapi_spec();
        let paths = spec.get("paths").unwrap().as_object().unwrap();
        // Health endpoint should have GET
        let health = paths.get("/health").unwrap().as_object().unwrap();
        assert!(health.contains_key("get"));
    }

    /// Verify that the spec loaded from the external file has all paths
    /// and schemas, not just the fallback stub.
    #[test]
    fn test_openapi_spec_from_file_has_content() {
        let spec = load_openapi_spec();
        let paths = spec.get("paths").unwrap().as_object().unwrap();
        // The fallback has an empty paths object; if the file loaded,
        // we should see real endpoints.
        assert!(
            paths.len() >= 5,
            "Expected at least 5 paths from spec file, got {}",
            paths.len()
        );
    }

    #[test]
    fn test_openapi_spec_endpoint_has_full_paths() {
        let spec = load_openapi_spec();
        let paths = spec.get("paths").unwrap().as_object().unwrap();
        // /health must exist
        assert!(paths.contains_key("/health"));
        // /pipelines must exist
        assert!(paths.contains_key("/pipelines"));
    }
}
