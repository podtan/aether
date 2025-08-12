//! Web UI for Aether config management

use askama::Template;
use axum::{
    extract::{Multipart, State, Query},
    http::StatusCode,
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use tower_sessions::{MemoryStore, SessionManagerLayer, Session};
use tracing::{error, info};

use aether_config::AetherConfig;

// Re-export the MultiConfigServer for use in public functions
use aether_config::generator;

mod auth;
pub use auth::{AuthState, UserInfo, UserRole, get_current_user};

mod mcp_client;
pub use mcp_client::McpClient;

#[cfg(test)]
mod tests;

/// Application state combining web and auth functionality
#[derive(Clone)]
pub struct AppState {
    pub web: WebState,
    pub auth: AuthState,
}

/// Application state with MCP client for standalone web service
#[derive(Clone)]
pub struct AppStateWithClient {
    pub web: WebStateWithClient,
    pub auth: AuthState,
}

/// Configuration management state
#[derive(Clone)]
pub struct WebState {
    pub configs: Arc<RwLock<HashMap<String, ConfigInfo>>>,
    /// Optional callback to get runtime APIs list
    pub runtime_apis_callback: Option<Arc<dyn Fn() -> futures::future::BoxFuture<'static, Vec<String>> + Send + Sync>>,
}

/// Configuration management state with MCP client
#[derive(Clone)]
pub struct WebStateWithClient {
    pub configs: Arc<RwLock<HashMap<String, ConfigInfo>>>,
    pub mcp_client: McpClient,
}

impl WebStateWithClient {
    pub fn new(mcp_url: &str) -> Self {
        Self {
            configs: Arc::new(RwLock::new(HashMap::new())),
            mcp_client: McpClient::new(mcp_url),
        }
    }
}

/// Information about a managed configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigInfo {
    pub api_name: String,
    pub openapi_content: String,
    pub config_content: String,
    pub base_url: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl AppState {
    pub async fn new(config: &AetherConfig) -> anyhow::Result<Self> {
        let web = WebState::new();
        let auth = AuthState::new(
            config.oidc.as_ref(), 
            config.dev.clone()
        ).await?;
        
        Ok(Self { web, auth })
    }
}

impl AppStateWithClient {
    pub async fn new(config: &AetherConfig, mcp_url: &str) -> anyhow::Result<Self> {
        let web = WebStateWithClient::new(mcp_url);
        let auth = AuthState::new(
            config.oidc.as_ref(), 
            config.dev.clone()
        ).await?;
        
        Ok(Self { web, auth })
    }
}

impl WebState {
    pub fn new() -> Self {
        Self {
            configs: Arc::new(RwLock::new(HashMap::new())),
            runtime_apis_callback: None,
        }
    }

    /// Create a new WebState with runtime APIs callback
    pub fn new_with_runtime_apis_callback<F, Fut>(callback: F) -> Self 
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Vec<String>> + Send + 'static,
    {
        let boxed_callback = move || Box::pin(callback()) as futures::future::BoxFuture<'static, Vec<String>>;
        Self {
            configs: Arc::new(RwLock::new(HashMap::new())),
            runtime_apis_callback: Some(Arc::new(boxed_callback)),
        }
    }
}

/// Home page template
#[derive(Template)]
#[template(path = "home.html")]
pub struct HomeTemplate {
    pub user: Option<UserInfo>,
}

impl IntoResponse for HomeTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Template error: {}", err)).into_response(),
        }
    }
}

/// Login page template
#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate {
    pub is_dev_mode: bool,
    pub error: Option<String>,
}

impl IntoResponse for LoginTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Template error: {}", err)).into_response(),
        }
    }
}

/// API list template
#[derive(Template)]
#[template(path = "apis.html")]
pub struct ApiListTemplate {
    pub configs: Vec<ConfigInfo>,
    pub user: Option<UserInfo>,
}

impl IntoResponse for ApiListTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Template error: {}", err)).into_response(),
        }
    }
}

