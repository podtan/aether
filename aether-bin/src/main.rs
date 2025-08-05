//! Aether - Dynamic API to MCP Bridge
//! 
//! A Rust-based application for dynamically exposing external RESTful APIs
//! as Model Context Protocol (MCP) endpoints.

use aether_config::AetherConfig;
use aether_mcp::{server, MultiConfigServer, MultiConfigTransport};
use clap::{Arg, Command};
use std::path::PathBuf;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use dotenv;
use std::net::SocketAddr;

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
                .default_value("config.yaml")
        )
        .arg(
            Arg::new("log-level")
                .long("log-level")
                .value_name("LEVEL")
                .help("Log level (trace, debug, info, warn, error)")
                .default_value("info")
        )
        .arg(
            Arg::new("port")
                .short('p')
                .long("port")
                .value_name("PORT")
                .help("HTTP server port")
                .default_value("8000")
        )
        .arg(
            Arg::new("host")
                .long("host")
                .value_name("HOST")
                .help("HTTP server host")
                .default_value("127.0.0.1")
        )
        .arg(
            Arg::new("stdio")
                .long("stdio")
                .action(clap::ArgAction::SetTrue)
                .help("Use stdio transport instead of HTTP")
        )
        .arg(
            Arg::new("multi-config")
                .long("multi-config")
                .action(clap::ArgAction::SetTrue)
                .help("Use multi-config mode with directory-based configuration")
        )
        .arg(
            Arg::new("configs-dir")
                .long("configs-dir")
                .value_name("DIR")
                .help("Directory containing API configurations (for multi-config mode)")
                .default_value("configs")
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
        info!("Starting in multi-config mode with directory: {}", configs_dir);
        
        if use_stdio {
            error!("Multi-config mode is not compatible with stdio transport");
            std::process::exit(1);
        }
        
        // Parse host and port for HTTP mode
        let host = matches.get_one::<String>("host").unwrap();
        let port = matches.get_one::<String>("port").unwrap()
            .parse::<u16>()
            .unwrap_or_else(|_| {
                error!("Invalid port number");
                std::process::exit(1);
            });
        
        let bind_addr: SocketAddr = format!("{}:{}", host, port)
            .parse()
            .unwrap_or_else(|_| {
                error!("Invalid host:port combination");
                std::process::exit(1);
            });
        
        info!("Starting Multi-Config MCP server on {}:{}", host, port);
        
        // Check if we have a web config file for authentication
        let web_config_path = PathBuf::from("config.toml");
        if web_config_path.exists() {
            info!("Found web configuration file, starting combined multi-config + web server");
            
            // Load web authentication configuration
            let web_config = match AetherConfig::load_toml_from_file(&web_config_path).await {
                Ok(config) => config,
                Err(e) => {
                    error!("Failed to load web configuration: {}", e);
                    std::process::exit(1);
                }
            };
            
            // Start combined multi-config + web server
            if let Err(e) = start_multi_config_web_server(configs_dir, web_config, bind_addr).await {
                error!("Combined multi-config + web server error: {}", e);
                std::process::exit(1);
            }
        } else {
            // Start pure multi-config server (existing behavior)
            let multi_server = match MultiConfigServer::new(configs_dir).await {
                Ok(server) => server,
                Err(e) => {
                    error!("Failed to create multi-config server: {}", e);
                    std::process::exit(1);
                }
            };
            
            let transport = MultiConfigTransport::new(multi_server, port);
            if let Err(e) = transport.start().await {
                error!("Multi-config server error: {}", e);
                std::process::exit(1);
            }
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
            let port = matches.get_one::<String>("port").unwrap()
                .parse::<u16>()
                .unwrap_or_else(|_| {
                    error!("Invalid port number");
                    std::process::exit(1);
                });
            
            let bind_addr: SocketAddr = format!("{}:{}", host, port)
                .parse()
                .unwrap_or_else(|_| {
                    error!("Invalid host:port combination");
                    std::process::exit(1);
                });
            
            info!("Starting MCP server with HTTP transport on {}", bind_addr);
            
            // Check if web authentication is configured
            if config.has_web_authentication() {
                info!("Web authentication detected, starting combined MCP + Web server");
                
                // Start the combined MCP + Web server
                if let Err(e) = server::start_combined_server(config, bind_addr).await {
                    error!("Combined server error: {}", e);
                    std::process::exit(1);
                }
            } else {
                // Start the MCP SSE server only
                if let Err(e) = server::start_sse_server(config, bind_addr).await {
                    error!("Server error: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }
    
    info!("Aether stopped");
    Ok(())
}

/// Start combined multi-config MCP server with web authentication
async fn start_multi_config_web_server(
    configs_dir: &str,
    web_config: AetherConfig,
    bind_addr: SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    use aether_mcp::transport::SseState;
    use axum::{routing::get, Router};
    use tower::ServiceBuilder;
    use tower_http::{cors::CorsLayer, trace::TraceLayer};
    use std::sync::Arc;
    
    info!("Starting combined multi-config MCP + Web Auth server on {}", bind_addr);

    // Create multi-config server for MCP APIs
    let multi_server = Arc::new(MultiConfigServer::new(configs_dir).await?);

    // Create web authentication router
    let web_router = aether_web::create_router(&web_config).await?;

    // Create MCP transport state for multi-config (using the existing private struct)
    let sse_state = SseState::new();
    
    // Create multi-config MCP routes using existing handlers
    let mcp_router = Router::new()
        // Legacy single-API endpoints (for backwards compatibility)
        .route("/message", axum::routing::post(|state, query, json| async move {
            aether_mcp::transport::handle_single_message(axum::extract::State(state), axum::extract::Query(query), axum::extract::Json(json)).await
        }))
        .route("/messages", axum::routing::post(|state, query, json| async move {
            aether_mcp::transport::handle_single_message(axum::extract::State(state), axum::extract::Query(query), axum::extract::Json(json)).await
        }))
        .route("/sse", get(|state, query| async move {
            aether_mcp::transport::handle_single_sse(axum::extract::State(state), axum::extract::Query(query)).await
        }))

        // Multi-API endpoints  
        .route("/apis/:api_name/message", axum::routing::post(|state, path, query, json| async move {
            aether_mcp::transport::handle_multi_message(axum::extract::State(state), axum::extract::Path(path), axum::extract::Query(query), axum::extract::Json(json)).await
        }))
        .route("/apis/:api_name/messages", axum::routing::post(|state, path, query, json| async move {
            aether_mcp::transport::handle_multi_message(axum::extract::State(state), axum::extract::Path(path), axum::extract::Query(query), axum::extract::Json(json)).await
        }))
        .route("/apis/:api_name/sse", get(|state, path, query| async move {
            aether_mcp::transport::handle_multi_sse(axum::extract::State(state), axum::extract::Path(path), axum::extract::Query(query)).await
        }))

        // API management endpoints
        .route("/api/generate-config", axum::routing::post(|state, json| async move {
            aether_mcp::transport::handle_generate_config(axum::extract::State(state), axum::extract::Json(json)).await
        }))
        .route("/api/upload-openapi", axum::routing::post(|state, multipart| async move {
            aether_mcp::transport::handle_upload_openapi(axum::extract::State(state), multipart).await
        }))
        .route("/api/list", get(|state| async move {
            aether_mcp::transport::handle_list_apis(axum::extract::State(state)).await
        }))

        // Health and stats
        .route("/health", get(|state| async move {
            aether_mcp::transport::handle_multi_health(axum::extract::State(state)).await
        }))
        .route("/stats", get(|state| async move {
            aether_mcp::transport::handle_all_stats(axum::extract::State(state)).await
        }))
        .with_state((multi_server, sse_state, aether_web::WebState::new()));

    // Combine both routers
    let app = Router::new()
        .nest("/web", web_router)  // Web auth routes under /web
        .route("/web", axum::routing::get(|| async { axum::response::Redirect::permanent("/web/") }))  // Redirect /web to /web/
        .merge(mcp_router)  // Multi-config MCP routes at root level
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        );

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}