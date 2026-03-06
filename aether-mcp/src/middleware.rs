//! Authentication middleware for MCP endpoints

use pep::oidc::resource_server::ResourceServerClient;
use aether_config::McpAuthConfig;
use axum::{
    extract::State,
    http::{header, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Shared state for auth middleware
#[derive(Clone)]
pub struct AuthMiddlewareState {
    pub config: McpAuthConfig,
    pub validator: Arc<ResourceServerClient>,
    pub server_base_url: Option<String>,
    pub api_name: Option<String>,
}

impl AuthMiddlewareState {
    pub async fn new(config: McpAuthConfig) -> anyhow::Result<Self> {
        Self::new_with_base_url(config, None, None).await
    }
    
    pub async fn new_with_base_url(
        config: McpAuthConfig, 
        server_base_url: Option<String>,
        api_name: Option<String>
    ) -> anyhow::Result<Self> {
        if !config.enabled {
            return Ok(Self {
                config,
                validator: Arc::new(ResourceServerClient::new()),
                server_base_url,
                api_name,
            });
        }

        let issuer_url = config.issuer_url
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("issuer_url required when mcp_auth.enabled=true"))?;

        let validator = ResourceServerClient::new();

        info!("Created auth middleware for issuer: {}", issuer_url);

        Ok(Self {
            config,
            validator: Arc::new(validator),
            server_base_url,
            api_name,
        })
    }
}

/// Middleware to validate Bearer tokens on MCP endpoints
pub async fn mcp_auth_middleware(
    State(auth_state): State<AuthMiddlewareState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // Skip auth if disabled
    if !auth_state.config.enabled {
        debug!("MCP auth disabled, allowing request");
        return Ok(next.run(request).await);
    }
    
    // Skip auth for public endpoints (OAuth facade, health, etc.)
    let path = request.uri().path();
    if is_public_endpoint_single(path) {
        debug!("Skipping auth for public endpoint: {}", path);
        return Ok(next.run(request).await);
    }

    // Extract Authorization header
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let token = match auth_header {
        Some(header_value) => {
            // Parse "Bearer <token>"
            match header_value.strip_prefix("Bearer ") {
                Some(token) => token,
                None => {
                    warn!("Invalid Authorization header format (expected 'Bearer <token>')");
                    if auth_state.config.require_auth {
                        return Ok(unauthorized_response(
                            auth_state.api_name.as_deref(),
                            auth_state.server_base_url.as_deref()
                        ));
                    } else {
                        // Auth not required, allow request without token
                        return Ok(next.run(request).await);
                    }
                }
            }
        }
        None => {
            debug!("No Authorization header present");
            if auth_state.config.require_auth {
                warn!("Missing Authorization header, rejecting request");
                return Ok(unauthorized_response(
                    auth_state.api_name.as_deref(),
                    auth_state.server_base_url.as_deref()
                ));
            } else {
                // Auth not required, allow request
                return Ok(next.run(request).await);
            }
        }
    };

    // Get issuer URL from config
    let issuer_url = match &auth_state.config.issuer_url {
        Some(url) => url.clone(),
        None => {
            warn!("No issuer_url configured for auth");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Get client_id (audience) from config
    let client_id = auth_state.config.audience.clone().unwrap_or_else(|| "unknown".to_string());

    // Validate token using PEP's ResourceServerClient
    match auth_state.validator.validate_jwt(token, &issuer_url, &client_id).await {
        Ok(claims) => {
            info!(
                "MCP auth successful: user={} sub={}",
                claims.email.as_deref().unwrap_or("unknown"),
                claims.sub
            );

            // Attach claims to request for downstream handlers
            let mut request = request;
            request.extensions_mut().insert(claims);

            Ok(next.run(request).await)
        }
        Err(e) => {
            warn!("Token validation failed: {}", e);
            Ok(unauthorized_response(
                auth_state.api_name.as_deref(),
                auth_state.server_base_url.as_deref()
            ))
        }
    }
}

/// Check if endpoint should be public (no auth required) - single-config mode
fn is_public_endpoint_single(path: &str) -> bool {
    path == "/health" ||
    path.starts_with("/api/") ||
    path == "/authorize" ||
    path == "/token" ||
    path == "/.well-known/oauth-authorization-server" ||
    path == "/.well-known/oauth-protected-resource"
}

/// Error response with WWW-Authenticate header (RFC 6750 + RFC 9728)
pub fn unauthorized_response(api_name: Option<&str>, server_base_url: Option<&str>) -> Response {
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    
    // Build WWW-Authenticate header with resource_metadata per RFC 9728
    let www_authenticate = if let (Some(name), Some(base_url)) = (api_name, server_base_url) {
        // Per-API mode: include resource_metadata pointing to PRM endpoint
        let resource_metadata = format!(
            "{}/{}/.well-known/oauth-protected-resource",
            base_url.trim_end_matches('/'),
            name
        );
        format!(
            r#"Bearer realm="aether-mcp", resource_metadata="{}""#,
            resource_metadata
        )
    } else if let Some(base_url) = server_base_url {
        // Single-config mode: include resource_metadata for global PRM endpoint
        let resource_metadata = format!(
            "{}/.well-known/oauth-protected-resource",
            base_url.trim_end_matches('/')
        );
        format!(
            r#"Bearer realm="aether-mcp", resource_metadata="{}""#,
            resource_metadata
        )
    } else {
        // Fallback without resource_metadata (shouldn't happen in normal operation)
        r#"Bearer realm="aether-mcp", error="invalid_token""#.to_string()
    };
    
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        www_authenticate.parse().unwrap(),
    );
    response
}

/// Per-API authentication middleware for multi-config mode
pub async fn per_api_auth_middleware(
    State(app_state): State<crate::transport::MultiAppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // Extract API name from path
    let path = request.uri().path();
    let api_name = extract_api_name(path);
    
    // Skip auth for public endpoints
    if is_public_endpoint(path) {
        debug!("Skipping auth for public endpoint: {}", path);
        return Ok(next.run(request).await);
    }
    
    // Get auth config for this API
    let auth_state = if let Some(ref name) = api_name {
        app_state.get_auth_config(name).await
    } else {
        None
    };
    
    // If no auth configured for this API, allow request
    let auth_state = match auth_state {
        Some(state) => state,
        None => {
            debug!("No auth configured for API {:?}, allowing request", api_name);
            return Ok(next.run(request).await);
        }
    };
    
    // Create a new state with API name and base URL for proper RFC 9728 WWW-Authenticate header
    let auth_state_with_url = AuthMiddlewareState {
        config: auth_state.config,
        validator: auth_state.validator,
        server_base_url: Some(app_state.get_server_base_url().to_string()),
        api_name: api_name.clone(),
    };
    
    // Use existing auth validation logic
    mcp_auth_middleware(State(auth_state_with_url), request, next).await
}

/// Extract API name from request path
fn extract_api_name(path: &str) -> Option<String> {
    // Path patterns: /{api_name}/sse, /{api_name}/message, etc.
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    
    // First segment should be the API name (skip empty and special paths)
    if segments.is_empty() {
        return None;
    }
    
    let first = segments[0];
    
    // Skip special paths
    if first.is_empty() || first == "api" || first == "health" || first == "stats" {
        return None;
    }
    
    Some(first.to_string())
}

/// Check if endpoint should be public (no auth required)
fn is_public_endpoint(path: &str) -> bool {
    path.starts_with("/health") ||
    path.starts_with("/api/") ||
    path.ends_with("/authorize") ||
    path.ends_with("/token") ||
    path.ends_with("/.well-known/oauth-authorization-server") ||
    path.ends_with("/.well-known/oauth-protected-resource")
}
