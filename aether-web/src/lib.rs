//! Web UI for Aether config management

use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::Html,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use tracing::{error, info};

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

/// Create the web UI router
pub fn create_router(state: WebState) -> Router {
    Router::new()
        .route("/", get(home_page))
        .route("/upload", post(handle_upload_openapi))
        .route("/apis", get(handle_list_apis))
        .with_state(state)
}

/// Main dashboard page  
async fn home_page() -> Html<&'static str> {
    Html(r#"<!DOCTYPE html>
<html>
<head>
    <title>Aether Config Manager</title>
    <style>
        body { font-family: system-ui, sans-serif; margin: 2rem; }
        .card { border: 1px solid #ddd; padding: 1rem; margin: 1rem 0; border-radius: 8px; }
        .btn { background: #007bff; color: white; padding: 0.5rem 1rem; border: none; border-radius: 4px; }
        .success { color: green; background: #d4edda; padding: 0.75rem; border-radius: 4px; }
        .error { color: red; background: #f8d7da; padding: 0.75rem; border-radius: 4px; }
    </style>
</head>
<body>
    <h1>Aether Config Manager</h1>
    
    <div class="card">
        <h2>Upload OpenAPI Specification</h2>
        <form action="/upload" method="post" enctype="multipart/form-data">
            <div>
                <label>API Name:</label>
                <input type="text" name="api_name" required>
            </div>
            <div>
                <label>Base URL:</label>
                <input type="url" name="base_url" required>
            </div>
            <div>
                <label>OpenAPI File:</label>
                <input type="file" name="openapi_file" accept=".yaml,.yml,.json" required>
            </div>
            <button type="submit" class="btn">Upload Config</button>
        </form>
    </div>

    <div class="card">
        <h2>Configured APIs</h2>
        <div id="api-list">
            <a href="/apis">View APIs</a>
        </div>
    </div>
</body>
</html>"#)
}

/// Handle OpenAPI file upload
async fn handle_upload_openapi(
    State(state): State<WebState>,
    mut multipart: Multipart,
) -> Result<Html<String>, StatusCode> {
    let mut api_name = String::new();
    let mut base_url = String::new();
    let mut openapi_content = String::new();

    // Process multipart form data
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or_default();
        
        match name {
            "api_name" => {
                api_name = field.text().await.map_err(|_| StatusCode::BAD_REQUEST)?;
            }
            "base_url" => {
                base_url = field.text().await.map_err(|_| StatusCode::BAD_REQUEST)?;
            }
            "openapi_file" => {
                openapi_content = field.text().await.map_err(|_| StatusCode::BAD_REQUEST)?;
            }
            _ => {}
        }
    }

    if api_name.is_empty() || base_url.is_empty() || openapi_content.is_empty() {
        return Ok(Html(r#"<div class="error">Missing required fields</div>"#.to_string()));
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

                    Ok(Html(format!(
                        r#"<div class="success">Successfully generated config for API: {}</div>"#,
                        api_name
                    )))
                }
                Err(e) => {
                    error!("Failed to serialize config: {}", e);
                    Ok(Html(r#"<div class="error">Failed to serialize configuration</div>"#.to_string()))
                }
            }
        }
        Err(e) => {
            error!("Failed to generate config: {}", e);
            Ok(Html(format!(r#"<div class="error">Failed to generate config: {}</div>"#, e)))
        }
    }
}

/// List all configured APIs
async fn handle_list_apis(State(state): State<WebState>) -> Html<String> {
    let configs = state.configs.read().await;
    
    if configs.is_empty() {
        return Html(r#"<p>No APIs configured yet.</p>"#.to_string());
    }

    let mut html = String::new();
    
    for (api_name, config_info) in configs.iter() {
        html.push_str(&format!(
            r#"
            <div class="card">
                <h3>API: {}</h3>
                <p>Base URL: {}</p>
                <p>Created: {}</p>
                <p>SSE Endpoint: /{}/sse</p>
            </div>
            "#,
            api_name,
            config_info.base_url,
            config_info.created_at.format("%Y-%m-%d %H:%M UTC"),
            api_name
        ));
    }
    
    Html(html)
}
