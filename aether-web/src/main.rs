//! Aether Web - Standalone Web UI Service
//!
//! A Rust-based web application for managing Aether API configurations
//! via a web interface with OIDC authentication.

use aether_config::AetherConfig;
use aether_web;
use axum::Router;
use clap::{Arg, Command};
use std::net::SocketAddr;
use std::path::PathBuf;
use tower_http::cors::CorsLayer;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments
    let matches = Command::new("aether-web")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Aether Web UI Service")
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .value_name("FILE")
                .help("Web configuration file path")
                .default_value("/opt/aether/web/config.toml"),
        )
        .arg(
            Arg::new("log-level")
                .long("log-level")
                .value_name("LEVEL")
                .help("Log level (trace, debug, info, warn, error)")
                .default_value("info"),
        )
        .arg(
            Arg::new("port")
                .short('p')
                .long("port")
                .value_name("PORT")
                .help("HTTP server port")
                .default_value("9001"),
        )
        .arg(
            Arg::new("host")
                .long("host")
                .value_name("HOST")
                .help("HTTP server host")
                .default_value("0.0.0.0"),
        )
        .arg(
            Arg::new("mcp-url")
                .long("mcp-url")
                .value_name("URL")
                .help("MCP service base URL for admin API calls")
                .default_value("http://localhost:9000"),
        )
        .get_matches();

    // Initialize logging
    let log_level = matches.get_one::<String>("log-level").unwrap();
    let log_filter = match log_level.as_str() {
        "trace" => tracing::Level::TRACE,
        "debug" => tracing::Level::DEBUG,
        "info" => tracing::Level::INFO,
        "warn" => tracing::Level::WARN,
        "error" => tracing::Level::ERROR,
        _ => {
            eprintln!("Invalid log level: {}", log_level);
            std::process::exit(1);
        }
    };

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(format!(
                    "aether_web={},aether_config={},aether_auth={}",
                    log_level, log_level, log_level
                ))
            }),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    info!("Starting Aether Web v{}", env!("CARGO_PKG_VERSION"));

    // Load web configuration
    let config_path = PathBuf::from(matches.get_one::<String>("config").unwrap());
    let config = match AetherConfig::load_toml_from_file(&config_path).await {
        Ok(config) => config,
        Err(e) => {
            error!(
                "Failed to load web configuration from {:?}: {}",
                config_path, e
            );
            std::process::exit(1);
        }
    };

    info!("Web configuration loaded from: {:?}", config_path);

    // Parse host and port
    let host = matches.get_one::<String>("host").unwrap();
    let port = matches
        .get_one::<String>("port")
        .unwrap()
        .parse::<u16>()
        .unwrap_or_else(|_| {
            error!("Invalid port number");
            std::process::exit(1);
        });

    let bind_addr: SocketAddr = format!("{}:{}", host, port).parse().unwrap_or_else(|_| {
        error!("Invalid host:port combination");
        std::process::exit(1);
    });

    let mcp_url = matches.get_one::<String>("mcp-url").unwrap();
    info!("MCP service URL: {}", mcp_url);

    info!("Starting Aether Web server on {}", bind_addr);

    // Create the web router with MCP client integration
    let app = create_web_app(&config, mcp_url).await?;

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;

    info!("Aether Web stopped");
    Ok(())
}

/// Create the web application with MCP client integration
async fn create_web_app(config: &AetherConfig, mcp_url: &str) -> anyhow::Result<Router> {
    // Create the web router with HTTP client for MCP admin API calls
    let router = aether_web::create_router_with_mcp_client(config, mcp_url).await?;

    // Add CORS middleware - in production this should be configured with specific origins
    let app = router.layer(CorsLayer::permissive());

    Ok(app)
}
