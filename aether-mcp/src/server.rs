//! MCP server using manual protocol implementation

use crate::{McpServer, HttpSseTransport, StdioTransport, transport::SseState};
use aether_config::AetherConfig;
use std::net::SocketAddr;
use std::sync::Arc;
use axum::{routing::get, Router};
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;

/// Start the MCP server with HTTP SSE transport
pub async fn start_sse_server(config: AetherConfig, bind_addr: SocketAddr) -> anyhow::Result<()> {
    // Initialize tracing if not already done
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }

    info!("Starting Aether MCP Server with HTTP SSE transport on {}", bind_addr);

    // Create the MCP server
    let server = McpServer::new(config).await
        .map_err(|e| anyhow::anyhow!("Failed to create MCP server: {}", e))?;

    // Create and start the HTTP SSE transport
    let transport = HttpSseTransport::new(server, bind_addr.port());
    transport.start().await?;

    Ok(())
}

/// Start a combined MCP + Web Authentication server
pub async fn start_combined_server(config: AetherConfig, bind_addr: SocketAddr) -> anyhow::Result<()> {
    // Initialize tracing if not already done
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }

    info!("Starting combined Aether MCP + Web Auth server on {}", bind_addr);

    // Create the MCP server
    let mcp_server = Arc::new(McpServer::new(config.clone()).await
        .map_err(|e| anyhow::anyhow!("Failed to create MCP server: {}", e))?);

    // Create the web authentication router
    let web_router = aether_web::create_router(&config).await
        .map_err(|e| anyhow::anyhow!("Failed to create web router: {}", e))?;

    // Create MCP transport state
    let sse_state = SseState::new();
    let mcp_state = crate::transport::AppState {
        server: mcp_server,
        sse_state,
    };

    // Create MCP routes
    let mcp_router = Router::new()
        .route("/message", axum::routing::post(crate::transport::handle_message))
        .route("/messages", axum::routing::post(crate::transport::handle_message))
        .route("/messages/", axum::routing::post(crate::transport::handle_message))
        .route("/sse", get(crate::transport::handle_sse))
        .route("/sse", axum::routing::post(crate::transport::handle_sse_message))
        .route("/health", get(crate::transport::health_check))
        .route("/api/generate-config", axum::routing::post(crate::transport::handle_generate_config))
        .with_state(mcp_state);

    // Combine both routers
    let app = Router::new()
        .nest("/web", web_router)  // Web auth routes under /web
        .merge(mcp_router)  // MCP routes at root level
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        );

    let listener = tokio::net::TcpListener::bind(&bind_addr).await
        .map_err(|e| anyhow::anyhow!("Failed to bind to {}: {}", bind_addr, e))?;
    
    axum::serve(listener, app).await
        .map_err(|e| anyhow::anyhow!("Server error: {}", e))?;

    Ok(())
}

/// Start the MCP server with stdio transport
pub async fn start_stdio_server(config: AetherConfig) -> anyhow::Result<()> {
    // Initialize tracing if not already done
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }

    info!("Starting Aether MCP Server with stdio transport");

    // Create the MCP server
    let server = McpServer::new(config).await
        .map_err(|e| anyhow::anyhow!("Failed to create MCP server: {}", e))?;

    // Create and start the stdio transport
    let transport = StdioTransport::new(server);
    transport.start().await?;

    Ok(())
}