//! Aether - Dynamic API to MCP Bridge
//!
//! A Rust-based application for dynamically exposing external RESTful APIs
//! as Model Context Protocol (MCP) endpoints.

use aether_config::AetherConfig;
use aether_mcp::{server, MultiConfigServer, MultiConfigTransport};
use clap::{Arg, Command};
use dotenv;
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load environment variables from .env file if it exists
    if let Err(e) = dotenv::dotenv() {
        // It's okay if .env file doesn't exist, just log it at debug level
        eprintln!("Note: .env file not found or could not be loaded: {}", e);
    }

    // Parse command line arguments
    let matches = Command::new("aether")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Dynamic API to MCP Bridge")
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .value_name("FILE")
                .help("Configuration file path")
                .default_value("config.yaml"),
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
                .default_value("9000"),
        )
        .arg(
            Arg::new("host")
                .long("host")
                .value_name("HOST")
                .help("HTTP server host")
                .default_value("127.0.0.1"),
        )
        .arg(
            Arg::new("stdio")
                .long("stdio")
                .action(clap::ArgAction::SetTrue)
                .help("Use stdio transport instead of HTTP"),
        )
        .arg(
            Arg::new("multi-config")
                .long("multi-config")
                .action(clap::ArgAction::SetTrue)
                .help("Use multi-config mode with directory-based configuration"),
        )
        .arg(
            Arg::new("configs-dir")
                .long("configs-dir")
                .value_name("DIR")
                .help("Directory containing API configurations (for multi-config mode)")
                .default_value("configs"),
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

    // Check if multi-config mode is requested
    let use_multi_config = matches.get_flag("multi-config");

    // Check if stdio mode is requested (determine early for logging setup)
    let use_stdio = matches.get_flag("stdio");

    if use_stdio {
        // In stdio mode, all logs must go to stderr to avoid interfering with JSON-RPC on stdout
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| {
                        tracing_subscriber::EnvFilter::new(format!("aether_core={},aether_config={},aether_api={},aether_auth={},aether_mcp={},aether_bin={}", 
                            log_level, log_level, log_level, log_level, log_level, log_level))
                    })
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(false)
                    .with_writer(std::io::stderr)
            )
            .init();
    } else {
        // HTTP mode - normal logging to stdout
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| {
                        tracing_subscriber::EnvFilter::new(format!("aether_core={},aether_config={},aether_api={},aether_auth={},aether_mcp={},aether_bin={}", 
                            log_level, log_level, log_level, log_level, log_level, log_level))
                    })
            )
            .with(tracing_subscriber::fmt::layer().with_target(false))
            .init();
    }

    info!("Starting Aether v{}", env!("CARGO_PKG_VERSION"));

    if use_multi_config {
        // Multi-config mode
        let configs_dir = matches.get_one::<String>("configs-dir").unwrap();
        info!(
            "Starting in multi-config mode with directory: {}",
            configs_dir
        );

        if use_stdio {
            error!("Multi-config mode is not compatible with stdio transport");
            std::process::exit(1);
        }

        // Parse host and port for HTTP mode
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

        info!("Starting Multi-Config MCP server on {}", bind_addr);

        // Start pure multi-config server (no web integration)
        let multi_server = match MultiConfigServer::new(configs_dir).await {
            Ok(server) => server,
            Err(e) => {
                error!("Failed to create multi-config server: {}", e);
                std::process::exit(1);
            }
        };

        let transport = MultiConfigTransport::new(multi_server, bind_addr);
        // Per-API auth is now supported - auth configs are loaded from each API's config
        if let Err(e) = transport.start(None, None, None).await {
            error!("Multi-config server error: {}", e);
            std::process::exit(1);
        }
    } else {
        // Single-config mode (existing behavior)
        // Load configuration
        let config_path = PathBuf::from(matches.get_one::<String>("config").unwrap());
        let config = match config_path.extension().and_then(|ext| ext.to_str()) {
            Some("toml") => AetherConfig::load_toml_from_file(&config_path).await,
            _ => AetherConfig::load_from_file(&config_path).await,
        };
        let config = match config {
            Ok(config) => config,
            Err(e) => {
                error!("Failed to load configuration: {}", e);
                std::process::exit(1);
            }
        };

        info!("Configuration loaded from: {:?}", config_path);

        // Validate MCP auth configuration
        if let Some(mcp_auth) = &config.mcp_auth {
            if let Err(e) = mcp_auth.validate(config.oidc.as_ref()) {
                error!("MCP auth configuration validation failed: {}", e);
                std::process::exit(1);
            }
        }

        if use_stdio {
            info!("Starting MCP server with stdio transport");

            // Start the MCP stdio server
            if let Err(e) = server::start_stdio_server(config).await {
                error!("Server error: {}", e);
                std::process::exit(1);
            }
        } else {
            // Parse host and port for HTTP mode
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

            info!("Starting MCP server with HTTP transport on {}", bind_addr);

            // Start the MCP SSE server only (no web integration)
            if let Err(e) = server::start_sse_server(config, bind_addr).await {
                error!("Server error: {}", e);
                std::process::exit(1);
            }
        }
    }

    info!("Aether stopped");
    Ok(())
}
