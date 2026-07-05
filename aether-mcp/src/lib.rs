//! MCP Server implementation for Aether using manual MCP protocol

use aether_api::ApiClient;
use aether_config::{
    openapi::{OpenApiProcessor, Operation},
    AetherConfig, ApiAuthentication,
};
use aether_core::{
    types::{Authorization, Tool, ToolResponse},
    AetherError,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use strsim::jaro_winkler;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

// Import the protocol structures
use crate::protocol::{
    tool_response_to_mcp, tools_to_response, CallToolParams, ListToolsParams, McpError, McpRequest,
    McpResponse,
};

pub mod middleware;
pub mod multi_server;
pub mod protocol;
pub mod server;
pub mod transport;

// Re-export core structures
pub use middleware::{mcp_auth_middleware, per_api_auth_middleware, AuthMiddlewareState, ForwardedAuth};
pub use multi_server::MultiConfigServer;
pub use transport::{HttpSseTransport, MultiConfigTransport, StdioTransport};

/// Internal tool definition with operation and API context
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub operation: Operation,
    pub api_name: String,
}

/// Aether MCP Server that dynamically generates tools from OpenAPI specs
#[derive(Clone)]
pub struct McpServer {
    config: Arc<RwLock<AetherConfig>>,
    tools: Arc<RwLock<HashMap<String, ToolDefinition>>>,
    api_clients: Arc<RwLock<HashMap<String, ApiClient>>>,
}

impl McpServer {
    /// Create a new Aether MCP server
    pub async fn new(config: AetherConfig) -> std::result::Result<Self, AetherError> {
        let server = Self {
            config: Arc::new(RwLock::new(config)),
            tools: Arc::new(RwLock::new(HashMap::new())),
            api_clients: Arc::new(RwLock::new(HashMap::new())),
        };

        server.reload_configuration().await?;
        Ok(server)
    }

    /// Get the server's configuration (for PRM)
    pub async fn get_config(&self) -> AetherConfig {
        self.config.read().await.clone()
    }

    /// Reload configuration and rebuild tools
    pub async fn reload_configuration(&self) -> std::result::Result<(), AetherError> {
        info!("Reloading MCP server configuration");

        let config = self.config.read().await;
        let mut tools = HashMap::new();
        let mut api_clients = HashMap::new();

        // Create API clients first
        for (api_name, api_config) in &config.apis {
            info!("Creating API client for: {}", api_name);

            // Create API client
            let timeout = api_config.timeout.map(Duration::from_secs);
            let client = ApiClient::new(api_config.base_url.clone(), timeout)?;
            api_clients.insert(api_name.clone(), client);
        }

        // Check if we have enhanced tools (from generated configs)
        if !config.enhanced_tools.is_empty() {
            info!(
                "Using enhanced tools from generated configuration ({} tools)",
                config.enhanced_tools.len()
            );
            // Enhanced tools don't need dynamic generation - they're already defined
        } else {
            info!("Generating tools dynamically from OpenAPI specs");

            // Process each API configuration for dynamic tool generation
            for (api_name, api_config) in &config.apis {
                if api_config.openapi_spec.is_empty() {
                    warn!(
                        "Skipping API '{}' - no OpenAPI spec file configured",
                        api_name
                    );
                    continue;
                }

                info!("Processing API: {}", api_name);

                // Load and process OpenAPI specification
                let processor = OpenApiProcessor::load_from_file(&api_config.openapi_spec).await?;
                let operations = processor.extract_operations()?;

                // Convert operations to tools
                for operation in operations {
                    let tool_name = format!("{}_{}", api_name, operation.operation_id);

                    tools.insert(
                        tool_name.clone(),
                        ToolDefinition {
                            operation: operation.clone(),
                            api_name: api_name.clone(),
                        },
                    );
                }
            }
        }

        // Update server state
        *self.tools.write().await = tools.clone();
        *self.api_clients.write().await = api_clients;

        let tool_count = tools.len() + config.enhanced_tools.len();
        info!(
            "Loaded {} tools from {} APIs",
            tool_count,
            config.apis.len()
        );

        Ok(())
    }

