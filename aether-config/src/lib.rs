//! Configuration management for Aether

use aether_core::{types::{ToolConfig, AuthSpec, AuthType, EnhancedToolConfig}, AetherError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::info;

pub mod openapi;
pub mod generator;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AetherConfig {
    pub server: ServerConfig,
    #[serde(default)]
    pub tools: HashMap<String, ToolConfig>,
    #[serde(default)]
    pub enhanced_tools: HashMap<String, EnhancedToolConfig>,
    pub apis: HashMap<String, ApiConfig>,
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub log_level: String,
}

/// API configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub base_url: String,
    pub openapi_spec: String,
    pub timeout: Option<u64>,
    pub authentication: Option<ApiAuthentication>,
}

/// API-level authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiAuthentication {
    #[serde(rename = "type")]
    pub auth_type: String,
    pub headers: Option<HashMap<String, String>>,
}

impl ApiAuthentication {
    /// Convert API authentication config to AuthSpec
    pub fn to_auth_spec(&self) -> Result<AuthSpec> {
        let auth_type = match self.auth_type.as_str() {
            "custom" => {
                if let Some(headers) = &self.headers {
                    // Resolve environment variables in header values
                    let mut resolved_headers = HashMap::new();
                    for (key, value) in headers {
                        let resolved_value = Self::resolve_env_var(value)?;
                        resolved_headers.insert(key.clone(), resolved_value);
                    }
                    AuthType::Custom { headers: resolved_headers }
                } else {
                    return Err(AetherError::Configuration(
                        "Custom authentication requires headers".to_string()
                    ));
                }
            },
            "bearer" => AuthType::Bearer,
            "basic" => AuthType::Basic,
            _ => return Err(AetherError::Configuration(
                format!("Unsupported authentication type: {}", self.auth_type)
            )),
        };
        
        Ok(AuthSpec {
            auth_type,
            required: true,
        })
    }
    
    /// Resolve environment variables in format ${VAR_NAME}
    pub fn resolve_env_var(value: &str) -> Result<String> {
        if value.starts_with("${") && value.ends_with("}") {
            let env_var = &value[2..value.len()-1];
            std::env::var(env_var).map_err(|_| 
                AetherError::Configuration(format!("Environment variable '{}' not found", env_var))
            )
        } else {
            Ok(value.to_string())
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8080,
            log_level: "info".to_string(),
        }
    }
}

impl AetherConfig {
    /// Load configuration from YAML file
    pub async fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        info!("Loading configuration from {:?}", path.as_ref());
        
        let content = tokio::fs::read_to_string(path).await
            .map_err(|e| AetherError::Configuration(format!("Failed to read config file: {}", e)))?;
        
        let config: AetherConfig = serde_yaml::from_str(&content)
            .map_err(|e| AetherError::Configuration(format!("Failed to parse config: {}", e)))?;
        
        config.validate()?;
        info!("Configuration loaded successfully");
        Ok(config)
    }
    
    /// Validate configuration
    pub fn validate(&self) -> Result<()> {
        if self.server.host.is_empty() {
            return Err(AetherError::Configuration("Server host cannot be empty".to_string()));
        }
        
        if self.server.port == 0 {
            return Err(AetherError::Configuration("Server port must be greater than 0".to_string()));
        }
        
        for (name, api_config) in &self.apis {
            if api_config.base_url.is_empty() {
                return Err(AetherError::Configuration(format!("API '{}' base_url cannot be empty", name)));
            }
            
            if api_config.openapi_spec.is_empty() {
                return Err(AetherError::Configuration(format!("API '{}' openapi_spec cannot be empty", name)));
            }
        }
        
        Ok(())
    }
    
    /// Create a default configuration
    pub fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            tools: HashMap::new(),
            enhanced_tools: HashMap::new(),
            apis: HashMap::new(),
        }
    }
}