//! Directory structure management for multiple API configurations

use crate::AetherConfig;
use aether_core::{AetherError, Result};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{info, error};

/// Configuration directory manager
pub struct ConfigDirectoryManager {
    base_path: PathBuf,
}

impl ConfigDirectoryManager {
    /// Create a new directory manager
    pub fn new<P: AsRef<Path>>(base_path: P) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
        }
    }

    /// Initialize the base configs directory
    pub async fn initialize(&self) -> Result<()> {
        if !self.base_path.exists() {
            info!("Creating configs directory: {:?}", self.base_path);
            fs::create_dir_all(&self.base_path).await
                .map_err(|e| AetherError::Configuration(format!("Failed to create configs directory: {}", e)))?;
        }
        Ok(())
    }

    /// Save an API configuration to its directory
    pub async fn save_api_config(
        &self,
        api_name: &str,
        config: &AetherConfig,
        openapi_content: &str,
    ) -> Result<()> {
        let api_dir = self.base_path.join(api_name);
        
        // Create API directory if it doesn't exist
        if !api_dir.exists() {
            info!("Creating directory for API '{}': {:?}", api_name, api_dir);
            fs::create_dir_all(&api_dir).await
                .map_err(|e| AetherError::Configuration(format!("Failed to create API directory: {}", e)))?;
        }

        // Save config file
        let config_path = api_dir.join("config.yaml");
        let config_yaml = serde_yaml::to_string(config)
            .map_err(|e| AetherError::Configuration(format!("Failed to serialize config: {}", e)))?;
        
        fs::write(&config_path, config_yaml).await
            .map_err(|e| AetherError::Configuration(format!("Failed to write config file: {}", e)))?;

        // Save OpenAPI spec
        let openapi_path = api_dir.join("openapi.yaml");
        fs::write(&openapi_path, openapi_content).await
            .map_err(|e| AetherError::Configuration(format!("Failed to write OpenAPI file: {}", e)))?;

        info!("Saved configuration for API '{}' in {:?}", api_name, api_dir);
        Ok(())
    }

    /// Load an API configuration from its directory
    pub async fn load_api_config(&self, api_name: &str) -> Result<(AetherConfig, String)> {
        let api_dir = self.base_path.join(api_name);
        
        if !api_dir.exists() {
            return Err(AetherError::Configuration(format!("API directory not found: {}", api_name)));
        }

        // Load config file
        let config_path = api_dir.join("config.yaml");
        let config_content = fs::read_to_string(&config_path).await
            .map_err(|e| AetherError::Configuration(format!("Failed to read config file: {}", e)))?;
        
        let config: AetherConfig = serde_yaml::from_str(&config_content)
            .map_err(|e| AetherError::Configuration(format!("Failed to parse config: {}", e)))?;

        // Load OpenAPI spec
        let openapi_path = api_dir.join("openapi.yaml");
        let openapi_content = fs::read_to_string(&openapi_path).await
            .unwrap_or_default(); // OpenAPI file is optional

        Ok((config, openapi_content))
    }

    /// List all configured APIs
    pub async fn list_apis(&self) -> Result<Vec<String>> {
        if !self.base_path.exists() {
            return Ok(Vec::new());
        }

        let mut apis = Vec::new();
        let mut entries = fs::read_dir(&self.base_path).await
            .map_err(|e| AetherError::Configuration(format!("Failed to read configs directory: {}", e)))?;

        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_type().await.map(|ft| ft.is_dir()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    // Check if this directory has a config.yaml file
                    let config_path = entry.path().join("config.yaml");
                    if config_path.exists() {
                        apis.push(name.to_string());
                    }
                }
            }
        }

        apis.sort();
        Ok(apis)
    }

    /// Delete an API configuration directory
    pub async fn delete_api_config(&self, api_name: &str) -> Result<()> {
        let api_dir = self.base_path.join(api_name);
        
        if api_dir.exists() {
            fs::remove_dir_all(&api_dir).await
                .map_err(|e| AetherError::Configuration(format!("Failed to delete API directory: {}", e)))?;
            info!("Deleted configuration directory for API '{}'", api_name);
        }

        Ok(())
    }

    /// Load all API configurations from the directory
    pub async fn load_all_configs(&self) -> Result<HashMap<String, AetherConfig>> {
        let mut configs = HashMap::new();
        let api_names = self.list_apis().await?;

        for api_name in api_names {
            match self.load_api_config(&api_name).await {
                Ok((config, _)) => {
                    configs.insert(api_name.clone(), config);
                }
                Err(e) => {
                    error!("Failed to load config for API '{}': {}", api_name, e);
                    // Continue loading other configs
                }
            }
        }

        info!("Loaded {} API configurations", configs.len());
        Ok(configs)
    }

    /// Get the path for an API directory
    pub fn get_api_path(&self, api_name: &str) -> PathBuf {
        self.base_path.join(api_name)
    }

    /// Get the base configs directory path
    pub fn get_base_path(&self) -> &Path {
        &self.base_path
    }
}

use std::collections::HashMap;