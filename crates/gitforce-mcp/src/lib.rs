//! GitForge MCP Server
//!
//! Exposes GitForge CI/CD tools to Claude Code CLI via the Model Context Protocol.
//! Uses JSON-RPC over stdio as the transport layer.
//!
//! # Protocol
//!
//! Claude Code CLI sends JSON-RPC requests to stdin. This server responds on stdout.
//! stderr is used for server-side logging only.
//!
//! # Available Tools
//!
//! - `ci_run` — Trigger a CI run with optional delta analysis
//! - `ci_status` — Get the status of a CI run
//! - `ci_cancel` — Cancel a running CI job
//! - `delta_plan` — Analyze changed files and preview what CI would run
//! - `security_scan` — Run Atheon security scan on a directory
//! - `list_repos` — List repos accessible to the authenticated user
//! - `get_repo_config` — Get CI configuration for a repo

pub mod handlers;
pub mod server;
pub mod tools;

pub use handlers::{McPHandler, ToolCall};
pub use server::McpServer;
pub use tools::{CiRunOptions, CiStatus, DeltaResult, ToolResult};

use serde::{Deserialize, Serialize};

/// JSON-RPC request envelope
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC response envelope
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    pub fn ok<T: Serialize>(id: serde_json::Value, result: T) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(serde_json::to_value(result).unwrap()),
            error: None,
        }
    }

    pub fn err(id: serde_json::Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}