    /// Handle MCP request
    pub async fn handle_request(&self, request: McpRequest) -> McpResponse {
        debug!("Handling MCP request: {}", request.method);

        match request.method.as_str() {
            "initialize" => match self.handle_initialize(request.params).await {
                Ok(result) => McpResponse::success(request.id, result),
                Err(error) => McpResponse::error(request.id, McpError::from_aether_error(&error)),
            },
            "initialized" => match self.handle_initialized(request.params).await {
                Ok(result) => McpResponse::success(request.id, result),
                Err(error) => McpResponse::error(request.id, McpError::from_aether_error(&error)),
            },
            "notifications/initialized" => {
                debug!("Received notifications/initialized message at lower level");
                match self.handle_initialized(request.params).await {
                    Ok(result) => McpResponse::success(request.id, result),
                    Err(error) => {
                        McpResponse::error(request.id, McpError::from_aether_error(&error))
                    }
                }
            }
            "tools/list" => match self.handle_list_tools(request.params).await {
                Ok(result) => McpResponse::success(request.id, result),
                Err(error) => McpResponse::error(request.id, McpError::from_aether_error(&error)),
            },
            "tools/call" => match self.handle_call_tool(request.params, request.forwarded_authorization).await {
                Ok(result) => McpResponse::success(request.id, result),
                Err(error) => McpResponse::error(request.id, McpError::from_aether_error(&error)),
            },
            "resources/list" => match self.handle_list_resources(request.params).await {
                Ok(result) => McpResponse::success(request.id, result),
                Err(error) => McpResponse::error(request.id, McpError::from_aether_error(&error)),
            },
            "prompts/list" => match self.handle_list_prompts(request.params).await {
                Ok(result) => McpResponse::success(request.id, result),
                Err(error) => McpResponse::error(request.id, McpError::from_aether_error(&error)),
            },
            "ping" => match self.handle_ping().await {
                Ok(result) => McpResponse::success(request.id, result),
                Err(error) => McpResponse::error(request.id, McpError::from_aether_error(&error)),
            },
            _ => {
                warn!("Unknown method: {}", request.method);
                McpResponse::error(request.id, McpError::method_not_found())
            }
        }
    }

    /// List all available tools
    pub async fn list_tools(&self) -> Result<Vec<Tool>, AetherError> {
        let config = self.config.read().await;
        let pinned_count = config.pinned_tools.len();
        
        info!("📋 list_tools called - pinned_tools count: {}", pinned_count);
        if pinned_count > 0 {
            info!("📋 Pinned tools: {:?}", config.pinned_tools);
        }
        
        if config.pinned_tools.is_empty() {
            info!("📋 Using full mode (all tools)");
            drop(config);
            self.list_all_tools().await
        } else {
            info!("📋 Using compact mode (pinned + search tools)");
            let pinned = config.pinned_tools.clone();
            let apis: Vec<String> = config.apis.keys().cloned().collect();
            drop(config);
            self.list_compact_tools(&pinned, &apis).await
        }
    }

    /// List all tools (full mode - no pinning)
    async fn list_all_tools(&self) -> Result<Vec<Tool>, AetherError> {
        let config = self.config.read().await;
        let mut tool_list = Vec::new();

        if !config.enhanced_tools.is_empty() {
            info!("Using enhanced tools from generated config");

            for (tool_name, enhanced_tool) in &config.enhanced_tools {
                let input_schema = self.create_enhanced_tool_schema(enhanced_tool).await;

                let auth_spec = if let Some(api_config) = config.apis.get(&enhanced_tool.api_name) {
                    if let Some(api_auth) = &api_config.authentication {
                        Some(api_auth.to_auth_spec()?)
                    } else {
                        None
                    }
                } else {
                    None
                };

                tool_list.push(Tool {
                    name: tool_name.clone(),
                    description: enhanced_tool.description.clone(),
                    parameters: input_schema,
                    auth_spec,
                });
            }
        } else {
            info!("Using dynamically generated tools");
            let tools = self.tools.read().await;

            for (tool_name, tool_def) in tools.iter() {
                let input_schema = self.create_tool_schema(&tool_def.operation).await;

                let auth_spec = if let Some(api_config) = config.apis.get(&tool_def.api_name) {
                    if let Some(api_auth) = &api_config.authentication {
                        Some(api_auth.to_auth_spec()?)
                    } else {
                        None
                    }
                } else {
                    None
                };

                tool_list.push(Tool {
                    name: tool_name.clone(),
                    description: if tool_def.operation.description.is_empty() {
                        format!(
                            "{} operation from {} API",
                            tool_def.operation.operation_id, tool_def.api_name
                        )
                    } else {
                        tool_def.operation.description.clone()
                    },
                    parameters: input_schema,
                    auth_spec,
                });
            }
        }

        debug!("Listed {} tools", tool_list.len());
        Ok(tool_list)
    }

