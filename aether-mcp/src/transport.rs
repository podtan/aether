//! Transport implementations for MCP protocol
//!
//! Supports both stdio and HTTP SSE (Server-Sent Events) transports

use crate::{multi_server::MultiConfigServer, protocol::McpRequest, McpServer};
use aether_config::generator;
use aether_web::WebState;
use anyhow;
use axum::{
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::{sse::{Event, KeepAlive, Sse}, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashMap, convert::Infallible, net::SocketAddr, sync::Arc};
use tokio::sync::{mpsc, RwLock};
use tokio_stream::StreamExt;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{debug, error, info, warn};

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
        prm_config: Option<&aether_config::PrmConfig>,
    ) -> anyhow::Result<()> {
        // Convert PRM config to PEP format
        let pep_prm_config = prm_config.and_then(|p| {
            Some(pep::PrmConfig {
                resource: p.resource.clone(),
                authorization_servers: p.authorization_servers.clone(),
                jwks_uri: p.jwks_uri.clone(),
                scopes_supported: p.scopes_supported.clone(),
                bearer_methods_supported: p.bearer_methods_supported.clone(),
                resource_signing_alg_values_supported: p.resource_signing_alg_values_supported.clone(),
                resource_name: p.resource_name.clone(),
                resource_documentation: p.resource_documentation.clone(),
                resource_policy_uri: p.resource_policy_uri.clone(),
                resource_tos_uri: p.resource_tos_uri.clone(),
            })
        });
        
        let app_state = AppState {
            server: self.server,
            sse_state: self.sse_state,
            prm_config: pep_prm_config.clone(),
        };
        
        // Create auth middleware state if configured
        let server_base_url = format!("http://{}", self.bind_addr);
        let auth_middleware_state = if let Some(mcp_auth_config) = auth_config {
            Some(crate::middleware::AuthMiddlewareState::new_with_base_url(
                mcp_auth_config,
                Some(server_base_url),
                None,  // No API name in single-config mode
            ).await?)
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

        // Add RFC 9728 PRM endpoint if configured
        if pep_prm_config.is_some() {
            app = app.route("/.well-known/oauth-protected-resource", axum::routing::get(handle_prm));
            info!("RFC 9728 PRM endpoint enabled at /.well-known/oauth-protected-resource");
        }

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
    pub prm_config: Option<pep::PrmConfig>,
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
        debug!(
            "Received and validating JSON-RPC {} request",
            request.method
        );
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
                    warn!(
                        "Failed to send response to SSE session {}: {}",
                        session_id, e
                    );
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
    let connection_id = params
        .get("session_id")
        .map(|s| s.clone())
        .unwrap_or_else(|| {
            // Generate a simple session ID
            format!(
                "session_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            )
        });

    debug!("Created new session with ID: {}", connection_id);

    // Create a channel for sending events to this specific connection
    let (tx, rx) = mpsc::unbounded_channel();

    // Store the sender in our global state
    app_state
        .sse_state
        .connections
        .write()
        .await
        .insert(connection_id.clone(), tx.clone());
    debug!("SSE connection {} registered in state", connection_id);

    debug!("Starting SSE response task");

    // CRITICAL: Send the endpoint event immediately after connection setup
    // This is what rmcp's SseClientTransport::start() is waiting for!
    let endpoint_url = format!("/messages/?session_id={}", connection_id);
    let endpoint_event = Event::default().event("endpoint").data(endpoint_url);

    debug!(
        "Sending endpoint event with URL: /messages/?session_id={}",
        connection_id
    );
    if let Err(e) = tx.send(endpoint_event) {
        error!("Failed to send endpoint event: {}", e);
    }

    // Note: We use axum's built-in KeepAlive instead of manual ping events.
    // Manual ping events with `event: ping\ndata: keep-alive` cause parsing errors
    // in rmcp because it tries to parse the data as JSON-RPC.

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

    // Use axum's built-in KeepAlive which sends SSE comments (`: keep-alive`)
    // instead of data events that rmcp would try to parse as JSON
    (
        headers,
        Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(30))),
    )
}

