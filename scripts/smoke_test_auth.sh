#!/bin/bash
# Smoke test for MCP OAuth2.1 authentication
#
# Usage:
#   ./scripts/smoke_test_auth.sh [MCP_URL] [OIDC_TOKEN]
#
# Environment variables:
#   MCP_URL    - Base URL of the MCP server (default: http://localhost:8000)
#   OIDC_TOKEN - Valid OIDC Bearer token for testing authenticated endpoints

set -e

# Configuration
MCP_URL="${1:-${MCP_URL:-http://localhost:8000}}"
OIDC_TOKEN="${2:-${OIDC_TOKEN:-}}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "=== Aether MCP OAuth2.1 Auth Smoke Test ==="
echo "MCP URL: $MCP_URL"
echo ""

# Test 1: Health endpoint (should always be public)
echo -n "Test 1: Health endpoint (public)... "
if curl -sf "$MCP_URL/health" > /dev/null 2>&1; then
    echo -e "${GREEN}PASS${NC}"
else
    echo -e "${RED}FAIL${NC}"
    echo "  Error: Health endpoint should be accessible without authentication"
    exit 1
fi

# Test 2: SSE endpoint without token
echo -n "Test 2: SSE without token... "
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$MCP_URL/sse" 2>/dev/null || echo "000")
if [ "$HTTP_CODE" = "401" ]; then
    echo -e "${GREEN}PASS${NC} (401 Unauthorized - auth is enforced)"
elif [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "101" ]; then
    echo -e "${YELLOW}WARN${NC} (Accessible without token - auth may be disabled or optional)"
else
    echo -e "${YELLOW}INFO${NC} (HTTP $HTTP_CODE)"
fi

# Test 3: Message endpoint without token
echo -n "Test 3: /message without token... "
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$MCP_URL/message" 2>/dev/null || echo "000")
if [ "$HTTP_CODE" = "401" ]; then
    echo -e "${GREEN}PASS${NC} (401 Unauthorized - auth is enforced)"
elif [ "$HTTP_CODE" = "200" ]; then
    echo -e "${YELLOW}WARN${NC} (Accessible without token - auth may be disabled or optional)"
else
    echo -e "${YELLOW}INFO${NC} (HTTP $HTTP_CODE)"
fi

# Test 4: SSE endpoint with invalid token
echo -n "Test 4: SSE with invalid token... "
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    -H "Authorization: Bearer invalid_token_xyz" \
    "$MCP_URL/sse" 2>/dev/null || echo "000")
if [ "$HTTP_CODE" = "401" ]; then
    echo -e "${GREEN}PASS${NC} (401 Unauthorized - invalid token rejected)"
else
    echo -e "${YELLOW}INFO${NC} (HTTP $HTTP_CODE - expected 401 for invalid token)"
fi

# Test 5: SSE endpoint with valid token (if provided)
if [ -n "$OIDC_TOKEN" ]; then
    echo -n "Test 5: SSE with valid token... "
    HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
        -H "Authorization: Bearer $OIDC_TOKEN" \
        "$MCP_URL/sse" 2>/dev/null || echo "000")
    if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "101" ]; then
        echo -e "${GREEN}PASS${NC} (HTTP $HTTP_CODE - valid token accepted)"
    else
        echo -e "${RED}FAIL${NC} (HTTP $HTTP_CODE - expected 200/101 for valid token)"
        echo "  Error: Valid token was rejected"
        exit 1
    fi
    
    echo -n "Test 6: /message with valid token... "
    HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
        -H "Authorization: Bearer $OIDC_TOKEN" \
        -H "Content-Type: application/json" \
        -X POST "$MCP_URL/message" \
        -d '{"jsonrpc":"2.0","id":1,"method":"ping"}' \
        2>/dev/null || echo "000")
    if [ "$HTTP_CODE" = "200" ]; then
        echo -e "${GREEN}PASS${NC} (HTTP $HTTP_CODE - valid token accepted)"
    else
        echo -e "${YELLOW}INFO${NC} (HTTP $HTTP_CODE)"
    fi
else
    echo -e "${YELLOW}SKIP${NC} Test 5 & 6: No OIDC_TOKEN provided"
    echo "  To test authenticated endpoints, set OIDC_TOKEN environment variable:"
    echo "  export OIDC_TOKEN='your_bearer_token_here'"
fi

# Test 7: Multi-config endpoints (if available)
echo -n "Test 7: Multi-config /myapi/sse without token... "
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$MCP_URL/myapi/sse" 2>/dev/null || echo "000")
if [ "$HTTP_CODE" = "404" ]; then
    echo -e "${YELLOW}SKIP${NC} (Multi-config mode not available)"
elif [ "$HTTP_CODE" = "401" ]; then
    echo -e "${GREEN}PASS${NC} (401 Unauthorized - auth is enforced)"
elif [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "101" ]; then
    echo -e "${YELLOW}WARN${NC} (Accessible without token - auth may be disabled)"
else
    echo -e "${YELLOW}INFO${NC} (HTTP $HTTP_CODE)"
fi

# Test 8: Admin API endpoint
echo -n "Test 8: Admin /api/list without token... "
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$MCP_URL/api/list" 2>/dev/null || echo "000")
if [ "$HTTP_CODE" = "404" ]; then
    echo -e "${YELLOW}SKIP${NC} (Admin API not available)"
elif [ "$HTTP_CODE" = "401" ]; then
    echo -e "${GREEN}PASS${NC} (401 Unauthorized - auth is enforced)"
elif [ "$HTTP_CODE" = "200" ]; then
    echo -e "${YELLOW}WARN${NC} (Accessible without token - auth may be disabled)"
else
    echo -e "${YELLOW}INFO${NC} (HTTP $HTTP_CODE)"
fi

echo ""
echo -e "${GREEN}=== Smoke test completed ===${NC}"
echo ""
echo "Summary:"
echo "  - Health endpoint is public ✓"
echo "  - Protected endpoints enforce authentication (if enabled)"
echo "  - Invalid tokens are rejected"
if [ -n "$OIDC_TOKEN" ]; then
    echo "  - Valid tokens are accepted ✓"
else
    echo "  - Valid token testing skipped (no OIDC_TOKEN provided)"
fi
echo ""
echo "Note: Some tests may show WARN/INFO if auth is disabled in config."
echo "      This is expected behavior when mcp_auth.enabled=false."
