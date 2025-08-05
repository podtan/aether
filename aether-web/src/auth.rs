//! OIDC Authentication implementation for aether-web

use anyhow::Result;
use axum::{
    extract::{Query, State},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use openid::{DiscoveredClient, StandardClaims, Userinfo};
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use tracing::{debug, info, warn};
use url::Url;
use uuid::Uuid;

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

impl std::fmt::Display for UserRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserRole::User => write!(f, "User"),
            UserRole::Admin => write!(f, "Admin"),
        }
    }
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

/// Authentication middleware to protect routes
pub async fn auth_middleware(
    session: Session,
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    // Check if user is authenticated
    if let Ok(Some(_user_info)) = session.get::<UserInfo>("user_info").await {
        // User is authenticated, continue
        next.run(request).await
    } else {
        // User is not authenticated, redirect to login
        Redirect::to("/web/login").into_response()
    }
}

/// Get current user from session
pub async fn get_current_user(session: Session) -> Option<UserInfo> {
    session.get::<UserInfo>("user_info").await.unwrap_or(None)
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
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let template = crate::LoginTemplate {
        is_dev_mode: app_state.auth.is_dev_mode(),
        error: params.get("error").cloned(),
    };
    template
}

/// Initiate OIDC login flow
async fn oidc_login(
    State(app_state): State<AppState>,
    session: Session,
) -> impl IntoResponse {
    if let Some(oidc_client) = &app_state.auth.oidc_client {
        // Generate a random state for CSRF protection
        let state = Uuid::new_v4().to_string();
        
        // Store the state in the session
        if let Err(e) = session.insert("oauth_state", &state).await {
            warn!("Failed to store OAuth state in session: {}", e);
            return Redirect::to("/web/login?error=session_error");
        }
        
        // Build the authorization URL
        let auth_url = oidc_client.auth_url(&openid::Options {
            scope: Some("openid email profile".to_string()),
            state: Some(state),
            ..Default::default()
        });
        
        debug!("Redirecting to OIDC authorization URL: {}", auth_url);
        Redirect::to(&auth_url.to_string())
    } else {
        // OIDC client not available, redirect with error
        Redirect::to("/web/login?error=oidc_not_configured")
    }
}

/// Handle OIDC callback
async fn oidc_callback(
    State(app_state): State<AppState>,
    session: Session,
    Query(callback): Query<AuthCallback>,
) -> impl IntoResponse {
    if let Some(oidc_client) = &app_state.auth.oidc_client {
        // Verify the state parameter
        match session.get::<String>("oauth_state").await {
            Ok(Some(stored_state)) if stored_state == callback.state => {
                // State is valid, proceed with token exchange
                debug!("OAuth state verified, exchanging code for token");
                
                match oidc_client.request_token(&callback.code).await {
                    Ok(_token) => {
                        // For now, create a basic user info from what we have
                        // In a real implementation, you'd decode the ID token or call userinfo endpoint
                        let user_info = UserInfo {
                            sub: "user".to_string(), // Would come from ID token claims
                            email: "user@example.com".to_string(), // Would come from ID token claims  
                            name: "OIDC User".to_string(), // Would come from ID token claims
                            role: UserRole::User,
                        };
                        
                        // Store user info in session
                        if let Err(e) = session.insert("user_info", &user_info).await {
                            warn!("Failed to store user info in session: {}", e);
                            return Redirect::to("/web/login?error=session_error");
                        }
                        
                        info!("User {} logged in successfully", user_info.email);
                        return Redirect::to("/web/");
                    }
                    Err(e) => {
                        warn!("Failed to exchange code for token: {}", e);
                        return Redirect::to("/web/login?error=token_exchange_failed");
                    }
                }
            }
            Ok(Some(_)) => {
                warn!("OAuth state mismatch");
                return Redirect::to("/web/login?error=state_mismatch");
            }
            Ok(None) => {
                warn!("No OAuth state found in session");
                return Redirect::to("/web/login?error=no_state");
            }
            Err(e) => {
                warn!("Failed to get OAuth state from session: {}", e);
                return Redirect::to("/web/login?error=session_error");
            }
        }
    }
    
    // Fallback error
    Redirect::to("/web/login?error=callback_failed")
}

/// Development mode login
async fn dev_login(
    State(app_state): State<AppState>,
    session: Session,
) -> impl IntoResponse {
    if let Some(dev_config) = &app_state.auth.dev_config {
        if dev_config.local_dev_mode {
            // Create a dev user
            let user_info = UserInfo {
                sub: "dev_user".to_string(),
                email: dev_config.local_dev_email.clone(),
                name: dev_config.local_dev_name.clone(),
                role: if dev_config.local_dev_role == "admin" { 
                    UserRole::Admin 
                } else { 
                    UserRole::User 
                },
            };
            
            // Store user info in session
            if let Err(e) = session.insert("user_info", &user_info).await {
                warn!("Failed to store dev user info in session: {}", e);
                return Redirect::to("/web/login?error=session_error");
            }
            
            info!("Dev user {} logged in successfully", user_info.email);
            return Redirect::to("/web/");
        }
    }
    
    Redirect::to("/web/login?error=dev_mode_disabled")
}

/// Handle logout
async fn logout(session: Session) -> Redirect {
    // Remove user from session
    if let Err(e) = session.remove::<UserInfo>("user_info").await {
        warn!("Failed to remove user from session during logout: {}", e);
    }
    
    // Also remove any OAuth state
    if let Err(e) = session.remove::<String>("oauth_state").await {
        warn!("Failed to remove OAuth state during logout: {}", e);
    }
    
    info!("User logged out");
    Redirect::to("/web/login")
}

/// Check if user has admin role
pub fn is_admin(user: &UserInfo) -> bool {
    user.role == UserRole::Admin
}