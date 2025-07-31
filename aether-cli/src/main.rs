//! Aether CLI - Command line tool for Aether configuration management

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::{info, error};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate Aether configuration from OpenAPI specification
    GenerateConfig {
        /// Path to OpenAPI specification file
        #[arg(short, long)]
        input: PathBuf,
        
        /// Path to output Aether configuration file
        #[arg(short, long)]
        output: PathBuf,
        
        /// API server URL for config generation (default: http://localhost:8000)
        #[arg(long, default_value = "http://localhost:8000")]
        server: String,
        
        /// API name to use in the generated config
        #[arg(short, long)]
        api_name: String,
        
        /// Base URL for the API being configured (optional - will use OpenAPI servers if not provided)
        #[arg(short, long)]
        base_url: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("aether_cli=info")
        .init();

    let cli = Cli::parse();

    match &cli.command {
        Commands::GenerateConfig { input, output, server, api_name, base_url } => {
            generate_config(input, output, server, api_name, base_url).await?;
        }
    }

    Ok(())
}

async fn generate_config(
    input_path: &PathBuf,
    output_path: &PathBuf,
    server_url: &str,
    api_name: &str,
    base_url: &Option<String>,
) -> Result<()> {
    info!("🔧 Generating Aether config from OpenAPI spec");
    info!("   Input: {:?}", input_path);
    info!("   Output: {:?}", output_path);
    info!("   Server: {}", server_url);
    info!("   API Name: {}", api_name);
    if let Some(url) = base_url {
        info!("   Base URL: {}", url);
    } else {
        info!("   Base URL: <will extract from OpenAPI spec>");
    }

    // Read the OpenAPI file
    let openapi_content = tokio::fs::read_to_string(input_path).await?;
    info!("✅ Read OpenAPI file ({} bytes)", openapi_content.len());

    // Extract base URL from OpenAPI spec if not provided
    let final_base_url = if let Some(url) = base_url {
        url.clone()
    } else {
        info!("🔍 Extracting base URL from OpenAPI specification...");
        extract_base_url_from_openapi(&openapi_content)?
    };

    // Create HTTP client
    let client = reqwest::Client::new();
    
    // Prepare the request payload
    let payload = serde_json::json!({
        "openapi_content": openapi_content,
        "api_name": api_name,
        "base_url": final_base_url
    });

    // Send request to the config generation API
    let response = client
        .post(&format!("{}/api/generate-config", server_url))
        .json(&payload)
        .send()
        .await?;

    if !response.status().is_success() {
        let error_text = response.text().await?;
        error!("❌ Config generation failed: {}", error_text);
        anyhow::bail!("Config generation API returned error: {}", error_text);
    }

    let generated_config = response.text().await?;
    info!("✅ Received generated config ({} bytes)", generated_config.len());

    // Write the generated config to output file
    tokio::fs::write(output_path, generated_config).await?;
    info!("✅ Generated config written to: {:?}", output_path);

    println!("🎉 Successfully generated Aether config from OpenAPI spec!");
    println!("   Input:  {:?}", input_path);
    println!("   Output: {:?}", output_path);

    Ok(())
}

/// Extract base URL from OpenAPI specification
fn extract_base_url_from_openapi(openapi_content: &str) -> Result<String> {
    // Parse OpenAPI specification
    let spec: serde_json::Value = if openapi_content.trim_start().starts_with('{') {
        serde_json::from_str(openapi_content)
            .map_err(|e| anyhow::anyhow!("Failed to parse OpenAPI JSON: {}", e))?
    } else {
        serde_yaml::from_str(openapi_content)
            .map_err(|e| anyhow::anyhow!("Failed to parse OpenAPI YAML: {}", e))?
    };

    // Extract servers section
    if let Some(servers) = spec.get("servers").and_then(|s| s.as_array()) {
        if let Some(first_server) = servers.first() {
            if let Some(url) = first_server.get("url").and_then(|u| u.as_str()) {
                info!("✅ Extracted base URL from OpenAPI spec: {}", url);
                return Ok(url.to_string());
            }
        }
    }

    anyhow::bail!("No base URL found in OpenAPI specification servers section")
}