//! OpenAPI Documentation for GitForge API
//!
//! Interactive API documentation available at /swagger-ui when server is running.

use axum::{
    extract::Extension,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// OpenAPI 3.0 specification for GitForge API
pub fn get_openapi_spec() -> serde_json::Value {
    json!({
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
            {"name": "repos", "description": "Repository management"},
            {"name": "ci", "description": "CI/CD pipelines"},
            {"name": "runners", "description": "Runner management"},
            {"name": "artifacts", "description": "Artifact management"}
        ],
        "paths": {
            "/health": {
                "get": {
                    "tags": ["health"],
                    "summary": "Health check",
                    "description": "Returns the health status of the API server",
                    "responses": {
                        "200": {
                            "description": "Server is healthy",
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/HealthResponse"}
                                }
                            }
                        }
                    }
                }
            },
            "/repos": {
                "get": {
                    "tags": ["repos"],
                    "summary": "List repositories",
                    "description": "Returns a list of all repositories",
                    "responses": {
                        "200": {
                            "description": "List of repositories",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": {"$ref": "#/components/schemas/RepoResponse"}
                                    }
                                }
                            }
                        }
                    }
                },
                "post": {
                    "tags": ["repos"],
                    "summary": "Create repository",
                    "description": "Creates a new repository",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {"$ref": "#/components/schemas/CreateRepoRequest"}
                            }
                        }
                    },
                    "responses": {
                        "201": {
                            "description": "Repository created",
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/RepoResponse"}
                                }
                            }
                        },
                        "400": {"description": "Invalid input"}
                    }
                }
            },
            "/repos/{id}": {
                "get": {
                    "tags": ["repos"],
                    "summary": "Get repository",
                    "description": "Returns a repository by ID",
                    "parameters": [
                        {
                            "name": "id",
                            "in": "path",
                            "required": true,
                            "schema": {"type": "string"}
                        }
                    ],
                    "responses": {
                        "200": {
                            "description": "Repository found",
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/RepoResponse"}
                                }
                            }
                        },
                        "404": {"description": "Repository not found"}
                    }
                },
                "delete": {
                    "tags": ["repos"],
                    "summary": "Delete repository",
                    "description": "Deletes a repository by ID",
                    "parameters": [
                        {
                            "name": "id",
                            "in": "path",
                            "required": true,
                            "schema": {"type": "string"}
                        }
                    ],
                    "responses": {
                        "204": {"description": "Repository deleted"},
                        "404": {"description": "Repository not found"}
                    }
                }
            },
            "/pipelines": {
                "get": {
                    "tags": ["ci"],
                    "summary": "List pipelines",
                    "description": "Returns a list of all pipelines",
                    "responses": {
                        "200": {"description": "List of pipelines"}
                    }
                }
            },
            "/pipelines/{id}": {
                "get": {
                    "tags": ["ci"],
                    "summary": "Get pipeline",
                    "parameters": [
                        {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
                    ],
                    "responses": {
                        "200": {"description": "Pipeline found"},
                        "404": {"description": "Pipeline not found"}
                    }
                }
            },
            "/pipeline-runs": {
                "get": {
                    "tags": ["ci"],
                    "summary": "List pipeline runs",
                    "responses": {
                        "200": {"description": "List of pipeline runs"}
                    }
                }
            },
            "/pipeline-runs/{id}": {
                "get": {
                    "tags": ["ci"],
                    "summary": "Get pipeline run",
                    "parameters": [
                        {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
                    ],
                    "responses": {
                        "200": {
                            "description": "Pipeline run found",
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/PipelineRunResponse"}
                                }
                            }
                        },
                        "404": {"description": "Pipeline run not found"}
                    }
                }
            },
            "/pipeline-runs/{id}/jobs": {
                "get": {
                    "tags": ["ci"],
                    "summary": "Get pipeline run jobs",
                    "parameters": [
                        {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
                    ],
                    "responses": {
                        "200": {"description": "List of jobs"}
                    }
                }
            },
            "/jobs/{id}": {
                "get": {
                    "tags": ["ci"],
                    "summary": "Get job",
                    "parameters": [
                        {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
                    ],
                    "responses": {
                        "200": {
                            "description": "Job found",
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/JobResponse"}
                                }
                            }
                        },
                        "404": {"description": "Job not found"}
                    }
                }
            },
            "/jobs/{id}/logs": {
                "get": {
                    "tags": ["ci"],
                    "summary": "Get job logs",
                    "parameters": [
                        {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
                    ],
                    "responses": {
                        "200": {"description": "Job logs"}
                    }
                }
            },
            "/runners": {
                "get": {
                    "tags": ["runners"],
                    "summary": "List runners",
                    "responses": {
                        "200": {"description": "List of runners"}
                    }
                },
                "post": {
                    "tags": ["runners"],
                    "summary": "Register runner",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {"type": "object"}
                            }
                        }
                    },
                    "responses": {
                        "201": {
                            "description": "Runner registered",
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/RunnerResponse"}
                                }
                            }
                        }
                    }
                }
            },
            "/runners/{id}": {
                "get": {
                    "tags": ["runners"],
                    "summary": "Get runner",
                    "parameters": [
                        {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
                    ],
                    "responses": {
                        "200": {
                            "description": "Runner found",
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/RunnerResponse"}
                                }
                            }
                        },
                        "404": {"description": "Runner not found"}
                    }
                }
            },
            "/artifacts": {
                "get": {
                    "tags": ["artifacts"],
                    "summary": "List artifacts",
                    "responses": {
                        "200": {"description": "List of artifacts"}
                    }
                }
            },
            "/artifacts/{id}": {
                "get": {
                    "tags": ["artifacts"],
                    "summary": "Get artifact",
                    "parameters": [
                        {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
                    ],
                    "responses": {
                        "200": {
                            "description": "Artifact found",
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/ArtifactResponse"}
                                }
                            }
                        },
                        "404": {"description": "Artifact not found"}
                    }
                },
                "delete": {
                    "tags": ["artifacts"],
                    "summary": "Delete artifact",
                    "parameters": [
                        {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
                    ],
                    "responses": {
                        "204": {"description": "Artifact deleted"},
                        "404": {"description": "Artifact not found"}
                    }
                }
            },
            "/jobs/{job_id}/artifacts": {
                "get": {
                    "tags": ["artifacts"],
                    "summary": "Get job artifacts",
                    "parameters": [
                        {"name": "job_id", "in": "path", "required": true, "schema": {"type": "string"}}
                    ],
                    "responses": {
                        "200": {"description": "List of job artifacts"}
                    }
                }
            }
        },
        "components": {
            "schemas": {
                "HealthResponse": {
                    "type": "object",
                    "properties": {
                        "status": {"type": "string", "example": "healthy"},
                        "timestamp": {"type": "string", "example": "2024-01-01T00:00:00Z"}
                    }
                },
                "RepoResponse": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "name": {"type": "string"},
                        "owner_id": {"type": "string"},
                        "visibility": {"type": "string"},
                        "git_path": {"type": "string"},
                        "created_at": {"type": "string"}
                    }
                },
                "CreateRepoRequest": {
                    "type": "object",
                    "required": ["name"],
                    "properties": {
                        "name": {"type": "string"},
                        "visibility": {"type": "string", "nullable": true}
                    }
                },
                "PipelineRunResponse": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "pipeline_id": {"type": "string"},
                        "status": {"type": "string"},
                        "commit_hash": {"type": "string"},
                        "triggered_by": {"type": "string"},
                        "started_at": {"type": "string", "nullable": true},
                        "finished_at": {"type": "string", "nullable": true}
                    }
                },
                "JobResponse": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "name": {"type": "string"},
                        "status": {"type": "string"},
                        "runner_id": {"type": "string", "nullable": true},
                        "started_at": {"type": "string", "nullable": true},
                        "finished_at": {"type": "string", "nullable": true}
                    }
                },
                "RunnerResponse": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "name": {"type": "string"},
                        "type": {"type": "string"},
                        "status": {"type": "string"},
                        "capacity": {"type": "integer"},
                        "last_heartbeat": {"type": "string", "nullable": true}
                    }
                },
                "ArtifactResponse": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "job_id": {"type": "string"},
                        "name": {"type": "string"},
                        "path": {"type": "string"},
                        "checksum": {"type": "string"},
                        "size_bytes": {"type": "integer", "format": "int64"},
                        "created_at": {"type": "string"}
                    }
                }
            }
        }
    })
}

/// OpenAPI spec response
pub async fn openapi_spec() -> impl IntoResponse {
    Json(get_openapi_spec())
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
        .route("/openapi.json", get(openapi_spec))
        .route("/", get(swagger_ui))
}
