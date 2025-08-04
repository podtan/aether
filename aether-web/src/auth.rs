//! OIDC Authentication implementation for aether-web

use anyhow::{anyhow, Result};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::{get, post},
    Router,
};
use openid::{DiscoveredClient, Options, StandardClaims, Bearer, Userinfo};
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use tracing::{debug, error, info, warn};
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

impl UserRole {
    pub fn from_claims(claims: &StandardClaims, userinfo: Option<&Userinfo>) -> Self {
        // Try to extract role from various possible claim locations
        if let Some(groups) = claims.groups() {
            if groups.iter().any(|g| g.contains("admin") || g.contains("administrators")) {
                return UserRole::Admin;
            }
        }
        
        // Check userinfo for additional claims
        if let Some(info) = userinfo {
            if let Some(role_val) = info.claim("role") {
                if let Some(role_str) = role_val.as_str() {
                    if role_str.to_lowercase().contains("admin") {
                        return UserRole::Admin;
                    }
                }
            }
            
            if let Some(groups_val) = info.claim("groups") {
                if let Some(groups_arr) = groups_val.as_array() {
                    for group in groups_arr {
                        if let Some(group_str) = group.as_str() {
                            if group_str.to_lowercase().contains("admin") {
                                return UserRole::Admin;
                            }
                        }
                    }
                }
            }
        }
        
        // Default to User role
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
        let oidc_client = if let Some(config) = oidc_config {
            Some(Self::create_oidc_client(config).await?)
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

/// Session keys
const USER_INFO_KEY: &str = "user_info";
const AUTH_STATE_KEY: &str = "auth_state";

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
    State(app_state): State<AppState>,
    session: Session,
) -> impl IntoResponse {
    let Some(ref client) = app_state.auth.oidc_client else {
        error!("OIDC client not configured");
        return (StatusCode::INTERNAL_SERVER_ERROR, "OIDC not configured").into_response();
    };

    // Generate state parameter for CSRF protection
    let state = Uuid::new_v4().to_string();
    
    // Store state in session
    if let Err(e) = session.insert(AUTH_STATE_KEY, &state).await {
        error!("Failed to store auth state in session: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "Session error").into_response();
    }

    // Build authorization URL
    let auth_url = client.auth_url(&Options {
        scope: Some("openid email profile".to_string()),
        state: Some(state),
        ..Default::default()
    });

    debug!("Redirecting to OIDC provider: {}", auth_url);
    Redirect::to(auth_url.as_str()).into_response()
}

/// Handle OIDC callback
async fn oidc_callback(
    State(app_state): State<AppState>,
    Query(params): Query<AuthCallback>,
    session: Session,
) -> impl IntoResponse {
    let Some(ref client) = app_state.auth.oidc_client else {
        error!("OIDC client not configured");
        return Redirect::to("/login?error=config").into_response();
    };

    // Verify state parameter
    let stored_state: Option<String> = session.get(AUTH_STATE_KEY).await.unwrap_or(None);
    if stored_state.as_deref() != Some(&params.state) {
        warn!("Invalid state parameter in OIDC callback");
        return Redirect::to("/login?error=state").into_response();
    }

    // Remove state from session
    let _ = session.remove::<String>(AUTH_STATE_KEY).await;

    // Exchange authorization code for tokens
    let token = match client.request_token(&params.code).await {
        Ok(token) => token,
        Err(e) => {
            error!("Failed to exchange authorization code: {}", e);
            return Redirect::to("/login?error=token").into_response();
        }
    };

    // Get user info
    let user_info = match get_user_info(client, &token).await {
        Ok(info) => info,
        Err(e) => {
            error!("Failed to get user info: {}", e);
            return Redirect::to("/login?error=userinfo").into_response();
        }
    };

    // Store user info in session  
    if let Err(e) = session.insert(USER_INFO_KEY, &user_info).await {
        error!("Failed to store user info in session: {}", e);
        return Redirect::to("/login?error=session").into_response();
    }

    info!("User logged in successfully: {}", user_info.email);
    Redirect::to("/").into_response()
}

/// Get user information from OIDC token
async fn get_user_info(client: &DiscoveredClient, token: &openid::Bearer) -> Result<UserInfo> {
    let userinfo = client.request_userinfo(token).await?;
    let claims = token.id_token.as_ref()
        .ok_or_else(|| anyhow!("No ID token present"))?
        .payload()?;

    let email = userinfo.email()
        .or_else(|| claims.email())
        .ok_or_else(|| anyhow!("No email found in token or userinfo"))?
        .to_string();

    let name = userinfo.name()
        .or_else(|| claims.name())
        .unwrap_or("Unknown User")
        .to_string();

    let role = UserRole::from_claims(&claims, Some(&userinfo));

    Ok(UserInfo {
        sub: claims.sub.clone(),
        email,
        name,
        role,
    })
}

/// Development mode login
async fn dev_login(
    State(app_state): State<AppState>,
    session: Session,
) -> impl IntoResponse {
    if !app_state.auth.is_dev_mode() {
        warn!("Dev login attempted but dev mode is disabled");
        return Redirect::to("/login?error=devmode").into_response();
    }

    let dev_config = app_state.auth.dev_config.as_ref().unwrap();
    
    let user_info = UserInfo {
        sub: "dev-user".to_string(),
        email: dev_config.local_dev_email.clone(),
        name: dev_config.local_dev_name.clone(),
        role: if dev_config.local_dev_role.to_lowercase().contains("admin") {
            UserRole::Admin
        } else {
            UserRole::User
        },
    };

    if let Err(e) = session.insert(USER_INFO_KEY, &user_info).await {
        error!("Failed to store dev user info in session: {}", e);
        return Redirect::to("/login?error=session").into_response();
    }

    info!("Dev user logged in: {}", user_info.email);
    Redirect::to("/").into_response()
}

/// Handle logout
async fn logout(session: Session) -> impl IntoResponse {
    let _ = session.remove::<UserInfo>(USER_INFO_KEY).await;
    info!("User logged out");
    Redirect::to("/login").into_response()
}

/// Get current user info from session
pub async fn get_current_user(session: &Session) -> Option<UserInfo> {
    session.get(USER_INFO_KEY).await.unwrap_or(None)
}

/// Check if user has admin role
pub fn is_admin(user: &UserInfo) -> bool {
    user.role == UserRole::Admin
}