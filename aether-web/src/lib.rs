//! Web UI for Aether config management

use askama::Template;
use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use tracing::{error, info};

// Re-export the MultiConfigServer for use in public functions
use aether_config::generator;

/// Configuration management state
#[derive(Clone)]
pub struct WebState {
    pub configs: Arc<RwLock<HashMap<String, ConfigInfo>>>,
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

impl WebState {
    pub fn new() -> Self {
        Self {
            configs: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

/// Home page template
#[derive(Template)]
#[template(path = "home.html")]
pub struct HomeTemplate;

impl IntoResponse for HomeTemplate {
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
    configs: Vec<ConfigInfo>,
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
    success: bool,
    message: String,
}

impl IntoResponse for UploadResultTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Template error: {}", err)).into_response(),
        }
    }
}

/// Create the web UI router
pub fn create_router(state: WebState) -> Router {
    Router::new()
        .route("/", get(home_page))
        .route("/upload", post(handle_upload_openapi))
        .route("/apis", get(handle_list_apis))
        .with_state(state)
}

/// Main dashboard page  
async fn home_page() -> impl IntoResponse {
    HomeTemplate
}

/// Handle OpenAPI file upload
async fn handle_upload_openapi(
    State(state): State<WebState>,
    mut multipart: Multipart,
) -> impl IntoResponse {
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
                    },
                }
            }
            "base_url" => {
                match field.text().await {
                    Ok(text) => base_url = text,
                    Err(_) => return UploadResultTemplate {
                        success: false,
                        message: "Failed to read base URL".to_string(),
                    },
                }
            }
            "openapi_file" => {
                match field.text().await {
                    Ok(text) => openapi_content = text,
                    Err(_) => return UploadResultTemplate {
                        success: false,
                        message: "Failed to read OpenAPI file".to_string(),
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

                    state.configs.write().await.insert(api_name.clone(), config_info);
                    info!("Generated config for API: {}", api_name);

                    UploadResultTemplate {
                        success: true,
                        message: format!("Successfully generated config for API: {}", api_name),
                    }
                }
                Err(e) => {
                    error!("Failed to serialize config: {}", e);
                    UploadResultTemplate {
                        success: false,
                        message: "Failed to serialize configuration".to_string(),
                    }
                }
            }
        }
        Err(e) => {
            error!("Failed to generate config: {}", e);
            UploadResultTemplate {
                success: false,
                message: format!("Failed to generate config: {}", e),
            }
        }
    }
}

/// List all configured APIs
async fn handle_list_apis(State(state): State<WebState>) -> impl IntoResponse {
    let configs = state.configs.read().await;
    let configs_vec: Vec<ConfigInfo> = configs.values().cloned().collect();
    
    ApiListTemplate {
        configs: configs_vec,
    }
}

// Public functions for use by aether-mcp

/// Render home page template
pub fn render_home() -> HomeTemplate {
    HomeTemplate
}

/// Render APIs list template
pub async fn render_apis(web_state: &WebState) -> ApiListTemplate {
    let configs = web_state.configs.read().await;
    let configs_vec: Vec<ConfigInfo> = configs.values().cloned().collect();
    
    ApiListTemplate {
        configs: configs_vec,
    }
}

/// Handle upload with integration to multi-server
/// This function accepts a callback to handle the multi-server integration
pub async fn handle_upload_with_multi_server<F, Fut>(
    mut multipart: axum::extract::Multipart,
    web_state: &WebState,
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
                    },
                }
            }
            "base_url" => {
                match field.text().await {
                    Ok(text) => base_url = text,
                    Err(_) => return UploadResultTemplate {
                        success: false,
                        message: "Failed to read base URL".to_string(),
                    },
                }
            }
            "openapi_file" => {
                match field.text().await {
                    Ok(text) => openapi_content = text,
                    Err(_) => return UploadResultTemplate {
                        success: false,
                        message: "Failed to read OpenAPI file".to_string(),
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
            match add_config_fn(api_name.clone(), config.clone(), openapi_content.clone()).await {
                Ok(()) => {
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
                            }
                        }
                        Err(e) => {
                            error!("Failed to serialize config: {}", e);
                            UploadResultTemplate {
                                success: false,
                                message: "Failed to serialize configuration".to_string(),
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to add API to multi-server: {}", e);
                    UploadResultTemplate {
                        success: false,
                        message: format!("Failed to configure API: {}", e),
                    }
                }
            }
        }
        Err(e) => {
            error!("Failed to generate config: {}", e);
            UploadResultTemplate {
                success: false,
                message: format!("Failed to generate config: {}", e),
            }
        }
    }
}