/// Handle config generation API
pub async fn handle_generate_config(
    Json(request): Json<GenerateConfigRequest>,
) -> Result<String, StatusCode> {
    info!(
        "🔧 Received config generation request for API: {}",
        request.api_name
    );
    debug!(
        "OpenAPI content length: {} bytes",
        request.openapi_content.len()
    );
    debug!("Base URL: {}", request.base_url);

    // Generate the config using the generator module
    match generator::generate_config_from_openapi(
        &request.openapi_content,
        &request.api_name,
        &request.base_url,
    )
    .await
    {
        Ok(config) => {
            info!(
                "✅ Successfully generated config for API: {}",
                request.api_name
            );

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

/// RFC 9728 Protected Resource Metadata endpoint
pub async fn handle_prm(State(app_state): State<AppState>) -> impl axum::response::IntoResponse {
    if let Some(prm_config) = &app_state.prm_config {
        let metadata: pep::ProtectedResourceMetadata = prm_config.clone().into();
        match serde_json::to_string(&metadata) {
            Ok(json) => ([("content-type", "application/json")], json).into_response(),
            Err(e) => {
                error!("Failed to serialize PRM: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// Health check endpoint
pub async fn health_check(State(app_state): State<AppState>) -> Json<Value> {
    let stats = app_state.server.get_stats().await;
    let mut health = serde_json::Map::new();
    health.insert("status".to_string(), Value::String("healthy".to_string()));
    health.insert(
        "server".to_string(),
        Value::String("aether-mcp".to_string()),
    );
    health.insert(
        "version".to_string(),
        Value::String(env!("CARGO_PKG_VERSION").to_string()),
    );
    health.insert(
        "stats".to_string(),
        Value::Object(stats.into_iter().collect()),
    );

    // Add SSE connection count
    let connection_count = app_state.sse_state.connections.read().await.len();
    health.insert(
        "sse_connections".to_string(),
        Value::Number(connection_count.into()),
    );

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
                                    if let Err(e) = stdout.write_all(response_json.as_bytes()).await
                                    {
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
            async move { multi_server.list_apis().await }
        };

        Self {
            web_state: WebState::new_with_runtime_apis_callback(runtime_apis_callback),
            multi_server: multi_server_arc,
            bind_addr,
        }
    }

    /// Create an app with web routes added
    pub async fn create_app_with_web_routes(
        self,
        web_config: &aether_config::AetherConfig,
    ) -> anyhow::Result<Router> {
        let multi_state = MultiAppState {
            multi_server: self.multi_server.clone(),
            sse_state: SseState::new(),
            web_state: self.web_state.clone(),
            auth_configs: Arc::new(RwLock::new(HashMap::new())),
            oauth_facades: Arc::new(RwLock::new(HashMap::new())),
            server_base_url: format!("http://{}", self.bind_addr),
        };

        // Create upload callback that uses the multi-config server
        let multi_server_for_callback = self.multi_server.clone();
        let web_state_for_callback = self.web_state.clone();
        let upload_callback =
            move |multipart: axum::extract::Multipart, user: Option<aether_web::UserInfo>| {
                let multi_server = multi_server_for_callback.clone();
                let web_state = web_state_for_callback.clone();
                async move {
                    let callback = |api_name: String,
                                    config: aether_config::AetherConfig,
                                    openapi_content: String| {
                        let multi_server = multi_server.clone();
                        async move {
                            info!("🚀 Transport callback called for API: {}", api_name);
                            match multi_server
                                .add_api_config(&api_name, config, &openapi_content)
                                .await
                            {
                                Ok(()) => {
                                    info!(
                                        "✅ MultiConfigServer.add_api_config succeeded for API: {}",
                                        api_name
                                    );
                                    Ok(())
                                }
                                Err(e) => {
                                    error!(
                                        "❌ MultiConfigServer.add_api_config failed for API {}: {}",
                                        api_name, e
                                    );
                                    Err(e.to_string())
                                }
                            }
                        }
                    };

                    aether_web::handle_upload_with_multi_server(
                        multipart, &web_state, user, callback,
                    )
                    .await
                }
            };

        // Create web router with multi-config upload integration using existing web_state
        let web_router = aether_web::create_router_with_existing_web_state(
            web_config,
            self.web_state.clone(),
            upload_callback,
        )
        .await?;

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
            .nest("/web", web_router) // Web auth routes under /web
            .route(
                "/oauth2/callback",
                axum::routing::get(oauth2_callback_redirect),
            ) // Handle OAuth2 callback redirects
            .merge(mcp_router) // Multi-config MCP routes at root level
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
        _auth_config: Option<aether_config::McpAuthConfig>,
        _oidc_config: Option<&aether_config::OidcConfig>,
        _prm_config: Option<&aether_config::PrmConfig>,  // Not used in multi-config
    ) -> anyhow::Result<()> {
        // Load per-API auth configurations
        let mut auth_configs = HashMap::new();
        let apis = self.multi_server.list_apis().await;
        let server_base_url = format!("http://{}", self.bind_addr);
        
        for api_name in &apis {
            if let Some(server) = self.multi_server.get_server(api_name).await {
                let config = server.get_config().await;
                if let Some(mcp_auth) = &config.mcp_auth {
                    if mcp_auth.enabled {
                        // Create auth middleware state for this API with server_base_url
                        match crate::middleware::AuthMiddlewareState::new_with_base_url(
                            mcp_auth.clone(),
                            Some(server_base_url.clone()),
                            Some(api_name.clone()),
                        ).await {
                            Ok(auth_state) => {
                                info!("Enabled auth for API: {}", api_name);
                                auth_configs.insert(api_name.clone(), auth_state);
                            }
                            Err(e) => {
                                error!("Failed to create auth state for API {}: {}", api_name, e);
                            }
                        }
                    }
                }
            }
        }
        
        // Load per-API OAuth facades
        let mut oauth_facades = HashMap::new();
        for api_name in &apis {
            if let Some(server) = self.multi_server.get_server(api_name).await {
                let config = server.get_config().await;
                if let Some(oidc_config) = &config.oidc {
                    match OAuthFacade::from_oidc_config(oidc_config).await {
                        Ok(facade) => {
                            info!("Enabled OAuth facade for API: {}", api_name);
                            oauth_facades.insert(api_name.clone(), facade);
                        }
                        Err(e) => {
                            warn!("Failed to create OAuth facade for API {}: {}", api_name, e);
                        }
                    }
                }
            }
        }
        
        let multi_state = MultiAppState {
            multi_server: self.multi_server.clone(),
            sse_state: SseState::new(),
            web_state: self.web_state.clone(),
            auth_configs: Arc::new(RwLock::new(auth_configs.clone())),
            oauth_facades: Arc::new(RwLock::new(oauth_facades.clone())),
            server_base_url: format!("http://{}", self.bind_addr),
        };
        
        // Group APIs by dedicated vs shared
        let mut dedicated_apis: Vec<(String, u16)> = Vec::new();
        let mut shared_apis: Vec<String> = Vec::new();
        
        for api_name in &apis {
            if let Some(server) = self.multi_server.get_server(api_name).await {
                let config = server.get_config().await;
                if let Some(port) = config.dedicated_port() {
                    dedicated_apis.push((api_name.clone(), port));
                    info!("API {} will use dedicated port {}", api_name, port);
                } else {
                    shared_apis.push(api_name.clone());
                }
            }
        }
        
        // Spawn dedicated listeners for APIs with configured ports
        let dedicated_count = dedicated_apis.len();
        for (api_name, port) in dedicated_apis {
            let bind_addr: SocketAddr = format!("{}:{}", self.bind_addr.ip(), port).parse()
                .map_err(|e| anyhow::anyhow!("Invalid dedicated port address for {}: {}", api_name, e))?;
            
            let dedicated_state = DedicatedApiState {
                multi_server: self.multi_server.clone(),
                sse_state: SseState::new(),
                api_name: api_name.clone(),
                auth_config: auth_configs.get(&api_name).cloned(),
                oauth_facade: oauth_facades.get(&api_name).cloned(),
                dedicated_port_url: format!("http://{}", bind_addr),
            };
            
            info!("Starting dedicated listener for API {} on {}", api_name, bind_addr);
            
            let dedicated_app = create_dedicated_api_router(dedicated_state);
            
            tokio::spawn(async move {
                match tokio::net::TcpListener::bind(bind_addr).await {
                    Ok(listener) => {
                        if let Err(e) = axum::serve(listener, dedicated_app).await {
                            error!("Dedicated listener error for API {}: {}", api_name, e);
                        }
                    }
                    Err(e) => {
                        error!("Failed to bind dedicated port {} for API {}: {}", port, api_name, e);
                    }
                }
            });
        }

        let mut mcp_router = Router::new();
        
        // Only add shared API routes (skip dedicated APIs)
        if !shared_apis.is_empty() {
            mcp_router = mcp_router
                // Legacy single-API endpoints (for backwards compatibility)
                .route("/message", post(handle_single_message))
                .route("/messages", post(handle_single_message))
                .route("/sse", get(handle_single_sse))
                // Multi-API endpoints for shared APIs
                .route("/{api_name}/message", post(handle_multi_message))
                .route("/{api_name}/messages", post(handle_multi_message))
                .route("/{api_name}/sse", get(handle_multi_sse))
                // Per-API PRM endpoint for shared APIs
                .route("/{api_name}/.well-known/oauth-protected-resource", get(handle_api_prm));
        }
        
        // API management endpoints (always on global port)
        mcp_router = mcp_router
            .route("/api/generate-config", post(handle_generate_config))
            .route("/api/upload-openapi", post(handle_upload_openapi))
            .route("/api/list", get(handle_list_apis))
            .route("/api/{api_name}/config", get(handle_get_config))
            .route("/api/{api_name}/config", post(handle_update_config))
            .route("/api/{api_name}/delete", post(handle_delete_api))
            // Stats
            .route("/stats", get(handle_all_stats));

        // Apply per-API auth middleware to protected routes
        mcp_router = mcp_router.route_layer(axum::middleware::from_fn_with_state(
            multi_state.clone(),
            crate::middleware::per_api_auth_middleware,
        ));

        // Health endpoint added AFTER auth middleware (remains public)
        mcp_router = mcp_router.route("/health", get(handle_multi_health));

        let app = mcp_router
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http())
                    .layer(CorsLayer::permissive()),
            )
            .with_state(multi_state);

        info!(
            "Starting Multi-Config MCP HTTP server on {} ({} shared APIs, {} dedicated APIs)",
            self.bind_addr,
            shared_apis.len(),
            dedicated_count
        );

        let listener = tokio::net::TcpListener::bind(self.bind_addr)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to bind to {}: {}", self.bind_addr, e))?;
        axum::serve(listener, app)
            .await
            .map_err(|e| anyhow::anyhow!("Server error: {}", e))?;

        Ok(())
    }
}
/// OAuth facade for VSCode MCP client compatibility
#[derive(Clone)]
pub struct OAuthFacade {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub issuer_url: String,
}

impl OAuthFacade {
    pub async fn from_oidc_config(oidc_config: &aether_config::OidcConfig) -> anyhow::Result<Self> {
        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            oidc_config.issuer_url.trim_end_matches('/')
        );
        
        let client = reqwest::Client::new();
        let response = client.get(&discovery_url).send().await?;
        let discovery: serde_json::Value = response.json().await?;
        
        Ok(Self {
            authorization_endpoint: discovery["authorization_endpoint"].as_str().unwrap().to_string(),
            token_endpoint: discovery["token_endpoint"].as_str().unwrap().to_string(),
            issuer_url: oidc_config.issuer_url.clone(),
        })
    }
}

/// State for dedicated API listener (single API on dedicated port)
#[derive(Clone)]
pub struct DedicatedApiState {
    pub multi_server: Arc<MultiConfigServer>,
    pub sse_state: Arc<SseState>,
    pub api_name: String,
    pub auth_config: Option<crate::middleware::AuthMiddlewareState>,
    pub oauth_facade: Option<OAuthFacade>,
    pub dedicated_port_url: String,
}

/// Create router for dedicated API port with root-level endpoints
fn create_dedicated_api_router(state: DedicatedApiState) -> Router {
    let mut router = Router::new()
        // Root-level MCP endpoints (VSCode expects these at root)
        .route("/sse", get(handle_dedicated_sse))
        .route("/message", post(handle_dedicated_message))
        .route("/messages", post(handle_dedicated_message))
        // Root-level OAuth endpoints (VSCode expects these at root)
        .route("/authorize", get(handle_dedicated_oauth_authorize))
        .route("/token", post(handle_dedicated_oauth_token))
        // Root-level well-known endpoints
        .route("/.well-known/oauth-protected-resource", get(handle_dedicated_prm))
        .route("/.well-known/oauth-authorization-server", get(handle_dedicated_oauth_metadata))
        .route("/.well-known/openid-configuration", get(handle_dedicated_oauth_metadata));
    
    // Apply auth middleware for protected endpoints
    router = router.route_layer(axum::middleware::from_fn_with_state(
        state.clone(),
        crate::middleware::dedicated_api_auth_middleware,
    ));
    
    // Health check (public, added after middleware)
    router
        .route("/health", get(handle_dedicated_health))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

/// Multi-app state for managing multiple configurations  
#[derive(Clone)]
pub struct MultiAppState {
    multi_server: Arc<MultiConfigServer>,
    sse_state: Arc<SseState>,
    web_state: WebState,
    auth_configs: Arc<RwLock<HashMap<String, crate::middleware::AuthMiddlewareState>>>,
    oauth_facades: Arc<RwLock<HashMap<String, OAuthFacade>>>,
    server_base_url: String,
}

impl MultiAppState {
    /// List all configured APIs
    pub async fn list_apis(&self) -> Vec<String> {
        self.multi_server.list_apis().await
    }

    /// Get MCP server for a specific API
    pub async fn get_server(&self, api_name: &str) -> Option<McpServer> {
        self.multi_server.get_server(api_name).await
    }

    /// Get OAuth facade for a specific API
    pub async fn get_oauth_facade(&self, api_name: &str) -> Option<OAuthFacade> {
        self.oauth_facades.read().await.get(api_name).cloned()
    }
    
    /// Get auth config for a specific API
    pub async fn get_auth_config(&self, api_name: &str) -> Option<crate::middleware::AuthMiddlewareState> {
        self.auth_configs.read().await.get(api_name).cloned()
    }
    
    /// Get server base URL for constructing RFC 9728 WWW-Authenticate headers
    pub fn get_server_base_url(&self) -> &str {
        &self.server_base_url
    }
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
    debug!(
        "Received MCP request for API {}: {}",
        api_name, request.method
    );

    // Get the MCP server for this API
    let server = app_state
        .multi_server
        .get_server(api_name)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;

    // Check if this is a session-based request
    if let Some(session_id) = params.get("session_id") {
        let session_key = format!("{}:{}", api_name, session_id);
        debug!("Handling POST message for session: {}", session_key);

        // Handle the request
        let response = server.handle_request(request).await;
        debug!(
            "Generated MCP response for API {}: {:?}",
            api_name, response
        );

        // Send response via SSE if session exists
        if let Ok(connections) = app_state.sse_state.connections.try_read() {
            if let Some(sender) = connections.get(&session_key) {
                let response_json = serde_json::to_string(&response).unwrap_or_default();
                let event = Event::default()
                    .event("message")
                    .data(response_json.clone());

                debug!("Sending JSON-RPC response via SSE for API {}", api_name);

                if let Err(e) = sender.send(event) {
                    warn!(
                        "Failed to send response to SSE session {}: {}",
                        session_key, e
                    );
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
) -> Result<
    (
        axum::http::HeaderMap,
        Sse<impl Stream<Item = Result<Event, Infallible>>>,
    ),
    StatusCode,
> {
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
) -> Result<
    (
        axum::http::HeaderMap,
        Sse<impl Stream<Item = Result<Event, Infallible>>>,
    ),
    StatusCode,
> {
    debug!("New SSE connection request for API: {}", api_name);

    // Verify API exists
    if app_state.multi_server.get_server(&api_name).await.is_none() {
        warn!("SSE connection requested for unknown API: {}", api_name);
        return Err(StatusCode::NOT_FOUND);
    }

    let session_id = params
        .get("session_id")
        .map(|s| s.clone())
        .unwrap_or_else(|| {
            format!(
                "session_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            )
        });

    let session_key = format!("{}:{}", api_name, session_id);
    info!(
        "Establishing SSE connection for API {} with session: {}",
        api_name, session_key
    );

    let (tx, rx) = mpsc::unbounded_channel();

    // Store the sender for this session
    app_state
        .sse_state
        .connections
        .write()
        .await
        .insert(session_key.clone(), tx.clone());

    // CRITICAL: Send the endpoint event that rmcp client is waiting for
    let endpoint_url = format!("/{}/messages?session_id={}", api_name, session_id);
    let endpoint_event = Event::default().event("endpoint").data(endpoint_url);

    debug!(
        "Sending endpoint event with URL: /{}/messages?session_id={}",
        api_name, session_id
    );
    if let Err(e) = tx.send(endpoint_event) {
        error!("Failed to send endpoint event: {}", e);
    }

    // Note: We use axum's built-in KeepAlive instead of manual ping events.
    // Manual ping events with `event: ping\ndata: keep-alive` cause parsing errors
    // in rmcp because it tries to parse the data as JSON-RPC.

    // Create the stream
    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx).map(|event| Ok(event));

    // Set headers for SSE - match the single-config version format
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-session-id", session_id.parse().unwrap());
    headers.insert("cache-control", "no-cache".parse().unwrap());
    headers.insert("connection", "keep-alive".parse().unwrap());
    headers.insert("content-type", "text/event-stream".parse().unwrap());
    headers.insert("access-control-allow-origin", "*".parse().unwrap());
    headers.insert("access-control-allow-headers", "*".parse().unwrap());

    // Use axum's built-in KeepAlive which sends SSE comments (`: keep-alive`)
    // instead of data events that rmcp would try to parse as JSON
    Ok((
        headers,
        Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(30))),
    ))
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
    )
    .await
    {
        Ok(config) => {
            // Add the configuration to the multi-server
            match app_state
                .multi_server
                .add_api_config(&request.api_name, config, &request.openapi_content)
                .await
            {
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
                    app_state
                        .web_state
                        .configs
                        .write()
                        .await
                        .insert(request.api_name.clone(), config_info);

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
pub async fn handle_list_apis(State(app_state): State<MultiAppState>) -> Json<Value> {
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
        Ok((config, openapi_content)) => Ok(Json(serde_json::json!({
            "api_name": api_name,
            "config": config,
            "openapi_content": openapi_content
        }))),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

/// Update configuration for an API
async fn handle_update_config(
    State(app_state): State<MultiAppState>,
    AxumPath(api_name): AxumPath<String>,
    Json(request): Json<UpdateConfigRequest>,
) -> Result<Json<Value>, StatusCode> {
    match app_state
        .multi_server
        .update_api_config(
            &api_name,
            request.config,
            &request.openapi_content.unwrap_or_default(),
        )
        .await
    {
        Ok(()) => Ok(Json(serde_json::json!({
            "success": true,
            "message": format!("Configuration updated for API: {}", api_name)
        }))),
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

/// RFC 9728 Protected Resource Metadata endpoint for multi-config (global - deprecated)
pub async fn handle_multi_prm(State(_app_state): State<MultiAppState>) -> impl IntoResponse {
    // This is deprecated - per-API PRM should be used instead
    StatusCode::NOT_FOUND.into_response()
}

/// Per-API RFC 9728 Protected Resource Metadata endpoint
pub async fn handle_api_prm(
    State(app_state): State<MultiAppState>,
    AxumPath(api_name): AxumPath<String>,
) -> impl IntoResponse {
    // Get the server for this API
    let server = match app_state.get_server(&api_name).await {
        Some(s) => s,
        None => return StatusCode::NOT_FOUND.into_response(),
    };
    
    // Get the config from the server
    let config = server.get_config().await;
    
    // Get PRM config from the API's config
    if let Some(prm) = &config.prm {
        let pep_prm_config = pep::PrmConfig {
            resource: prm.resource.clone(),
            authorization_servers: prm.authorization_servers.clone(),
            jwks_uri: prm.jwks_uri.clone(),
            scopes_supported: prm.scopes_supported.clone(),
            bearer_methods_supported: prm.bearer_methods_supported.clone(),
            resource_signing_alg_values_supported: prm.resource_signing_alg_values_supported.clone(),
            resource_name: prm.resource_name.clone(),
            resource_documentation: prm.resource_documentation.clone(),
            resource_policy_uri: prm.resource_policy_uri.clone(),
            resource_tos_uri: prm.resource_tos_uri.clone(),
        };
        
        let metadata: pep::ProtectedResourceMetadata = pep_prm_config.into();
        match serde_json::to_string(&metadata) {
            Ok(json) => ([("content-type", "application/json")], json).into_response(),
            Err(e) => {
                error!("Failed to serialize PRM for API {}: {}", api_name, e);
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    } else {
        debug!("No PRM config found for API: {}", api_name);
        StatusCode::NOT_FOUND.into_response()
    }
}

/// Per-API OAuth facade: Redirect /{api_name}/authorize to actual OAuth server
pub async fn handle_multi_oauth_authorize(
    State(app_state): State<MultiAppState>,
    AxumPath(api_name): AxumPath<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    // Get OAuth facade for this API
    let facade = match app_state.get_oauth_facade(&api_name).await {
        Some(f) => f,
        None => {
            warn!("No OAuth facade configured for API: {}", api_name);
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    
    // Build redirect URL with all query parameters preserved
    let query_string: String = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&");
    
    let redirect_url = if query_string.is_empty() {
        facade.authorization_endpoint.clone()
    } else {
        format!("{}?{}", facade.authorization_endpoint, query_string)
    };
    
    debug!("OAuth facade: Redirecting /{}/authorize to {}", api_name, redirect_url);
    
    axum::response::Redirect::temporary(&redirect_url).into_response()
}

 
/// Per-API OAuth facade: Proxy /{api_name}/token requests to actual OAuth server
pub async fn handle_multi_oauth_token(
    State(app_state): State<MultiAppState>,
    AxumPath(api_name): AxumPath<String>,
    body: String,
) -> impl IntoResponse {
    // Get OAuth facade for this API
    let facade = match app_state.get_oauth_facade(&api_name).await {
        Some(f) => f,
        None => {
            warn!("No OAuth facade configured for API: {}", api_name);
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    
    debug!("OAuth facade: Proxying /{}/token to {}", api_name, facade.token_endpoint);
    
    let client = reqwest::Client::new();
    
    match client
        .post(&facade.token_endpoint)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status().as_u16();
            match response.text().await {
                Ok(response_body) => {
                    debug!("OAuth facade: Token proxy successful for API {} (status {})", api_name, status);
                    (
                        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                        [("content-type", "application/json")],
                        response_body,
                    )
                        .into_response()
                }
                Err(e) => {
                    error!("OAuth facade: Failed to read token response for API {}: {}", api_name, e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            error!("OAuth facade: Token proxy failed for API {}: {}", api_name, e);
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// Multi-config health check
pub async fn handle_multi_health(State(app_state): State<MultiAppState>) -> Json<Value> {
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
pub async fn handle_all_stats(State(app_state): State<MultiAppState>) -> Json<Value> {
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

// ============================================================================
// Dedicated API Port Handlers (Root-Level Endpoints for VSCode)
// ============================================================================

/// Handle SSE connection on dedicated port (root-level /sse)
pub async fn handle_dedicated_sse(
    State(state): State<DedicatedApiState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<
    (
        axum::http::HeaderMap,
        Sse<impl Stream<Item = Result<Event, Infallible>>>,
    ),
    StatusCode,
> {
    debug!("New SSE connection on dedicated port for API: {}", state.api_name);

    // Verify API exists
    if state.multi_server.get_server(&state.api_name).await.is_none() {
        warn!("SSE connection requested for unknown API: {}", state.api_name);
        return Err(StatusCode::NOT_FOUND);
    }

    let session_id = params
        .get("session_id")
        .map(|s| s.clone())
        .unwrap_or_else(|| {
            format!(
                "session_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            )
        });

    let session_key = format!("{}:{}", state.api_name, session_id);
    info!(
        "Establishing SSE connection for API {} (dedicated port) with session: {}",
        state.api_name, session_key
    );

    let (tx, rx) = mpsc::unbounded_channel();

    // Store the sender for this session
    state
        .sse_state
        .connections
        .write()
        .await
        .insert(session_key.clone(), tx.clone());

    // Send endpoint event that client is waiting for (root-level path)
    let endpoint_url = format!("/messages?session_id={}", session_id);
    let endpoint_event = Event::default().event("endpoint").data(endpoint_url);

    debug!(
        "Sending endpoint event with URL: /messages?session_id={}",
        session_id
    );
    if let Err(e) = tx.send(endpoint_event) {
        error!("Failed to send endpoint event: {}", e);
    }

    // Create the stream
    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx).map(|event| Ok(event));

    // Set headers for SSE
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-session-id", session_id.parse().unwrap());
    headers.insert("cache-control", "no-cache".parse().unwrap());
    headers.insert("connection", "keep-alive".parse().unwrap());
    headers.insert("content-type", "text/event-stream".parse().unwrap());
    headers.insert("access-control-allow-origin", "*".parse().unwrap());
    headers.insert("access-control-allow-headers", "*".parse().unwrap());

    Ok((
        headers,
        Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(30))),
    ))
}

/// Handle MCP message on dedicated port (root-level /message or /messages)
pub async fn handle_dedicated_message(
    State(state): State<DedicatedApiState>,
    Query(params): Query<HashMap<String, String>>,
    Json(request): Json<McpRequest>,
) -> Result<Json<Value>, StatusCode> {
    debug!(
        "Received MCP request for API {} (dedicated port): {}",
        state.api_name, request.method
    );

    // Get the MCP server for this API
    let server = state
        .multi_server
        .get_server(&state.api_name)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;

    // Check if this is a session-based request
    if let Some(session_id) = params.get("session_id") {
        let session_key = format!("{}:{}", state.api_name, session_id);
        debug!("Handling POST message for session: {}", session_key);

        // Handle the request
        let response = server.handle_request(request).await;
        debug!(
            "Generated MCP response for API {} (dedicated port): {:?}",
            state.api_name, response
        );

        // Send response via SSE if session exists
        if let Ok(connections) = state.sse_state.connections.try_read() {
            if let Some(sender) = connections.get(&session_key) {
                let response_json = serde_json::to_string(&response).unwrap_or_default();
                let event = Event::default()
                    .event("message")
                    .data(response_json.clone());

                debug!("Sending JSON-RPC response via SSE for API {} (dedicated port)", state.api_name);

                if let Err(e) = sender.send(event) {
                    warn!(
                        "Failed to send response to SSE session {}: {}",
                        session_key, e
                    );
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

/// Handle OAuth authorize on dedicated port (root-level /authorize)
pub async fn handle_dedicated_oauth_authorize(
    State(state): State<DedicatedApiState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let facade = match &state.oauth_facade {
        Some(f) => f,
        None => {
            warn!("No OAuth facade configured for API: {}", state.api_name);
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    
    let query_string: String = params
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    
    let redirect_url = if query_string.is_empty() {
        facade.authorization_endpoint.clone()
    } else {
        format!("{}?{}", facade.authorization_endpoint, query_string)
    };
    
    debug!(
        "OAuth facade (dedicated port): Redirecting /authorize to {}",
        redirect_url
    );
    
    axum::response::Redirect::temporary(&redirect_url).into_response()
}

/// Handle OAuth token on dedicated port (root-level /token)
pub async fn handle_dedicated_oauth_token(
    State(state): State<DedicatedApiState>,
    body: String,
) -> impl IntoResponse {
    // Get OAuth facade for this API
    let facade = match &state.oauth_facade {
        Some(f) => f,
        None => {
            warn!("No OAuth facade configured for API: {}", state.api_name);
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    
    debug!(
        "OAuth facade (dedicated port): Proxying /token to {}",
        facade.token_endpoint
    );
    
    let client = reqwest::Client::new();
    
    match client
        .post(&facade.token_endpoint)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status().as_u16();
            match response.text().await {
                Ok(response_body) => {
                    debug!(
                        "OAuth facade (dedicated port): Token proxy successful for API {} (status {})",
                        state.api_name, status
                    );
                    (
                        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                        [("content-type", "application/json")],
                        response_body,
                    )
                        .into_response()
                }
                Err(e) => {
                    error!(
                        "OAuth facade (dedicated port): Failed to read token response for API {}: {}",
                        state.api_name, e
                    );
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            error!(
                "OAuth facade (dedicated port): Token proxy failed for API {}: {}",
                state.api_name, e
            );
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// Handle PRM on dedicated port (root-level /.well-known/oauth-protected-resource)
pub async fn handle_dedicated_prm(
    State(state): State<DedicatedApiState>,
) -> impl IntoResponse {
    // Get the server for this API
    let server = match state.multi_server.get_server(&state.api_name).await {
        Some(s) => s,
        None => return StatusCode::NOT_FOUND.into_response(),
    };
    
    // Get the config from the server
    let config = server.get_config().await;
    
    // Get PRM config from the API's config
    if let Some(prm) = &config.prm {
        let pep_prm_config = pep::PrmConfig {
            resource: prm.resource.clone(),
            authorization_servers: prm.authorization_servers.clone(),
            jwks_uri: prm.jwks_uri.clone(),
            scopes_supported: prm.scopes_supported.clone(),
            bearer_methods_supported: prm.bearer_methods_supported.clone(),
            resource_signing_alg_values_supported: prm.resource_signing_alg_values_supported.clone(),
            resource_name: prm.resource_name.clone(),
            resource_documentation: prm.resource_documentation.clone(),
            resource_policy_uri: prm.resource_policy_uri.clone(),
            resource_tos_uri: prm.resource_tos_uri.clone(),
        };
        
        let metadata: pep::ProtectedResourceMetadata = pep_prm_config.into();
        match serde_json::to_string(&metadata) {
            Ok(json) => ([("content-type", "application/json")], json).into_response(),
            Err(e) => {
                error!("Failed to serialize PRM for API {}: {}", state.api_name, e);
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    } else {
        debug!("No PRM config found for API: {}", state.api_name);
        StatusCode::NOT_FOUND.into_response()
    }
}

/// Handle OAuth metadata on dedicated port (root-level /.well-known/oauth-authorization-server)
pub async fn handle_dedicated_oauth_metadata(
    State(state): State<DedicatedApiState>,
) -> impl IntoResponse {
    let issuer_url = match &state.oauth_facade {
        Some(f) => &f.issuer_url,
        None => {
            warn!("No OAuth facade configured for API: {}", state.api_name);
            return StatusCode::NOT_FOUND.into_response();
        }
    };

    if let Some(auth_config) = &state.auth_config {
        match auth_config.validator.get_discovery_document_raw(issuer_url).await {
            Ok(raw_json) => {
                (
                    StatusCode::OK,
                    [("content-type", "application/json")],
                    raw_json,
                )
                    .into_response()
            }
            Err(e) => {
                error!("Failed to fetch cached OAuth metadata for API {}: {}", state.api_name, e);
                StatusCode::BAD_GATEWAY.into_response()
            }
        }
    } else {
        warn!("No auth config for API {}, cannot fetch discovery", state.api_name);
        StatusCode::NOT_FOUND.into_response()
    }
}

/// Health check for dedicated API port
pub async fn handle_dedicated_health(
    State(state): State<DedicatedApiState>,
) -> Json<Value> {
    let connection_count = state.sse_state.connections.read().await.len();

    Json(serde_json::json!({
        "status": "healthy",
        "server": "aether-mcp-dedicated",
        "version": env!("CARGO_PKG_VERSION"),
        "api": state.api_name,
        "sse_connections": connection_count
    }))
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
            .map(|(k, v)| format!("{}={}", k, v)) // Simple format for now, query params should be safe
            .collect::<Vec<_>>()
            .join("&");
        redirect_url.push('?');
        redirect_url.push_str(&query_string);
    }

    axum::response::Redirect::to(&redirect_url)
}
