//! HTTP Transport for MCP — compatible with LM Studio and HTTP-based MCP clients
//!
//! Endpoints:
//!   POST /  — JSON-RPC request → JSON-RPC response
//!   GET  /  — Server-Sent Events stream for server-initiated messages

use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::post,
    Router,
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;
use tracing::info;

use crate::handlers::{McPHandler, ToolCall};
use crate::JsonRpcRequest;

/// Shared state
#[derive(Clone)]
struct HttpState {
    handler: McPHandler,
}

impl HttpState {
    fn new() -> Self {
        Self { handler: McPHandler }
    }

    fn handle_rpc(&self, req: JsonRpcRequest) -> Result<serde_json::Value, String> {
        let id = req.id.clone();
        let response = match req.method.as_str() {
            "initialize" => serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": { "listChanged": true }, "prompts": {}, "resources": {} },
                "serverInfo": { "name": "gitforge-mcp", "version": "0.1.0" }
            }),

            "tools/list" => serde_json::json!({
                "tools": get_tools_list()
            }),

            "tools/call" => {
                let tool_name = req.params.get("name")
                    .and_then(|v| v.as_str()).unwrap_or("");
                let arguments = req.params.get("arguments")
                    .and_then(|v| v.as_object()).cloned().unwrap_or_default();
                let call = ToolCall { name: tool_name.to_string(), arguments };
                let result = self.handler.handle(call);
                return Ok(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "toolResult": result }
                }));
            }

            "initialized" | "shutdown" => serde_json::json!({ "ok": true }),

            _ => return Err(format!("Method not found: {}", req.method)),
        };

        Ok(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": response
        }))
    }
}

fn get_tools_list() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "ci_run",
            "description": "Trigger a CI run on a repository",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "owner/repo" },
                    "branch": { "type": "string", "description": "Branch name" },
                    "delta": { "type": "boolean", "description": "Run delta analysis" },
                    "scope": { "type": "string", "enum": ["trivial","fast","standard","full","heavy"] },
                    "workflows": { "type": "string", "description": "Comma-separated workflow names" }
                }
            }
        }),
        serde_json::json!({
            "name": "ci_status",
            "description": "Get the status of a CI run",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string" },
                    "run_id": { "type": "string" },
                    "branch": { "type": "string" }
                }
            }
        }),
        serde_json::json!({
            "name": "ci_cancel",
            "description": "Cancel a running CI job",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string" },
                    "run_id": { "type": "string" }
                },
                "required": ["run_id"]
            }
        }),
        serde_json::json!({
            "name": "delta_plan",
            "description": "Analyze git diff and preview what CI would run",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo_root": { "type": "string", "default": "." },
                    "base_ref": { "type": "string", "default": "HEAD~1" }
                }
            }
        }),
        serde_json::json!({
            "name": "security_scan",
            "description": "Run Atheon security scan",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "default": "." },
                    "categories": { "type": "string", "default": "secrets,pii,security" }
                }
            }
        }),
        serde_json::json!({
            "name": "list_repos",
            "description": "List repositories accessible to the authenticated user",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "get_repo_config",
            "description": "Get CI configuration for a repository",
            "inputSchema": {
                "type": "object",
                "properties": { "repo": { "type": "string" } }
            }
        }),
        serde_json::json!({
            "name": "pr_create",
            "description": "Create a pull request",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string" },
                    "title": { "type": "string" },
                    "body": { "type": "string" },
                    "head": { "type": "string" },
                    "base": { "type": "string", "default": "main" }
                },
                "required": ["title"]
            }
        }),
        serde_json::json!({
            "name": "pr_merge",
            "description": "Merge a pull request",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string" },
                    "pr_number": { "type": "number" },
                    "method": { "type": "string", "enum": ["squash","merge","rebase"], "default": "squash" }
                },
                "required": ["pr_number"]
            }
        }),
    ]
}

/// POST / — handle JSON-RPC request
async fn handle_rpc(
    State(state): State<Arc<HttpState>>,
    Json(req): Json<JsonRpcRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match state.handle_rpc(req) {
        Ok(resp) => Ok(Json(resp)),
        Err(msg) => {
            let resp = serde_json::json!({
                "jsonrpc": "2.0",
                "id": serde_json::Value::Null,
                "error": { "code": -32601, "message": msg }
            });
            Ok(Json(resp))
        }
    }
}

/// Start the HTTP MCP server
pub async fn run_http(port: u16) -> Result<()> {
    let state = Arc::new(HttpState::new());
    let app = Router::new()
        .route("/", post(handle_rpc))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!("Starting HTTP MCP server on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
