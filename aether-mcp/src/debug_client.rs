use rmcp::{
    model::{ClientCapabilities, Implementation, InitializeRequestParam},
    transport::SseClientTransport,
    ServiceExt,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpServerConfig {
    pub name: String,
    pub url: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let server = McpServerConfig {
        name: "backwards-compat".to_string(),
        url: "http://localhost:8000/sse".to_string(),
    };

    println!(
        "🔍 Debug: Starting connection to MCP server: {} at {}",
        server.name, server.url
    );

    // First debug line - this is where it hangs according to the user
    println!("🔍 Debug: About to call SseClientTransport::start()...");
    let transport = match SseClientTransport::start(server.url.clone()).await {
        Ok(t) => {
            println!("✅ Debug: Successfully created SSE transport!");
            t
        }
        Err(e) => {
            println!("❌ Debug: Failed to start SSE transport: {:?}", e);
            return Err(e.into());
        }
    };

    // Second debug line - should reach here if transport works
    println!("🔍 Debug: Transport created, now creating client info...");
    let client_info = InitializeRequestParam {
        protocol_version: Default::default(),
        capabilities: ClientCapabilities::default(),
        client_info: Implementation {
            name: "debug sse client".to_string(),
            version: "0.1.0".to_string(),
        },
    };

    println!("🔍 Debug: About to call client_info.serve(transport)...");
    let client = match client_info.serve(transport).await {
        Ok(c) => {
            println!("✅ Debug: Successfully created client service!");
            c
        }
        Err(e) => {
            println!("❌ Debug: Failed to serve client: {:?}", e);
            return Err(e.into());
        }
    };

    println!(
        "✅ Debug: Successfully connected to MCP server: {}",
        server.name
    );

    Ok(())
}
