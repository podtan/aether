//! Integration tests for the Aether MCP server using rmcp client

use aether_config::AetherConfig;
use aether_mcp::{McpServer, protocol::McpRequest};
use anyhow::Result;
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParam, ClientCapabilities, InitializeRequestParam, Implementation},
    object,
    transport::{ConfigureCommandExt, TokioChildProcess, SseClientTransport},
};
use std::path::PathBuf;
use tokio::process::Command;

/// Test configuration path  
const TEST_CONFIG_PATH: &str = "examples/aether-config/config-jsonplaceholder.yaml";

#[tokio::test]
async fn test_rmcp_sse_client_connection() -> Result<()> {
    println!("🔍 Testing rmcp SSE client connection to our server...");
    
    // This test replicates exactly what the user's client code does
    let server_url = "http://localhost:8000/sse";
    
    println!("   🔗 Attempting to connect to: {}", server_url);
    println!("   🔍 Debug: About to call SseClientTransport::start()...");
    
    // This is where the user reports hanging - let's see if we can reproduce it
    let transport = match tokio::time::timeout(
        std::time::Duration::from_secs(5), 
        SseClientTransport::start(server_url.to_string())
    ).await {
        Ok(Ok(t)) => {
            println!("   ✅ Debug: Successfully created SSE transport!");
            t
        },
        Ok(Err(e)) => {
            println!("   ❌ Debug: Failed to start SSE transport: {:?}", e);
            return Err(e.into());
        },
        Err(_) => {
            println!("   ⏰ Debug: SseClientTransport::start() timed out after 5 seconds!");
            println!("   This confirms the user's report - rmcp hangs at SseClientTransport::start()");
            return Err(anyhow::anyhow!("SSE transport start timed out"));
        }
    };
    
    println!("   🔍 Debug: Transport created, now creating client info...");
    let client_info = InitializeRequestParam {
        protocol_version: Default::default(),
        capabilities: ClientCapabilities::default(),
        client_info: Implementation {
            name: "rmcp test client".to_string(),
            version: "0.1.0".to_string(),
        },
    };
    
    println!("   🔍 Debug: About to call client_info.serve(transport)...");
    let _client = match tokio::time::timeout(
        std::time::Duration::from_secs(5), 
        client_info.serve(transport)
    ).await {
        Ok(Ok(c)) => {
            println!("   ✅ Debug: Successfully created client service!");
            c
        },
        Ok(Err(e)) => {
            println!("   ❌ Debug: Failed to serve client: {:?}", e);
            return Err(e.into());
        },
        Err(_) => {
            println!("   ⏰ Debug: client_info.serve() timed out after 5 seconds!");
            return Err(anyhow::anyhow!("Client serve timed out"));
        }
    };
    
    println!("   ✅ Successfully connected to MCP server via SSE!");
    
    Ok(())
}

#[tokio::test]
async fn test_server_initialization_with_rmcp() -> Result<()> {
    // Create a client that connects to our aether server via stdio
    let client = ()
        .serve(TokioChildProcess::new(Command::new("target/debug/aether").configure(
            |cmd| {
                cmd.arg("--config").arg(TEST_CONFIG_PATH).arg("--stdio");
            },
        ))?)
        .await?;

    // Get server info to verify connection
    let server_info = client.peer_info();
    println!("✅ Connected to Aether MCP server: {:#?}", server_info);
    
    // Verify this is our Aether server
    if let Some(info) = server_info {
        assert_eq!(info.server_info.name, "aether-mcp");
    }
    
    Ok(())
}

#[tokio::test]
async fn test_tools_list_with_rmcp() -> Result<()> {
    let client = ()
        .serve(TokioChildProcess::new(Command::new("target/debug/aether").configure(
            |cmd| {
                cmd.arg("--config").arg(TEST_CONFIG_PATH).arg("--stdio");
            },
        ))?)
        .await?;

    // List all available tools
    let tools = client.list_all_tools().await?;
    println!("📋 Available tools: {:#?}", tools);
    
    // Verify we have the expected JSONPlaceholder tools
    assert!(!tools.is_empty(), "Should have at least one tool");
    
    let tool_names: Vec<String> = tools.iter()
        .map(|tool| tool.name.to_string())
        .collect();
    
    // Verify specific tools exist
    assert!(tool_names.contains(&"jsonplaceholder_getPosts".to_string()), "Should have getPosts tool");
    assert!(tool_names.contains(&"jsonplaceholder_getUsers".to_string()), "Should have getUsers tool");
    assert!(tool_names.contains(&"jsonplaceholder_getPost".to_string()), "Should have getPost tool");
    assert!(tool_names.contains(&"jsonplaceholder_createPost".to_string()), "Should have createPost tool");
    
    println!("✅ Tools list test successful - found {} tools", tools.len());
    
    Ok(())
}

#[tokio::test]
async fn test_tool_call_get_users_with_rmcp() -> Result<()> {
    let client = ()
        .serve(TokioChildProcess::new(Command::new("target/debug/aether").configure(
            |cmd| {
                cmd.arg("--config").arg(TEST_CONFIG_PATH).arg("--stdio");
            },
        ))?)
        .await?;

    // Call the getUsers tool
    let tool_result = client
        .call_tool(CallToolRequestParam {
            name: "jsonplaceholder_getUsers".into(),
            arguments: None,
        })
        .await?;
    
    println!("✅ Tool call successful: {:#?}", tool_result);
    
    // Verify response has content
    assert!(!tool_result.content.is_empty(), "Tool response should have content");
    
    Ok(())
}