/// Upload result template
#[derive(Template)]
#[template(path = "upload_result.html")]
pub struct UploadResultTemplate {
    pub success: bool,
    pub message: String,
    pub user: Option<UserInfo>,
}

impl IntoResponse for UploadResultTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Template error: {}", err)).into_response(),
        }
    }
}

/// Create the web UI router with authentication and MCP client integration
pub async fn create_router_with_mcp_client(config: &AetherConfig, mcp_url: &str) -> anyhow::Result<Router> {
    let app_state = AppStateWithClient::new(config, mcp_url).await?;
    
    // Session configuration
    let session_config = config.session.as_ref().ok_or_else(|| {
        anyhow::anyhow!("Session configuration is required")
    })?;
    
    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(false) // Set to true in production with HTTPS
        .with_http_only(true)
        .with_name(session_config.cookie_name.clone());

    let router = Router::new()
        // Public routes (no authentication required)
        .route("/login", get(login_redirect))
        .route("/health", get(health_check))
        
        // Auth routes (always public)
        .route("/auth/login", get(auth_login_with_client))
        .route("/auth/callback", get(auth_callback_with_client))
        .route("/auth/logout", post(auth_logout_with_client))
        
        // Protected routes (require authentication)
        .route("/", get(home_page_with_client))
        .route("/ui", get(home_page_with_client))  // Also accessible as /ui
        .route("/upload", post(handle_upload_openapi_with_client))
        .route("/apis", get(handle_list_apis_with_client))
        
        .with_state(app_state)
        .layer(middleware::from_fn(auth_middleware_with_client))
        .layer(session_layer);

    Ok(router)
}

/// Create the web UI router with authentication and multi-config callback
pub async fn create_router_with_upload_callback<F, Fut>(
    config: &AetherConfig, 
    upload_callback: F
) -> anyhow::Result<Router> 
where
    F: Fn(axum::extract::Multipart, Option<UserInfo>) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = UploadResultTemplate> + Send + 'static,
{
    let app_state = AppState::new(config).await?;
    
    // Session configuration
    let session_config = config.session.as_ref().ok_or_else(|| {
        anyhow::anyhow!("Session configuration is required")
    })?;
    
    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(false) // Set to true in production with HTTPS
        .with_http_only(true)
        .with_name(session_config.cookie_name.clone());

    // Create the upload handler with callback integration
    let upload_handler = {
        let callback = upload_callback.clone();
        move |session: Session, multipart: axum::extract::Multipart| {
            let callback = callback.clone();
            async move {
                let user = auth::get_current_user(session).await;
                callback(multipart, user).await
            }
        }
    };

    // Protected routes (require authentication)
    let protected_routes = Router::new()
        .route("/", get(home_page))
        .route("/ui", get(home_page))  // Also accessible as /ui
        .route("/upload", post(upload_handler))  // Use the callback-based handler
        .route("/apis", get(handle_list_apis))
        .layer(middleware::from_fn(auth::auth_middleware));

    let router = Router::new()
        // Public routes (no authentication required)
        .route("/login", get(login_redirect))
        .route("/health", get(health_check))
        
        // Auth routes (always public)
        .nest("/auth", auth::create_auth_router())
        
        // Protected routes
        .merge(protected_routes)
        
        .with_state(app_state)
        .layer(session_layer);

    Ok(router)
}

