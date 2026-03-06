#!/bin/bash
# Setup VS Code OAuth2 client in Kanidm for MCP authentication

set -e

echo "🚀 Setting up VS Code OAuth2 client for MCP..."
echo ""

# Create VS Code OAuth2 public client
echo "Creating VS Code OAuth2 client..."
kanidm system oauth2 create-public vscode-mcp "VS Code MCP Client" http://127.0.0.1:33418

# Add additional redirect URIs
echo "Adding redirect URIs..."
kanidm system oauth2 add-redirect-url vscode-mcp https://vscode.dev/redirect

# Enable localhost redirects
echo "Enabling localhost redirects..."
kanidm system oauth2 enable-localhost-redirects vscode-mcp

# Set scope map
echo "Setting scope map..."
kanidm system oauth2 update-scope-map vscode-mcp idm_all_persons openid profile email

echo ""
echo "✅ VS Code OAuth2 client created successfully!"
echo ""
echo "Client ID: vscode-mcp"
echo ""
echo "Now in VS Code:"
echo "   1. Click [Copy URIs & Proceed]"
echo "   2. Enter Client ID: vscode-mcp"
echo "   3. Click OK"
echo "   4. Login to Kanidm when redirected"
echo "   5. ✅ VS Code will connect to Aether!"
echo ""
echo "Showing client details:"
kanidm system oauth2 get vscode-mcp
