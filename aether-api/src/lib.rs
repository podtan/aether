//! API client for making HTTP requests to external APIs

use aether_auth::AuthProcessor;
use aether_config::openapi::{Operation, Parameter};
use aether_core::{
    types::{AuthSpec, Authorization, ToolResponse},
    AetherError, Result,
};
use reqwest::{Client, Method, Response};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// API client for making HTTP requests
pub struct ApiClient {
    client: Client,
    base_url: String,
}

impl ApiClient {
    /// Create a new API client
    pub fn new(base_url: String, timeout: Option<Duration>) -> Result<Self> {
        let client = Client::builder()
            .timeout(timeout.unwrap_or(Duration::from_secs(30)))
            .build()
            .map_err(|e| AetherError::Api(format!("Failed to create HTTP client: {}", e)))?;
        
        Ok(Self { client, base_url })
    }
    
    /// Execute an API call based on operation and parameters
    pub async fn execute_operation(
        &self,
        operation: &Operation,
        parameters: &HashMap<String, Value>,
        auth_spec: Option<&AuthSpec>,
        authorization: Option<&Authorization>,
    ) -> Result<ToolResponse> {
        info!("🚀 Executing operation: {}", operation.operation_id);
        debug!("📝 Parameters: {:?}", parameters);
        debug!("🔐 Auth spec provided: {}", auth_spec.is_some());
        debug!("🔑 Authorization provided: {}", authorization.is_some());
        
        // Build the request
        let mut request = self.build_request(operation, parameters)?;
        
        // Apply authentication if required
        if let (Some(auth_spec), Some(authorization)) = (auth_spec, authorization) {
            info!("🔐 Applying authentication to request");
            AuthProcessor::validate_credentials(auth_spec, authorization)?;
            request = AuthProcessor::apply_auth(request, auth_spec, authorization)?;
            info!("✅ Authentication applied successfully");
        } else {
            info!("ℹ️  No authentication to apply");
        }
        
        // Execute the request
        let response = request.send().await
            .map_err(|e| AetherError::Api(format!("Request failed: {}", e)))?;
        
        // Process the response
        self.process_response(response).await
    }
    
    /// Build an HTTP request from operation and parameters
    fn build_request(
        &self,
        operation: &Operation,
        parameters: &HashMap<String, Value>,
    ) -> Result<reqwest::RequestBuilder> {
        // Parse HTTP method
        let method = Method::from_bytes(operation.method.as_bytes())
            .map_err(|e| AetherError::Api(format!("Invalid HTTP method '{}': {}", operation.method, e)))?;
        
        // Build URL with path parameters
        let url = self.build_url(&operation.path, parameters)?;
        
        // Create request builder
        let mut request = self.client.request(method, &url);
        
        // Separate form parameters, body parameters, and other parameters
        let mut form_params = Vec::new();
        let mut body_params = Vec::new();
        let mut other_params = Vec::new();
        
        for param in &operation.parameters {
            match param.param_type.as_str() {
                "form" => form_params.push(param),
                "body" => body_params.push(param),
                _ => other_params.push(param),
            }
        }
        
        // Apply non-form, non-body parameters first
        for param in other_params {
            if let Some(value) = parameters.get(&param.name) {
                request = self.apply_parameter(request, param, value)?;
            } else if param.required {
                return Err(AetherError::Api(format!(
                    "Required parameter '{}' not provided", 
                    param.name
                )));
            }
        }
        
        // Handle body parameters - collect them into a single JSON object
        if !body_params.is_empty() {
            let mut body_json = serde_json::Map::new();
            
            for param in body_params {
                if let Some(value) = parameters.get(&param.name) {
                    body_json.insert(param.name.clone(), value.clone());
                } else if param.required {
                    return Err(AetherError::Api(format!(
                        "Required body parameter '{}' not provided", 
                        param.name
                    )));
                }
            }
            
            if !body_json.is_empty() {
                request = request.json(&Value::Object(body_json));
            }
        }
        
        // If we have form parameters, create form-encoded body
        if !form_params.is_empty() {
            let mut form_data = Vec::new();
            
            for param in form_params {
                let param_value = if let Some(value) = parameters.get(&param.name) {
                    // User provided the parameter
                    self.value_to_string(value)
                } else {
                    // Check if parameter has a default value with environment variable
                    if let Some(default_value) = self.get_parameter_default_value(param)? {
                        default_value
                    } else if param.required {
                        return Err(AetherError::Api(format!(
                            "Required form parameter '{}' not provided", 
                            param.name
                        )));
                    } else {
                        continue; // Skip optional parameters
                    }
                };
                
                form_data.push((param.name.clone(), param_value));
            }
            
            request = request.form(&form_data);
        }
        
        debug!("Built request for URL: {}", url);
        Ok(request)
    }