    /// List compact tools (pinned + search tools)
    async fn list_compact_tools(&self, pinned_tools: &[String], apis: &[String]) -> Result<Vec<Tool>, AetherError> {
        let mut tool_list = Vec::new();
        let all_tools = self.list_all_tools_for_compact().await?;
        let tools_map: HashMap<String, Tool> = all_tools.into_iter().map(|t| (t.name.clone(), t)).collect();

        for pinned_name in pinned_tools {
            if let Some(tool) = tools_map.get(pinned_name) {
                tool_list.push(tool.clone());
            } else {
                warn!("Pinned tool '{}' not found", pinned_name);
            }
        }

        for api_name in apis {
            tool_list.extend(self.generate_search_tools(api_name));
        }

        info!("Listed {} tools (compact mode)", tool_list.len());
        Ok(tool_list)
    }

    /// List all tools without filtering (helper for compact mode)
    async fn list_all_tools_for_compact(&self) -> Result<Vec<Tool>, AetherError> {
        let config = self.config.read().await;
        let mut tool_list = Vec::new();

        if !config.enhanced_tools.is_empty() {
            for (tool_name, enhanced_tool) in &config.enhanced_tools {
                let input_schema = self.create_enhanced_tool_schema(enhanced_tool).await;
                let auth_spec = if let Some(api_config) = config.apis.get(&enhanced_tool.api_name) {
                    if let Some(api_auth) = &api_config.authentication {
                        Some(api_auth.to_auth_spec()?)
                    } else {
                        None
                    }
                } else {
                    None
                };

                tool_list.push(Tool {
                    name: tool_name.clone(),
                    description: enhanced_tool.description.clone(),
                    parameters: input_schema,
                    auth_spec,
                });
            }
        } else {
            let tools = self.tools.read().await;
            for (tool_name, tool_def) in tools.iter() {
                let input_schema = self.create_tool_schema(&tool_def.operation).await;
                let auth_spec = if let Some(api_config) = config.apis.get(&tool_def.api_name) {
                    if let Some(api_auth) = &api_config.authentication {
                        Some(api_auth.to_auth_spec()?)
                    } else {
                        None
                    }
                } else {
                    None
                };

                tool_list.push(Tool {
                    name: tool_name.clone(),
                    description: if tool_def.operation.description.is_empty() {
                        format!(
                            "{} operation from {} API",
                            tool_def.operation.operation_id, tool_def.api_name
                        )
                    } else {
                        tool_def.operation.description.clone()
                    },
                    parameters: input_schema,
                    auth_spec,
                });
            }
        }

        Ok(tool_list)
    }

