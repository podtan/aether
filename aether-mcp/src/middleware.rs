//! Authentication middleware for MCP endpoints

use aether_auth::OidcValidator;
use aether_config::{McpAuthConfig, OidcConfig};
use axum::{
    extract::State,
    http::{Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;
use tracing::{debug, warn, info};

/// Shared state for auth middleware
#[derive(Clone)]
pub struct AuthMiddlewareState {
    pub config: McpAuthConfig,
    pub validator: Option<Arc<OidcValidator>>,
}

impl AuthMiddlewareState {
    pub async fn new(
        config: McpAuthConfig,
        oidc_config: Option<&OidcConfig>,
    ) -> anyhow::Result<Self> {
        let validator = if config.enabled {
            let oidc = oidc_config.ok_or_else(|| {
                anyhow::anyhow!("[oidc] config required when mcp_auth.enabled=true")
            })?;

            let issuer = config
                .issuer_url
                .as_deref()
                .unwrap_or(&oidc.issuer_url);

            let validator = OidcValidator::new(
                issuer,
                &oidc.client_id,
                &oidc.client_secret,
            )
            .await?;

            Some(Arc::new(validator))
        } else {
            None
        };

        Ok(Self { config, validator })
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
                    warn!(
                        "Invalid Authorization header format (expected 'Bearer <token>')"
                    );
                    if auth_state.config.require_auth {
                        return Err(StatusCode::UNAUTHORIZED);
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
                return Err(StatusCode::UNAUTHORIZED);
            } else {
                // Auth not required, allow request
                return Ok(next.run(request).await);
            }
        }
    };

    // Validate token
    let validator = auth_state
        .validator
        .as_ref()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    match validator.validate_bearer_token(token).await {
        Ok(claims) => {
            info!(
                "MCP auth successful: user={} sub={}",
                claims.email, claims.sub
            );

            // Attach claims to request for downstream handlers
            let mut request = request;
            request.extensions_mut().insert(claims);

            Ok(next.run(request).await)
        }
        Err(e) => {
            warn!("Token validation failed: {}", e);
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Error response with WWW-Authenticate header (RFC 6750)
pub fn unauthorized_response() -> Response {
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        r#"Bearer realm="aether-mcp", error="invalid_token""#
            .parse()
            .unwrap(),
    );
    response
}
