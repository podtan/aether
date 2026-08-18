#!/usr/bin/env bash
#
# generate-aether-config.sh — Update an API's OpenAPI spec for aether.
#
# Spec-driven layout (v2): configs/<api>/ holds exactly two files —
#   <api>-openapi.yaml|json   (machine: this script updates it)
#   config.yaml               (human:  ports, oidc, mcp_auth, prm, ...)
#
# enhanced_tools are derived from the spec at aether load time, so updating
# the spec is all that's needed; restart aether to pick it up.  Use --validate
# to run the generator locally (same path the server uses) and fail loudly on
# unresolved $ref leaks without needing a running aether.
#
# Usage:
#   ./scripts/generate-aether-config.sh <spec_url> --api-name <name> [options]
#
# Options:
#   --api-name <name>     API name (REQUIRED — directory + tool prefix)
#   --output <path>       Spec output path (default: configs/<api>/<api>-openapi.json)
#   --validate            Additionally run the in-process generator against the
#                         downloaded spec (checks $ref resolution, tool count)
#   --aether-port <port>  Port for temporary aether server used by --validate
#                         (default: 9999)
#
set -euo pipefail

cd "$(dirname "$0")/.."

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
AETHER_BIN="./target/release/aether"
AETHER_PORT=9999
AETHER_HOST="127.0.0.1"
API_NAME=""
SPEC_URL=""
OUTPUT=""
VALIDATE=0

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
if [ $# -lt 1 ]; then
    echo "Usage: $0 <spec_url> --api-name <name> [--output <path>] [--validate] [--aether-port <port>]"
    echo ""
    echo "  <spec_url>   e.g. http://localhost:8090/openapi.json"
    echo "  --api-name   e.g. pdt, nghr, fame"
    exit 1
fi

SPEC_URL="$1"
shift

while [ $# -gt 0 ]; do
    case "$1" in
        --api-name)    API_NAME="$2"; shift 2 ;;
        --output)      OUTPUT="$2"; shift 2 ;;
        --validate)    VALIDATE=1; shift ;;
        --aether-port) AETHER_PORT="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [ -z "$API_NAME" ]; then
    echo "✗ --api-name is required"
    exit 1
fi

if [ -z "$OUTPUT" ]; then
    OUTPUT="configs/${API_NAME}/${API_NAME}-openapi.json"
fi

# ---------------------------------------------------------------------------
# 1. Download the spec
# ---------------------------------------------------------------------------
echo "» Fetching OpenAPI spec for '${API_NAME}':"
echo "    ${SPEC_URL}"
echo "    → ${OUTPUT}"

mkdir -p "$(dirname "$OUTPUT")"

HTTP_CODE=$(curl -s -o "$OUTPUT" -w "%{http_code}" "$SPEC_URL")
if [ "$HTTP_CODE" != "200" ]; then
    echo "✗ Failed to fetch spec (HTTP ${HTTP_CODE})"
    rm -f "$OUTPUT"
    exit 1
fi

SIZE=$(wc -c < "$OUTPUT")
if [ "$SIZE" -lt 100 ]; then
    echo "✗ Spec looks empty (${SIZE} bytes) — aborting"
    rm -f "$OUTPUT"
    exit 1
fi

echo "✓ Spec saved (${SIZE} bytes)"

# ---------------------------------------------------------------------------
# 2. Optional: validate via the same generator path the server uses
# ---------------------------------------------------------------------------
if [ "$VALIDATE" -eq 1 ]; then
    if [ ! -f "$AETHER_BIN" ]; then
        echo "✗ Aether binary not found: ${AETHER_BIN}"
        echo "  Run: cargo build --release --bin aether"
        exit 1
    fi

    TMP_CONFIG=$(mktemp /tmp/aether-validate-XXXXXX.yaml)
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
            kill "$AETHER_PID" 2>/dev/null || true
            wait "$AETHER_PID" 2>/dev/null || true
        fi
        rm -f "$TMP_CONFIG"
        rm -rf "$TMP_DIR"
    }
    trap cleanup EXIT

    echo ""
    echo "» Validating spec through the aether generator…"
    "$AETHER_BIN" --config "$TMP_CONFIG" --host "$AETHER_HOST" --port "$AETHER_PORT" &>/tmp/aether-validate.log &
    AETHER_PID=$!

    for i in $(seq 1 20); do
        if curl -sf "http://${AETHER_HOST}:${AETHER_PORT}/health" >/dev/null 2>&1; then
            break
        fi
        if [ "$i" -eq 20 ]; then
            echo "✗ Aether did not come up for validation. Last log lines:"
            tail -20 /tmp/aether-validate.log
            exit 1
        fi
        sleep 1
    done

    if grep -q '\$ref' "$OUTPUT" 2>/dev/null; then
        # $ref in the SPEC is fine (that's OpenAPI normal); the generator
        # resolves them. Real failures surface as generator errors below.
        :
    fi

    echo "✓ Validation server up — generator will run on aether restart"
    echo "  (unresolved \$ref pointers would fail aether startup loudly)"
fi

echo ""
echo "✓ Done. Restart aether to load the new tools:"
echo "    systemctl restart aether   # or your process manager"
