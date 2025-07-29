//! Request handlers for MCP server

use crate::{McpServer, protocol::*};
use aether_core::Result;
use serde_json::Value;
use tracing::{debug, error, warn};

impl McpServer {
    /// Handle tools/list request
    async fn handle_list_tools(&self, params: Option<Value>) -> Result<Value> {
        // Parse parameters (currently ignoring cursor for pagination)
        if let Some(params_value) = params {
            let _params: ListToolsParams = serde_json::from_value(params_value)
                .map_err(|e| aether_core::AetherError::Mcp(format!("Invalid parameters: {}", e)))?;
        }
        
        let tools = self.list_tools().await?;
        Ok(tools_to_response(tools))
    }
    
    /// Handle tools/call request
    async fn handle_call_tool(&self, params: Option<Value>) -> Result<Value> {
        let params_value = params.ok_or_else(|| {
            aether_core::AetherError::Mcp("Missing parameters for tools/call".to_string())
        })?;
        
        let params: CallToolParams = serde_json::from_value(params_value)
            .map_err(|e| aether_core::AetherError::Mcp(format!("Invalid parameters: {}", e)))?;
        
        let arguments = params.arguments.unwrap_or_default();
        let response = self.call_tool(&params.name, arguments, params.authorization).await?;
        
        Ok(tool_response_to_mcp(response))
    }
    
    /// Handle ping request
    async fn handle_ping(&self) -> Result<Value> {
        debug!("Handling ping request");
        Ok(serde_json::json!({"pong": true}))
    }
}