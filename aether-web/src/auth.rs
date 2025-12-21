//! OIDC Authentication implementation for aether-web

use anyhow::Result;
use axum::{
    extract::{Query, State},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use openid::{DiscoveredClient, StandardClaims, Userinfo};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tower_sessions::Session;
use tracing::{debug, info, warn};
use url::Url;
use uuid::Uuid;

use crate::AppState;
use aether_config::{DevConfig, OidcConfig};

/// Base path for standalone mode (empty string)
const STANDALONE_BASE_PATH: &str = "";

/// Helper function to create redirects with the correct base path for standalone mode
fn redirect_to_path(path: &str) -> Redirect {
    let full_path = if path.starts_with('/') {
        format!("{}{}", STANDALONE_BASE_PATH, path)
    } else {
        format!("{}/{}", STANDALONE_BASE_PATH, path)
    };
    Redirect::to(&full_path)
}

/// PKCE parameters for OAuth2 authorization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PkceParams {
    pub code_verifier: String,
    pub code_challenge: String,
}

impl PkceParams {
    /// Generate new PKCE parameters
    pub fn new() -> Self {
        // Generate a cryptographically random code verifier (43-128 characters)
        let code_verifier: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(128)
            .map(char::from)
            .collect();

        // Create code challenge using SHA256
        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let hash = hasher.finalize();
        let code_challenge = URL_SAFE_NO_PAD.encode(&hash);

        Self {
            code_verifier,
            code_challenge,
        }
    }
}

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
    pub async fn new(
        oidc_config: Option<&OidcConfig>,
        dev_config: Option<DevConfig>,
    ) -> Result<Self> {
        // Only create OIDC client if not in dev mode
        let in_dev_mode = dev_config
            .as_ref()
            .map(|c| c.local_dev_mode)
            .unwrap_or(false);

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
        )
        .await?;

        Ok(client)
    }

    pub fn is_dev_mode(&self) -> bool {
        self.dev_config
            .as_ref()
            .map(|c| c.local_dev_mode)
            .unwrap_or(false)
    }
}

/// OAuth2 callback parameters
#[derive(Deserialize)]
pub struct AuthCallback {
    pub code: String,
    pub state: String,
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
        redirect_to_path("/login").into_response()
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
    let template = crate::LoginTemplate::new_standalone(
        app_state.auth.is_dev_mode(),
        params.get("error").cloned(),
    );
    template
}

/// Initiate OIDC login flow
async fn oidc_login(State(app_state): State<AppState>, session: Session) -> impl IntoResponse {
    if let Some(oidc_client) = &app_state.auth.oidc_client {
        // Generate a random state for CSRF protection
        let state = Uuid::new_v4().to_string();

        // Generate PKCE parameters
        let pkce_params = PkceParams::new();

        // Store the state and PKCE verifier in the session
        if let Err(e) = session.insert("oauth_state", &state).await {
            warn!("Failed to store OAuth state in session: {}", e);
            return redirect_to_path("/login?error=session_error");
        }

        if let Err(e) = session
            .insert("pkce_verifier", &pkce_params.code_verifier)
            .await
        {
            warn!("Failed to store PKCE verifier in session: {}", e);
            return redirect_to_path("/login?error=session_error");
        }

        // Manually build the authorization URL with PKCE parameters
        let auth_url = oidc_client.auth_url(&openid::Options {
            scope: Some("openid email profile".to_string()),
            state: Some(state),
            ..Default::default()
        });

        // Add PKCE parameters to the URL
        let mut url_with_pkce = auth_url.clone();
        url_with_pkce
            .query_pairs_mut()
            .append_pair("code_challenge", &pkce_params.code_challenge)
            .append_pair("code_challenge_method", "S256");

        debug!(
            "Redirecting to OIDC authorization URL with PKCE: {}",
            url_with_pkce
        );
        Redirect::to(&url_with_pkce.to_string())
    } else {
        // OIDC client not available, redirect with error
        redirect_to_path("/login?error=oidc_not_configured")
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
                // State is valid, get the PKCE verifier
                match session.get::<String>("pkce_verifier").await {
                    Ok(Some(code_verifier)) => {
                        debug!("OAuth state verified, exchanging code for token with PKCE");

                        // Perform token exchange with PKCE
                        match exchange_token_with_pkce(oidc_client, &callback.code, &code_verifier)
                            .await
                        {
                            Ok(user_info) => {
                                // Store user info in session
                                if let Err(e) = session.insert("user_info", &user_info).await {
                                    warn!("Failed to store user info in session: {}", e);
                                    return redirect_to_path("/login?error=session_error");
                                }

                                // Clean up session data
                                let _ = session.remove::<String>("oauth_state").await;
                                let _ = session.remove::<String>("pkce_verifier").await;

                                info!("User {} logged in successfully", user_info.email);
                                return redirect_to_path("/ui");
                            }
                            Err(e) => {
                                warn!("Failed to exchange code for token: {}", e);
                                return redirect_to_path("/login?error=token_exchange_failed");
                            }
                        }
                    }
                    Ok(None) => {
                        warn!("No PKCE verifier found in session");
                        return redirect_to_path("/login?error=no_pkce_verifier");
                    }
                    Err(e) => {
                        warn!("Failed to get PKCE verifier from session: {}", e);
                        return redirect_to_path("/login?error=session_error");
                    }
                }
            }
            Ok(Some(_)) => {
                warn!("OAuth state mismatch");
                return redirect_to_path("/login?error=state_mismatch");
            }
            Ok(None) => {
                warn!("No OAuth state found in session");
                return redirect_to_path("/login?error=no_state");
            }
            Err(e) => {
                warn!("Failed to get OAuth state from session: {}", e);
                return redirect_to_path("/login?error=session_error");
            }
        }
    }

    // Fallback error
    redirect_to_path("/login?error=callback_failed")
}

