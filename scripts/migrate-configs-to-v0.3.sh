#!/usr/bin/env bash
#
# migrate-configs-to-v0.3.sh — Break monolithic aether configs into the
# spec-driven layout (aether >= 0.3.0).
#
# For each configs/<dir>/ with a monolithic config.yaml this script:
#   1. Backs up config.yaml -> config.yaml.pre-v0.3.bak
#   2. Determines the real api name (from the `apis:` key — NOT the dir name,
#      which may carry deployment suffixes like "_auth")
#   3. Ensures an OpenAPI spec exists, in priority order:
#        a. existing <dir>/openapi.json            -> rename <api>-openapi.json
#        b. existing <dir>/<api>-openapi.*         -> keep
#        c. fetch from apis.<api>.base_url/openapi.json   (curl, 5s timeout)
#        d. EXTRACT from the monolithic config's inline enhanced_tools
#           (rebuilds a minimal spec from the tools — last resort when the
#            backend is unreachable or the spec is known-stale)
#      --extract <dir> forces (d) for that dir: any stale stored spec
#      (openapi.json/yaml) is deleted and the spec is rebuilt from the
#      monolithic config's current tools.
#   4. Rewrites config.yaml: strips `tools:`/`enhanced_tools:`, keeps all
#      hand-curated sections verbatim
#   5. Deletes stale artifact files (.bak/.bad from old manual edits) unless
#      --keep-artifacts
#
# Idempotent: safe to re-run; already-migrated dirs are detected and skipped.
#
# Usage:
#   ./migrate-configs-to-v0.3.sh [--dry-run] [--keep-artifacts] [--configs-dir DIR]
#                                [--extract <dirname>]...
#
# NOTE: deploy the aether >= 0.3.0 binary BEFORE restarting with migrated
# configs — the slim config.yaml requires the spec-driven loader.
#
set -euo pipefail

CONFIGS_DIR="/opt/aether/mcp/configs"
DRY_RUN=0
KEEP_ARTIFACTS=0
FORCE_EXTRACT=""

while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run)        DRY_RUN=1; shift ;;
        --keep-artifacts) KEEP_ARTIFACTS=1; shift ;;
        --configs-dir)    CONFIGS_DIR="$2"; shift 2 ;;
        --extract)        FORCE_EXTRACT="$FORCE_EXTRACT,$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [ ! -d "$CONFIGS_DIR" ]; then
    echo "✗ configs dir not found: $CONFIGS_DIR"
    exit 1
fi

run() { # run <cmd...> — respect dry-run
    if [ "$DRY_RUN" -eq 1 ]; then
        echo "      [dry-run] $*"
    else
        "$@"
    fi
}

