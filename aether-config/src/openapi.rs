//! OpenAPI specification processing

use aether_core::{AetherError, Result};
use serde_json::Value;
use std::path::Path;
use tracing::info;

/// OpenAPI specification loader and processor
#[derive(Debug, Clone)]
pub struct OpenApiProcessor {
    spec: Value,
}

impl OpenApiProcessor {
    /// Load OpenAPI specification from file
    pub async fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        info!("Loading OpenAPI specification from {:?}", path.as_ref());
        
        let content = tokio::fs::read_to_string(path.as_ref()).await
            .map_err(|e| AetherError::Configuration(format!("Failed to read OpenAPI file: {}", e)))?;
        
        // Try parsing as JSON first, then YAML
        let spec = if path.as_ref().extension().map_or(false, |ext| ext == "json") {
            serde_json::from_str(&content)
                .map_err(|e| AetherError::Configuration(format!("Failed to parse OpenAPI JSON: {}", e)))?
        } else {
            serde_yaml::from_str(&content)
                .map_err(|e| AetherError::Configuration(format!("Failed to parse OpenAPI YAML: {}", e)))?
        };
        
        let processor = Self { spec };
        processor.validate()?;
        
        info!("OpenAPI specification loaded successfully");
        Ok(processor)
    }
    
    /// Create OpenAPI processor from parsed specification
    pub fn from_spec(spec: Value) -> Self {
        Self { spec }
    }
    
    /// Validate the OpenAPI specification
    pub fn validate(&self) -> Result<()> {
        if !self.spec.is_object() {
            return Err(AetherError::Configuration("OpenAPI spec must be an object".to_string()));
        }
        
        let obj = self.spec.as_object().unwrap();
        
        if !obj.contains_key("openapi") && !obj.contains_key("swagger") {
            return Err(AetherError::Configuration("Invalid OpenAPI specification: missing version field".to_string()));
        }
        
        if !obj.contains_key("paths") {
            return Err(AetherError::Configuration("OpenAPI specification must contain 'paths'".to_string()));
        }
        
        Ok(())
    }
    
    /// Extract all operations from the OpenAPI specification
    pub fn extract_operations(&self) -> Result<Vec<Operation>> {
        let paths = self.spec.get("paths")
            .and_then(|p| p.as_object())
            .ok_or_else(|| AetherError::Configuration("Invalid paths in OpenAPI spec".to_string()))?;
        
        let mut operations = Vec::new();
        
        for (path, path_item) in paths {
            if let Some(path_obj) = path_item.as_object() {
                for (method, operation) in path_obj {
                    if method.starts_with('x') || !is_http_method(method) {
                        continue;
                    }
                    
                    if let Some(op_obj) = operation.as_object() {
                        let operation_id = op_obj.get("operationId")
                            .and_then(|v| v.as_str())
                            .unwrap_or(&format!("{}_{}", method, path.replace('/', "_")))
                            .to_string();
                        
                        let summary = op_obj.get("summary")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        
                        let description = op_obj.get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or(&summary)
                            .to_string();
                        
                        let mut parameters = extract_parameters(op_obj)?;
                        
                        // Also extract parameters from requestBody for form-encoded requests
                        if let Some(request_body_params) = extract_request_body_parameters(op_obj)? {
                            parameters.extend(request_body_params);
                        }
                        
                        operations.push(Operation {
                            operation_id,
                            path: path.clone(),
                            method: method.to_uppercase(),
                            summary,
                            description,
                            parameters,
                        });
                    }
                }
            }
        }
        
        info!("Extracted {} operations from OpenAPI spec", operations.len());
        Ok(operations)
    }
}

/// Represents an OpenAPI operation
#[derive(Debug, Clone)]
pub struct Operation {
    pub operation_id: String,
    pub path: String,
    pub method: String,
    pub summary: String,
    pub description: String,
    pub parameters: Vec<Parameter>,
}

/// Represents an OpenAPI parameter
#[derive(Debug, Clone)]
pub struct Parameter {
    pub name: String,
    pub param_type: String, // "query", "path", "header", "body"
    pub required: bool,
    pub schema: Value,
}

fn is_http_method(method: &str) -> bool {
    matches!(method.to_lowercase().as_str(), "get" | "post" | "put" | "delete" | "patch" | "head" | "options")
}

fn extract_parameters(operation: &serde_json::Map<String, Value>) -> Result<Vec<Parameter>> {
    let mut parameters = Vec::new();
    
    if let Some(params) = operation.get("parameters").and_then(|p| p.as_array()) {
        for param in params {
            if let Some(param_obj) = param.as_object() {
                let name = param_obj.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                
                let param_type = param_obj.get("in")
                    .and_then(|v| v.as_str())
                    .unwrap_or("query")
                    .to_string();
                
                let required = param_obj.get("required")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                
                let schema = param_obj.get("schema")
                    .unwrap_or(&Value::Object(serde_json::Map::new()))
                    .clone();
                
                parameters.push(Parameter {
                    name,
                    param_type,
                    required,
                    schema,
                });
            }
        }
    }
    
    Ok(parameters)
}

fn extract_request_body_parameters(operation: &serde_json::Map<String, Value>) -> Result<Option<Vec<Parameter>>> {
    let mut parameters = Vec::new();
    
    if let Some(request_body) = operation.get("requestBody") {
        if let Some(content) = request_body.get("content") {
            // Look for application/x-www-form-urlencoded content
            if let Some(form_content) = content.get("application/x-www-form-urlencoded") {
                if let Some(schema) = form_content.get("schema") {
                    if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
                        let required_fields = schema.get("required")
                            .and_then(|r| r.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str())
                                    .collect::<std::collections::HashSet<_>>()
                            })
                            .unwrap_or_default();
                        
                        for (name, property_schema) in properties {
                            parameters.push(Parameter {
                                name: name.clone(),
                                param_type: "form".to_string(), // Mark as form parameter
                                required: required_fields.contains(name.as_str()),
                                schema: property_schema.clone(),
                            });
                        }
                    }
                }
            }
            // Look for application/json content
            else if let Some(json_content) = content.get("application/json") {
                if let Some(schema) = json_content.get("schema") {
                    // For JSON bodies, expand the schema properties into individual parameters
                    if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
                        let required_fields = schema.get("required")
                            .and_then(|r| r.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str())
                                    .collect::<std::collections::HashSet<_>>()
                            })
                            .unwrap_or_default();
                        
                        for (name, property_schema) in properties {
                            parameters.push(Parameter {
                                name: name.clone(),
                                param_type: "body".to_string(), // Mark as body parameter
                                required: required_fields.contains(name.as_str()),
                                schema: property_schema.clone(),
                            });
                        }
                    } else {
                        // If no properties (e.g., free-form object), create a single body parameter
                        parameters.push(Parameter {
                            name: "body".to_string(),
                            param_type: "body".to_string(),
                            required: request_body.get("required")
                                .and_then(|r| r.as_bool())
                                .unwrap_or(false),
                            schema: schema.clone(),
                        });
                    }
                }
            }
        }
    }
    
    if parameters.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parameters))
    }
}