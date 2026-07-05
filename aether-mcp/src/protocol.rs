//! MCP protocol message handlers

use aether_core::{
    types::{Authorization, Tool, ToolResponse},
    AetherError,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// MCP Request message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
    /// Forwarded Authorization header from the incoming client request.
    /// Set by the auth middleware when `type: "passthrough"` is used.
    #[serde(skip)]
    pub forwarded_authorization: Option<String>,
}

impl McpRequest {
    /// Returns true if this request is a JSON-RPC notification (no `id` field
    /// and method starts with `notifications/`). Per the MCP Streamable HTTP
    /// spec, notifications should receive a `202 Accepted` response with no body.
    pub fn is_notification(&self) -> bool {
        self.id.is_none() && self.method.starts_with("notifications/")
    }
}

/// MCP Response message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<McpError>,
}

/// MCP Error object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// ListToolsRequest parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// CallToolRequest parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<HashMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorization: Option<Authorization>,
    /// Incoming Bearer token from the HTTP Authorization header (set by transport layer).
    /// Used by `passthrough` auth to relay the client's token to the backend.
    #[serde(skip)]
    pub incoming_bearer: Option<String>,
}

impl McpResponse {
    /// Create a successful response
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Create an error response
    pub fn error(id: Option<Value>, error: McpError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

impl McpError {
    /// Create an error from AetherError
    pub fn from_aether_error(error: &AetherError) -> Self {
        let (code, message) = match error {
            AetherError::Configuration(_) => (-32602, error.to_string()),
            AetherError::Api(_) => (-32603, error.to_string()),
            AetherError::Mcp(_) => (-32600, error.to_string()),
            AetherError::Authentication(_) => (-32001, error.to_string()),
            _ => (-32603, error.to_string()),
        };

        Self {
            code,
            message,
            data: None,
        }
    }

    /// Create a method not found error
    pub fn method_not_found() -> Self {
        Self {
            code: -32601,
            message: "Method not found".to_string(),
            data: None,
        }
    }

    /// Create an invalid params error
    pub fn invalid_params(message: &str) -> Self {
        Self {
            code: -32602,
            message: format!("Invalid params: {}", message),
            data: None,
        }
    }
}

/// Convert tools list to MCP response format
pub fn tools_to_response(tools: Vec<Tool>) -> Value {
    let tools_array: Vec<Value> = tools
        .into_iter()
        .map(|tool| {
            let mut tool_obj = Map::new();
            tool_obj.insert("name".to_string(), Value::String(tool.name));
            tool_obj.insert("description".to_string(), Value::String(tool.description));
            tool_obj.insert("inputSchema".to_string(), tool.parameters);

            if let Some(auth_spec) = tool.auth_spec {
                tool_obj.insert(
                    "authSpec".to_string(),
                    serde_json::to_value(auth_spec).unwrap_or(Value::Null),
                );
            }

            Value::Object(tool_obj)
        })
        .collect();

    let mut result = Map::new();
    result.insert("tools".to_string(), Value::Array(tools_array));

    Value::Object(result)
}

/// Convert ToolResponse to MCP response format
pub fn tool_response_to_mcp(response: ToolResponse) -> Value {
    let mut result = Map::new();

    if response.is_error {
        result.insert("isError".to_string(), Value::Bool(true));
    }

    // Convert content to MCP content format (array of content objects)
    let content_array = vec![serde_json::json!({
        "type": "text",
        "text": serde_json::to_string_pretty(&response.content).unwrap_or_else(|_| response.content.to_string())
    })];

    result.insert("content".to_string(), Value::Array(content_array));

    Value::Object(result)
}
