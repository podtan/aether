# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Aether is a Rust-based application that dynamically exposes external RESTful APIs as Model Context Protocol (MCP) endpoints. It enables AI models to discover and interact with APIs as "tools" at runtime through configuration files, eliminating the need for per-API coding.

## Build and Run Commands

### Build Commands
```bash
# Build all workspace members
cargo build --release

# Build specific binaries
cargo build --release --bin aether      # MCP service
cargo build --release --bin aether-web  # Web UI service
cargo build --release --bin aether-cli  # CLI config generator

# Development build (faster, with debug info)
cargo build
```

### Run Commands

#### MCP Service (Port 8000)
```bash
# Single-config mode with stdio transport (for MCP client integration)
./target/release/aether --config config.yaml --stdio

# Single-config mode with HTTP/SSE transport
./target/release/aether --config config.yaml --host 127.0.0.1 --port 8000

# Multi-config mode (directory-based, HTTP only)
./target/release/aether --multi-config --configs-dir configs --host 0.0.0.0 --port 8000
```

#### Web UI Service (Port 8001)
```bash
./target/release/aether-web --config config.toml --host 0.0.0.0 --port 8001 --mcp-url http://localhost:8000
```

#### CLI Config Generator
```bash
# Generate config from OpenAPI spec
./target/release/aether-cli generate-config \
  --input examples/configs/jsonplaceholder/openapi.yaml \
  --output generated-config.yaml \
  --api-name jsonplaceholder \
  --base-url "https://jsonplaceholder.typicode.com"
```

### Test Commands
```bash
# Run all tests
cargo test

# Run tests for specific crate
cargo test -p aether-core
cargo test -p aether-config
cargo test -p aether-mcp

# Run with logging output
cargo test -- --nocapture
```

### Linting and Formatting
```bash
cargo fmt          # Format code
cargo clippy       # Lint code
cargo doc --open   # Generate and open documentation
```

## Architecture

### Workspace Structure

This is a Cargo workspace with 7 crates organized by responsibility:

- **aether-core**: Shared types, traits, error handling. Foundation for all other crates.
- **aether-config**: Configuration management (YAML/TOML), OpenAPI spec processing, enhanced tools generation.
- **aether-auth**: Authentication handling (Bearer, API key, Basic auth, custom headers).
- **aether-api**: HTTP client for making authenticated API requests.
- **aether-mcp**: MCP server implementation, protocol handling, transport layers (stdio/SSE).
- **aether-bin**: Main MCP service binary with single/multi-config modes.
- **aether-web**: Standalone web UI with OIDC authentication and MCP client integration.
- **aether-cli**: CLI tool for generating runtime-ready configs from OpenAPI specs.

### Split Services Architecture

Aether runs as **two separate services** in production:

1. **aether** (MCP Service): Handles MCP protocol, tool execution, API calls, admin API endpoints
   - Default port: 8000
   - Transports: stdio (for MCP clients) or HTTP/SSE (for web/remote access)
   - Admin API: `/api/upload-openapi`, `/api/list`, `/api/{api}/config`, etc.

2. **aether-web** (Web UI): Provides authenticated web interface for config management
   - Default port: 8001
   - Communicates with MCP service via HTTP client
   - OIDC authentication support

See `SPLIT_SERVICES_ARCHITECTURE.md` for detailed deployment architecture.

### Configuration System: Two-Phase Approach

**Phase 1 (Build-time)**: CLI generates config from OpenAPI
- Parses OpenAPI spec
- Detects environment variables (patterns like `${VAR_NAME}`)
- Classifies parameters as `server` (auto-filled from env) vs `user` (exposed in tool schema)
- Generates `enhanced_tools` section with complete metadata
- Produces runtime-ready YAML config

**Phase 2 (Runtime)**: Server loads pre-processed config
- No OpenAPI parsing needed
- Auto-fills server parameters from environment
- Exposes only user parameters in MCP tool schemas
- Optimal performance

**Why this matters**:
- `openapi_spec` field can be empty when `enhanced_tools` are present
- `--base-url` in CLI allows environment-specific overrides (dev/staging/prod)
- Parameter source intelligence: automatically detects auth params, common patterns
- Eliminates runtime OpenAPI parsing overhead

### Key Components

#### Config Loading (`aether-config`)
- Supports both YAML (MCP configs) and TOML (web configs)
- Enhanced tools: pre-processed metadata with parameter source detection
- Environment variable substitution at runtime

#### MCP Protocol (`aether-mcp`)
- Implements MCP specification
- Supported messages: `tools/list`, `tools/call`, `ping`
- Multi-config support: dynamic tool discovery from directory
- Transport layers: stdio (for Claude Desktop) and SSE (for web)

