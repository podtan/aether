//! Configuration management for Aether

use aether_core::{
    types::{AuthSpec, AuthType, EnhancedToolConfig, ToolConfig},
    AetherError, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::info;

pub mod directory;
pub mod generator;
pub mod openapi;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AetherConfig {
    pub server: ServerConfig,
    #[serde(default)]
    pub tools: HashMap<String, ToolConfig>,
    #[serde(default)]
    pub enhanced_tools: HashMap<String, EnhancedToolConfig>,
    pub apis: HashMap<String, ApiConfig>,
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
    #[serde(default)]
    pub session: Option<SessionConfig>,
    #[serde(default)]
    pub dev: Option<DevConfig>,
    #[serde(default)]
    pub mcp_auth: Option<McpAuthConfig>,
    #[serde(default)]
    pub pinned_tools: Vec<String>,
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

/// OIDC configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    pub provider: String,
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    #[serde(default = "default_code_challenge_method")]
    pub code_challenge_method: String,
    #[serde(default = "default_scope")]
    pub scope: String,
}

/// Session configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub secret_key: String,
    #[serde(default = "default_cookie_name")]
    pub cookie_name: String,
    #[serde(default = "default_max_age")]
    pub max_age: u64, // in seconds
}

/// Development configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevConfig {
    #[serde(default)]
    pub local_dev_mode: bool,
    #[serde(default = "default_dev_email")]
    pub local_dev_email: String,
    #[serde(default = "default_dev_role")]
    pub local_dev_role: String,
    #[serde(default = "default_dev_name")]
    pub local_dev_name: String,
}

fn default_code_challenge_method() -> String {
    "S256".to_string()
}

fn default_scope() -> String {
    "openid email profile".to_string()
}

fn default_cookie_name() -> String {
    "aether_web_session".to_string()
}

fn default_max_age() -> u64 {
    28800 // 8 hours
}

fn default_dev_email() -> String {
    "developer@example.com".to_string()
}

fn default_dev_role() -> String {
    "user".to_string()
}

fn default_dev_name() -> String {
    "Local Developer".to_string()
}

/// MCP authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpAuthConfig {
    /// Enable OAuth2.1 protection for MCP endpoints
    #[serde(default)]
    pub enabled: bool,

    /// Require authentication (reject unauthenticated requests)
    #[serde(default)]
    pub require_auth: bool,

    /// Keep health endpoint public
    #[serde(default = "default_true")]
    pub public_health: bool,

    /// Override OIDC issuer URL (optional, defaults to [oidc].issuer_url)
    pub issuer_url: Option<String>,

    /// Expected audience claim (optional, defaults to http://HOST:PORT)
    pub audience: Option<String>,
}

fn default_true() -> bool {
    true
}

impl McpAuthConfig {
    /// Validate configuration at startup
    pub fn validate(&self, oidc_config: Option<&OidcConfig>) -> Result<()> {
        if self.enabled {
            // If auth enabled, OIDC config must be present
            if oidc_config.is_none() {
                return Err(AetherError::Configuration(
                    "[mcp_auth] enabled=true requires [oidc] configuration. \
                     See docs/oauth2/CLIENTS.md for setup instructions."
                        .to_string(),
                ));
            }
        }

        if self.require_auth && !self.enabled {
            return Err(AetherError::Configuration(
                "[mcp_auth] require_auth=true requires enabled=true".to_string(),
            ));
        }

        Ok(())
    }
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
                    AuthType::Custom {
                        headers: resolved_headers,
                    }
                } else {
                    return Err(AetherError::Configuration(
                        "Custom authentication requires headers".to_string(),
                    ));
                }
            }
            "bearer" => AuthType::Bearer,
            "basic" => AuthType::Basic,
            _ => {
                return Err(AetherError::Configuration(format!(
                    "Unsupported authentication type: {}",
                    self.auth_type
                )))
            }
        };

        Ok(AuthSpec {
            auth_type,
            required: true,
        })
    }

    /// Resolve environment variables in format ${VAR_NAME}
    pub fn resolve_env_var(value: &str) -> Result<String> {
        if value.starts_with("${") && value.ends_with("}") {
            let env_var = &value[2..value.len() - 1];
            std::env::var(env_var).map_err(|_| {
                AetherError::Configuration(format!("Environment variable '{}' not found", env_var))
            })
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

impl Default for AetherConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl AetherConfig {
    /// Load configuration from YAML file
    pub async fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        info!("Loading configuration from {:?}", path.as_ref());

        let content = tokio::fs::read_to_string(path).await.map_err(|e| {
            AetherError::Configuration(format!("Failed to read config file: {}", e))
        })?;

        let config: AetherConfig = serde_yaml::from_str(&content)
            .map_err(|e| AetherError::Configuration(format!("Failed to parse config: {}", e)))?;

        config.validate()?;
        info!("Configuration loaded successfully");
        Ok(config)
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<()> {
        if self.server.host.is_empty() {
            return Err(AetherError::Configuration(
                "Server host cannot be empty".to_string(),
            ));
        }

        if self.server.port == 0 {
            return Err(AetherError::Configuration(
                "Server port must be greater than 0".to_string(),
            ));
        }

        for (name, api_config) in &self.apis {
            if api_config.base_url.is_empty() {
                return Err(AetherError::Configuration(format!(
                    "API '{}' base_url cannot be empty",
                    name
                )));
            }

            // Only require openapi_spec if there are no enhanced tools for this API
            let has_enhanced_tools_for_api = self
                .enhanced_tools
                .values()
                .any(|tool| tool.api_name == *name);

            if api_config.openapi_spec.is_empty() && !has_enhanced_tools_for_api {
                return Err(AetherError::Configuration(format!(
                    "API '{}' openapi_spec cannot be empty unless enhanced tools are provided",
                    name
                )));
            }
        }

        Ok(())
    }

    /// Create a default configuration
    pub fn new() -> Self {
        Self {
            server: ServerConfig::default(),
            tools: HashMap::new(),
            enhanced_tools: HashMap::new(),
            apis: HashMap::new(),
            oidc: None,
            session: None,
            dev: None,
            mcp_auth: None,
            pinned_tools: Vec::new(),
        }
    }

    /// Load configuration from TOML file
    pub async fn load_toml_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        info!("Loading TOML configuration from {:?}", path.as_ref());

        let content = tokio::fs::read_to_string(path).await.map_err(|e| {
            AetherError::Configuration(format!("Failed to read config file: {}", e))
        })?;

        let config: AetherConfig = toml::from_str(&content).map_err(|e| {
            AetherError::Configuration(format!("Failed to parse TOML config: {}", e))
        })?;

        config.validate()?;
        info!("TOML configuration loaded successfully");
        Ok(config)
    }

    /// Check if web authentication is configured (has OIDC, session, or dev config)
    pub fn has_web_authentication(&self) -> bool {
        self.oidc.is_some() || self.session.is_some() || self.dev.is_some()
    }
}
