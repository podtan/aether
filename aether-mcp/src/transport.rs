//! Transport implementations for MCP protocol
//! 
//! Supports both stdio and HTTP SSE (Server-Sent Events) transports

use crate::{McpServer, multi_server::MultiConfigServer, protocol::McpRequest};
use aether_config::generator;
use aether_web::WebState;
use axum::{
    extract::{Query, State, Path as AxumPath},
    http::StatusCode,
    response::sse::{Event, Sse},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize};
use serde_json::Value;
use std::{collections::HashMap, convert::Infallible, sync::Arc, net::SocketAddr};
use tokio::sync::{mpsc, RwLock};
use tokio_stream::StreamExt;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{debug, error, info, warn};
use anyhow;

/// Request payload for config generation API
#[derive(Debug, Deserialize)]
pub struct GenerateConfigRequest {
    pub openapi_content: String,
    pub api_name: String,
    pub base_url: String,
}

/// Global state for managing SSE connections
pub struct SseState {
    connections: RwLock<HashMap<String, mpsc::UnboundedSender<Event>>>,
}

impl SseState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            connections: RwLock::new(HashMap::new()),
        })
    }
}

/// HTTP SSE Transport for MCP Server
pub struct HttpSseTransport {
    server: Arc<McpServer>,
    sse_state: Arc<SseState>,
    bind_addr: SocketAddr,
}

impl HttpSseTransport {
    /// Create a new HTTP SSE transport
    pub fn new(server: McpServer, bind_addr: SocketAddr) -> Self {
        Self {
            server: Arc::new(server),
            sse_state: SseState::new(),
            bind_addr,
        }
    }

    /// Start the HTTP server
    pub async fn start(
        self,
        auth_config: Option<aether_config::McpAuthConfig>,
        oidc_config: Option<&aether_config::OidcConfig>,
    ) -> anyhow::Result<()> {
        let app_state = AppState {
            server: self.server,
            sse_state: self.sse_state,
        };

        // Create auth middleware state if configured
        let auth_middleware_state = if let Some(mcp_auth_config) = auth_config {
            Some(
                crate::middleware::AuthMiddlewareState::new(
                    mcp_auth_config,
                    oidc_config,
                )
                .await?,
            )
        } else {
            None
        };

        let mut app = Router::new()
            .route("/message", post(handle_message))
            .route("/messages", post(handle_message)) // Support both /message and /messages
            .route("/messages/", post(handle_message)) // Support /messages/ (with trailing slash)
            .route("/sse", get(handle_sse))
            .route("/sse", post(handle_sse_message)) // Handle MCP messages over SSE
            .route("/api/generate-config", post(handle_generate_config)); // New config generation API

        // Apply auth middleware to protected routes
        if let Some(auth_state) = auth_middleware_state {
            app = app.route_layer(axum::middleware::from_fn_with_state(
                auth_state.clone(),
                crate::middleware::mcp_auth_middleware,
            ));
        }

        // Health endpoint added AFTER auth middleware (remains public)
        app = app.route("/health", get(health_check));

        let app = app
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http())
                    .layer(CorsLayer::permissive()),
            )
            .with_state(app_state);

        info!("Starting MCP HTTP SSE server on {}", self.bind_addr);

        let listener = tokio::net::TcpListener::bind(self.bind_addr)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to bind to {}: {}", self.bind_addr, e))?;
        axum::serve(listener, app)
            .await
            .map_err(|e| anyhow::anyhow!("Server error: {}", e))?;

        Ok(())
    }
}

/// Application state combining server and SSE state
#[derive(Clone)]
pub struct AppState {
    pub server: Arc<McpServer>,
    pub sse_state: Arc<SseState>,
}

