# Split Services Architecture

This document describes the new split architecture for Aether services.

## Overview

Aether now runs as two separate services:

1. **aether** - MCP service (port 9000)
2. **aether-web** - Web UI service (port 9001)

## Service Details

### MCP Service (aether)
- **Port**: 9000 (default)
- **Purpose**: Provides MCP protocol endpoints and admin API
- **Location**: `/opt/aether/mcp/`
- **Binary**: `/opt/aether/mcp/bin/aether`
- **Configs**: `/opt/aether/mcp/configs/`
- **Service**: `aether.service`

**Key Endpoints:**
- `/sse` - SSE endpoint for MCP clients
- `/message`, `/messages` - MCP message endpoints
- `/api/upload-openapi` - Upload OpenAPI specs
- `/api/list` - List configured APIs
- `/api/{api_name}/config` - Get/update API config
- `/api/{api_name}/delete` - Delete API
- `/health` - Health check

**Command line:**
```bash
aether --multi-config --configs-dir /opt/aether/mcp/configs --host 0.0.0.0 --port 9000
```

### Web Service (aether-web)
- **Port**: 9001 (default)
- **Purpose**: Provides web UI with OIDC authentication
- **Location**: `/opt/aether/web/`
- **Binary**: `/opt/aether/web/bin/aether-web`
- **Config**: `/opt/aether/web/config.toml`
- **Service**: `aether-web.service`

**Key Endpoints:**
- `/login` - Login page
- `/auth/*` - OIDC authentication flow
- `/upload` - File upload form
- `/apis` - API management UI
- `/health` - Health check

**Command line:**
```bash
aether-web --host 0.0.0.0 --port 9001 --mcp-url http://localhost:9000
```

## Communication

The Web service communicates with the MCP service via HTTP client calls to the admin API. When users upload OpenAPI specs through the web interface, the web service:

1. Receives the multipart form upload
2. Calls `POST /api/upload-openapi` on the MCP service
3. Displays the result to the user

## Deployment

The deployment workflow now:

1. Builds both binaries (`aether` and `aether-web`)
2. Creates directory structure:
   - `/opt/aether/mcp/bin/` - MCP binary
   - `/opt/aether/mcp/configs/` - API configurations
   - `/opt/aether/web/bin/` - Web binary
   - `/opt/aether/web/config.toml` - Web configuration
3. Creates systemd services for both
4. Restarts both services

## Systemd Services

### aether.service
```ini
[Unit]
Description=Aether MCP Service
After=network.target

[Service]
Type=simple
ExecStart=/opt/aether/mcp/bin/aether --multi-config --configs-dir /opt/aether/mcp/configs --host 0.0.0.0 --port 9000
WorkingDirectory=/opt/aether/mcp
Restart=on-failure
User=aether
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

### aether-web.service
```ini
[Unit]
Description=Aether Web Service
After=network.target

[Service]
Type=simple
ExecStart=/opt/aether/web/bin/aether-web --host 0.0.0.0 --port 9001
WorkingDirectory=/opt/aether/web
Restart=on-failure
User=aether
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

## Reverse Proxy Configuration

For production deployment, use a reverse proxy to route traffic:

### Nginx Example
```nginx
# Route web UI traffic
location /web/ {
    proxy_pass http://localhost:9001/;
}

location /oauth2/callback {
    proxy_pass http://localhost:9001/oauth2/callback;
}

# Route MCP traffic  
location /sse {
    proxy_pass http://localhost:9000/sse;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
}

location /message {
    proxy_pass http://localhost:9000/message;
}

location /api/ {
    proxy_pass http://localhost:9000/api/;
}

location /health {
    proxy_pass http://localhost:9000/health;
}
```

## Configuration

### Web Configuration (config.toml)
The web service requires a TOML configuration file with OIDC and session settings:

```toml
[server]
host = "127.0.0.1"
port = 9001
log_level = "info"

[session]
secret_key = "your-secret-key-here"
cookie_name = "aether_web_session"
max_age = 28800

[oidc]
provider = "your-provider"
issuer_url = "https://your-oidc-provider.com"
client_id = "your-client-id"
client_secret = "your-client-secret"
redirect_url = "https://yourdomain.com/oauth2/callback"

[dev]
local_dev_mode = false  # Set to true for development

[apis]
# Empty for separated architecture
```

## Testing

To test the split services:

1. Start MCP service:
   ```bash
   aether --multi-config --configs-dir configs --host 127.0.0.1 --port 9000
   ```

2. Start Web service:
   ```bash
   aether-web --config config.toml --host 127.0.0.1 --port 9001 --mcp-url http://127.0.0.1:9000
   ```

3. Test endpoints:
   ```bash
   # MCP health
   curl http://127.0.0.1:9000/health
   
   # Web health  
   curl http://127.0.0.1:9001/health
   
   # Upload API via MCP directly
   curl -X POST -H "Content-Type: application/json" \
     -d '{"api_name":"test","base_url":"https://api.example.com","openapi_content":"..."}' \
     http://127.0.0.1:9000/api/upload-openapi
   ```

## Backward Compatibility

- MCP endpoints remain unchanged, now on port 9000
- Web endpoints remain under `/web/`, now on port 9001
- Combined mode is removed; services must run separately

## Security Considerations

- Use HTTPS termination at reverse proxy
- Configure CORS appropriately for production
- Store sensitive OIDC credentials securely
- Consider authentication for MCP admin API in production