#### Tool Execution Flow
1. MCP client requests `tools/list`
2. Server returns tools from config (`enhanced_tools` preferred)
3. Client invokes `tools/call` with tool name and user parameters
4. Server auto-fills server parameters from environment
5. `aether-api` makes authenticated HTTP request via `aether-auth`
6. Response converted to MCP format and returned

#### Authentication (`aether-auth`)
- Bearer token: `Authorization: Bearer ${TOKEN}`
- API key: Header, query, or body placement
- Basic auth: Username/password encoding
- Custom headers: Arbitrary header injection

## Configuration Files

### MCP Service Config (config.yaml)
```yaml
server:
  host: 127.0.0.1
  port: 8000
  log_level: info

apis:
  my_api:
    base_url: "https://api.example.com"
    openapi_spec: ""  # Can be empty when enhanced_tools present
    timeout: 30

enhanced_tools:
  my_api_getThing:
    name: my_api_getThing
    description: "Get thing by ID"
    api_name: my_api
    operation_id: getThing
    path: /things/{id}
    method: GET
    parameters:
      - name: api_key
        param_type: header
        required: true
        source: server      # Auto-filled from env
        env_var: MY_API_KEY
      - name: id
        param_type: path
        required: true
        source: user        # Exposed in tool schema
    auth_spec:
      auth_type: api_key
      api_key:
        location: header
        name: X-API-Key
```

### Web Service Config (config.toml)
```toml
[server]
host = "0.0.0.0"
port = 8001
log_level = "info"

[oidc]
provider = "kanidm"
issuer_url = "https://auth.example.com"
client_id = "aether-web"
client_secret = "secret"
redirect_url = "https://aether.example.com/auth/callback"

[session]
secret_key = "long-random-string"
cookie_name = "aether_web_session"
max_age = 28800

[dev]
local_dev_mode = false  # Set true to bypass OIDC
```

### Environment Variables
Load from `.env` file (see `.env.example` for examples):
```bash
# Common patterns
EXAMPLE_API_KEY=xxx              # Auto-detected by CLI
EXAMPLE_TOKEN=yyy                # Referenced as ${EXAMPLE_TOKEN}
AETHER_LOG_LEVEL=debug
```

## Development Workflow

### Adding a New API

1. **Obtain OpenAPI spec** (YAML or JSON)

2. **Generate config using CLI**:
   ```bash
   cargo build --release --bin aether-cli
   ./target/release/aether-cli generate-config \
     --input my-api-openapi.yaml \
     --output configs/my-api/config.yaml \
     --api-name my-api \
     --base-url "https://api.example.com"
   ```

3. **Set environment variables** for server-managed params (check generated config for `env_var` fields)

4. **Test with multi-config mode**:
   ```bash
   ./target/release/aether --multi-config --configs-dir configs
   ```

5. **Verify tools are exposed**: Use MCP client (e.g., Claude Desktop) or test with `/api/list` endpoint

### Modifying Core Types

When changing types in `aether-core`:
1. Ensure backward compatibility with existing configs
2. Update all dependent crates (config, api, auth, mcp)
3. Run `cargo test` across workspace
4. Update examples in `examples/configs/`

### Adding Authentication Support

1. Add new auth type to `aether-auth/src/lib.rs`
2. Update `AuthSpec` enum in `aether-core`
3. Implement authentication in `aether-auth::apply_auth()`
4. Update config schema and examples
5. Test with real API requiring that auth method

## Testing

### Manual Testing
```bash
# Test MCP health endpoint
curl http://127.0.0.1:8000/health

# List configured APIs (multi-config mode)
curl http://127.0.0.1:8000/api/list

# Upload new API via admin API
curl -X POST http://127.0.0.1:8000/api/upload-openapi \
  -H "Content-Type: application/json" \
  -d '{"api_name":"test","base_url":"https://api.example.com","openapi_content":"..."}'
```

### Stdio Mode Testing
For testing with MCP clients like Claude Desktop:
```bash
# Run in stdio mode (logs go to stderr)
./target/release/aether --config config.yaml --stdio --log-level debug 2>aether.log
```

## Important Notes

### Multi-Config vs Single-Config
- **Multi-config**: Directory-based, dynamic API loading, HTTP/SSE only, production-ready
- **Single-config**: One YAML file, supports stdio or HTTP/SSE, simpler for development

### Stdio Transport Limitations
- Logs MUST go to stderr (stdout reserved for JSON-RPC)
- Not compatible with multi-config mode
- Used by MCP clients (Claude Desktop, etc.)

### Enhanced Tools Priority
When both `openapi_spec` and `enhanced_tools` are present, server prefers `enhanced_tools` for performance. Use CLI to generate enhanced tools from OpenAPI specs.

### Environment Variable Substitution
Variables like `${EXAMPLE_TOKEN}` in config are resolved at runtime from environment. CLI detects these patterns and marks parameters as `source: server`.