    /// Get default value for parameter, resolving environment variables if needed
    fn get_parameter_default_value(&self, param: &Parameter) -> Result<Option<String>> {
        // Check if parameter schema has a default value
        if let Some(default) = param.schema.get("default") {
            if let Some(default_str) = default.as_str() {
                // Check if default value is an environment variable placeholder
                if default_str.starts_with("${") && default_str.ends_with("}") {
                    let env_var = &default_str[2..default_str.len()-1];
                    match std::env::var(env_var) {
                        Ok(value) => return Ok(Some(value)),
                        Err(_) => {
                            // If environment variable is not set, check if it's in enum (for validation)
                            if let Some(enum_values) = param.schema.get("enum") {
                                if let Some(enum_array) = enum_values.as_array() {
                                    // If the env var placeholder is in enum, it means this parameter is auto-filled
                                    if enum_array.iter().any(|v| v.as_str() == Some(default_str)) {
                                        return Err(AetherError::Api(format!(
                                            "Environment variable '{}' is not set for parameter '{}'", 
                                            env_var, param.name
                                        )));
                                    }
                                }
                            }
                        }
                    }
                } else {
                    // Regular default value
                    return Ok(Some(default_str.to_string()));
                }
            } else if let Some(default_num) = default.as_number() {
                return Ok(Some(default_num.to_string()));
            } else if let Some(default_bool) = default.as_bool() {
                return Ok(Some(default_bool.to_string()));
            }
        }
        
        Ok(None)
    }
    
    /// Build URL with path parameters substituted
    fn build_url(&self, path: &str, parameters: &HashMap<String, Value>) -> Result<String> {
        let mut url = format!("{}{}", self.base_url, path);
        
        // Replace path parameters
        for (name, value) in parameters {
            let placeholder = format!("{{{}}}", name);
            if url.contains(&placeholder) {
                let value_str = match value {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => value.to_string(),
                };
                url = url.replace(&placeholder, &value_str);
            }
        }
        
        Ok(url)
    }
    
    /// Apply a parameter to the request
    fn apply_parameter(
        &self,
        request: reqwest::RequestBuilder,
        param: &Parameter,
        value: &Value,
    ) -> Result<reqwest::RequestBuilder> {
        match param.param_type.as_str() {
            "query" => {
                let value_str = self.value_to_string(value);
                Ok(request.query(&[(param.name.as_str(), value_str)]))
            },
            "header" => {
                let value_str = self.value_to_string(value);
                Ok(request.header(&param.name, value_str))
            },
            "body" => {
                // Body parameters are now handled collectively in build_request
                Ok(request)
            },
            "form" => {
                // Form parameters are handled in build_request
                Ok(request)
            },
            "path" => {
                // Path parameters are handled in build_url
                Ok(request)
            },
            _ => {
                warn!("Unknown parameter type: {}", param.param_type);
                Ok(request)
            }
        }
    }
    
    /// Convert a JSON value to string for use in headers/query params
    fn value_to_string(&self, value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Array(arr) => {
                // Join array values with commas
                arr.iter()
                    .map(|v| self.value_to_string(v))
                    .collect::<Vec<_>>()
                    .join(",")
            },
            _ => value.to_string(),
        }
    }
    
    /// Process HTTP response into ToolResponse
    async fn process_response(&self, response: Response) -> Result<ToolResponse> {
        let status = response.status();
        let headers = response.headers().clone();
        
        debug!("Received response with status: {}", status);
        
        // Check if response indicates an error
        let is_error = !status.is_success();
        
        // Get response body
        let body_text = response.text().await
            .map_err(|e| AetherError::Api(format!("Failed to read response body: {}", e)))?;
        
        // Try to parse as JSON, fall back to string
        let content = if body_text.is_empty() {
            Value::Null
        } else {
            match serde_json::from_str::<Value>(&body_text) {
                Ok(json) => json,
                Err(_) => {
                    // If not valid JSON, return as string
                    Value::String(body_text)
                }
            }
        };
        
        // Create response with metadata
        let mut response_data = Map::new();
        response_data.insert("status".to_string(), Value::Number(status.as_u16().into()));
        response_data.insert("body".to_string(), content);
        
        // Add headers if they contain useful information
        let mut headers_map = Map::new();
        for (name, value) in headers.iter() {
            if let Ok(value_str) = value.to_str() {
                headers_map.insert(name.to_string(), Value::String(value_str.to_string()));
            }
        }
        if !headers_map.is_empty() {
            response_data.insert("headers".to_string(), Value::Object(headers_map));
        }
        
        if is_error {
            error!("API request failed with status: {}", status);
        } else {
            info!("API request completed successfully");
        }
        
        Ok(ToolResponse {
            content: Value::Object(response_data),
            is_error,
        })
    }
}