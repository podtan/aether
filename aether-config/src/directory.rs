//! Directory structure management for multiple API configurations
//!
//! Per-API directory layout (v2 — spec-driven):
//!
//! ```text
//! configs/<api>/
//!   <api>-openapi.yaml   # machine: OpenAPI spec (curl'd from the service)
//!   config.yaml          # human: server, apis, oidc, mcp_auth, prm, ...
//! ```
//!
//! `config.yaml` is purely hand-curated; `enhanced_tools` are derived from
//! the sibling OpenAPI spec at load time (in-memory).  Spec discovery by
//! convention: `<api>-openapi.{yaml,yml,json}` first, falling back to
//! `openapi.{yaml,yml,json}` for pre-rename directories.  When a spec is
//! present the loader regenerates the tools through the same generator the
//! CLI uses — including the no-unresolved-`$ref` assertion — so bad specs
//! fail loudly at boot rather than poisoning MCP tool schemas downstream.

use crate::generator::generate_config_from_openapi;
use crate::AetherConfig;
use aether_core::{AetherError, Result};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{error, info, warn};

/// Candidate OpenAPI spec filenames, in preference order.
const SPEC_FILENAME_PATTERNS: [&str; 6] = [
    "{api}-openapi.yaml",
    "{api}-openapi.yml",
    "{api}-openapi.json",
    "openapi.yaml",
    "openapi.yml",
    "openapi.json",
];

/// Find the OpenAPI spec file for an API directory, by convention.
async fn find_spec_file(api_dir: &Path, api_name: &str) -> Option<PathBuf> {
    for pattern in SPEC_FILENAME_PATTERNS {
        let filename = pattern.replace("{api}", api_name);
        let candidate = api_dir.join(&filename);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

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
            fs::create_dir_all(&self.base_path).await.map_err(|e| {
                AetherError::Configuration(format!("Failed to create configs directory: {}", e))
            })?;
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
            fs::create_dir_all(&api_dir).await.map_err(|e| {
                AetherError::Configuration(format!("Failed to create API directory: {}", e))
            })?;
        }

        // Save config file
        let config_path = api_dir.join("config.yaml");
        let config_yaml = serde_yaml::to_string(config).map_err(|e| {
            AetherError::Configuration(format!("Failed to serialize config: {}", e))
        })?;

        fs::write(&config_path, config_yaml).await.map_err(|e| {
            AetherError::Configuration(format!("Failed to write config file: {}", e))
        })?;

        // Save OpenAPI spec under the conventional name so the loader's
        // spec-discovery finds it (preferred over the legacy "openapi.yaml").
        let openapi_path = api_dir.join(format!("{}-openapi.yaml", api_name));
        fs::write(&openapi_path, openapi_content)
            .await
            .map_err(|e| {
                AetherError::Configuration(format!("Failed to write OpenAPI file: {}", e))
            })?;

        info!(
            "Saved configuration for API '{}' in {:?}",
            api_name, api_dir
        );
        Ok(())
    }

    /// Load an API configuration from its directory
    ///
    /// Parses the hand-curated `config.yaml`, then — when a sibling OpenAPI
    /// spec exists — regenerates `enhanced_tools` from it in-memory.  This
    /// keeps generated tool definitions out of the hand-maintained file
    /// (no clobbering on regeneration) while remaining a drop-in for
    /// configs that still carry inline `enhanced_tools`.
    pub async fn load_api_config(&self, api_name: &str) -> Result<(AetherConfig, String)> {
        let api_dir = self.base_path.join(api_name);

        if !api_dir.exists() {
            return Err(AetherError::Configuration(format!(
                "API directory not found: {}",
                api_name
            )));
        }

        // Load config file
        let config_path = api_dir.join("config.yaml");
        let config_content = fs::read_to_string(&config_path).await.map_err(|e| {
            AetherError::Configuration(format!("Failed to read config file: {}", e))
        })?;

        let mut config: AetherConfig = serde_yaml::from_str(&config_content).map_err(|e| {
            AetherError::Configuration(format!(
                "Failed to parse config for '{}': {}",
                api_name, e
            ))
        })?;

        // Generation api_name: prefer the apis key from the hand-curated
        // config over the directory name.  Directory names sometimes carry
        // deployment suffixes (e.g. "nghr_auth") that must NOT leak into
        // tool prefixes (tools are "nghr_*", not "nghr_auth_*").  Falls
        // back to the directory name when no apis block is declared.
        let gen_api_name = config
            .apis
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| api_name.to_string());
        if &gen_api_name != api_name {
            info!(
                "API '{}': generating tools with api_name '{}' (from apis key, dir name differs)",
                api_name, gen_api_name
            );
        }

        // Locate sibling OpenAPI spec (by convention) and derive tools.
        match find_spec_file(&api_dir, &gen_api_name).await {
            Some(spec_path) => {
                let openapi_content = fs::read_to_string(&spec_path).await.map_err(|e| {
                    AetherError::Configuration(format!(
                        "Failed to read OpenAPI spec {}: {}",
                        spec_path.display(),
                        e
                    ))
                })?;

                // base_url only feeds the generated apis block, which we
                // discard when the hand-curated config declares its own —
                // so an empty placeholder is fine here.
                let generated =
                    generate_config_from_openapi(&openapi_content, &gen_api_name, "").await?;

                if !config.enhanced_tools.is_empty() {
                    info!(
                        "API '{}': deriving {} tools from {} (ignoring {} inline enhanced_tools)",
                        api_name,
                        generated.enhanced_tools.len(),
                        spec_path.display(),
                        config.enhanced_tools.len()
                    );
                } else {
                    info!(
                        "API '{}': derived {} tools from {}",
                        api_name,
                        generated.enhanced_tools.len(),
                        spec_path.display()
                    );
                }
                config.enhanced_tools = generated.enhanced_tools;
                config.tools = std::collections::HashMap::new();

                // Keep the hand-curated apis block (base_url, auth, timeout);
                // fall back to the generated one if the config omitted it.
                if config.apis.is_empty() {
                    config.apis = generated.apis;
                }

                Ok((config, openapi_content))
            }
            None => {
                // No spec file: legacy layout.  config.yaml must carry its
                // own enhanced_tools (checked by AetherConfig::validate).
                if config.enhanced_tools.is_empty() && !config.apis.is_empty() {
                    warn!(
                        "API '{}': no OpenAPI spec found in {} and config.yaml has no \
                         enhanced_tools — tools will be empty",
                        api_name,
                        api_dir.display()
                    );
                }
                Ok((config, String::new()))
            }
        }
    }

    /// List all configured APIs
    pub async fn list_apis(&self) -> Result<Vec<String>> {
        if !self.base_path.exists() {
            return Ok(Vec::new());
        }

        let mut apis = Vec::new();
        let mut entries = fs::read_dir(&self.base_path).await.map_err(|e| {
            AetherError::Configuration(format!("Failed to read configs directory: {}", e))
        })?;

        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry
                .file_type()
                .await
                .map(|ft| ft.is_dir())
                .unwrap_or(false)
            {
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
            fs::remove_dir_all(&api_dir).await.map_err(|e| {
                AetherError::Configuration(format!("Failed to delete API directory: {}", e))
            })?;
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
