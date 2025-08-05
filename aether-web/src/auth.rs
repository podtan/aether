//! OIDC Authentication implementation for aether-web

use anyhow::Result;
use axum::{
    extract::State,
    response::{IntoResponse, Redirect},
    routing::{get, post},
    Router,
};
use openid::{DiscoveredClient, StandardClaims, Userinfo};
use serde::{Deserialize, Serialize};
use tracing::info;
use url::Url;

use aether_config::{OidcConfig, DevConfig};
use crate::AppState;

/// User information stored in session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub sub: String,
    pub email: String,
    pub name: String,
    pub role: UserRole,
}

/// User roles supported by the system
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum UserRole {
    User,
    Admin,
}

impl UserRole {
    pub fn from_claims(_claims: &StandardClaims, _userinfo: Option<&Userinfo>) -> Self {
        // For now, default to User role
        // TODO: Implement proper role extraction from OIDC claims
        UserRole::User
    }
}

/// Authentication state
#[derive(Clone)]
pub struct AuthState {
    pub oidc_client: Option<DiscoveredClient>,
    pub dev_config: Option<DevConfig>,
}

impl AuthState {
    pub async fn new(oidc_config: Option<&OidcConfig>, dev_config: Option<DevConfig>) -> Result<Self> {
        // Only create OIDC client if not in dev mode
        let in_dev_mode = dev_config.as_ref().map(|c| c.local_dev_mode).unwrap_or(false);
        
        let oidc_client = if let Some(config) = oidc_config {
            if !in_dev_mode {
                Some(Self::create_oidc_client(config).await?)
            } else {
                None // Skip OIDC client creation in dev mode
            }
        } else {
            None
        };
        
        Ok(Self {
            oidc_client,
            dev_config,
        })
    }
    
    async fn create_oidc_client(config: &OidcConfig) -> Result<DiscoveredClient> {
        info!("Creating OIDC client for issuer: {}", config.issuer_url);
        
        let client = DiscoveredClient::discover(
            config.client_id.clone(),
            config.client_secret.clone(),
            Some(config.redirect_url.clone()),
            Url::parse(&config.issuer_url)?,
        ).await?;
        
        Ok(client)
    }
    
    pub fn is_dev_mode(&self) -> bool {
        self.dev_config.as_ref().map(|c| c.local_dev_mode).unwrap_or(false)
    }
}

/// OAuth2 callback parameters
#[derive(Deserialize)]
pub struct AuthCallback {
    code: String,
    state: String,
}

/// Create authentication router
pub fn create_auth_router() -> Router<AppState> {
    Router::new()
        .route("/login", get(login_page))
        .route("/oidc/login", post(oidc_login))
        .route("/callback", get(oidc_callback))
        .route("/logout", post(logout))
        .route("/dev/login", post(dev_login))
}

/// Display login page
async fn login_page(
    State(app_state): State<AppState>,
) -> impl IntoResponse {
    let template = crate::LoginTemplate {
        is_dev_mode: app_state.auth.is_dev_mode(),
    };
    template
}

/// Initiate OIDC login flow
async fn oidc_login(
    State(_app_state): State<AppState>,
) -> impl IntoResponse {
    // For now, just redirect to a placeholder
    Redirect::to("/login?error=not_implemented")
}

/// Handle OIDC callback
async fn oidc_callback() -> impl IntoResponse {
    // For now, just redirect to home
    Redirect::to("/")
}

/// Development mode login
async fn dev_login(
    State(_app_state): State<AppState>,
) -> impl IntoResponse {
    // For now, just redirect to home
    Redirect::to("/")
}

/// Handle logout
async fn logout() -> impl IntoResponse {
    info!("User logged out");
    Redirect::to("/login")
}

/// Check if user has admin role
pub fn is_admin(user: &UserInfo) -> bool {
    user.role == UserRole::Admin
}