//! Configuration generation from OpenAPI specifications

use crate::{
    openapi::{OpenApiProcessor, Parameter},
    AetherConfig, ApiConfig, ServerConfig,
};
use aether_core::{
    types::{EnhancedToolConfig, ParameterInfo, ParameterSource},
    AetherError, Result,
};
use serde_json::Value;
use std::collections::HashMap;
use tracing::info;

/// Generate Aether configuration from OpenAPI specification content
pub async fn generate_config_from_openapi(
    openapi_content: &str,
    api_name: &str,
    base_url: &str,
) -> Result<AetherConfig> {
    info!("🔧 Generating Aether config for API: {}", api_name);

    // Parse OpenAPI specification
    let spec: Value = if openapi_content.trim_start().starts_with('{') {
        serde_json::from_str(openapi_content).map_err(|e| {
            AetherError::Configuration(format!("Failed to parse OpenAPI JSON: {}", e))
        })?
    } else {
        serde_yaml::from_str(openapi_content).map_err(|e| {
            AetherError::Configuration(format!("Failed to parse OpenAPI YAML: {}", e))
        })?
    };

    // Create OpenAPI processor
    let processor = OpenApiProcessor::from_spec(spec);
    let operations = processor.extract_operations()?;

    info!(
        "✅ Extracted {} operations from OpenAPI spec",
        operations.len()
    );

    // Generate enhanced tools with parameter source analysis
    let mut enhanced_tools = HashMap::new();

    for operation in operations {
        let tool_name = format!("{}_{}", api_name, operation.operation_id);

        // Analyze parameters to determine their source
        let parameters = analyze_parameters(&operation.parameters)?;

        let enhanced_tool = EnhancedToolConfig {
            name: tool_name.clone(),
            description: if operation.description.is_empty() {
                format!("{} operation from {} API", operation.operation_id, api_name)
            } else {
                operation.description.clone()
            },
            api_name: api_name.to_string(),
            operation_id: operation.operation_id,
            path: operation.path,
            method: operation.method,
            parameters,
            auth_spec: None, // Will be determined from API config if needed
        };

        enhanced_tools.insert(tool_name, enhanced_tool);
    }

    info!(
        "✅ Generated {} enhanced tools with parameter source analysis",
        enhanced_tools.len()
    );

    // Create API configuration
    let mut apis = HashMap::new();
    apis.insert(
        api_name.to_string(),
        ApiConfig {
            base_url: base_url.to_string(),
            openapi_spec: "".to_string(), // Not needed for generated configs
            timeout: Some(30),
            authentication: None, // Will be manually configured if needed
        },
    );

    // Create the configuration
    let config = AetherConfig {
        server: Some(ServerConfig::default()),
        tools: HashMap::new(), // Use enhanced_tools instead
        enhanced_tools,
        apis,
        oidc: None,
        session: None,
        dev: None,
        mcp_auth: None,
        pinned_tools: Vec::new(),
        prm: None,
    };

    info!("🎉 Successfully generated Aether configuration");
    Ok(config)
}

/// Analyze OpenAPI parameters to determine their source (user vs server)
fn analyze_parameters(params: &[Parameter]) -> Result<Vec<ParameterInfo>> {
    let mut parameter_infos = Vec::new();

    for param in params {
        let (source, env_var) = determine_parameter_source(param);

        let description = param
            .schema
            .get("description")
            .and_then(|d| d.as_str())
            .map(|s| s.to_string());

        let param_info = ParameterInfo {
            name: param.name.clone(),
            param_type: param.param_type.clone(),
            required: param.required,
            schema: param.schema.clone(),
            source,
            env_var,
            description,
        };

        parameter_infos.push(param_info);
    }

    Ok(parameter_infos)
}

/// Determine if a parameter should be provided by user or server
fn determine_parameter_source(param: &Parameter) -> (ParameterSource, Option<String>) {
    // Check if parameter has a default value that looks like an environment variable
    if let Some(default) = param.schema.get("default").and_then(|v| v.as_str()) {
        if default.starts_with("${") && default.ends_with("}") {
            let env_var = default[2..default.len() - 1].to_string();

            // Check if this env var is also in the enum (indicating it's server-provided)
            if let Some(enum_values) = param.schema.get("enum").and_then(|v| v.as_array()) {
                if enum_values.iter().any(|v| v.as_str() == Some(default)) {
                    return (ParameterSource::Server, Some(env_var));
                }
            }

            // Even if not in enum, if it has env var default, it's likely server-provided
            return (ParameterSource::Server, Some(env_var));
        }
    }

    // Check common server-provided parameter names
    let server_param_patterns = [
        "api_key",
        "apikey",
        "token",
        "key",
        "secret",
        "auth",
        "sc_product_id",
        "scproductid",
        "sc_api_key",
        "scapikey",
    ];

    let param_name_lower = param.name.to_lowercase();
    for pattern in &server_param_patterns {
        if param_name_lower.contains(pattern) {
            return (ParameterSource::Server, None);
        }
    }

    // Default to user-provided for all other parameters
    (ParameterSource::User, None)
}
