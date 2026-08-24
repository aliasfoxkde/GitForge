//! Read-only JSON-RPC dispatcher and line-oriented stdio adapter.

use crate::{
    InitializeResult, JsonRpcRequest, JsonRpcResponse, ServerCapabilities, ServerInfo,
    ToolDefinition, ToolsCapability, ToolsListResult,
};
use std::io::{self, BufRead, Write};

const JSON_RPC_VERSION: &str = "2.0";
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "gitforge-mcp";
const SERVER_VERSION: &str = "0.1.0";

/// Errors emitted by the stdio adapter itself.
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("input error: {0}")]
    Input(#[source] io::Error),
    #[error("output error: {0}")]
    Output(#[source] io::Error),
    #[error("response serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Dispatch one validated JSON-RPC request.
///
/// The dispatcher only answers protocol discovery methods. It has no access
/// to a repository, filesystem, process, network, or provider.
pub fn dispatch(request: JsonRpcRequest) -> Option<JsonRpcResponse> {
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
                tools: Vec::<ToolDefinition>::new(),
            },
        )),
        "notifications/initialized" => None,
        _ => Some(JsonRpcResponse::failure(
            request.id,
            -32601,
            "method not found",
        )),
    }
}

/// Process newline-delimited JSON-RPC requests and write responses.
pub fn run_stdio<R: BufRead, W: Write>(input: R, mut output: W) -> Result<(), DispatchError> {
    for line in input.lines() {
        let line = line.map_err(DispatchError::Input)?;
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(request) => dispatch(request),
            Err(error) => Some(JsonRpcResponse::failure(
                serde_json::Value::Null,
                -32700,
                format!("parse error: {error}"),
            )),
        };

        if let Some(response) = response {
            serde_json::to_writer(&mut output, &response)?;
            output.write_all(b"\n").map_err(DispatchError::Output)?;
            output.flush().map_err(DispatchError::Output)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn request(method: &str) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: JSON_RPC_VERSION.to_string(),
            id: serde_json::json!(1),
            method: method.to_string(),
            params: serde_json::Value::Null,
        }
    }

    #[test]
    fn initialize_returns_server_capabilities() {
        let response = dispatch(request("initialize")).expect("response");
        let result = response.result.expect("result");

        assert_eq!(result["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], SERVER_NAME);
        assert_eq!(result["capabilities"]["tools"]["listChanged"], false);
    }

    #[test]
    fn tools_list_is_read_only_and_empty_until_tools_are_registered() {
        let response = dispatch(request("tools/list")).expect("response");
        let result = response.result.expect("result");

        assert_eq!(result["tools"], serde_json::json!([]));
    }

    #[test]
    fn unknown_method_is_a_protocol_error() {
        let response = dispatch(request("tools/call")).expect("response");
        let error = response.error.expect("error");

        assert_eq!(error.code, -32601);
    }

    #[test]
    fn invalid_jsonrpc_version_is_rejected() {
        let mut invalid = request("initialize");
        invalid.jsonrpc = "1.0".to_string();
        let response = dispatch(invalid).expect("response");

        assert_eq!(response.error.expect("error").code, -32600);
    }

    #[test]
    fn notifications_do_not_produce_a_response() {
        assert!(dispatch(request("notifications/initialized")).is_none());
    }

    #[test]
    fn stdio_reports_parse_errors_and_valid_responses() {
        let input =
            Cursor::new(b"not-json\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n");
        let mut output = Vec::new();

        run_stdio(input, &mut output).expect("stdio processing");

        let responses: Vec<serde_json::Value> = output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).expect("JSON response"))
            .collect();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["error"]["code"], -32700);
        assert_eq!(responses[1]["result"]["tools"], serde_json::json!([]));
    }
}
