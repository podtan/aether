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
        
        /// Base URL for the API being configured
        #[arg(short, long)]
        base_url: String,
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
    base_url: &str,
) -> Result<()> {
    info!("🔧 Generating Aether config from OpenAPI spec");
    info!("   Input: {:?}", input_path);
    info!("   Output: {:?}", output_path);
    info!("   Server: {}", server_url);
    info!("   API Name: {}", api_name);
    info!("   Base URL: {}", base_url);

    // Read the OpenAPI file
    let openapi_content = tokio::fs::read_to_string(input_path).await?;
    info!("✅ Read OpenAPI file ({} bytes)", openapi_content.len());

    // Create HTTP client
    let client = reqwest::Client::new();
    
    // Prepare the request payload
    let payload = serde_json::json!({
        "openapi_content": openapi_content,
        "api_name": api_name,
        "base_url": base_url
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