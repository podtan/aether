 Summary

  I've successfully started implementing OAuth2.1/OIDC authentication for MCP endpoints as outlined in docs/oauth2/TASKS.md. Here's what was completed:

  ✅ Phase 1: Extract Shared Authentication Logic (Tasks 1.2)

  Created aether-auth/src/oidc.rs - Shared OIDC validator module:
  - OidcValidator - Validates Bearer tokens via OIDC userinfo endpoint
  - UserClaims - Extracted user claims structure
  - OidcMetadata - OIDC discovery metadata
  - Automatic OIDC discovery on initialization
  - Health check and metadata refresh capabilities

  ✅ Phase 2: MCP Authentication Middleware (Tasks 2.1-2.4)

  Added MCP auth configuration (aether-config/src/lib.rs):
  - McpAuthConfig struct with fields:
    - enabled - Enable/disable OAuth2.1 protection
    - require_auth - Require authentication (reject unauthenticated)
    - public_health - Keep /health endpoint public
    - issuer_url - Optional OIDC issuer override
    - audience - Optional audience claim
  - Config validation method to ensure OIDC is configured when MCP auth is enabled

  Created middleware (aether-mcp/src/middleware.rs):
  - AuthMiddlewareState - Shared state with OIDC validator
  - mcp_auth_middleware - Axum middleware to validate Bearer tokens
  - Proper error handling with 401 responses
  - Logs authentication events (success/failure)
  - Attaches user claims to request extensions for downstream handlers

  Wired middleware into routers:
  - Single-config mode (HttpSseTransport::start) - aether-mcp/src/transport.rs:64
  - Multi-config mode (MultiConfigTransport::start) - aether-mcp/src/transport.rs:557
  - /health endpoint remains public (added after auth middleware)
  - All MCP endpoints protected: /sse, /message, /messages, admin API endpoints

  ✅ Phase 3: Configuration & Startup Validation (Tasks 3.1-3.2)

  Added startup validation (aether-bin/src/main.rs:190):
  - Validates MCP auth config before server starts
  - Clear error messages with documentation references
  - Prevents server from starting with invalid config

  Updated example configs:
  - config.toml.example - Added [mcp_auth] section with comprehensive comments
  - Includes setup instructions referencing docs/oauth2/CLIENTS.md

  Files Modified/Created:

  New Files:
  - aether-auth/src/oidc.rs - OIDC validator implementation
  - aether-mcp/src/middleware.rs - MCP auth middleware

  Modified Files:
  - aether-auth/Cargo.toml - Added anyhow and tokio dependencies
  - aether-auth/src/lib.rs - Exported OIDC module
  - aether-config/src/lib.rs - Added McpAuthConfig and validation
  - aether-config/src/generator.rs - Added mcp_auth field initialization
  - aether-mcp/src/lib.rs - Re-exported middleware types
  - aether-mcp/src/transport.rs - Updated both transport start methods
  - aether-mcp/src/server.rs - Updated start_sse_server to pass auth config
  - aether-bin/src/main.rs - Added validation and auth config passing
  - config.toml.example - Added MCP auth configuration example

  Build Status:

  ✅ Successful build - All core implementation compiles without errors (only minor unused variable warnings remain)

  Next Steps (Not Implemented Yet):

  - Phase 4: Testing (unit tests, integration tests, smoke tests)
  - Phase 5: Documentation (CLIENTS.md, deployment docs updates)
  - Optional: Update aether-web to use shared OidcValidator (Task 1.3)