migrate_dir() {
    local dir="$1"
    local cfg="$dir/config.yaml"
    [ -f "$cfg" ] || return 0

    echo "── $(basename "$dir")"

    # Idempotency: already migrated = no inline enhanced_tools
    if ! grep -q '^enhanced_tools:' "$cfg"; then
        echo "   ✓ already migrated (no enhanced_tools block) — skipping"
        return 0
    fi

    # 1. Backup
    if [ ! -f "$dir/config.yaml.pre-v0.3.bak" ]; then
        run cp "$cfg" "$dir/config.yaml.pre-v0.3.bak"
        echo "   ✓ backed up -> config.yaml.pre-v0.3.bak"
    fi

    # 2. Real api name from the `apis:` key (dir name may differ)
    local api_name
    api_name=$(awk '/^apis:/{found=1; next} found && /^  [a-zA-Z0-9_-]+:/{gsub(/[: ]/,"",$1); print $1; exit}' "$cfg")
    if [ -z "$api_name" ]; then
        api_name=$(basename "$dir")
        echo "   ⚠ no apis key found — falling back to dir name '$api_name'"
    fi
    echo "   • api name: $api_name (dir: $(basename "$dir"))"

    # 3. Spec provisioning
    local spec_file=""
    local dirbase
    dirbase=$(basename "$dir")
    local forced=0
    if [[ ",${FORCE_EXTRACT}," == *",${dirbase},"* ]]; then
        forced=1
        echo "   • forced extraction (--extract $dirbase): ignoring any stored spec"
        for legacy in openapi.json openapi.yaml; do
            [ -f "$dir/$legacy" ] && run rm -f "$dir/$legacy" \
                && echo "   ✓ deleted stale $legacy"
        done
    elif [ -f "$dir/${api_name}-openapi.yaml" ] || [ -f "$dir/${api_name}-openapi.yml" ] || [ -f "$dir/${api_name}-openapi.json" ]; then
        spec_file=$(ls "$dir/${api_name}-openapi."* 2>/dev/null | head -1)
        echo "   ✓ spec already present: $(basename "$spec_file")"
    elif [ -f "$dir/openapi.json" ]; then
        run mv "$dir/openapi.json" "$dir/${api_name}-openapi.json"
        spec_file="$dir/${api_name}-openapi.json"
        echo "   ✓ renamed legacy openapi.json -> ${api_name}-openapi.json"
    fi

    # Fetch from backend (only when nothing usable on disk and not forced)
    if [ -z "$spec_file" ] && [ "$forced" -eq 0 ]; then
        local base_url
        base_url=$(awk '/^apis:/{found=1; next} found && /base_url/{print $2; exit}' "$cfg")
        if [ -n "$base_url" ]; then
            local code
            code=$(curl -sk -o /tmp/spec-fetch-$$.json -w "%{http_code}" -m 5 "$base_url/openapi.json" 2>/dev/null || echo "000")
            if [ "$code" = "200" ] && [ "$(wc -c < /tmp/spec-fetch-$$.json)" -gt 500 ]; then
                run cp "/tmp/spec-fetch-$$.json" "$dir/${api_name}-openapi.json"
                spec_file="$dir/${api_name}-openapi.json"
                echo "   ✓ fetched fresh spec from $base_url ($(wc -c < /tmp/spec-fetch-$$.json) bytes)"
            else
                echo "   ⚠ fetch from $base_url failed (HTTP $code)"
            fi
            rm -f "/tmp/spec-fetch-$$.json"
        fi
    fi

    # Last resort (or --extract): build spec from the monolithic config's tools
    if [ -z "$spec_file" ] || [ "$forced" -eq 1 ]; then
        [ -n "$spec_file" ] && run rm -f "$spec_file"   # replace stored spec under --extract
        echo "   • extracting spec from inline enhanced_tools"
            run python3 - "$cfg" "$dir/${api_name}-openapi.json" "$api_name" <<'PYEOF'
import json, sys, yaml
cfg_path, out_path, api = sys.argv[1], sys.argv[2], sys.argv[3]
cfg = yaml.safe_load(open(cfg_path))
tools = cfg.get("enhanced_tools") or {}
paths = {}
for t in tools.values():
    if t.get("api_name") != api:
        continue
    params = []
    req_body_props, req_body_required = {}, []
    for p in t.get("parameters") or []:
        schema = dict(p.get("schema") or {"type": "string"})
        if p.get("param_type") in ("body", "form"):
            if "description" in schema and "description" not in p:
                pass
            req_body_props[p["name"]] = schema
            if p.get("required"):
                req_body_required.append(p["name"])
        else:
            schema.pop("description", None)
            params.append({"name": p["name"], "in": p.get("param_type", "query"),
                           "required": bool(p.get("required")), "schema": schema})
    op = {"operationId": t.get("operation_id"), "summary": t.get("description", "")[:80],
          "responses": {"200": {"description": "OK"}}}
    if params:
        op["parameters"] = params
    if req_body_props:
        op["requestBody"] = {"required": bool(req_body_required), "content": {"application/json": {
            "schema": {"type": "object", "properties": req_body_props,
                        "required": req_body_required}}}}
    path = t.get("path", "/")
    method = (t.get("method") or "GET").lower()
    # paths keyed by path; a path may carry multiple methods (GET/DELETE/
    # PATCH on the same route = separate tools) — never reset the dict.
    paths.setdefault(path, {})
    paths[path][method] = op
spec = {"openapi": "3.0.3", "info": {"title": f"{api} (extracted from monolithic config)",
       "version": "0.0.0-extracted"}, "paths": paths}
json.dump(spec, open(out_path, "w"), indent=1)
print(f"      extracted {len(paths)} paths")
PYEOF
        spec_file="$dir/${api_name}-openapi.json"
    fi

    # 4. Rewrite config.yaml — strip tool blocks, keep everything else
    run python3 - "$cfg" <<'PYEOF'
import sys, re
path = sys.argv[1]
text = open(path).read()
# Remove the `tools: {}` line and the whole `enhanced_tools:` block.
# Block scalars (description: |) contain blank lines, so the block match
# must accept indented lines AND blank lines, up to the next top-level key.
text = re.sub(r"^tools: \{\}\n", "", text, flags=re.M)
text = re.sub(r"^enhanced_tools:\n(?:^  .*\n?|^\n)+", "", text, flags=re.M)
open(path, "w").write(text)
PYEOF
    echo "   ✓ rewrote config.yaml ($(wc -l < "$cfg" | tr -d ' ') lines)"

    # 5. Clean old artifacts
    if [ "$KEEP_ARTIFACTS" -eq 0 ]; then
        for f in "$dir/config.yaml.bak" "$dir/config.yaml.bad"; do
            [ -f "$f" ] && run rm -f "$f" && echo "   ✓ removed $(basename "$f")"
        done
    fi

    echo "   → next: restart aether with the 0.3.0 binary"
}

echo "aether config migration -> spec-driven layout (v0.3.0)"
echo "configs dir: $CONFIGS_DIR"
[ "$DRY_RUN" -eq 1 ] && echo "MODE: DRY RUN (no changes)"
echo ""

for d in "$CONFIGS_DIR"/*/; do
    [ -d "$d" ] || continue
    migrate_dir "$d"
done

echo ""
echo "done. Deploy the >=0.3.0 binary and: sudo systemctl restart aether"
