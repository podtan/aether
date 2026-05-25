//! Authentication handling for Aether

pub mod oidc;

use aether_core::{
    types::{ApiKeyLocation, AuthSpec, AuthType, Authorization},
    AetherError, Result,
};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use tracing::{debug, warn};

// Re-export OIDC types for convenience
pub use oidc::{OidcValidator, UserClaims};

/// Authentication processor for applying auth to HTTP requests
pub struct AuthProcessor;

impl AuthProcessor {
    /// Apply authentication to a request builder
    pub fn apply_auth(
        request: reqwest::RequestBuilder,
        auth_spec: &AuthSpec,
        authorization: &Authorization,
    ) -> Result<reqwest::RequestBuilder> {
        if !auth_spec.required && authorization.credentials.is_empty() {
            debug!("Authentication not required and no credentials provided");
            return Ok(request);
        }

        if auth_spec.required && authorization.credentials.is_empty() {
            return Err(AetherError::Authentication(
                "Authentication required but no credentials provided".to_string(),
            ));
        }

        match &auth_spec.auth_type {
            AuthType::Bearer => Self::apply_bearer_auth(request, authorization),
            AuthType::ApiKey { location, name } => {
                Self::apply_api_key_auth(request, authorization, location, name)
            }
            AuthType::Basic => Self::apply_basic_auth(request, authorization),
            AuthType::Custom { headers: _ } => Self::apply_custom_auth(request, authorization),
            AuthType::Passthrough => Self::apply_passthrough_auth(request, authorization),
        }
    }

    /// Apply Bearer token authentication
    fn apply_bearer_auth(
        request: reqwest::RequestBuilder,
        authorization: &Authorization,
    ) -> Result<reqwest::RequestBuilder> {
        let token = authorization.credentials.get("token").ok_or_else(|| {
            AetherError::Authentication("Bearer token not found in credentials".to_string())
        })?;

        debug!("Applying Bearer token authentication");
        Ok(request.header(AUTHORIZATION, format!("Bearer {}", token)))
    }

    /// Apply API key authentication
    fn apply_api_key_auth(
        request: reqwest::RequestBuilder,
        authorization: &Authorization,
        location: &ApiKeyLocation,
        key_name: &str,
    ) -> Result<reqwest::RequestBuilder> {
        let api_key = authorization
            .credentials
            .get("api_key")
            .or_else(|| authorization.credentials.get(key_name))
            .ok_or_else(|| {
                AetherError::Authentication("API key not found in credentials".to_string())
            })?;

        debug!("Applying API key authentication in {:?}", location);

        match location {
            ApiKeyLocation::Header => Ok(request.header(key_name, api_key)),
            ApiKeyLocation::Query => Ok(request.query(&[(key_name, api_key)])),
            ApiKeyLocation::Body => {
                // For body location, we'll add it to form data
                // This is a simplified implementation
                warn!("API key in body location is not fully implemented");
                Ok(request)
            }
        }
    }

    /// Apply Basic authentication
    fn apply_basic_auth(
        request: reqwest::RequestBuilder,
        authorization: &Authorization,
    ) -> Result<reqwest::RequestBuilder> {
        let username = authorization.credentials.get("username").ok_or_else(|| {
            AetherError::Authentication("Username not found in credentials".to_string())
        })?;

        let password = authorization.credentials.get("password").ok_or_else(|| {
            AetherError::Authentication("Password not found in credentials".to_string())
        })?;

        debug!("Applying Basic authentication");
        Ok(request.basic_auth(username, Some(password)))
    }

    /// Apply custom authentication
    fn apply_custom_auth(
        request: reqwest::RequestBuilder,
        authorization: &Authorization,
    ) -> Result<reqwest::RequestBuilder> {
        debug!("Applying custom authentication");

        let mut headers = HeaderMap::new();

        for (key, value) in &authorization.credentials {
            if let Ok(header_value) = HeaderValue::from_str(value) {
                if let Ok(header_name) = key.parse::<reqwest::header::HeaderName>() {
                    headers.insert(header_name, header_value);
                } else {
                    warn!("Invalid header name: {}", key);
                }
            } else {
                warn!("Invalid header value for key: {}", key);
            }
        }

        Ok(request.headers(headers))
    }

    /// Apply passthrough authentication — forward the incoming Authorization header
    fn apply_passthrough_auth(
        request: reqwest::RequestBuilder,
        authorization: &Authorization,
    ) -> Result<reqwest::RequestBuilder> {
        let token = authorization.credentials.get("token").ok_or_else(|| {
            AetherError::Authentication("Passthrough: no token in credentials".to_string())
        })?;

        debug!("Applying passthrough authentication (forwarding token)");
        Ok(request.header(AUTHORIZATION, format!("Bearer {}", token)))
    }

    /// Validate that credentials match the auth spec requirements
    pub fn validate_credentials(auth_spec: &AuthSpec, authorization: &Authorization) -> Result<()> {
        if !auth_spec.required {
            return Ok(());
        }

        match &auth_spec.auth_type {
            AuthType::Bearer => {
                if !authorization.credentials.contains_key("token") {
                    return Err(AetherError::Authentication(
                        "Bearer token required but not provided".to_string(),
                    ));
                }
            }
            AuthType::ApiKey { name, .. } => {
                if !authorization.credentials.contains_key("api_key")
                    && !authorization.credentials.contains_key(name)
                {
                    return Err(AetherError::Authentication(format!(
                        "API key '{}' required but not provided",
                        name
                    )));
                }
            }
            AuthType::Basic => {
                if !authorization.credentials.contains_key("username")
                    || !authorization.credentials.contains_key("password")
                {
                    return Err(AetherError::Authentication(
                        "Username and password required for Basic auth".to_string(),
                    ));
                }
            }
            AuthType::Custom { headers } => {
                for header_name in headers.keys() {
                    if !authorization.credentials.contains_key(header_name) {
                        return Err(AetherError::Authentication(format!(
                            "Custom header '{}' required but not provided",
                            header_name
                        )));
                    }
                }
            }
            AuthType::Passthrough => {
                if !authorization.credentials.contains_key("token") {
                    return Err(AetherError::Authentication(
                        "Passthrough auth requires a token".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }
}