    /// Generate synthetic search tools for an API
    fn generate_search_tools(&self, api_name: &str) -> Vec<Tool> {
        vec![
            Tool {
                name: format!("{}_search_tools", api_name),
                description: format!(
                    "Search for tools in the {} API by name or description. Returns matching tool definitions that can be called directly.",
                    api_name
                ),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search terms to match against tool names and descriptions"
                        }
                    },
                    "required": ["query"]
                }),
                auth_spec: None,
            },
            Tool {
                name: format!("{}_list_tools", api_name),
                description: format!(
                    "List all available tools in the {} API. Returns complete tool definitions.",
                    api_name
                ),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                auth_spec: None,
            },
        ]
    }

    /// Call a specific tool
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: HashMap<String, Value>,
        authorization: Option<Authorization>,
    ) -> Result<ToolResponse, AetherError> {
        info!("🔧 Calling tool: {}", tool_name);

        if let Some(response) = self.handle_synthetic_tool(tool_name, &arguments).await? {
            return Ok(response);
        }

        let config = self.config.read().await;

        if let Some(enhanced_tool) = config.enhanced_tools.get(tool_name) {
            info!("📋 Found enhanced tool definition for: {}", tool_name);
            return self
                .call_enhanced_tool(enhanced_tool, arguments, authorization)
                .await;
        }

        let tools = self.tools.read().await;
        if let Some(tool_def) = tools.get(tool_name) {
            let tool_def = tool_def.clone();
            drop(tools);
            debug!(
                "📋 Found regular tool definition for: {} (api: {})",
                tool_name, tool_def.api_name
            );
            return self
                .call_regular_tool(&tool_def, arguments, authorization)
                .await;
        }

        error!("❌ Tool not found: {}", tool_name);
        Err(AetherError::Mcp(format!("Tool not found: {}", tool_name)))
    }

    /// Handle synthetic search/list tools
    async fn handle_synthetic_tool(
        &self,
        tool_name: &str,
        arguments: &HashMap<String, Value>,
    ) -> Result<Option<ToolResponse>, AetherError> {
        if let Some(api_name) = tool_name.strip_suffix("_search_tools") {
            let query = arguments
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AetherError::Mcp("query parameter required".to_string()))?;

            info!("🔍 Searching tools for API '{}' with query: {}", api_name, query);
            let tools = self.search_tools_by_api(api_name, query).await?;
            
            return Ok(Some(ToolResponse {
                content: serde_json::to_value(tools)
                    .unwrap_or_else(|_| serde_json::json!({"error": "Failed to serialize tools"})),
                is_error: false,
            }));
        }

        if let Some(api_name) = tool_name.strip_suffix("_list_tools") {
            info!("📋 Listing all tools for API: {}", api_name);
            let tools = self.list_tools_by_api(api_name).await?;
            
            return Ok(Some(ToolResponse {
                content: serde_json::to_value(tools)
                    .unwrap_or_else(|_| serde_json::json!({"error": "Failed to serialize tools"})),
                is_error: false,
            }));
        }

        Ok(None)
    }

    /// Search tools by API with fuzzy matching
    async fn search_tools_by_api(&self, api_name: &str, query: &str) -> Result<Vec<Tool>, AetherError> {
        let all_tools = self.list_all_tools_for_compact().await?;
        
        Ok(all_tools
            .into_iter()
            .filter(|t| t.name.starts_with(&format!("{}_", api_name)))
            .filter(|t| self.matches_query(&t.name, &t.description, query))
            .collect())
    }

    /// List all tools for a specific API
    async fn list_tools_by_api(&self, api_name: &str) -> Result<Vec<Tool>, AetherError> {
        let all_tools = self.list_all_tools_for_compact().await?;
        
        Ok(all_tools
            .into_iter()
            .filter(|t| t.name.starts_with(&format!("{}_", api_name)))
            .collect())
    }

    /// Fuzzy match query against tool name and description
    fn matches_query(&self, name: &str, description: &str, query: &str) -> bool {
        let query_lower = query.to_lowercase();
        let name_lower = name.to_lowercase();
        let desc_lower = description.to_lowercase();

        for word in query_lower.split_whitespace() {
            if name_lower.contains(word) || desc_lower.contains(word) {
                return true;
            }
        }

        let full_query_score = jaro_winkler(&query_lower, &name_lower).max(jaro_winkler(&query_lower, &desc_lower));
        if full_query_score > 0.6 {
            return true;
        }

        false
    }

    /// Call an enhanced tool (with parameter source metadata)
    async fn call_enhanced_tool(
        &self,
        enhanced_tool: &aether_core::types::EnhancedToolConfig,
        mut arguments: HashMap<String, Value>,
        authorization: Option<Authorization>,
    ) -> Result<ToolResponse, AetherError> {
        info!("🚀 Executing enhanced tool: {}", enhanced_tool.name);

        // Augment arguments with server-provided parameters
        for param in &enhanced_tool.parameters {
            if param.source == aether_core::types::ParameterSource::Server {
                if let Some(env_var) = &param.env_var {
                    match std::env::var(env_var) {
                        Ok(value) => {
                            info!(
                                "🔧 Auto-filling parameter '{}' from env var '{}' = '{}'",
                                param.name, env_var, value
                            );
                            arguments.insert(param.name.clone(), Value::String(value));
                        }
                        Err(_) => {
                            if param.required {
                                error!("❌ Required environment variable '{}' not set for parameter '{}'", env_var, param.name);
                                return Err(AetherError::Api(format!(
                                    "Environment variable '{}' is not set for parameter '{}'",
                                    env_var, param.name
                                )));
                            } else {
                                warn!("⚠️  Optional environment variable '{}' not set for parameter '{}'", env_var, param.name);
                            }
                        }
                    }
                } else {
                    warn!(
                        "⚠️  Server parameter '{}' has no env_var configured",
                        param.name
                    );
                }
            }
        }

        // Create an Operation from the enhanced tool config to execute with existing client
        let operation = aether_config::openapi::Operation {
            operation_id: enhanced_tool.operation_id.clone(),
            path: enhanced_tool.path.clone(),
            method: enhanced_tool.method.clone(),
            summary: enhanced_tool.name.clone(),
            description: enhanced_tool.description.clone(),
            parameters: enhanced_tool
                .parameters
                .iter()
                .map(|p| aether_config::openapi::Parameter {
                    name: p.name.clone(),
                    param_type: p.param_type.clone(),
                    required: p.required,
                    schema: p.schema.clone(),
                })
                .collect(),
        };

        // Get the API client and configuration
        let api_clients = self.api_clients.read().await;
        let api_client = api_clients.get(&enhanced_tool.api_name).ok_or_else(|| {
            error!("❌ API client not found: {}", enhanced_tool.api_name);
            AetherError::Mcp(format!("API client not found: {}", enhanced_tool.api_name))
        })?;

        // Determine authorization and auth_spec to use
        let config = self.config.read().await;
        let (effective_auth_spec, effective_authorization) =
            if let Some(api_config) = config.apis.get(&enhanced_tool.api_name) {
                if let Some(api_auth) = &api_config.authentication {
                    info!(
                        "🔐 Creating authentication for API: {}",
                        enhanced_tool.api_name
                    );
                    let auth_spec = api_auth.to_auth_spec()?;

                    if api_auth.auth_type == "passthrough" {
                        // Passthrough: use the forwarded token from the MCP request
                        if let Some(ref fwd) = authorization {
                            if let Some(token) = fwd.credentials.get("token") {
                                debug!("✅ Passthrough auth — using forwarded token for {}",
                                    enhanced_tool.api_name);
                                let mut creds = HashMap::new();
                                creds.insert("token".to_string(), token.clone());
                                (Some(auth_spec), Some(Authorization {
                                    auth_type: "bearer".to_string(),
                                    credentials: creds,
                                }))
                            } else {
                                return Err(AetherError::Authentication(
                                    "Passthrough auth requires a token".to_string(),
                                ));
                            }
                        } else {
                            return Err(AetherError::Authentication(
                                "Passthrough auth requires a token".to_string(),
                            ));
                        }
                    } else {
                        let authorization = self.create_authorization_from_api_auth(api_auth)?;
                        debug!("✅ Auth spec created: {:?}", auth_spec);
                        (Some(auth_spec), Some(authorization))
                    }
                } else {
                    if let Some(auth) = authorization {
                        info!(
                            "🔐 Using provided authorization for API: {}",
                            enhanced_tool.api_name
                        );
                        (None, Some(auth))
                    } else {
                        info!(
                            "ℹ️  No authentication configured for API: {}",
                            enhanced_tool.api_name
                        );
                        (None, None)
                    }
                }
            } else {
                error!("❌ API configuration not found: {}", enhanced_tool.api_name);
                (None, None)
            };

        info!(
            "🚀 Executing enhanced operation: {}",
            enhanced_tool.operation_id
        );
        debug!("📝 Parameters: {:?}", arguments);

        // Execute the operation
        let response = api_client
            .execute_operation(
                &operation,
                &arguments,
                effective_auth_spec.as_ref(),
                effective_authorization.as_ref(),
            )
            .await?;

        info!(
            "✅ Enhanced tool execution completed for: {}",
            enhanced_tool.name
        );

        Ok(response)
    }

    /// Call a regular tool (dynamically generated from OpenAPI)
    async fn call_regular_tool(
        &self,
        tool_def: &ToolDefinition,
        arguments: HashMap<String, Value>,
        authorization: Option<Authorization>,
    ) -> Result<ToolResponse, AetherError> {
        // Get the API client and configuration
        let api_clients = self.api_clients.read().await;
        let api_client = api_clients.get(&tool_def.api_name).ok_or_else(|| {
            error!("❌ API client not found: {}", tool_def.api_name);
            AetherError::Mcp(format!("API client not found: {}", tool_def.api_name))
        })?;

        // Determine authorization and auth_spec to use
        let config = self.config.read().await;
        let (effective_auth_spec, effective_authorization) =
            if let Some(api_config) = config.apis.get(&tool_def.api_name) {
                if let Some(api_auth) = &api_config.authentication {
                    info!("🔐 Creating authentication for API: {}", tool_def.api_name);
                    let auth_spec = api_auth.to_auth_spec()?;

                    if api_auth.auth_type == "passthrough" {
                        // Passthrough: use the forwarded token from the MCP request
                        if let Some(ref fwd) = authorization {
                            if let Some(token) = fwd.credentials.get("token") {
                                debug!("✅ Passthrough auth — using forwarded token for {}",
                                    tool_def.api_name);
                                let mut creds = HashMap::new();
                                creds.insert("token".to_string(), token.clone());
                                (Some(auth_spec), Some(Authorization {
                                    auth_type: "bearer".to_string(),
                                    credentials: creds,
                                }))
                            } else {
                                return Err(AetherError::Authentication(
                                    "Passthrough auth requires a token".to_string(),
                                ));
                            }
                        } else {
                            return Err(AetherError::Authentication(
                                "Passthrough auth requires a token".to_string(),
                            ));
                        }
                    } else {
                        let authorization = self.create_authorization_from_api_auth(api_auth)?;
                        debug!("✅ Auth spec created: {:?}", auth_spec);
                        debug!(
                            "✅ Authorization created with credentials: {:?}",
                            authorization.credentials.keys().collect::<Vec<_>>()
                        );
                        (Some(auth_spec), Some(authorization))
                    }
                } else {
                    if let Some(auth) = authorization {
                        info!(
                            "🔐 Using provided authorization for API: {}",
                            tool_def.api_name
                        );
                        (None, Some(auth))
                    } else {
                        info!(
                            "ℹ️  No authentication configured for API: {}",
                            tool_def.api_name
                        );
                        (None, None)
                    }
                }
            } else {
                error!("❌ API configuration not found: {}", tool_def.api_name);
                (None, None)
            };

        info!(
            "🚀 Executing operation: {}",
            tool_def.operation.operation_id
        );
        debug!("📝 Parameters: {:?}", arguments);

        // Execute the operation
        let response = api_client
            .execute_operation(
                &tool_def.operation,
                &arguments,
                effective_auth_spec.as_ref(),
                effective_authorization.as_ref(),
            )
            .await?;

        info!(
            "✅ Tool execution completed for: {}",
            tool_def.operation.operation_id
        );

        Ok(response)
    }

    /// Get server statistics
    pub async fn get_stats(&self) -> HashMap<String, Value> {
        let mut stats = HashMap::new();
        let tools = self.tools.read().await;
        let config = self.config.read().await;

        let total_tools = tools.len() + config.enhanced_tools.len();
        stats.insert("tools_count".to_string(), Value::Number(total_tools.into()));
        stats.insert(
            "regular_tools_count".to_string(),
            Value::Number(tools.len().into()),
        );
        stats.insert(
            "enhanced_tools_count".to_string(),
            Value::Number(config.enhanced_tools.len().into()),
        );
        stats.insert(
            "apis_count".to_string(),
            Value::Number(config.apis.len().into()),
        );

        stats
    }

    /// Create Authorization object from API authentication configuration
    fn create_authorization_from_api_auth(
        &self,
        api_auth: &ApiAuthentication,
    ) -> std::result::Result<Authorization, AetherError> {
        let credentials = match api_auth.auth_type.as_str() {
            "bearer" => {
                let mut creds = HashMap::new();
                if let Some(token) = &api_auth.token {
                    let resolved = ApiAuthentication::resolve_env_var(token)?;
                    creds.insert("token".to_string(), resolved);
                } else {
                    return Err(AetherError::Configuration(
                        "Bearer authentication requires a 'token' field".to_string(),
                    ));
                }
                creds
            }
            "custom" => {
                if let Some(headers) = &api_auth.headers {
                    // For custom auth, use the header values as credentials
                    let mut creds = HashMap::new();
                    for (key, value) in headers {
                        // Resolve environment variables
                        let resolved_value = ApiAuthentication::resolve_env_var(value)?;
                        creds.insert(key.clone(), resolved_value);
                    }
                    creds
                } else {
                    HashMap::new()
                }
            }
            "passthrough" => {
                // Passthrough: token comes from forwarded Authorization header,
                // not from config. Return empty credentials here; actual token
                // is injected via forwarded_authorization in handle_call_tool.
                HashMap::new()
            }
            _ => HashMap::new(),
        };

        Ok(Authorization {
            auth_type: api_auth.auth_type.clone(),
            credentials,
        })
    }

    /// Create tool input schema from OpenAPI operation
    async fn create_tool_schema(&self, operation: &Operation) -> serde_json::Value {
        let mut properties = std::collections::HashMap::new();

        // Add parameters from the operation
        for param in &operation.parameters {
            let param_name = param.name.clone();

            // Extract type from schema
            let param_type =
                if let Some(schema_type) = param.schema.get("type").and_then(|t| t.as_str()) {
                    schema_type
                } else {
                    "string" // default to string
                };

            let param_schema = match param_type {
                "string" => serde_json::json!({"type": "string"}),
                "integer" => serde_json::json!({"type": "integer"}),
                "number" => serde_json::json!({"type": "number"}),
                "boolean" => serde_json::json!({"type": "boolean"}),
                _ => serde_json::json!({"type": "string"}), // default to string
            };

            let mut property = serde_json::Map::new();
            property.insert("type".to_string(), param_schema["type"].clone());

            // Add description from parameter schema if available
            if let Some(desc) = param.schema.get("description").and_then(|d| d.as_str()) {
                property.insert(
                    "description".to_string(),
                    serde_json::Value::String(desc.to_string()),
                );
            }

            properties.insert(param_name, serde_json::Value::Object(property));
        }

        serde_json::json!({
            "type": "object",
            "properties": properties
        })
    }

    /// Create tool input schema from enhanced tool configuration (only user parameters)
    async fn create_enhanced_tool_schema(
        &self,
        enhanced_tool: &aether_core::types::EnhancedToolConfig,
    ) -> serde_json::Value {
        let mut properties = std::collections::HashMap::new();
        let mut required = Vec::new();

        // Only include user-provided parameters in the schema
        for param in &enhanced_tool.parameters {
            if param.source == aether_core::types::ParameterSource::User {
                let param_name = param.name.clone();

                // Use the full parameter schema as the base
                let mut property = param.schema.as_object().cloned().unwrap_or_else(|| {
                    let mut m = serde_json::Map::new();
                    m.insert(
                        "type".to_string(),
                        serde_json::Value::String("string".to_string()),
                    );
                    m
                });

                // Add or override description
                if let Some(desc) = &param.description {
                    property.insert(
                        "description".to_string(),
                        serde_json::Value::String(desc.clone()),
                    );
                } else if let Some(desc) = param.schema.get("description").and_then(|d| d.as_str())
                {
                    property.insert(
                        "description".to_string(),
                        serde_json::Value::String(desc.to_string()),
                    );
                }

                properties.insert(param_name.clone(), serde_json::Value::Object(property));

                // Add to required if this parameter is required
                if param.required {
                    required.push(param_name);
                }
            }
        }

        let mut schema = serde_json::json!({
            "type": "object",
            "properties": properties
        });

        if !required.is_empty() {
            schema["required"] = serde_json::json!(required);
        }

        schema
    }

    /// Handle tools/list request
    async fn handle_list_tools(
        &self,
        params: Option<Value>,
    ) -> std::result::Result<Value, AetherError> {
        // Parse parameters (currently ignoring cursor for pagination)
        if let Some(params_value) = params {
            let _params: ListToolsParams = serde_json::from_value(params_value)
                .map_err(|e| AetherError::Mcp(format!("Invalid parameters: {}", e)))?;
        }

        let tools = self.list_tools().await?;
        Ok(tools_to_response(tools))
    }

    /// Handle tools/call request
    async fn handle_call_tool(
        &self,
        params: Option<Value>,
        forwarded_authorization: Option<String>,
    ) -> std::result::Result<Value, AetherError> {
        let params_value = params
            .ok_or_else(|| AetherError::Mcp("Missing parameters for tools/call".to_string()))?;

        let params: CallToolParams = serde_json::from_value(params_value)
            .map_err(|e| AetherError::Mcp(format!("Invalid parameters: {}", e)))?;

        // Build effective authorization:
        // 1. MCP-level authorization from params (if client sends it)
        // 2. Forwarded Authorization header from middleware (for passthrough auth)
        let effective_auth = params.authorization.or_else(|| {
            forwarded_authorization.map(|auth_header| {
                // Parse "Bearer <token>" from the forwarded header
                let token = auth_header.strip_prefix("Bearer ").unwrap_or(&auth_header).to_string();
                let mut creds = HashMap::new();
                creds.insert("token".to_string(), token);
                Authorization {
                    auth_type: "bearer".to_string(),
                    credentials: creds,
                }
            })
        });

        let arguments = params.arguments.unwrap_or_default();
        let response = self
            .call_tool(&params.name, arguments, effective_auth)
            .await?;

        Ok(tool_response_to_mcp(response))
    }

    /// Handle ping request
    async fn handle_ping(&self) -> std::result::Result<Value, AetherError> {
        debug!("Handling ping request");
        Ok(serde_json::json!({"pong": true}))
    }

    /// Handle initialize request
    async fn handle_initialize(
        &self,
        _params: Option<Value>,
    ) -> std::result::Result<Value, AetherError> {
        debug!("Handling initialize request");

        // Return server capabilities
        Ok(serde_json::json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {
                "tools": {
                    "listChanged": false
                },
                "resources": {
                    "subscribe": false,
                    "listChanged": false
                },
                "prompts": {
                    "listChanged": false
                }
            },
            "serverInfo": {
                "name": "aether-mcp",
                "version": env!("CARGO_PKG_VERSION")
            }
        }))
    }

    /// Handle initialized notification
    async fn handle_initialized(
        &self,
        _params: Option<Value>,
    ) -> std::result::Result<Value, AetherError> {
        debug!("Handling initialized notification");
        // This is a notification, so we return an empty result
        Ok(serde_json::json!({}))
    }

    /// Handle resources/list request (return empty list as we don't support resources yet)
    async fn handle_list_resources(
        &self,
        _params: Option<Value>,
    ) -> std::result::Result<Value, AetherError> {
        debug!("Handling resources/list request");
        Ok(serde_json::json!({
            "resources": []
        }))
    }

    /// Handle prompts/list request (return empty list as we don't support prompts yet)
    async fn handle_list_prompts(
        &self,
        _params: Option<Value>,
    ) -> std::result::Result<Value, AetherError> {
        debug!("Handling prompts/list request");
        Ok(serde_json::json!({
            "prompts": []
        }))
    }
}