/// Exchange authorization code for tokens using PKCE
pub async fn exchange_token_with_pkce(
    oidc_client: &DiscoveredClient,
    code: &str,
    code_verifier: &str,
) -> Result<UserInfo> {
    // Build the request manually since we need to add PKCE
    let token_endpoint = &oidc_client.config().token_endpoint;
    let client_id = &oidc_client.client_id;
    let client_secret = &oidc_client.client_secret;
    let redirect_uri = oidc_client.redirect_url();

    let form_params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("code_verifier", code_verifier),
    ];

    debug!("Making PKCE token request to: {}", token_endpoint);

    // Use the same HTTP client that openid crate would use
    let client = reqwest::Client::new();

    let response = client
        .post(token_endpoint.as_str())
        .form(&form_params)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        warn!(
            "Token request failed with status {}: {}",
            status, error_text
        );
        return Err(anyhow::anyhow!("Token request failed: {}", error_text));
    }

    let token_response: serde_json::Value = response.json().await?;
    debug!("Token response received successfully");

    // Extract the access token
    let access_token = token_response["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access token in response"))?;

    // Get user info from the userinfo endpoint
    let userinfo_endpoint = &oidc_client.config().userinfo_endpoint;

    if userinfo_endpoint.is_none() {
        return Err(anyhow::anyhow!("No userinfo endpoint available"));
    }

    let userinfo_endpoint = userinfo_endpoint.as_ref().unwrap();
    debug!("Getting user info from: {}", userinfo_endpoint);

    let userinfo_response = client
        .get(userinfo_endpoint.as_str())
        .bearer_auth(access_token)
        .send()
        .await?;

    if !userinfo_response.status().is_success() {
        let status = userinfo_response.status();
        let error_text = userinfo_response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        warn!(
            "Userinfo request failed with status {}: {}",
            status, error_text
        );
        return Err(anyhow::anyhow!("Userinfo request failed: {}", error_text));
    }

    let userinfo: serde_json::Value = userinfo_response.json().await?;
    debug!("User info received: {:?}", userinfo);

    // Extract user information
    let user_info = UserInfo {
        sub: userinfo["sub"].as_str().unwrap_or("unknown").to_string(),
        email: userinfo["email"]
            .as_str()
            .unwrap_or("unknown@example.com")
            .to_string(),
        name: userinfo["name"]
            .as_str()
            .or_else(|| userinfo["preferred_username"].as_str())
            .unwrap_or("OIDC User")
            .to_string(),
        role: UserRole::User, // Default to User role
    };

    Ok(user_info)
}

/// Development mode login
async fn dev_login(State(app_state): State<AppState>, session: Session) -> impl IntoResponse {
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
                return redirect_to_path("/login?error=session_error");
            }

            info!("Dev user {} logged in successfully", user_info.email);
            return redirect_to_path("/ui");
        }
    }

    redirect_to_path("/login?error=dev_mode_disabled")
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
    redirect_to_path("/login")
}

/// Check if user has admin role
pub fn is_admin(user: &UserInfo) -> bool {
    user.role == UserRole::Admin
}
