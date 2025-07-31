# Aether - Dynamic API to MCP Bridge

A Rust-based application designed to dynamically expose external RESTful APIs as Model Context Protocol (MCP) endpoints. Aether enables AI models to discover and interact with APIs as "tools" at runtime, without requiring pre-generated or hardcoded API-specific integrations.

## Overview

Aether bridges the gap between AI models and the vast ecosystem of external APIs by providing a generalized, runtime-configurable solution that eliminates the need for per-API coding within AI applications.

### Key Features

- **Dynamic API Exposure**: Define new API integrations solely via configuration files
- **MCP Compliance**: Full compliance with the Model Context Protocol specification
- **Runtime Configuration**: No code changes required to add new APIs
- **Authentication Support**: Built-in support for Bearer tokens, API keys, Basic auth, and custom headers
- **Performance Optimized**: Written in Rust for high performance and memory safety
- **Modular Architecture**: Clean separation of concerns with a Cargo workspace

## Architecture

The project is structured as a Cargo workspace with the following crates:

- **aether-core**: Shared types, traits, and error handling
- **aether-config**: Configuration management and OpenAPI specification processing
- **aether-auth**: Authentication handling for various auth methods
- **aether-api**: HTTP client for making API requests
- **aether-mcp**: MCP server implementation and protocol handling
- **aether-bin**: Main binary application

## Quick Start

### Prerequisites

- Rust 1.70 or later
- An OpenAPI specification file for your target API

### Installation

1. Clone the repository:
```bash
git clone https://github.com/AAG81/aether.git
cd aether
```

2. Build the project:
```bash
cargo build --release
```

3. Create a configuration file (see `config.yaml` for an example):
```yaml
server:
  host: "127.0.0.1"
  port: 8080
  log_level: "info"

apis:
  my_api:
    base_url: "https://api.example.com"
    openapi_spec: "path/to/openapi.yaml"
    timeout: 30

tools:
  my_api_getUsers:
    description: "Get list of users"
    auth_spec:
      auth_type: "bearer"
      required: true
```

4. Run the server:
```bash
./target/release/aether --config config.yaml
```

## Configuration

### Server Configuration

```yaml
server:
  host: "127.0.0.1"        # Server bind address
  port: 8080               # Server port
  log_level: "info"        # Log level (trace, debug, info, warn, error)
```

### API Configuration

```yaml
apis:
  api_name:
    base_url: "https://api.example.com"
    openapi_spec: "path/to/spec.yaml"  # Path to OpenAPI specification
    timeout: 30                        # Request timeout in seconds
```

### Tool Configuration

Override tool behavior and add authentication:

```yaml
tools:
  api_name_operationId:
    description: "Custom tool description"
    auth_spec:
      auth_type: "bearer"              # bearer, api_key, basic, custom
      required: true
```

### Authentication Types

#### Bearer Token
```yaml
auth_spec:
  auth_type: "bearer"
  required: true
```

#### API Key
```yaml
auth_spec:
  auth_type:
    api_key:
      location: "header"  # header, query, body
      name: "X-API-Key"
  required: true
```

#### Basic Authentication
```yaml
auth_spec:
  auth_type: "basic"
  required: true
```

#### Custom Headers
```yaml
auth_spec:
  auth_type:
    custom:
      headers:
        "X-Custom-Auth": ""
  required: true
```

## MCP Protocol Support

Aether implements the Model Context Protocol for seamless integration with AI models:

### Supported Messages

- `tools/list`: Returns available tools based on loaded API configurations
- `tools/call`: Executes API calls with parameter validation and authentication
- `ping`: Health check endpoint

### Tool Discovery

Tools are automatically generated from OpenAPI specifications with:
- Parameter schemas derived from API specifications
- Authentication requirements when configured
- Custom descriptions and overrides

### Request/Response Handling

- Automatic parameter mapping (path, query, header, body)
- JSON response parsing and error handling
- HTTP status code mapping to MCP responses

## Examples

The `examples/` directory contains sample configurations and OpenAPI specifications:

- `jsonplaceholder-openapi.yaml`: Example API specification
- `config.yaml`: Sample configuration file

## Automated Config Generation

Aether supports automated configuration generation from OpenAPI specifications using a two-phase approach that eliminates the need for runtime OpenAPI parsing.

### CLI Tool (aether-cli)

Generate optimized configurations from OpenAPI specs:

```bash
# Build the CLI tool
cargo build --release --bin aether-cli

# Generate config from OpenAPI spec
./target/release/aether-cli generate-config \
  --input examples/openapi/example-openapi.yaml \
  --output generated-config.yaml \
  --api-name example \
  --base-url "https://api.example.com/api/serviceCall"
```

### Two-Phase Architecture

#### Phase 1: Configuration Generation
- CLI parses OpenAPI specification
- Analyzes parameter schemas to detect environment variables (`${VAR_NAME}`)
- Classifies parameters as server-managed vs user-provided
- Generates `enhanced_tools` with complete metadata
- Produces runtime-ready configuration

#### Phase 2: Runtime Execution
- Server loads configuration with pre-processed `enhanced_tools`
- No OpenAPI parsing required at runtime
- Auto-fills server parameters from environment variables
- Exposes only user parameters in tool schemas

### Why This Design?

**Base URL Flexibility**: The `--base-url` parameter allows environment-specific overrides (dev/staging/prod) even when OpenAPI spec contains servers section.

**Runtime Performance**: Pre-processing OpenAPI into `enhanced_tools` eliminates runtime parsing overhead and enables sophisticated parameter source detection.

**Empty openapi_spec**: When `enhanced_tools` are present, `openapi_spec` can be empty because all metadata is pre-processed:

```yaml
apis:
  example:
    base_url: "https://api.example.com/api/serviceCall"
    openapi_spec: ""  # Not needed - data is in enhanced_tools
    timeout: 30

enhanced_tools:
  example_toolBalance:
    parameters:
    - name: api_key
      source: server      # Auto-filled from environment
      env_var: EXAMPLE_VAR
```

**Parameter Source Intelligence**: Automatically detects which parameters should be server-managed:
- Environment variable patterns: `${VAR_NAME}`
- Common auth parameter names: `api_key`, `token`, `api_key`
- Parameter descriptions and schemas

### Generated vs Manual Configs

| Aspect | Manual Config | Generated Config |
|--------|---------------|------------------|
| OpenAPI parsing | Runtime | Build-time |
| Parameter classification | Manual | Automatic |
| Environment integration | Manual | Automatic |
| Tool schema optimization | No | Yes (user params only) |
| Maintenance | High | Low |

## Development

### Running Tests

```bash
cargo test
```

### Code Formatting

```bash
cargo fmt
```

### Linting

```bash
cargo clippy
```

### Documentation

Generate documentation:
```bash
cargo doc --open
```

## Architecture Details

### Component Interaction

1. **Configuration Loading**: `aether-config` loads and validates YAML configuration and OpenAPI specifications
2. **Tool Generation**: MCP tools are dynamically generated from API operations
3. **Request Handling**: `aether-mcp` handles MCP protocol messages
4. **API Execution**: `aether-api` makes HTTP requests with authentication via `aether-auth`
5. **Response Processing**: API responses are converted to MCP format

### Error Handling

Comprehensive error handling with user-friendly messages for:
- Configuration validation errors
- API communication failures
- Authentication issues
- MCP protocol violations

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests for new functionality
5. Run the test suite
6. Submit a pull request

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Acknowledgments

- Built with the Rust ecosystem including tokio, serde, and reqwest
- Inspired by the Model Context Protocol specification
- Designed for the AI and automation community