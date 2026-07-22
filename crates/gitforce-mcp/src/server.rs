//! MCP Server — JSON-RPC over stdio transport

use crate::{JsonRpcRequest, JsonRpcResponse};
use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use tracing::{error, info};

use crate::handlers::McPHandler;

pub struct McpServer;

impl McpServer {
    /// Run the MCP server — read JSON-RPC requests from stdin, write to stdout
    pub fn run() -> Result<()> {
        let handler = McPHandler;
        let stdin = BufReader::new(std::io::stdin());
        let mut stdout = std::io::stdout();

        info!("GitForge MCP server starting...");

        for line in stdin.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    error!("Failed to read stdin: {}", e);
                    break;
                }
            };

            // Skip empty lines
            if line.trim().is_empty() {
                continue;
            }

            // Parse JSON-RPC request
            let request: JsonRpcRequest = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    error!("Failed to parse JSON-RPC request: {}", e);
                    let resp = JsonRpcResponse::err(
                        serde_json::Value::from("null"),
                        -32700,
                        format!("Parse error: {}", e),
                    );
                    let _ = writeln!(stdout, "{}", serde_json::to_string(&resp).unwrap());
                    let _ = stdout.flush();
                    continue;
                }
            };

            // Route to handler
            let response = Self::handle_request(&handler, request);
            let _ = writeln!(stdout, "{}", serde_json::to_string(&response).unwrap());
            let _ = stdout.flush();
        }

        Ok(())
    }

    fn handle_request(handler: &McPHandler, req: JsonRpcRequest) -> JsonRpcResponse {
        match req.method.as_str() {
            "initialize" => JsonRpcResponse::ok(
                req.id.clone(),
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": { "listChanged": true },
                        "prompts": {},
                        "resources": {}
                    },
                    "serverInfo": { "name": "gitforge-mcp", "version": "0.1.0" }
                }),
            ),

            "tools/list" => JsonRpcResponse::ok(req.id.clone(), serde_json::json!({
                "tools": [
                    {"name": "ci_run", "description": "Trigger a CI run with optional delta analysis",
                     "inputSchema": {"type": "object", "properties": {
                        "repo": {"type": "string"}, "branch": {"type": "string"},
                        "delta": {"type": "boolean", "default": true},
                        "scope": {"type": "string", "enum": ["trivial","fast","standard","full","heavy"]},
                        "workflows": {"type": "string"}
                     }}},
                    {"name": "ci_status", "description": "Get CI run status",
                     "inputSchema": {"type": "object", "properties": {
                        "repo": {"type": "string"}, "run_id": {"type": "string"}, "branch": {"type": "string"}
                     }}},
                    {"name": "ci_cancel", "description": "Cancel a running CI job",
                     "inputSchema": {"type": "object", "properties": {
                        "repo": {"type": "string"}, "run_id": {"type": "string", "description": "Run ID to cancel"}
                     }, "required": ["run_id"]}},
                    {"name": "delta_plan", "description": "Analyze git diff and preview what CI would run",
                     "inputSchema": {"type": "object", "properties": {
                        "repo_root": {"type": "string", "default": "."},
                        "base_ref": {"type": "string", "default": "HEAD~1"}
                     }}},
                    {"name": "security_scan", "description": "Run Atheon security scan",
                     "inputSchema": {"type": "object", "properties": {
                        "path": {"type": "string", "default": "."},
                        "categories": {"type": "string", "default": "secrets,pii,security"}
                     }}},
                    {"name": "list_repos", "description": "List accessible repositories",
                     "inputSchema": {"type": "object", "properties": {}}},
                    {"name": "get_repo_config", "description": "Get CI configuration for a repo",
                     "inputSchema": {"type": "object", "properties": {"repo": {"type": "string"}}}},
                    {"name": "pr_create", "description": "Create a pull request",
                     "inputSchema": {"type": "object", "properties": {
                        "repo": {"type": "string"}, "title": {"type": "string"},
                        "body": {"type": "string"}, "head": {"type": "string"}, "base": {"type": "string", "default": "main"}
                     }, "required": ["title"]}},
                    {"name": "pr_merge", "description": "Merge a pull request",
                     "inputSchema": {"type": "object", "properties": {
                        "repo": {"type": "string"}, "pr_number": {"type": "number"},
                        "method": {"type": "string", "enum": ["squash","merge","rebase"], "default": "squash"}
                     }, "required": ["pr_number"]}}
                ]
            })),

            "tools/call" => {
                let tool_name = req.params.get("name")
                    .and_then(|v| v.as_str()).unwrap_or("");
                let arguments = req.params.get("arguments")
                    .and_then(|v| v.as_object()).cloned().unwrap_or_default();
                let call = crate::handlers::ToolCall { name: tool_name.to_string(), arguments };
                let result = handler.blocking_handle(call);
                JsonRpcResponse::ok(req.id.clone(), result)
            }

            "initialized" | "shutdown" => {
                JsonRpcResponse::ok(req.id, serde_json::json!({"ok": true}))
            }

            _ => {
                JsonRpcResponse::err(req.id, -32601, format!("Method not found: {}", req.method))
            }
        }
    }
}

impl McPHandler {
    fn blocking_handle(&self, call: crate::handlers::ToolCall) -> crate::tools::ToolResult {
        self.handle(call)
    }
}