/// Create the web UI router with authentication and existing WebState with runtime API callback
pub async fn create_router_with_existing_web_state<F, Fut>(
    config: &AetherConfig,
    web_state: WebState,
    upload_callback: F
) -> anyhow::Result<Router> 
where
    F: Fn(axum::extract::Multipart, Option<UserInfo>) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = UploadResultTemplate> + Send + 'static,
{
    let auth = AuthState::new(
        config.oidc.as_ref(), 
        config.dev.clone()
    ).await?;
    
    let app_state = AppState {
        web: web_state,
        auth,
    };
    
    // Session configuration
    let session_config = config.session.as_ref().ok_or_else(|| {
        anyhow::anyhow!("Session configuration is required")
    })?;
    
    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(false) // Set to true in production with HTTPS
        .with_http_only(true)
        .with_name(session_config.cookie_name.clone());

    // Create the upload handler with callback integration
    let upload_handler = {
        let callback = upload_callback.clone();
        move |session: Session, multipart: axum::extract::Multipart| {
            let callback = callback.clone();
            async move {
                let user = auth::get_current_user(session).await;
                callback(multipart, user).await
            }
        }
    };

    // Protected routes (require authentication)
    let protected_routes = Router::new()
        .route("/", get(home_page))
        .route("/ui", get(home_page))  // Also accessible as /ui
        .route("/upload", post(upload_handler))  // Use the callback-based handler
        .route("/apis", get(handle_list_apis))
        .layer(middleware::from_fn(auth::auth_middleware));

    let router = Router::new()
        // Public routes (no authentication required)
        .route("/login", get(login_redirect))
        .route("/health", get(health_check))
        
        // Auth routes (always public)
        .nest("/auth", auth::create_auth_router())
        
        // Protected routes
        .merge(protected_routes)
        
        .with_state(app_state)
        .layer(session_layer);

    Ok(router)
}

/// Redirect to login page
async fn login_redirect() -> Redirect {
    Redirect::to("/web/auth/login")
}

/// Health check endpoint
async fn health_check() -> &'static str {
    "OK"
}

/// Main dashboard page  
async fn home_page(session: Session) -> HomeTemplate {
    let user = auth::get_current_user(session).await;
    HomeTemplate { user }
}

/// Handle OpenAPI file upload
async fn handle_upload_openapi(
    State(app_state): State<AppState>,
    session: Session,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let user = auth::get_current_user(session).await;
    let mut api_name = String::new();
    let mut base_url = String::new();
    let mut openapi_content = String::new();

    // Process multipart form data
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or_default();
        
        match name {
            "api_name" => {
                match field.text().await {
                    Ok(text) => api_name = text,
                    Err(_) => return UploadResultTemplate {
                        success: false,
                        message: "Failed to read API name".to_string(),
                        user,
                    },
                }
            }
            "base_url" => {
                match field.text().await {
                    Ok(text) => base_url = text,
                    Err(_) => return UploadResultTemplate {
                        success: false,
                        message: "Failed to read base URL".to_string(),
                        user,
                    },
                }
            }
            "openapi_file" => {
                match field.text().await {
                    Ok(text) => openapi_content = text,
                    Err(_) => return UploadResultTemplate {
                        success: false,
                        message: "Failed to read OpenAPI file".to_string(),
                        user,
                    },
                }
            }
            _ => {}
        }
    }

    if api_name.is_empty() || base_url.is_empty() || openapi_content.is_empty() {
        return UploadResultTemplate {
            success: false,
            message: "Missing required fields".to_string(),
            user,
        };
    }

    // Generate config using existing generator
    match aether_config::generator::generate_config_from_openapi(
        &openapi_content,
        &api_name,
        &base_url,
    ).await {
        Ok(config) => {
            match serde_yaml::to_string(&config) {
                Ok(config_yaml) => {
                    let config_info = ConfigInfo {
                        api_name: api_name.clone(),
                        openapi_content,
                        config_content: config_yaml,
                        base_url,
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                    };

                    app_state.web.configs.write().await.insert(api_name.clone(), config_info);
                    info!("Generated config for API: {}", api_name);

                    UploadResultTemplate {
                        success: true,
                        message: format!("Successfully generated config for API: {}", api_name),
                        user,
                    }
                }
                Err(e) => {
                    error!("Failed to serialize config: {}", e);
                    UploadResultTemplate {
                        success: false,
                        message: "Failed to serialize configuration".to_string(),
                        user,
                    }
                }
            }
        }
        Err(e) => {
            error!("Failed to generate config: {}", e);
            UploadResultTemplate {
                success: false,
                message: format!("Failed to generate config: {}", e),
                user,
            }
        }
    }
}

