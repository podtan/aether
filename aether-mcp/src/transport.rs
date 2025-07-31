//! Transport implementations for MCP protocol
//! 
//! Supports both stdio and HTTP SSE (Server-Sent Events) transports

use crate::{McpServer, protocol::McpRequest};
use aether_config::generator;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize};
use serde_json::Value;
use std::{collections::HashMap, convert::Infallible, sync::Arc};
use tokio::sync::{mpsc, RwLock};
use tokio_stream::StreamExt;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{debug, error, info, warn};
use anyhow;

/// Request payload for config generation API
#[derive(Debug, Deserialize)]
struct GenerateConfigRequest {
    openapi_content: String,
    api_name: String,
    base_url: String,
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
    port: u16,
}

impl HttpSseTransport {
    /// Create a new HTTP SSE transport
    pub fn new(server: McpServer, port: u16) -> Self {
        Self {
            server: Arc::new(server),
            sse_state: SseState::new(),
            port,
        }
    }

    /// Start the HTTP server
    pub async fn start(self) -> anyhow::Result<()> {
        let app_state = AppState {
            server: self.server,
            sse_state: self.sse_state,
        };

        let app = Router::new()
            .route("/message", post(handle_message))
            .route("/messages", post(handle_message))  // Support both /message and /messages
            .route("/messages/", post(handle_message))  // Support /messages/ (with trailing slash)
            .route("/sse", get(handle_sse))
            .route("/sse", post(handle_sse_message))  // Handle MCP messages over SSE
            .route("/health", get(health_check))
            .route("/api/generate-config", post(handle_generate_config))  // New config generation API
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http())
                    .layer(CorsLayer::permissive()),
            )
            .with_state(app_state);

        let addr = format!("0.0.0.0:{}", self.port);
        info!("Starting MCP HTTP SSE server on {}", addr);

        let listener = tokio::net::TcpListener::bind(&addr).await
            .map_err(|e| anyhow::anyhow!("Failed to bind to {}: {}", addr, e))?;
        axum::serve(listener, app).await
            .map_err(|e| anyhow::anyhow!("Server error: {}", e))?;

        Ok(())
    }
}

/// Application state combining server and SSE state
#[derive(Clone)]
struct AppState {
    server: Arc<McpServer>,
    sse_state: Arc<SseState>,
}

/// Handle MCP messages via POST /message (session-based)
async fn handle_message(
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
async fn handle_sse_message(
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
async fn handle_sse(
    State(app_state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl axum::response::IntoResponse {
    debug!("Setting up SSE connection");

    // Generate or use provided connection ID - use UUID format like Python server
    let connection_id = params.get("session_id")
        .map(|s| s.clone())
        .unwrap_or_else(|| {
            // Generate a UUID-like session ID similar to working servers
            use std::time::{SystemTime, UNIX_EPOCH};
            let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let rand_part = timestamp % 1000000;
            format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", 
                timestamp & 0xffffffff,
                (timestamp >> 32) & 0xffff,
                (timestamp >> 48) & 0xffff, 
                rand_part & 0xffff,
                timestamp >> 16
            )
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
async fn handle_generate_config(
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
async fn health_check(State(app_state): State<AppState>) -> Json<Value> {
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