//! OIDC/OAuth2.1 validation for Bearer tokens

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// User claims extracted from validated tokens
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserClaims {
    pub sub: String,
    pub email: String,
    pub name: String,
    pub preferred_username: Option<String>,
}

/// OIDC metadata from discovery
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OidcDiscoveryDocument {
    issuer: String,
    userinfo_endpoint: String,
    jwks_uri: String,
    authorization_endpoint: String,
    token_endpoint: String,
}

/// OIDC metadata used for validation
#[derive(Debug, Clone)]
pub struct OidcMetadata {
    pub issuer: String,
    pub userinfo_endpoint: String,
    pub jwks_uri: String,
}

impl From<OidcDiscoveryDocument> for OidcMetadata {
    fn from(doc: OidcDiscoveryDocument) -> Self {
        Self {
            issuer: doc.issuer,
            userinfo_endpoint: doc.userinfo_endpoint,
            jwks_uri: doc.jwks_uri,
        }
    }
}

/// OIDC validator for Bearer tokens
pub struct OidcValidator {
    issuer_url: String,
    #[allow(dead_code)]
    client_id: String,
    #[allow(dead_code)]
    client_secret: String,
    metadata: Arc<RwLock<Option<OidcMetadata>>>,
}

impl OidcValidator {
    /// Create a new OIDC validator
    pub async fn new(
        issuer_url: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Result<Self> {
        let issuer_url = issuer_url.into();
        let client_id = client_id.into();
        let client_secret = client_secret.into();

        let validator = Self {
            issuer_url: issuer_url.clone(),
            client_id,
            client_secret,
            metadata: Arc::new(RwLock::new(None)),
        };

        // Perform discovery on initialization
        validator.discover().await?;

        Ok(validator)
    }

    /// Discover OIDC metadata from issuer
    async fn discover(&self) -> Result<OidcMetadata> {
        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            self.issuer_url.trim_end_matches('/')
        );

        debug!("Discovering OIDC metadata from: {}", discovery_url);

        let client = reqwest::Client::new();
        let response = client
            .get(&discovery_url)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to fetch OIDC discovery document: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "OIDC discovery request failed: {} - {}",
                status,
                error_text
            ));
        }

        let discovery_doc: OidcDiscoveryDocument = response
            .json()
            .await
            .map_err(|e| anyhow!("Failed to parse OIDC discovery document: {}", e))?;

        let metadata: OidcMetadata = discovery_doc.into();

        info!(
            "OIDC discovery successful: issuer={}, userinfo={}",
            metadata.issuer, metadata.userinfo_endpoint
        );

        // Cache the metadata
        let mut locked_metadata = self.metadata.write().await;
        *locked_metadata = Some(metadata.clone());

        Ok(metadata)
    }

    /// Validate a Bearer token by calling the userinfo endpoint
    pub async fn validate_bearer_token(&self, token: &str) -> Result<UserClaims> {
        let metadata = self.metadata.read().await;
        let metadata = metadata
            .as_ref()
            .ok_or_else(|| anyhow!("OIDC metadata not initialized"))?;

        debug!("Validating Bearer token via userinfo endpoint");

        // Call userinfo endpoint with Bearer token
        let client = reqwest::Client::new();
        let response = client
            .get(&metadata.userinfo_endpoint)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| anyhow!("Userinfo request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            warn!("Userinfo request failed: {} - {}", status, error_text);
            return Err(anyhow!(
                "Token validation failed: {} - {}",
                status,
                error_text
            ));
        }

        let userinfo: serde_json::Value = response
            .json()
            .await
            .map_err(|e| anyhow!("Failed to parse userinfo response: {}", e))?;

        let claims = UserClaims {
            sub: userinfo["sub"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing 'sub' claim in userinfo"))?
                .to_string(),
            email: userinfo["email"]
                .as_str()
                .unwrap_or("unknown@example.com")
                .to_string(),
            name: userinfo["name"]
                .as_str()
                .or_else(|| userinfo["preferred_username"].as_str())
                .unwrap_or("Unknown User")
                .to_string(),
            preferred_username: userinfo["preferred_username"]
                .as_str()
                .map(|s| s.to_string()),
        };

        debug!("Token validated successfully: sub={}", claims.sub);

        Ok(claims)
    }

    /// Validate validator is properly configured
    pub async fn health_check(&self) -> Result<()> {
        let metadata = self.metadata.read().await;
        if metadata.is_none() {
            return Err(anyhow!("OIDC metadata not discovered"));
        }
        Ok(())
    }

    /// Refresh OIDC metadata (e.g., in case of provider changes)
    pub async fn refresh_metadata(&self) -> Result<()> {
        self.discover().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires real OIDC provider
    async fn test_oidc_discovery() {
        // This test requires a real OIDC provider
        // Set OIDC_ISSUER_URL env var to test
        if let Ok(issuer_url) = std::env::var("OIDC_ISSUER_URL") {
            let validator = OidcValidator::new(issuer_url, "test-client", "test-secret").await;

            assert!(validator.is_ok());
        }
    }

    #[tokio::test]
    async fn test_invalid_issuer() {
        let result = OidcValidator::new(
            "http://invalid-issuer-that-does-not-exist.local",
            "test-client",
            "test-secret",
        )
        .await;

        assert!(result.is_err());
    }
}