/// Handle MCP messages via POST /message (session-based)
pub async fn handle_message(
    State(app_state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    Json(request): Json<McpRequest>,
) -> Result<Json<Value>, StatusCode> {
    debug!("Received MCP request via HTTP: {}", request.method);

    // Check if this is a session-based request
    if let Some(session_id) = params.get("session_id") {
        debug!("Handling POST message for session: {}", session_id);
        debug!("Parsing session ID: {}", session_id);
        debug!("Received and validating JSON-RPC {} request", request.method);
        debug!("Preparing to send response");
        
        // Handle the request
        let response = app_state.server.handle_request(request).await;
        debug!("Generated MCP response: {:?}", response);
        
        // Send response via SSE if session exists
        if let Ok(connections) = app_state.sse_state.connections.try_read() {
            if let Some(sender) = connections.get(session_id) {
                // Send the MCP response as a 'message' event in the SSE stream
                let response_json = serde_json::to_string(&response).unwrap_or_default();
                let event = Event::default()
                    .event("message")
                    .data(response_json.clone());
                
                debug!("Sending JSON-RPC response via SSE");
                debug!("Outgoing message: {}", response_json);
                debug!("Actual chunk sent: {}", response_json);
                
                if let Err(e) = sender.send(event) {
                    warn!("Failed to send response to SSE session {}: {}", session_id, e);
                } else {
                    debug!("Sent MCP response to SSE session: {}", session_id);
                }
            } else {
                warn!("SSE session not found: {}", session_id);
            }
        }
        
        // Return 202 Accepted for session-based requests (like standard MCP server)
        debug!("Responding with HTTP 202 Accepted");
        return Ok(Json(serde_json::json!({"status": "accepted"})));
    }

    // Non-session request - handle normally
    let response = app_state.server.handle_request(request).await;

    match serde_json::to_value(&response) {
        Ok(value) => Ok(Json(value)),
        Err(e) => {
            error!("Failed to serialize response: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Handle MCP messages via POST /sse (for SSE clients that send requests via POST)
pub async fn handle_sse_message(
    State(app_state): State<AppState>,
    Json(request): Json<McpRequest>,
) -> Result<Json<Value>, StatusCode> {
    debug!("Received MCP request via SSE POST: {}", request.method);

    let response = app_state.server.handle_request(request).await;

    match serde_json::to_value(&response) {
        Ok(value) => Ok(Json(value)),
        Err(e) => {
            error!("Failed to serialize response: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Handle SSE connections via GET /sse
pub async fn handle_sse(
    State(app_state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl axum::response::IntoResponse {
    debug!("Setting up SSE connection");

    // Generate or use provided connection ID - use UUID format like Python server
    let connection_id = params.get("session_id")
        .map(|s| s.clone())
        .unwrap_or_else(|| {
            // Generate a simple session ID
            format!("session_{}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs())
        });
    
    debug!("Created new session with ID: {}", connection_id);
    
    // Create a channel for sending events to this specific connection
    let (tx, rx) = mpsc::unbounded_channel();

    // Store the sender in our global state
    app_state.sse_state.connections.write().await
        .insert(connection_id.clone(), tx.clone());
    debug!("SSE connection {} registered in state", connection_id);

    debug!("Starting SSE response task");
    
    // CRITICAL: Send the endpoint event immediately after connection setup
    // This is what rmcp's SseClientTransport::start() is waiting for!
    let endpoint_url = format!("/messages/?session_id={}", connection_id);
    let endpoint_event = Event::default()
        .event("endpoint")
        .data(endpoint_url);
    
    debug!("Sending endpoint event with URL: /messages/?session_id={}", connection_id);
    if let Err(e) = tx.send(endpoint_event) {
        error!("Failed to send endpoint event: {}", e);
    }

    // Start a keep-alive ping task for this connection
    let ping_tx = tx.clone();
    let ping_session_id = connection_id.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            let ping_event = Event::default()
                .event("ping")
                .data("keep-alive");
            
            if ping_tx.send(ping_event).is_err() {
                debug!("SSE connection {} closed, stopping ping task", ping_session_id);
                break;
            }
        }
    });

    debug!("Yielding read and write streams");

    // Create a stream and return SSE response with custom headers
    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx)
        .map(|event| Ok::<_, Infallible>(event));

    // Return SSE response with session_id header - this is critical for rmcp
    let headers = [
        ("x-session-id", connection_id.clone()),
        ("cache-control", "no-cache".to_string()),  
        ("connection", "keep-alive".to_string()),
        ("content-type", "text/event-stream".to_string()),
        ("access-control-allow-origin", "*".to_string()),
        ("access-control-allow-headers", "*".to_string()),
    ];
    
    (headers, Sse::new(stream))
}

/// Handle config generation API
pub async fn handle_generate_config(
    Json(request): Json<GenerateConfigRequest>,
) -> Result<String, StatusCode> {
    info!("🔧 Received config generation request for API: {}", request.api_name);
    debug!("OpenAPI content length: {} bytes", request.openapi_content.len());
    debug!("Base URL: {}", request.base_url);

    // Generate the config using the generator module
    match generator::generate_config_from_openapi(
        &request.openapi_content,
        &request.api_name,
        &request.base_url,
    ).await {
        Ok(config) => {
            info!("✅ Successfully generated config for API: {}", request.api_name);
            
            // Serialize config to YAML
            match serde_yaml::to_string(&config) {
                Ok(yaml_config) => {
                    info!("✅ Config serialized to YAML ({} bytes)", yaml_config.len());
                    Ok(yaml_config)
                }
                Err(e) => {
                    error!("❌ Failed to serialize config to YAML: {}", e);
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
        Err(e) => {
            error!("❌ Failed to generate config: {}", e);
            Err(StatusCode::BAD_REQUEST)
        }
    }
}

/// Health check endpoint
pub async fn health_check(State(app_state): State<AppState>) -> Json<Value> {
    let stats = app_state.server.get_stats().await;
    let mut health = serde_json::Map::new();
    health.insert("status".to_string(), Value::String("healthy".to_string()));
    health.insert("server".to_string(), Value::String("aether-mcp".to_string()));
    health.insert("version".to_string(), Value::String(env!("CARGO_PKG_VERSION").to_string()));
    health.insert("stats".to_string(), Value::Object(stats.into_iter().collect()));

    // Add SSE connection count
    let connection_count = app_state.sse_state.connections.read().await.len();
    health.insert("sse_connections".to_string(), Value::Number(connection_count.into()));

    Json(Value::Object(health))
}

/// Stdio Transport for MCP Server (existing implementation)
pub struct StdioTransport {
    server: Arc<McpServer>,
}

impl StdioTransport {
    /// Create a new stdio transport
    pub fn new(server: McpServer) -> Self {
        Self {
            server: Arc::new(server),
        }
    }

    /// Start the stdio transport
    pub async fn start(self) -> anyhow::Result<()> {
        use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

        info!("Starting MCP server with stdio transport");

        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut stdout = io::stdout();

        let mut line = String::new();

        loop {
            line.clear();

            // Read a line from stdin
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    // EOF reached
                    info!("EOF reached, shutting down");
                    break;
                }
                Ok(_) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }

                    // Parse the JSON-RPC request
                    debug!("Received line: {}", line);
                    match serde_json::from_str::<McpRequest>(line) {
                        Ok(request) => {
                            debug!("Parsed request: {:?}", request);
                            // Handle the request
                            let response = self.server.handle_request(request).await;
                            debug!("Generated response: {:?}", response);

                            // Send the response
                            match serde_json::to_string(&response) {
                                Ok(response_json) => {
                                    if let Err(e) = stdout.write_all(response_json.as_bytes()).await {
                                        error!("Failed to write response: {}", e);
                                        break;
                                    }
                                    if let Err(e) = stdout.write_all(b"\n").await {
                                        error!("Failed to write newline: {}", e);
                                        break;
                                    }
                                    if let Err(e) = stdout.flush().await {
                                        error!("Failed to flush stdout: {}", e);
                                        break;
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to serialize response: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to parse request: {} - Input: {}", e, line);
                            // Send a parse error response
                            let error_response = crate::protocol::McpResponse::error(
                                None,
                                crate::protocol::McpError {
                                    code: -32700,
                                    message: format!("Parse error: {}", e),
                                    data: None,
                                },
                            );

                            if let Ok(response_json) = serde_json::to_string(&error_response) {
                                let _ = stdout.write_all(response_json.as_bytes()).await;
                                let _ = stdout.write_all(b"\n").await;
                                let _ = stdout.flush().await;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to read from stdin: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }
}

/// Multi-Configuration HTTP Transport for serving multiple APIs
pub struct MultiConfigTransport {
    multi_server: Arc<MultiConfigServer>,
    web_state: WebState,
    bind_addr: SocketAddr,
}

impl MultiConfigTransport {
    /// Create a new multi-config HTTP transport
    pub fn new(multi_server: MultiConfigServer, bind_addr: SocketAddr) -> Self {
        let multi_server_arc = Arc::new(multi_server);
        
        // Create a callback that captures the multi_server to list APIs
        let multi_server_for_callback = multi_server_arc.clone();
        let runtime_apis_callback = move || {
            let multi_server = multi_server_for_callback.clone();
            async move {
                multi_server.list_apis().await
            }
        };
        
        Self {
            web_state: WebState::new_with_runtime_apis_callback(runtime_apis_callback),
            multi_server: multi_server_arc,
            bind_addr,
        }
    }

    /// Create an app with web routes added
    pub async fn create_app_with_web_routes(self, web_config: &aether_config::AetherConfig) -> anyhow::Result<Router> {
        let multi_state = MultiAppState {
            multi_server: self.multi_server.clone(),
            sse_state: SseState::new(),
            web_state: self.web_state.clone(),
        };

        // Create upload callback that uses the multi-config server
        let multi_server_for_callback = self.multi_server.clone();
        let web_state_for_callback = self.web_state.clone();
        let upload_callback = move |multipart: axum::extract::Multipart, user: Option<aether_web::UserInfo>| {
            let multi_server = multi_server_for_callback.clone();
            let web_state = web_state_for_callback.clone();
            async move {
                let callback = |api_name: String, config: aether_config::AetherConfig, openapi_content: String| {
                    let multi_server = multi_server.clone();
                    async move {
                        info!("🚀 Transport callback called for API: {}", api_name);
                        match multi_server
                            .add_api_config(&api_name, config, &openapi_content)
                            .await
                        {
                            Ok(()) => {
                                info!("✅ MultiConfigServer.add_api_config succeeded for API: {}", api_name);
                                Ok(())
                            }
                            Err(e) => {
                                error!("❌ MultiConfigServer.add_api_config failed for API {}: {}", api_name, e);
                                Err(e.to_string())
                            }
                        }
                    }
                };

                aether_web::handle_upload_with_multi_server(
                    multipart,
                    &web_state,
                    user,
                    callback,
                ).await
            }
        };

        // Create web router with multi-config upload integration using existing web_state
        let web_router = aether_web::create_router_with_existing_web_state(web_config, self.web_state.clone(), upload_callback).await?;

        let mcp_router = Router::new()
            // Legacy single-API endpoints (for backwards compatibility)
            .route("/message", post(handle_single_message))
            .route("/messages", post(handle_single_message))
            .route("/sse", get(handle_single_sse))

            // Multi-API endpoints  
            .route("/{api_name}/message", post(handle_multi_message))
            .route("/{api_name}/messages", post(handle_multi_message))
            .route("/{api_name}/sse", get(handle_multi_sse))

            // API management endpoints
            .route("/api/generate-config", post(handle_generate_config))
            .route("/api/upload-openapi", post(handle_upload_openapi))
            .route("/api/list", get(handle_list_apis))
            .route("/api/{api_name}/config", get(handle_get_config))
            .route("/api/{api_name}/config", post(handle_update_config))
            .route("/api/{api_name}/delete", post(handle_delete_api))

            // Health and stats
            .route("/health", get(handle_multi_health))
            .route("/stats", get(handle_all_stats))
            .with_state(multi_state.clone());

        let app = Router::new()
            .nest("/web", web_router)  // Web auth routes under /web
            .route("/oauth2/callback", axum::routing::get(oauth2_callback_redirect))  // Handle OAuth2 callback redirects
            .merge(mcp_router)  // Multi-config MCP routes at root level
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http())
                    .layer(CorsLayer::permissive()),
            );

        Ok(app)
    }

    /// Start the HTTP server with multi-config support
    pub async fn start(
        self,
        auth_config: Option<aether_config::McpAuthConfig>,
        oidc_config: Option<&aether_config::OidcConfig>,
    ) -> anyhow::Result<()> {
        let multi_state = MultiAppState {
            multi_server: self.multi_server,
            sse_state: SseState::new(),
            web_state: self.web_state.clone(),
        };

        // Create auth middleware state if configured
        let auth_middleware_state = if let Some(mcp_auth_config) = auth_config {
            Some(
                crate::middleware::AuthMiddlewareState::new(
                    mcp_auth_config,
                    oidc_config,
                )
                .await?,
            )
        } else {
            None
        };

        let mut mcp_router = Router::new()
            // Legacy single-API endpoints (for backwards compatibility)
            .route("/message", post(handle_single_message))
            .route("/messages", post(handle_single_message))
            .route("/sse", get(handle_single_sse))
            // Multi-API endpoints
            .route("/{api_name}/message", post(handle_multi_message))
            .route("/{api_name}/messages", post(handle_multi_message))
            .route("/{api_name}/sse", get(handle_multi_sse))
            // API management endpoints
            .route("/api/generate-config", post(handle_generate_config))
            .route("/api/upload-openapi", post(handle_upload_openapi))
            .route("/api/list", get(handle_list_apis))
            .route("/api/{api_name}/config", get(handle_get_config))
            .route("/api/{api_name}/config", post(handle_update_config))
            .route("/api/{api_name}/delete", post(handle_delete_api))
            // Stats
            .route("/stats", get(handle_all_stats));

        // Apply auth middleware to protected routes
        if let Some(auth_state) = auth_middleware_state {
            mcp_router = mcp_router.route_layer(axum::middleware::from_fn_with_state(
                auth_state,
                crate::middleware::mcp_auth_middleware,
            ));
        }

        // Health endpoint added AFTER auth middleware (remains public)
        mcp_router = mcp_router.route("/health", get(handle_multi_health));

        let app = mcp_router
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http())
                    .layer(CorsLayer::permissive()),
            )
            .with_state(multi_state);

        info!("Starting Multi-Config MCP HTTP server on {}", self.bind_addr);

        let listener = tokio::net::TcpListener::bind(self.bind_addr)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to bind to {}: {}", self.bind_addr, e))?;
        axum::serve(listener, app)
            .await
            .map_err(|e| anyhow::anyhow!("Server error: {}", e))?;

        Ok(())
    }
}
/// Multi-app state for managing multiple configurations  
#[derive(Clone)]
pub struct MultiAppState {
    multi_server: Arc<MultiConfigServer>,
    sse_state: Arc<SseState>,
    web_state: WebState,
}

/// Handle single-API messages (backwards compatibility)
pub async fn handle_single_message(
    State(app_state): State<MultiAppState>,
    Query(params): Query<HashMap<String, String>>,
    Json(request): Json<McpRequest>,
) -> Result<Json<Value>, StatusCode> {
    // For backwards compatibility, use the first available API
    let apis = app_state.multi_server.list_apis().await;
    if let Some(first_api) = apis.first() {
        handle_api_message(&app_state, first_api, params, request).await
    } else {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

/// Handle multi-API messages
pub async fn handle_multi_message(
    State(app_state): State<MultiAppState>,
    AxumPath(api_name): AxumPath<String>,
    Query(params): Query<HashMap<String, String>>,
    Json(request): Json<McpRequest>,
) -> Result<Json<Value>, StatusCode> {
    handle_api_message(&app_state, &api_name, params, request).await
}

/// Common message handling logic
async fn handle_api_message(
    app_state: &MultiAppState,
    api_name: &str,
    params: HashMap<String, String>,
    request: McpRequest,
) -> Result<Json<Value>, StatusCode> {
    debug!("Received MCP request for API {}: {}", api_name, request.method);

    // Get the MCP server for this API
    let server = app_state.multi_server.get_server(api_name).await
        .ok_or(StatusCode::NOT_FOUND)?;

    // Check if this is a session-based request
    if let Some(session_id) = params.get("session_id") {
        let session_key = format!("{}:{}", api_name, session_id);
        debug!("Handling POST message for session: {}", session_key);
        
        // Handle the request
        let response = server.handle_request(request).await;
        debug!("Generated MCP response for API {}: {:?}", api_name, response);
        
        // Send response via SSE if session exists
        if let Ok(connections) = app_state.sse_state.connections.try_read() {
            if let Some(sender) = connections.get(&session_key) {
                let response_json = serde_json::to_string(&response).unwrap_or_default();
                let event = Event::default()
                    .event("message")
                    .data(response_json.clone());
                
                debug!("Sending JSON-RPC response via SSE for API {}", api_name);
                
                if let Err(e) = sender.send(event) {
                    warn!("Failed to send response to SSE session {}: {}", session_key, e);
                } else {
                    debug!("Sent MCP response to SSE session: {}", session_key);
                }
            } else {
                warn!("SSE session not found: {}", session_key);
            }
        }
        
        return Ok(Json(serde_json::json!({"status": "accepted"})));
    }

    // Non-session request - handle normally
    let response = server.handle_request(request).await;
    Ok(Json(serde_json::to_value(response).unwrap_or_default()))
}

/// Handle single-API SSE (backwards compatibility)
pub async fn handle_single_sse(
    State(app_state): State<MultiAppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<(axum::http::HeaderMap, Sse<impl Stream<Item = Result<Event, Infallible>>>), StatusCode> {
    // For backwards compatibility, use the first available API
    let apis = app_state.multi_server.list_apis().await;
    if let Some(first_api) = apis.first() {
        handle_multi_sse(State(app_state), AxumPath(first_api.clone()), Query(params)).await
    } else {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

/// Handle multi-API SSE connections
pub async fn handle_multi_sse(
    State(app_state): State<MultiAppState>,
    AxumPath(api_name): AxumPath<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<(axum::http::HeaderMap, Sse<impl Stream<Item = Result<Event, Infallible>>>), StatusCode> {
    debug!("New SSE connection request for API: {}", api_name);

    // Verify API exists
    if app_state.multi_server.get_server(&api_name).await.is_none() {
        warn!("SSE connection requested for unknown API: {}", api_name);
        return Err(StatusCode::NOT_FOUND);
    }

    let session_id = params.get("session_id")
        .map(|s| s.clone())
        .unwrap_or_else(|| format!("session_{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()));

    let session_key = format!("{}:{}", api_name, session_id);
    info!("Establishing SSE connection for API {} with session: {}", api_name, session_key);
    
    let (tx, rx) = mpsc::unbounded_channel();
    
    // Store the sender for this session
    app_state.sse_state.connections.write().await.insert(session_key.clone(), tx.clone());
    
    // CRITICAL: Send the endpoint event that rmcp client is waiting for
    let endpoint_url = format!("/{}/messages?session_id={}", api_name, session_id);
    let endpoint_event = Event::default()
        .event("endpoint")
        .data(endpoint_url);
    
    debug!("Sending endpoint event with URL: /{}/messages?session_id={}", api_name, session_id);
    if let Err(e) = tx.send(endpoint_event) {
        error!("Failed to send endpoint event: {}", e);
    }

    // Start a keep-alive ping task for this connection (like single-config version)
    let ping_tx = tx.clone();
    let ping_session_key = session_key.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            let ping_event = Event::default()
                .event("ping")
                .data("keep-alive");
            
            if ping_tx.send(ping_event).is_err() {
                debug!("SSE connection {} closed, stopping ping task", ping_session_key);
                break;
            }
        }
    });
    
    // Create the stream
    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx)
        .map(|event| Ok(event));
    
    // Set headers for SSE - match the single-config version format
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-session-id", session_id.parse().unwrap());
    headers.insert("cache-control", "no-cache".parse().unwrap());
    headers.insert("connection", "keep-alive".parse().unwrap());
    headers.insert("content-type", "text/event-stream".parse().unwrap());
    headers.insert("access-control-allow-origin", "*".parse().unwrap());
    headers.insert("access-control-allow-headers", "*".parse().unwrap());
    
    Ok((headers, Sse::new(stream)))
}


use futures::Stream;

/// Handle API upload
pub async fn handle_upload_openapi(
    State(app_state): State<MultiAppState>,
    Json(request): Json<UploadRequest>,
) -> Result<Json<Value>, StatusCode> {
    info!("Uploading OpenAPI spec for API: {}", request.api_name);

    // Generate config using the existing generator
    match generator::generate_config_from_openapi(
        &request.openapi_content,
        &request.api_name,
        &request.base_url,
    ).await {
        Ok(config) => {
            // Add the configuration to the multi-server
            match app_state.multi_server.add_api_config(
                &request.api_name,
                config,
                &request.openapi_content,
            ).await {
                Ok(()) => {
                    info!("Successfully added API configuration: {}", request.api_name);
                    
                    // Also update web state for UI
                    let config_info = aether_web::ConfigInfo {
                        api_name: request.api_name.clone(),
                        openapi_content: request.openapi_content,
                        config_content: "".to_string(), // Will be loaded from disk if needed
                        base_url: request.base_url,
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                    };
                    app_state.web_state.configs.write().await.insert(request.api_name.clone(), config_info);
                    
                    Ok(Json(serde_json::json!({
                        "success": true,
                        "message": format!("API {} configured successfully", request.api_name),
                        "api_name": request.api_name
                    })))
                }
                Err(e) => {
                    error!("Failed to add API configuration: {}", e);
                    Ok(Json(serde_json::json!({
                        "success": false,
                        "error": format!("Failed to add API configuration: {}", e)
                    })))
                }
            }
        }
        Err(e) => {
            error!("Failed to generate config: {}", e);
            Ok(Json(serde_json::json!({
                "success": false,
                "error": format!("Failed to generate config: {}", e)
            })))
        }
    }
}

/// List all configured APIs
pub async fn handle_list_apis(
    State(app_state): State<MultiAppState>,
) -> Json<Value> {
    let apis = app_state.multi_server.list_apis().await;
    Json(serde_json::json!({
        "apis": apis
    }))
}

/// Get configuration for an API
async fn handle_get_config(
    State(app_state): State<MultiAppState>,
    AxumPath(api_name): AxumPath<String>,
) -> Result<Json<Value>, StatusCode> {
    match app_state.multi_server.get_api_config(&api_name).await {
        Ok((config, openapi_content)) => {
            Ok(Json(serde_json::json!({
                "api_name": api_name,
                "config": config,
                "openapi_content": openapi_content
            })))
        }
        Err(_) => Err(StatusCode::NOT_FOUND)
    }
}

/// Update configuration for an API
async fn handle_update_config(
    State(app_state): State<MultiAppState>,
    AxumPath(api_name): AxumPath<String>,
    Json(request): Json<UpdateConfigRequest>,
) -> Result<Json<Value>, StatusCode> {
    match app_state.multi_server.update_api_config(
        &api_name,
        request.config,
        &request.openapi_content.unwrap_or_default(),
    ).await {
        Ok(()) => {
            Ok(Json(serde_json::json!({
                "success": true,
                "message": format!("Configuration updated for API: {}", api_name)
            })))
        }
        Err(e) => {
            error!("Failed to update config for API {}: {}", api_name, e);
            Ok(Json(serde_json::json!({
                "success": false,
                "error": format!("Failed to update configuration: {}", e)
            })))
        }
    }
}

/// Delete an API configuration
async fn handle_delete_api(
    State(app_state): State<MultiAppState>,
    AxumPath(api_name): AxumPath<String>,
) -> Result<Json<Value>, StatusCode> {
    match app_state.multi_server.remove_api_config(&api_name).await {
        Ok(()) => {
            // Also remove from web state
            app_state.web_state.configs.write().await.remove(&api_name);
            
            Ok(Json(serde_json::json!({
                "success": true,
                "message": format!("API {} deleted successfully", api_name)
            })))
        }
        Err(e) => {
            error!("Failed to delete API {}: {}", api_name, e);
            Ok(Json(serde_json::json!({
                "success": false,
                "error": format!("Failed to delete API: {}", e)
            })))
        }
    }
}

/// Multi-config health check
pub async fn handle_multi_health(
    State(app_state): State<MultiAppState>,
) -> Json<Value> {
    let apis = app_state.multi_server.list_apis().await;
    let connection_count = app_state.sse_state.connections.read().await.len();
    
    Json(serde_json::json!({
        "status": "healthy",
        "server": "aether-mcp-multi",
        "version": env!("CARGO_PKG_VERSION"),
        "configured_apis": apis.len(),
        "apis": apis,
        "sse_connections": connection_count
    }))
}

/// Get all API statistics
pub async fn handle_all_stats(
    State(app_state): State<MultiAppState>,
) -> Json<Value> {
    let stats = app_state.multi_server.get_all_stats().await;
    Json(serde_json::json!(stats))
}

/// Request payload for OpenAPI upload
#[derive(Debug, Deserialize)]
pub struct UploadRequest {
    api_name: String,
    base_url: String,
    openapi_content: String,
}

/// Request payload for config updates
#[derive(Debug, Deserialize)]
struct UpdateConfigRequest {
    config: aether_config::AetherConfig,
    openapi_content: Option<String>,
}

/// Handle OAuth2 callback redirects by forwarding to the proper web auth callback
async fn oauth2_callback_redirect(
    Query(params): Query<HashMap<String, String>>,
) -> axum::response::Redirect {
    // Build the redirect URL with all query parameters preserved
    let mut redirect_url = "/web/auth/callback".to_string();
    
    if !params.is_empty() {
        let query_string = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))  // Simple format for now, query params should be safe
            .collect::<Vec<_>>()
            .join("&");
        redirect_url.push('?');
        redirect_url.push_str(&query_string);
    }
    
    axum::response::Redirect::to(&redirect_url)
}

