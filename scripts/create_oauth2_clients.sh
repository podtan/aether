#!/bin/bash
# Create all OAuth2 clients in Kanidm for Aether services
# 
# This script creates:
# 1. Resource server clients (pdt-api, nghr-api) - for JWT validation
# 2. OAuth2 client applications (vscode-mcp) - for VS Code to authenticate

set -e

echo "🚀 Setting up OAuth2 clients in Kanidm..."
echo ""

# ============================================
# 1. RESOURCE SERVERS (for JWT validation per API)
# ============================================

echo "=== Creating Resource Servers ==="
echo ""

# PDT Resource Server
echo "1. Creating PDT resource server..."
kanidm system oauth2 create-public pdt-api "PDT API Resource Server" http://localhost:8000/pdt || true
kanidm system oauth2 add-redirect-url pdt-api http://localhost:8000/pdt/callback || true
kanidm system oauth2 enable-localhost-redirects pdt-api || true
kanidm system oauth2 update-scope-map pdt-api idm_all_persons openid profile email || true
echo "✅ PDT resource server ready"
echo ""

# NGHR Resource Server
echo "2. Creating NGHR resource server..."
kanidm system oauth2 create-public nghr-api "NGHR API Resource Server" http://localhost:8000/nghr || true
kanidm system oauth2 add-redirect-url nghr-api http://localhost:8000/nghr/callback || true
kanidm system oauth2 enable-localhost-redirects nghr-api || true
kanidm system oauth2 update-scope-map nghr-api idm_all_persons openid profile email || true
echo "✅ NGHR resource server ready"
echo ""

# ============================================
# 2. CLIENT APPLICATIONS (for OAuth2 flows)
# ============================================

echo "=== Creating Client Applications ==="
echo ""

# VS Code MCP Client
echo "3. Creating VS Code MCP client..."
kanidm system oauth2 create-public vscode-mcp "VS Code MCP Client" http://127.0.0.1:33418 || true
kanidm system oauth2 add-redirect-url vscode-mcp https://vscode.dev/redirect || true
kanidm system oauth2 enable-localhost-redirects vscode-mcp || true
kanidm system oauth2 update-scope-map vscode-mcp idm_all_persons openid profile email || true
echo "✅ VS Code client ready"
echo ""

# ============================================
# 3. DISPLAY SUMMARY
# ============================================

echo "=== OAuth2 Clients Summary ==="
echo ""
kanidm system oauth2 list

echo ""
echo "✅ All OAuth2 clients created successfully!"
echo ""
echo "📋 Resource Servers (for JWT validation):"
echo "   - pdt-api  → audience: pdt-api"
echo "   - nghr-api → audience: nghr-api"
echo ""
echo "📋 Client Applications (for OAuth2 flows):"
echo "   - vscode-mcp → for VS Code MCP integration"
echo ""
echo "📝 Discovery URLs:"
echo "   PDT:  https://idm.example.com/oauth2/openid/pdt-api/.well-known/openid-configuration"
echo "   NGHR: https://idm.example.com/oauth2/openid/nghr-api/.well-known/openid-configuration"
echo "   VS Code: https://idm.example.com/oauth2/openid/vscode-mcp/.well-known/openid-configuration"