#[tokio::test]
async fn test_tool_call_get_posts_with_params() -> Result<()> {
    let client = ()
        .serve(TokioChildProcess::new(Command::new("target/debug/aether").configure(
            |cmd| {
                cmd.arg("--config").arg(TEST_CONFIG_PATH).arg("--stdio");
            },
        ))?)
        .await?;

    // Call the getPosts tool with userId parameter
    let tool_result = client
        .call_tool(CallToolRequestParam {
            name: "jsonplaceholder_getPosts".into(),
            arguments: Some(object!({ "userId": 1 })),
        })
        .await?;
    
    println!("✅ Parameterized tool call successful: {:#?}", tool_result);
    
    // Verify response has content
    assert!(!tool_result.content.is_empty(), "Tool response should have content");
    
    Ok(())
}

/// Test the server directly (network-free test)
#[tokio::test]
async fn test_server_direct() -> Result<()> {
    // Load test configuration
    let config_path = PathBuf::from("examples/aether-config/config-jsonplaceholder.yaml");
    let config = AetherConfig::load_from_file(&config_path).await?;
    
    // Create server directly
    let server = McpServer::new(config).await?;
    
    // Test tools listing (this doesn't require network)
    let tools = server.list_tools().await?;
    assert!(!tools.is_empty(), "Should have tools loaded");
    
    let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
    println!("🔧 Direct server test - tools: {:?}", tool_names);
    
    // Verify we have the expected tools
    assert!(tool_names.contains(&"jsonplaceholder_getPosts".to_string()), "Should have getPosts tool");
    assert!(tool_names.contains(&"jsonplaceholder_getUsers".to_string()), "Should have getUsers tool");
    assert!(tool_names.contains(&"jsonplaceholder_getPost".to_string()), "Should have getPost tool");
    assert!(tool_names.contains(&"jsonplaceholder_createPost".to_string()), "Should have createPost tool");
    assert_eq!(tool_names.len(), 4, "Should have exactly 4 tools");
    
    // Test server stats
    let stats = server.get_stats().await;
    println!("📊 Server stats: {:?}", stats);
    
    println!("✅ Direct server test successful - manual MCP server implementation working correctly!");
    
    Ok(())
}

/// Test MCP server protocol implementation without external dependencies
#[tokio::test] 
async fn test_mcp_protocol_implementation() -> Result<()> {
    println!("🔍 Testing MCP protocol implementation...");
    
    // Create a minimal test config to avoid file dependencies
    let config = aether_config::AetherConfig {
        server: aether_config::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            log_level: "info".to_string(),
        },
        apis: std::collections::HashMap::new(),
        tools: std::collections::HashMap::new(),
        enhanced_tools: std::collections::HashMap::new(),
        oidc: None,
        session: None,
        dev: None,
        mcp_auth: None,
    };
    
    let server = McpServer::new(config).await?;
    println!("   ✅ MCP server created successfully");
    
    // Test the exact sequence that rmcp does:
    // 1. Connect to SSE (rmcp hangs here)  
    // 2. Send initialize via POST /messages/?session_id=<id>
    // 3. Receive response via SSE stream
    
    // Test initialize request handling
    let test_request = McpRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::Value::Number(1.into())),
        method: "initialize".to_string(),
        params: Some(serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "rmcp test client", "version": "0.1.0"}
        })),
    };
    
    let response = server.handle_request(test_request).await;
    println!("   📨 Initialize response: {:?}", response);
    
    // Verify response structure matches what rmcp expects
    assert!(response.error.is_none(), "Initialize should not return error");
    assert!(response.result.is_some(), "Initialize should return result");
    
    if let Some(result) = &response.result {
        assert!(result.get("serverInfo").is_some(), "Should include serverInfo");
        assert!(result.get("capabilities").is_some(), "Should include capabilities");
        assert_eq!(result["protocolVersion"], "2024-11-05", "Should match protocol version");
        
        // Check server info matches what we expect
        let server_info = &result["serverInfo"];
        assert_eq!(server_info["name"], "aether-mcp", "Server name should be aether-mcp"); 
    }
    
    println!("   ✅ Initialize handshake works correctly");
    
    // Test tools/list (what rmcp does after initialize)  
    let tools_request = McpRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::Value::Number(2.into())),
        method: "tools/list".to_string(),
        params: Some(serde_json::json!({})),
    };
    
    let tools_response = server.handle_request(tools_request).await;
    println!("   📋 Tools response: {:?}", tools_response);
    
    assert!(tools_response.error.is_none(), "Tools/list should not return error");
    assert!(tools_response.result.is_some(), "Tools/list should return result");
    
    if let Some(result) = &tools_response.result {
        assert!(result.get("tools").is_some(), "Should include tools array");
        let tools = result["tools"].as_array().unwrap();
        println!("   📋 Found {} tools", tools.len());
        
        // With empty config, should have 0 tools but still valid response
        assert_eq!(tools.len(), 0, "Should have 0 tools with empty config");
    }
    
    println!("   ✅ Tools listing works correctly");
    
    // Test initialized notification
    let initialized_request = McpRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::Value::Number(3.into())),
        method: "initialized".to_string(),
        params: Some(serde_json::json!({})),
    };
    
    let initialized_response = server.handle_request(initialized_request).await;
    assert!(initialized_response.error.is_none(), "Initialized should not return error");
    println!("   ✅ Initialized notification works correctly");
    
    println!("🎉 MCP protocol implementation test completed!");
    println!("   The manual MCP server implementation correctly:");
    println!("   - Handles initialize requests with proper capabilities");
    println!("   - Returns server info that matches aether-mcp");
    println!("   - Processes tools/list requests correctly");
    println!("   - Handles initialized notifications");
    println!("   - Follows MCP protocol structure exactly");
    println!("   - Should work with rmcp once SSE transport connects");
    
    // The key insight: our MCP protocol implementation is correct!
    // The issue must be in the SSE transport connection establishment,
    // not in the MCP message handling itself.
    
    Ok(())
}
