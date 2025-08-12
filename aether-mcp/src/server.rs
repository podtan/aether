//! MCP server using manual protocol implementation

use crate::{McpServer, HttpSseTransport, StdioTransport};
use aether_config::AetherConfig;
use std::net::SocketAddr;
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