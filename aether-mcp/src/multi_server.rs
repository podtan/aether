//! Multi-configuration server manager
//!
//! Manages multiple API configurations and serves them at different endpoints

use crate::McpServer;
use aether_config::{directory::ConfigDirectoryManager, AetherConfig};
use aether_core::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

/// Multi-configuration server manager
#[derive(Clone)]
pub struct MultiConfigServer {
    /// Individual MCP servers by API name
    servers: Arc<RwLock<HashMap<String, McpServer>>>,
    /// Directory manager for configurations
    directory_manager: Arc<ConfigDirectoryManager>,
}

impl MultiConfigServer {
    /// Create a new multi-config server
    pub async fn new<P: AsRef<Path>>(configs_dir: P) -> Result<Self> {
        let directory_manager = Arc::new(ConfigDirectoryManager::new(configs_dir));
        directory_manager.initialize().await?;

        let server = Self {
            servers: Arc::new(RwLock::new(HashMap::new())),
            directory_manager,
        };

        // Load all existing configurations
        server.reload_all_configurations().await?;

        Ok(server)
    }

    /// Add a new API configuration
    pub async fn add_api_config(
        &self,
        api_name: &str,
        config: AetherConfig,
        openapi_content: &str,
    ) -> Result<()> {
        // Save configuration to disk
        self.directory_manager
            .save_api_config(api_name, &config, openapi_content)
            .await?;

        // Create MCP server for this API
        let mcp_server = McpServer::new(config).await?;

        // Add to our servers map
        self.servers
            .write()
            .await
            .insert(api_name.to_string(), mcp_server);

        info!("Added API configuration: {}", api_name);
        Ok(())
    }

    /// Get MCP server for a specific API
    pub async fn get_server(&self, api_name: &str) -> Option<McpServer> {
        self.servers.read().await.get(api_name).cloned()
    }

    /// List all configured APIs
    pub async fn list_apis(&self) -> Vec<String> {
        let servers = self.servers.read().await;
        servers.keys().cloned().collect()
    }

    /// Remove an API configuration
    pub async fn remove_api_config(&self, api_name: &str) -> Result<()> {
        // Remove from disk
        self.directory_manager.delete_api_config(api_name).await?;

        // Remove from memory
        self.servers.write().await.remove(api_name);

        info!("Removed API configuration: {}", api_name);
        Ok(())
    }

    /// Update an existing API configuration
    pub async fn update_api_config(
        &self,
        api_name: &str,
        config: AetherConfig,
        openapi_content: &str,
    ) -> Result<()> {
        // Update on disk
        self.directory_manager
            .save_api_config(api_name, &config, openapi_content)
            .await?;

        // Update in memory
        let mcp_server = McpServer::new(config).await?;
        self.servers
            .write()
            .await
            .insert(api_name.to_string(), mcp_server);

        info!("Updated API configuration: {}", api_name);
        Ok(())
    }

    /// Reload all configurations from disk
    pub async fn reload_all_configurations(&self) -> Result<()> {
        info!("Reloading all API configurations from disk");

        let configs = self.directory_manager.load_all_configs().await?;
        let mut servers = self.servers.write().await;
        servers.clear();

        for (api_name, config) in configs {
            match McpServer::new(config).await {
                Ok(mcp_server) => {
                    servers.insert(api_name.clone(), mcp_server);
                    info!("Loaded MCP server for API: {}", api_name);
                }
                Err(e) => {
                    error!("Failed to create MCP server for API '{}': {}", api_name, e);
                    // Continue loading other servers
                }
            }
        }

        info!("Loaded {} MCP servers", servers.len());
        Ok(())
    }

    /// Get all server statistics
    pub async fn get_all_stats(&self) -> HashMap<String, HashMap<String, serde_json::Value>> {
        let mut all_stats = HashMap::new();
        let servers = self.servers.read().await;

        for (api_name, server) in servers.iter() {
            let stats = server.get_stats().await;
            all_stats.insert(api_name.clone(), stats);
        }

        all_stats
    }

    /// Get the PRM config for a specific API
    pub async fn get_prm_config(&self, api_name: &str) -> Option<aether_config::PrmConfig> {
        let servers = self.servers.read().await;
        if let Some(server) = servers.get(api_name) {
            let config = server.get_config().await;
            config.prm.clone()
        } else {
            None
        }
    }

    /// Get API configuration for editing
    pub async fn get_api_config(&self, api_name: &str) -> Result<(AetherConfig, String)> {
        self.directory_manager.load_api_config(api_name).await
    }
}
