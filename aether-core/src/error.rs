//! Error types for Aether

use thiserror::Error;

/// Main error type for Aether
#[derive(Error, Debug)]
pub enum AetherError {
    #[error("Configuration error: {0}")]
    Configuration(String),
    
    #[error("API error: {0}")]
    Api(String),
    
    #[error("MCP protocol error: {0}")]
    Mcp(String),
    
    #[error("Authentication error: {0}")]
    Authentication(String),
    
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Other error: {0}")]
    Other(String),
}

/// Result type alias for Aether operations
pub type Result<T> = std::result::Result<T, AetherError>;