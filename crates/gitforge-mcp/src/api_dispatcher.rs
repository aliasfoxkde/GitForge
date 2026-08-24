//! Authenticated read-only MCP tools backed by the GitForge API.

use crate::{
    api::ApiClient, InitializeResult, JsonRpcRequest, JsonRpcResponse, ServerCapabilities,
    ServerInfo, ToolDefinition, ToolsCapability, ToolsListResult,
};

const JSON_RPC_VERSION: &str = "2.0";
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "gitforge-mcp";
const SERVER_VERSION: &str = "0.1.0";

/// Dispatcher for explicitly allowlisted, read-only API tools.
pub struct ApiDispatcher {
    client: ApiClient,
}

impl ApiDispatcher {
    /// Create a dispatcher around a restricted API client.
    pub fn new(client: ApiClient) -> Self {
        Self { client }
    }

    /// Dispatch one request. API failures are intentionally redacted.
    pub async fn dispatch(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        if request.jsonrpc != JSON_RPC_VERSION {
            return Some(JsonRpcResponse::failure(
                request.id,
                -32600,
                "invalid Request",
            ));
        }

        match request.method.as_str() {
            "initialize" => Some(JsonRpcResponse::success(
                request.id,
                InitializeResult {
                    protocol_version: MCP_PROTOCOL_VERSION.to_string(),
                    capabilities: ServerCapabilities {
                        tools: Some(ToolsCapability {
                            list_changed: Some(false),
                        }),
                    },
                    server_info: ServerInfo {
                        name: SERVER_NAME.to_string(),
                        version: SERVER_VERSION.to_string(),
                    },
                },
            )),
            "tools/list" => Some(JsonRpcResponse::success(
                request.id,
                ToolsListResult {
                    tools: tool_definitions(),
                },
            )),
            "notifications/initialized" => None,
            "tools/call" => Some(self.call_tool(request).await),
            _ => Some(JsonRpcResponse::failure(
                request.id,
                -32601,
                "method not found",
            )),
        }
    }

    async fn call_tool(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let id = request.id;
        let Some(name) = request.params.get("name").and_then(|value| value.as_str()) else {
            return JsonRpcResponse::failure(id, -32602, "tool name is required");
        };

        let result = match name {
            "gitforge.health" => self.client.health().await,
            "gitforge.repositories.list" => self.client.repositories().await,
            "gitforge.pipelines.list" => self.client.pipelines().await,
            "gitforge.pipeline_runs.list" => self.client.pipeline_runs().await,
            _ => return JsonRpcResponse::failure(id, -32602, "unknown read-only tool"),
        };

        match result {
            Ok(value) => JsonRpcResponse::success(id, value),
            Err(_) => JsonRpcResponse::failure(id, -32001, "GitForge API request failed"),
        }
    }
}

fn tool_definitions() -> Vec<ToolDefinition> {
    [
        ("gitforge.health", "Read public GitForge health status."),
        (
            "gitforge.repositories.list",
            "List repositories visible to the authenticated principal.",
        ),
        (
            "gitforge.pipelines.list",
            "List pipelines visible to the authenticated principal.",
        ),
        (
            "gitforge.pipeline_runs.list",
            "List pipeline runs visible to the authenticated principal.",
        ),
    ]
    .into_iter()
    .map(|(name, description)| ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "additionalProperties": false
        }),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ApiClient;
    use std::time::Duration;

    fn request(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: JSON_RPC_VERSION.to_string(),
            id: serde_json::json!(1),
            method: method.to_string(),
            params,
        }
    }

    #[tokio::test]
    async fn tools_list_exposes_only_read_only_tools() {
        let server = mockito::Server::new_async().await;
        let client = ApiClient::new(
            &server.url(),
            None,
            &["127.0.0.1".to_string()],
            Duration::from_secs(1),
        )
        .expect("valid client");
        let dispatcher = ApiDispatcher::new(client);

        let response = dispatcher
            .dispatch(request("tools/list", serde_json::Value::Null))
            .await
            .expect("response");
        let tools = response.result.expect("result")["tools"]
            .as_array()
            .expect("tools array")
            .clone();

        assert_eq!(tools.len(), 4);
        assert!(tools.iter().all(|tool| tool["name"]
            .as_str()
            .is_some_and(|name| name.starts_with("gitforge."))));
    }

    #[tokio::test]
    async fn health_tool_returns_api_value() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/health")
            .with_status(200)
            .with_body(r#"{"status":"healthy"}"#)
            .create_async()
            .await;
        let dispatcher = ApiDispatcher::new(
            ApiClient::new(
                &server.url(),
                None,
                &["127.0.0.1".to_string()],
                Duration::from_secs(1),
            )
            .expect("valid client"),
        );

        let response = dispatcher
            .dispatch(request(
                "tools/call",
                serde_json::json!({"name":"gitforge.health","arguments":{}}),
            ))
            .await
            .expect("response");

        assert_eq!(response.result.expect("result")["status"], "healthy");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn upstream_failure_is_redacted() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/health")
            .with_status(503)
            .with_body("private upstream details")
            .create_async()
            .await;
        let dispatcher = ApiDispatcher::new(
            ApiClient::new(
                &server.url(),
                None,
                &["127.0.0.1".to_string()],
                Duration::from_secs(1),
            )
            .expect("valid client"),
        );

        let response = dispatcher
            .dispatch(request(
                "tools/call",
                serde_json::json!({"name":"gitforge.health"}),
            ))
            .await
            .expect("response");
        let error = response.error.expect("error");

        assert_eq!(error.code, -32001);
        assert!(!error.message.contains("private"));
        mock.assert_async().await;
    }
}
