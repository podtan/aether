//! Core types for Aether

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for a single tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    pub name: String,
    pub description: Option<String>,
    pub operation_id: String,
    pub auth_spec: Option<AuthSpec>,
}

/// Authentication specification for a tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSpec {
    pub auth_type: AuthType,
    pub required: bool,
}

    /// Types of authentication supported
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum AuthType {
        Bearer,
        ApiKey {
            location: ApiKeyLocation,
            name: String,
        },
        Basic,
        Custom {
            headers: HashMap<String, String>,
        },
        /// Passthrough authentication — forwards the client's Authorization header to the backend.
        Passthrough,
}

/// Location for API key authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyLocation {
    Header,
    Query,
    Body,
}

/// MCP Tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub auth_spec: Option<AuthSpec>,
}

/// MCP Tool response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResponse {
    pub content: serde_json::Value,
    pub is_error: bool,
}

/// Authorization data from MCP CallToolRequest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Authorization {
    pub auth_type: String,
    pub credentials: HashMap<String, String>,
}

/// Incoming Authorization header captured from the HTTP request.
/// Used by passthrough auth to forward the client's token to the backend.
#[derive(Debug, Clone)]
pub struct IncomingAuth {
    /// The raw value of the Authorization header (e.g. "Bearer eyJ...")
    pub header_value: String,
}

impl IncomingAuth {
    /// Create from an HTTP Authorization header value.
    pub fn new(header_value: String) -> Self {
        Self { header_value }
    }

    /// Extract the token part (strip "Bearer " prefix).
    pub fn bearer_token(&self) -> Option<&str> {
        self.header_value.strip_prefix("Bearer ")
    }
}

/// Source of parameter value (user-provided or server-provided)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ParameterSource {
    /// Parameter should be provided by the user when calling the tool
    User,
    /// Parameter should be provided by the server (from environment variables)
    Server,
}

/// Enhanced parameter definition with source metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterInfo {
    pub name: String,
    pub param_type: String,
    pub required: bool,
    pub schema: serde_json::Value,
    pub source: ParameterSource,
    pub env_var: Option<String>,
    pub description: Option<String>,
}

/// Enhanced tool configuration with parameter source information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedToolConfig {
    pub name: String,
    pub description: String,
    pub api_name: String,
    pub operation_id: String,
    pub path: String,
    pub method: String,
    pub parameters: Vec<ParameterInfo>,
    pub auth_spec: Option<AuthSpec>,
}