/// List all configured APIs
async fn handle_list_apis(
    State(app_state): State<AppState>,
    session: Session,
) -> impl IntoResponse {
    let user = auth::get_current_user(session).await;
    let web_configs = app_state.web.configs.read().await;
    let mut configs_vec: Vec<ConfigInfo> = web_configs.values().cloned().collect();
    
    // If we have a runtime APIs callback, also include APIs from there
    if let Some(callback) = &app_state.web.runtime_apis_callback {
        match callback().await {
            runtime_apis => {
                let runtime_apis_count = runtime_apis.len();
                info!("Found {} runtime APIs from callback", runtime_apis_count);
                
                // Add runtime APIs that aren't already in the web configs
                for api_name in runtime_apis {
                    if !web_configs.contains_key(&api_name) {
                        // For runtime APIs, we create a minimal ConfigInfo
                        // In a real scenario, we might want to load the actual config details
                        let config_info = ConfigInfo {
                            api_name: api_name.clone(),
                            openapi_content: "".to_string(), // Could be loaded from multi-server if needed
                            config_content: "".to_string(),  // Could be loaded from multi-server if needed
                            base_url: "".to_string(),        // Could be extracted from config if needed
                            created_at: chrono::Utc::now(),  // We don't have the actual creation time
                            updated_at: chrono::Utc::now(),
                        };
                        configs_vec.push(config_info);
                    }
                }
                
                info!("Total APIs to display: {} (web: {}, runtime: {})", 
                      configs_vec.len(), web_configs.len(), runtime_apis_count);
            }
        }
    }
    
    ApiListTemplate {
        configs: configs_vec,
        user,
    }
}

// Public functions for use by aether-mcp

/// Render home page template
pub fn render_home(user: Option<&UserInfo>) -> HomeTemplate {
    HomeTemplate { 
        user: user.cloned() 
    }
}

/// Render APIs list template
pub async fn render_apis(web_state: &WebState, user: Option<&UserInfo>) -> ApiListTemplate {
    let web_configs = web_state.configs.read().await;
    let mut configs_vec: Vec<ConfigInfo> = web_configs.values().cloned().collect();
    
    // If we have a runtime APIs callback, also include APIs from there
    if let Some(callback) = &web_state.runtime_apis_callback {
        match callback().await {
            runtime_apis => {
                // Add runtime APIs that aren't already in the web configs
                for api_name in runtime_apis {
                    if !web_configs.contains_key(&api_name) {
                        // For runtime APIs, we create a minimal ConfigInfo
                        let config_info = ConfigInfo {
                            api_name: api_name.clone(),
                            openapi_content: "".to_string(),
                            config_content: "".to_string(),
                            base_url: "".to_string(),
                            created_at: chrono::Utc::now(),
                            updated_at: chrono::Utc::now(),
                        };
                        configs_vec.push(config_info);
                    }
                }
            }
        }
    }
    
    ApiListTemplate {
        configs: configs_vec,
        user: user.cloned(),
    }
}

