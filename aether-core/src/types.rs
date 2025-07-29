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
    ApiKey { location: ApiKeyLocation, name: String },
    Basic,
    Custom { headers: HashMap<String, String> },
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