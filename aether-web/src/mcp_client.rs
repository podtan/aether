//! HTTP client for calling MCP admin endpoints

use aether_config::AetherConfig;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, error, info};

/// HTTP client for MCP admin API calls
#[derive(Clone)]
pub struct McpClient {
    client: Client,
    base_url: String,
}

/// Request payload for OpenAPI upload
#[derive(Debug, Serialize)]
pub struct UploadRequest {
    pub api_name: String,
    pub base_url: String,
    pub openapi_content: String,
}

/// Response from MCP admin API calls
#[derive(Debug, Deserialize)]
pub struct McpResponse {
    pub success: bool,
    pub message: Option<String>,
    pub error: Option<String>,
    pub api_name: Option<String>,
}

/// Response for list APIs call
#[derive(Debug, Deserialize)]
pub struct ListApisResponse {
    pub apis: Vec<String>,
}

/// Response for get config call
#[derive(Debug, Deserialize)]
pub struct GetConfigResponse {
    pub api_name: String,
    pub config: AetherConfig,
    pub openapi_content: String,
}

/// Request for updating config
#[derive(Debug, Serialize)]
pub struct UpdateConfigRequest {
    pub config: AetherConfig,
    pub openapi_content: Option<String>,
}

impl McpClient {
    /// Create a new MCP client
    pub fn new(base_url: &str) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Upload OpenAPI spec and configure API
    pub async fn upload_openapi(
        &self,
        api_name: &str,
        base_url: &str,
        openapi_content: &str,
    ) -> Result<McpResponse, anyhow::Error> {
        let url = format!("{}/api/upload-openapi", self.base_url);
        let request = UploadRequest {
            api_name: api_name.to_string(),
            base_url: base_url.to_string(),
            openapi_content: openapi_content.to_string(),
        };

        info!("Calling MCP upload API: {} for API: {}", url, api_name);
        debug!(
            "Request payload: {} bytes",
            serde_json::to_string(&request)?.len()
        );

        let response = self.client.post(&url).json(&request).send().await?;

        let status = response.status();
        let response_text = response.text().await?;

        debug!("MCP upload response status: {}", status);
        debug!("MCP upload response body: {}", response_text);

        if status.is_success() {
            let mcp_response: McpResponse = serde_json::from_str(&response_text)?;
            info!("MCP upload successful for API: {}", api_name);
            Ok(mcp_response)
        } else {
            error!(
                "MCP upload failed with status {}: {}",
                status, response_text
            );
            Err(anyhow::anyhow!(
                "Upload failed: HTTP {} - {}",
                status,
                response_text
            ))
        }
    }

    /// List all configured APIs
    pub async fn list_apis(&self) -> Result<Vec<String>, anyhow::Error> {
        let url = format!("{}/api/list", self.base_url);

        debug!("Calling MCP list APIs: {}", url);

        let response = self.client.get(&url).send().await?;

        let status = response.status();
        let response_text = response.text().await?;

        debug!("MCP list response status: {}", status);
        debug!("MCP list response body: {}", response_text);

        if status.is_success() {
            let list_response: ListApisResponse = serde_json::from_str(&response_text)?;
            debug!("Found {} APIs from MCP", list_response.apis.len());
            Ok(list_response.apis)
        } else {
            error!("MCP list failed with status {}: {}", status, response_text);
            Err(anyhow::anyhow!(
                "List APIs failed: HTTP {} - {}",
                status,
                response_text
            ))
        }
    }

    /// Get configuration for a specific API
    pub async fn get_config(&self, api_name: &str) -> Result<GetConfigResponse, anyhow::Error> {
        let url = format!("{}/api/{}/config", self.base_url, api_name);

        debug!("Calling MCP get config: {}", url);

        let response = self.client.get(&url).send().await?;

        let status = response.status();
        let response_text = response.text().await?;

        debug!("MCP get config response status: {}", status);
        debug!("MCP get config response body: {}", response_text);

        if status.is_success() {
            let config_response: GetConfigResponse = serde_json::from_str(&response_text)?;
            debug!("Retrieved config for API: {}", api_name);
            Ok(config_response)
        } else {
            error!(
                "MCP get config failed with status {}: {}",
                status, response_text
            );
            Err(anyhow::anyhow!(
                "Get config failed: HTTP {} - {}",
                status,
                response_text
            ))
        }
    }

    /// Update configuration for a specific API
    pub async fn update_config(
        &self,
        api_name: &str,
        config: AetherConfig,
        openapi_content: Option<&str>,
    ) -> Result<McpResponse, anyhow::Error> {
        let url = format!("{}/api/{}/config", self.base_url, api_name);
        let request = UpdateConfigRequest {
            config,
            openapi_content: openapi_content.map(|s| s.to_string()),
        };

        debug!("Calling MCP update config: {}", url);

        let response = self.client.post(&url).json(&request).send().await?;

        let status = response.status();
        let response_text = response.text().await?;

        debug!("MCP update config response status: {}", status);
        debug!("MCP update config response body: {}", response_text);

        if status.is_success() {
            let mcp_response: McpResponse = serde_json::from_str(&response_text)?;
            info!("MCP update config successful for API: {}", api_name);
            Ok(mcp_response)
        } else {
            error!(
                "MCP update config failed with status {}: {}",
                status, response_text
            );
            Err(anyhow::anyhow!(
                "Update config failed: HTTP {} - {}",
                status,
                response_text
            ))
        }
    }

    /// Delete an API configuration
    pub async fn delete_api(&self, api_name: &str) -> Result<McpResponse, anyhow::Error> {
        let url = format!("{}/api/{}/delete", self.base_url, api_name);

        debug!("Calling MCP delete API: {}", url);

        let response = self.client.post(&url).send().await?;

        let status = response.status();
        let response_text = response.text().await?;

        debug!("MCP delete response status: {}", status);
        debug!("MCP delete response body: {}", response_text);

        if status.is_success() {
            let mcp_response: McpResponse = serde_json::from_str(&response_text)?;
            info!("MCP delete successful for API: {}", api_name);
            Ok(mcp_response)
        } else {
            error!(
                "MCP delete failed with status {}: {}",
                status, response_text
            );
            Err(anyhow::anyhow!(
                "Delete API failed: HTTP {} - {}",
                status,
                response_text
            ))
        }
    }

    /// Health check on MCP service
    pub async fn health_check(&self) -> Result<HashMap<String, serde_json::Value>, anyhow::Error> {
        let url = format!("{}/health", self.base_url);

        debug!("Calling MCP health check: {}", url);

        let response = self.client.get(&url).send().await?;

        let status = response.status();
        let response_text = response.text().await?;

        debug!("MCP health response status: {}", status);
        debug!("MCP health response body: {}", response_text);

        if status.is_success() {
            let health_response: HashMap<String, serde_json::Value> =
                serde_json::from_str(&response_text)?;
            debug!("MCP health check successful");
            Ok(health_response)
        } else {
            error!(
                "MCP health check failed with status {}: {}",
                status, response_text
            );
            Err(anyhow::anyhow!(
                "Health check failed: HTTP {} - {}",
                status,
                response_text
            ))
        }
    }
}