/// Handle upload with integration to multi-server
/// This function accepts a callback to handle the multi-server integration
pub async fn handle_upload_with_multi_server<F, Fut>(
    mut multipart: axum::extract::Multipart,
    web_state: &WebState,
    user: Option<UserInfo>,
    add_config_fn: F,
) -> UploadResultTemplate 
where 
    F: FnOnce(String, aether_config::AetherConfig, String) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let mut api_name = String::new();
    let mut base_url = String::new();
    let mut openapi_content = String::new();

    // Process multipart form data  
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or_default();
        
        match name {
            "api_name" => {
                match field.text().await {
                    Ok(text) => api_name = text,
                    Err(_) => return UploadResultTemplate {
                        success: false,
                        message: "Failed to read API name".to_string(),
                        user,
                    },
                }
            }
            "base_url" => {
                match field.text().await {
                    Ok(text) => base_url = text,
                    Err(_) => return UploadResultTemplate {
                        success: false,
                        message: "Failed to read base URL".to_string(),
                        user,
                    },
                }
            }
            "openapi_file" => {
                match field.text().await {
                    Ok(text) => openapi_content = text,
                    Err(_) => return UploadResultTemplate {
                        success: false,
                        message: "Failed to read OpenAPI file".to_string(),
                        user,
                    },
                }
            }
            _ => {}
        }
    }

    if api_name.is_empty() || base_url.is_empty() || openapi_content.is_empty() {
        return UploadResultTemplate {
            success: false,
            message: "Missing required fields".to_string(),
            user,
        };
    }

    // Generate config using existing generator
    match generator::generate_config_from_openapi(
        &openapi_content,
        &api_name,
        &base_url,
    ).await {
        Ok(config) => {
            // Add to multi-server using the provided callback
            info!("🔄 Calling multi-server callback for API: {}", api_name);
            match add_config_fn(api_name.clone(), config.clone(), openapi_content.clone()).await {
                Ok(()) => {
                    info!("✅ Multi-server callback succeeded for API: {}", api_name);
                    // Also update web state for UI
                    match serde_yaml::to_string(&config) {
                        Ok(config_yaml) => {
                            let config_info = ConfigInfo {
                                api_name: api_name.clone(),
                                openapi_content,
                                config_content: config_yaml,  
                                base_url,
                                created_at: chrono::Utc::now(),
                                updated_at: chrono::Utc::now(),
                            };

                            web_state.configs.write().await.insert(api_name.clone(), config_info);
                            info!("Generated config for API: {}", api_name);

                            UploadResultTemplate {
                                success: true,
                                message: format!("Successfully generated and configured API: {}", api_name),
                                user,
                            }
                        }
                        Err(e) => {
                            error!("Failed to serialize config: {}", e);
                            UploadResultTemplate {
                                success: false,
                                message: "Failed to serialize configuration".to_string(),
                                user,
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("❌ Multi-server callback failed for API {}: {}", api_name, e);
                    UploadResultTemplate {
                        success: false,
                        message: format!("Failed to configure API: {}", e),
                        user,
                    }
                }
            }
        }
        Err(e) => {
            error!("Failed to generate config: {}", e);
            UploadResultTemplate {
                success: false,
                message: format!("Failed to generate config: {}", e),
                user,
            }
        }
    }
}
/// Handle OpenAPI file upload using MCP client
async fn handle_upload_openapi_with_client(
    State(app_state): State<AppStateWithClient>,
    session: Session,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let user = auth::get_current_user(session).await;
    let mut api_name = String::new();
    let mut base_url = String::new();
    let mut openapi_content = String::new();

    // Process multipart form data
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or_default();
        
        match name {
            "api_name" => {
                match field.text().await {
                    Ok(text) => api_name = text,
                    Err(_) => return UploadResultTemplate {
                        success: false,
                        message: "Failed to read API name".to_string(),
                        user,
                    },
                }
            }
            "base_url" => {
                match field.text().await {
                    Ok(text) => base_url = text,
                    Err(_) => return UploadResultTemplate {
                        success: false,
                        message: "Failed to read base URL".to_string(),
                        user,
                    },
                }
            }
            "openapi_file" => {
                match field.text().await {
                    Ok(text) => openapi_content = text,
                    Err(_) => return UploadResultTemplate {
                        success: false,
                        message: "Failed to read OpenAPI file".to_string(),
                        user,
                    },
                }
            }
            _ => {}
        }
    }

    if api_name.is_empty() || base_url.is_empty() || openapi_content.is_empty() {
        return UploadResultTemplate {
            success: false,
            message: "Missing required fields".to_string(),
            user,
        };
    }

    // Call MCP admin API to upload the configuration
    match app_state.web.mcp_client.upload_openapi(&api_name, &base_url, &openapi_content).await {
        Ok(response) => {
            if response.success {
                // Also update local web state for UI
                let config_info = ConfigInfo {
                    api_name: api_name.clone(),
                    openapi_content,
                    config_content: "".to_string(), // We don't get the config back from MCP
                    base_url,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                };

                app_state.web.configs.write().await.insert(api_name.clone(), config_info);
                info!("Successfully uploaded config for API: {}", api_name);

                UploadResultTemplate {
                    success: true,
                    message: response.message.unwrap_or_else(|| format!("Successfully configured API: {}", api_name)),
                    user,
                }
            } else {
                UploadResultTemplate {
                    success: false,
                    message: response.error.unwrap_or_else(|| "Upload failed".to_string()),
                    user,
                }
            }
        }
        Err(e) => {
            error!("Failed to upload via MCP client: {}", e);
            UploadResultTemplate {
                success: false,
                message: format!("Failed to configure API: {}", e),
                user,
            }
        }
    }
}

/// List all configured APIs using MCP client
async fn handle_list_apis_with_client(
    State(app_state): State<AppStateWithClient>,
    session: Session,
) -> impl IntoResponse {
    let user = auth::get_current_user(session).await;
    let mut configs_vec: Vec<ConfigInfo> = Vec::new();
    
    // Get APIs from MCP service
    match app_state.web.mcp_client.list_apis().await {
        Ok(runtime_apis) => {
            info!("Found {} APIs from MCP service", runtime_apis.len());
            
            // Load local web state configs
            let web_configs = app_state.web.configs.read().await;
            
            // Merge runtime APIs with local state
            for api_name in runtime_apis {
                if let Some(local_config) = web_configs.get(&api_name) {
                    // Use local config if available
                    configs_vec.push(local_config.clone());
                } else {
                    // Create minimal config for runtime API
                    let config_info = ConfigInfo {
                        api_name: api_name.clone(),
                        openapi_content: "".to_string(),
                        config_content: "".to_string(),
                        base_url: "".to_string(),
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                    };
                    configs_vec.push(config_info);
                }
            }
            
            info!("Total APIs to display: {}", configs_vec.len());
        }
        Err(e) => {
            error!("Failed to get APIs from MCP service: {}", e);
            // Fall back to local state only
            let web_configs = app_state.web.configs.read().await;
            configs_vec = web_configs.values().cloned().collect();
        }
    }
    
    ApiListTemplate {
        configs: configs_vec,
        user,
    }
}
/// Main dashboard page for client-based app
async fn home_page_with_client(session: Session) -> HomeTemplate {
    let user = auth::get_current_user(session).await;
    HomeTemplate { user }
}

/// Auth middleware for client-based app
async fn auth_middleware_with_client(
    session: Session,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, Response> {
    let path = request.uri().path();
    
    // Allow public routes
    if path.starts_with("/auth/") || path == "/login" || path == "/health" {
        return Ok(next.run(request).await);
    }
    
    // Check if user is authenticated
    if let Some(_user_info) = session.get::<auth::UserInfo>("user_info").await.unwrap_or(None) {
        Ok(next.run(request).await)
    } else {
        Err(Redirect::to("/login").into_response())
    }
}

/// Login handler for client-based app
async fn auth_login_with_client(
    State(app_state): State<AppStateWithClient>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    crate::LoginTemplate {
        is_dev_mode: app_state.auth.is_dev_mode(),
        error: params.get("error").cloned(),
    }
}

/// Callback handler for client-based app  
async fn auth_callback_with_client(
    State(_app_state): State<AppStateWithClient>,
    _session: Session,
    Query(_callback): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    // Handle the OIDC callback - for now, redirect to login with error
    // Full implementation would need OIDC flow
    Redirect::to("/login?error=callback_not_implemented")
}

/// Logout handler for client-based app
async fn auth_logout_with_client(
    session: Session,
) -> impl IntoResponse {
    // Remove user from session
    if let Err(e) = session.remove::<auth::UserInfo>("user_info").await {
        tracing::warn!("Failed to remove user from session during logout: {}", e);
    }
    
    // Clear session - this method doesn't return a Result
    session.clear().await;
    
    Redirect::to("/login")
}
