//! Transport-agnostic protocol types for GitForge's MCP integration.
//!
//! This crate deliberately contains no transport, filesystem, process, or
//! provider access. Adapters can depend on these types without gaining the
//! ability to execute work.

use serde::{Deserialize, Serialize};

pub mod api;
mod api_dispatcher;
mod dispatcher;

pub use api::{ApiClient, ApiClientError};
pub use api_dispatcher::ApiDispatcher;
pub use dispatcher::{dispatch, run_stdio, DispatchError};

/// JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// Build a successful response.
    pub fn success<T: Serialize>(id: serde_json::Value, result: T) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(serde_json::to_value(result).expect("serializable JSON-RPC result")),
            error: None,
        }
    }

    /// Build a failed response without exposing implementation details.
    pub fn failure(id: serde_json::Value, code: i32, message: impl Into<String>) -> Self {
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

/// MCP initialize result model.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
}

/// Capabilities exposed by the server.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct ServerCapabilities {
    pub tools: Option<ToolsCapability>,
}

/// Tool capability metadata.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct ToolsCapability {
    #[serde(rename = "listChanged", skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// MCP server identity.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// MCP tool definition.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// Result returned by `tools/list`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolsListResult {
    pub tools: Vec<ToolDefinition>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_defaults_missing_params_to_null() {
        let request: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#)
                .expect("valid request");

        assert_eq!(request.params, serde_json::Value::Null);
    }

    #[test]
    fn malformed_request_is_rejected() {
        let result = serde_json::from_str::<JsonRpcRequest>(r#"{"jsonrpc":"2.0""#);

        assert!(result.is_err());
    }

    #[test]
    fn error_response_serializes_without_result() {
        let response = JsonRpcResponse::failure(serde_json::json!(7), -32601, "not found");
        let value = serde_json::to_value(response).expect("serializable response");

        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["error"]["code"], -32601);
        assert!(value.get("result").is_none());
    }

    #[test]
    fn initialize_and_tools_models_round_trip() {
        let result = InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: Some(true),
                }),
            },
            server_info: ServerInfo {
                name: "gitforge-mcp".to_string(),
                version: "0.1.0".to_string(),
            },
        };
        let value = serde_json::to_value(&result).expect("serializable initialize result");
        let decoded: InitializeResult = serde_json::from_value(value).expect("round trip");

        assert_eq!(decoded, result);
    }
}
