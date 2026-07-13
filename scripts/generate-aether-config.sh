#!/usr/bin/env bash
#
# generate-aether-config.sh — Generate Aether config from an OpenAPI spec.
#
# Two-phase process:
#   1. Start aether MCP server (HTTP mode, temporarily)
#   2. Run aether-cli against it to generate the config
#   3. Stop the server
#
# Usage:
#   ./scripts/generate-aether-config.sh <openapi_path> [options]
#
# Options:
#   --api-name <name>     API name (default: nghr)
#   --base-url <url>      Base URL for the API (default: extracted from spec)
#   --output <path>       Output config path (default: configs/<api-name>/config.yaml)
#   --aether-port <port>  Port for temporary aether server (default: 9999)
#
set -euo pipefail

cd "$(dirname "$0")/.."

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
AETHER_BIN="./target/release/aether"
AETHER_CLI="./target/release/aether-cli"
AETHER_PORT=9999
AETHER_HOST="127.0.0.1"
API_NAME="nghr"
BASE_URL=""
OUTPUT=""
OPENAPI_PATH=""

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
if [ $# -lt 1 ]; then
    echo "Usage: $0 <openapi_path> [--api-name <name>] [--base-url <url>] [--output <path>] [--aether-port <port>]"
    exit 1
fi

OPENAPI_PATH="$1"
shift

while [ $# -gt 0 ]; do
    case "$1" in
        --api-name)    API_NAME="$2"; shift 2 ;;
        --base-url)    BASE_URL="$2"; shift 2 ;;
        --output)      OUTPUT="$2"; shift 2 ;;
        --aether-port) AETHER_PORT="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [ -z "$OUTPUT" ]; then
    OUTPUT="configs/${API_NAME}/config.yaml"
fi

# ---------------------------------------------------------------------------
# Validate
# ---------------------------------------------------------------------------
if [ ! -f "$OPENAPI_PATH" ]; then
    echo "✗ OpenAPI spec not found: $OPENAPI_PATH"
    exit 1
fi

if [ ! -f "$AETHER_BIN" ]; then
    echo "✗ Aether binary not found: $AETHER_BIN"
    echo "  Run: cargo build --release"
    exit 1
fi

if [ ! -f "$AETHER_CLI" ]; then
    echo "✗ Aether CLI binary not found: $AETHER_CLI"
    echo "  Run: cargo build --release"
    exit 1
fi

# ---------------------------------------------------------------------------
# Create a minimal temporary config for the MCP server
# ---------------------------------------------------------------------------
TMP_CONFIG="/tmp/aether-generate-config.yaml"
TMP_DIR=$(mktemp -d)
cat > "$TMP_CONFIG" <<EOF
server:
  host: ${AETHER_HOST}
  port: ${AETHER_PORT}
  log_level: info
apis: {}
enhanced_tools: {}
EOF

cleanup() {
    if [ -n "${AETHER_PID:-}" ] && kill -0 "$AETHER_PID" 2>/dev/null; then
        echo "» Stopping aether MCP server (PID ${AETHER_PID})…"
        kill "$AETHER_PID" 2>/dev/null || true
        wait "$AETHER_PID" 2>/dev/null || true
    fi
    rm -f "$TMP_CONFIG"
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# 1. Start aether MCP server
# ---------------------------------------------------------------------------
echo "» Starting aether MCP server on ${AETHER_HOST}:${AETHER_PORT}…"
"$AETHER_BIN" --config "$TMP_CONFIG" --host "$AETHER_HOST" --port "$AETHER_PORT" &>/tmp/aether-generate.log &
AETHER_PID=$!

echo "» Waiting for aether to come up…"
for i in $(seq 1 20); do
    if curl -sf "http://${AETHER_HOST}:${AETHER_PORT}/health" >/dev/null 2>&1; then
        echo "✓ Aether server is up (attempt ${i})"
        break
    fi
    if [ "$i" -eq 20 ]; then
        echo "✗ Aether did not come up in time. Last log lines:"
        tail -20 /tmp/aether-generate.log
        exit 1
    fi
    sleep 1
done

# ---------------------------------------------------------------------------
# 2. Run aether-cli to generate config
# ---------------------------------------------------------------------------
echo ""
echo "» Generating aether config:"
echo "    OpenAPI:  ${OPENAPI_PATH}"
echo "    API name: ${API_NAME}"
if [ -n "$BASE_URL" ]; then
    echo "    Base URL: ${BASE_URL}"
fi
echo "    Output:   ${OUTPUT}"
echo ""

mkdir -p "$(dirname "$OUTPUT")"

CLI_ARGS=(
    generate-config
    --input "$OPENAPI_PATH"
    --output "$OUTPUT"
    --api-name "$API_NAME"
    --server "http://${AETHER_HOST}:${AETHER_PORT}"
)

if [ -n "$BASE_URL" ]; then
    CLI_ARGS+=(--base-url "$BASE_URL")
fi

"$AETHER_CLI" "${CLI_ARGS[@]}"

# ---------------------------------------------------------------------------
# 3. Report
# ---------------------------------------------------------------------------
if [ -f "$OUTPUT" ]; then
    TOOL_COUNT=$(grep -c '^  [a-z]' "$OUTPUT" 2>/dev/null || echo "?")
    SIZE=$(wc -c < "$OUTPUT")
    echo ""
    echo "✓ Config generated successfully: ${OUTPUT}"
    echo "  Size: ${SIZE} bytes"
else
    echo ""
    echo "✗ Config generation may have failed — output file not found"
    exit 1
fi
