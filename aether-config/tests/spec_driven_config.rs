//! Tests for the spec-driven config layout (directory.rs).
//!
//! Layout under test:
//! ```text
//! configs/<api>/
//!   <api>-openapi.yaml   # machine: OpenAPI spec
//!   config.yaml          # human: server/apis/oidc/... (NO enhanced_tools)
//! ```
//! The loader must derive enhanced_tools from the sibling spec, keep the
//! hand-curated apis block, and reject specs that leak $ref pointers.

use aether_config::directory::ConfigDirectoryManager;
use serde_json::json;

fn openapi_spec() -> String {
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Test API", "version": "1.0.0"},
        "paths": {
            "/api/things/{id}": {
                "get": {
                    "operationId": "getThing",
                    "summary": "Get a thing",
                    "parameters": [
                        {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
                    ],
                    "responses": {"200": {"description": "OK"}}
                }
            },
            "/api/things": {
                "post": {
                    "operationId": "createThing",
                    "summary": "Create a thing",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {"$ref": "#/components/schemas/CreateThingRequest"}
                            }
                        }
                    },
                    "responses": {"200": {"description": "OK"}}
                }
            }
        },
        "components": {
            "schemas": {
                "AddTagRequest": {
                    "type": "object",
                    "required": ["category", "value"],
                    "properties": {
                        "category": {"type": "string"},
                        "value": {"type": "string"}
                    }
                },
                "CreateThingRequest": {
                    "type": "object",
                    "required": ["title"],
                    "properties": {
                        "title": {"type": "string"},
                        "tags": {
                            "type": "array",
                            "items": {"$ref": "#/components/schemas/AddTagRequest"}
                        }
                    }
                }
            }
        }
    })
    .to_string()
}

fn hand_config_yaml() -> String {
    // Minimal hand-curated config: no enhanced_tools, no openapi_spec.
    r#"server:
  host: 127.0.0.1
  port: 8001
  log_level: info
apis:
  pdt:
    base_url: http://localhost:8090
    openapi_spec: ''
    timeout: 30
"#
    .to_string()
}

async fn write_layout(dir: &std::path::Path, spec_name: &str, config_yaml: &str) {
    tokio::fs::create_dir_all(dir).await.unwrap();
    tokio::fs::write(dir.join(spec_name), openapi_spec()).await.unwrap();
    tokio::fs::write(dir.join("config.yaml"), config_yaml)
        .await
        .unwrap();
}

#[tokio::test]
async fn tools_derived_from_conventional_spec_name() {
    let tmp = std::env::temp_dir().join(format!("aether-test-{}", std::process::id()));
    let api_dir = tmp.join("apis/pdt");
    let _ = tokio::fs::remove_dir_all(&tmp).await;

    // Preferred naming: <api>-openapi.yaml
    write_layout(&api_dir, "pdt-openapi.yaml", &hand_config_yaml()).await;

    let manager = ConfigDirectoryManager::new(tmp.join("apis"));
    let (config, openapi) = manager.load_api_config("pdt").await.expect("must load");

    assert_eq!(config.enhanced_tools.len(), 2, "2 tools from spec");
    assert!(config.enhanced_tools.contains_key("pdt_getThing"));
    assert!(config.enhanced_tools.contains_key("pdt_createThing"));

    // Hand-curated apis block must survive (base_url preserved).
    assert_eq!(
        config.apis.get("pdt").unwrap().base_url,
        "http://localhost:8090"
    );

    // The returned openapi content is the spec we read.
    assert!(openapi.contains("CreateThingRequest"));

    let _ = tokio::fs::remove_dir_all(&tmp).await;
}

#[tokio::test]
async fn legacy_openapi_json_fallback_name_works() {
    let tmp = std::env::temp_dir().join(format!("aether-test-fb-{}", std::process::id()));
    let api_dir = tmp.join("apis/nghr");
    let _ = tokio::fs::remove_dir_all(&tmp).await;

    // Pre-rename layout: openapi.json
    write_layout(&api_dir, "openapi.json", &hand_config_yaml()).await;

    let manager = ConfigDirectoryManager::new(tmp.join("apis"));
    let (config, _) = manager.load_api_config("nghr").await.expect("must load");
    assert_eq!(config.enhanced_tools.len(), 2);

    let _ = tokio::fs::remove_dir_all(&tmp).await;
}

#[tokio::test]
async fn spec_with_ref_leak_fails_at_load() {
    let tmp = std::env::temp_dir().join(format!("aether-test-leak-{}", std::process::id()));
    let api_dir = tmp.join("apis/bad");
    let _ = tokio::fs::remove_dir_all(&tmp).await;

    // Spec with an unresolvable EXTERNAL ref — must fail loudly at load,
    // not silently produce broken tool schemas.
    let mut spec: serde_json::Value = serde_json::from_str(&openapi_spec()).unwrap();
    spec["components"]["schemas"]["CreateThingRequest"]["properties"]["ext"] =
        json!({"$ref": "https://example.com/other.json#/Foo"});

    tokio::fs::create_dir_all(&api_dir).await.unwrap();
    // Spec name must match the apis key ("bad"), not the dir name.
    tokio::fs::write(api_dir.join("bad-openapi.yaml"), spec.to_string())
        .await
        .unwrap();
    let bad_yaml = r#"server:
  host: 127.0.0.1
  port: 8001
  log_level: info
apis:
  bad:
    base_url: http://localhost:9999
    openapi_spec: ''
    timeout: 30
"#;
    tokio::fs::write(api_dir.join("config.yaml"), bad_yaml)
        .await
        .unwrap();

    let manager = ConfigDirectoryManager::new(tmp.join("apis"));
    let err = manager
        .load_api_config("bad")
        .await
        .expect_err("must fail on $ref leak");
    let msg = format!("{}", err);
    assert!(
        msg.contains("unresolvable $ref"),
        "expected unresolvable $ref error, got: {}",
        msg
    );

    let _ = tokio::fs::remove_dir_all(&tmp).await;
}

#[tokio::test]
async fn inline_enhanced_tools_still_work_without_spec() {
    let tmp = std::env::temp_dir().join(format!("aether-test-legacy-{}", std::process::id()));
    let api_dir = tmp.join("apis/old");
    let _ = tokio::fs::remove_dir_all(&tmp).await;

    // Legacy layout: no spec file at all, tools inline in config.yaml.
    let legacy_yaml = r#"server:
  host: 127.0.0.1
  port: 8001
  log_level: info
enhanced_tools:
  old_ping:
    name: old_ping
    description: Ping
    api_name: old
    operation_id: ping
    path: /ping
    method: GET
    parameters: []
    auth_spec: null
apis:
  old:
    base_url: http://localhost:9000
    openapi_spec: ''
    timeout: 30
"#;

    tokio::fs::create_dir_all(&api_dir).await.unwrap();
    tokio::fs::write(api_dir.join("config.yaml"), legacy_yaml)
        .await
        .unwrap();

    let manager = ConfigDirectoryManager::new(tmp.join("apis"));
    let (config, openapi) = manager.load_api_config("old").await.expect("must load");
    assert_eq!(config.enhanced_tools.len(), 1, "inline tools kept");
    assert!(config.enhanced_tools.contains_key("old_ping"));
    assert!(openapi.is_empty(), "no spec -> empty openapi content");

    let _ = tokio::fs::remove_dir_all(&tmp).await;
}

#[tokio::test]
async fn spec_overrides_inline_tools_when_both_present() {
    let tmp = std::env::temp_dir().join(format!("aether-test-both-{}", std::process::id()));
    let api_dir = tmp.join("apis/both");
    let _ = tokio::fs::remove_dir_all(&tmp).await;

    // Both spec and inline tools: spec wins (single source of truth).
    let mut yaml = r#"server:
  host: 127.0.0.1
  port: 8001
  log_level: info
apis:
  both:
    base_url: http://localhost:9998
    openapi_spec: ''
    timeout: 30
"#
    .to_string();
    yaml.push_str(
        r#"enhanced_tools:
  both_stale:
    name: both_stale
    description: Stale inline tool
    api_name: both
    operation_id: stale
    path: /stale
    method: GET
    parameters: []
    auth_spec: null
"#,
    );

    write_layout(&api_dir, "both-openapi.yaml", &yaml).await;

    let manager = ConfigDirectoryManager::new(tmp.join("apis"));
    let (config, _) = manager.load_api_config("both").await.expect("must load");
    // apis key is "pdt" (hand_config_yaml), so tools are pdt_*
    assert_eq!(config.enhanced_tools.len(), 2, "spec-derived tools win");
    assert!(!config.enhanced_tools.contains_key("both_stale"));
    assert!(config.enhanced_tools.contains_key("both_getThing"));

    let _ = tokio::fs::remove_dir_all(&tmp).await;
}

#[tokio::test]
async fn dir_suffix_does_not_leak_into_tool_prefix() {
    // Deployment reality: dirs named "nghr_auth"/"pdt_auth" while tools are
    // "nghr_*"/"pdt_*".  The apis key must drive the generation api_name,
    // NOT the directory name.
    let tmp = std::env::temp_dir().join(format!("aether-test-sfx-{}", std::process::id()));
    let api_dir = tmp.join("apis/nghr_auth");
    let _ = tokio::fs::remove_dir_all(&tmp).await;

    // Spec named after the apis key (nghr), not the dir (nghr_auth).
    let mut spec: serde_json::Value = serde_json::from_str(&openapi_spec()).unwrap();
    // Trim to a single operation for clarity
    spec["paths"] = json!({
        "/ping": {"get": {"operationId": "ping", "responses": {"200": {"description": "OK"}}}}
    });

    let yaml = r#"server:
  host: 127.0.0.1
  port: 8007
  log_level: info
apis:
  nghr:
    base_url: http://localhost:3004
    openapi_spec: ''
    timeout: 30
"#;

    tokio::fs::create_dir_all(&api_dir).await.unwrap();
    tokio::fs::write(api_dir.join("nghr-openapi.json"), spec.to_string())
        .await
        .unwrap();
    tokio::fs::write(api_dir.join("config.yaml"), yaml).await.unwrap();

    let manager = ConfigDirectoryManager::new(tmp.join("apis"));
    let (config, _) = manager.load_api_config("nghr_auth").await.expect("must load");

    // Tool prefix must be "nghr_ping", NOT "nghr_auth_ping"
    assert!(
        config.enhanced_tools.contains_key("nghr_ping"),
        "expected nghr_ping, got: {:?}",
        config.enhanced_tools.keys().collect::<Vec<_>>()
    );
    assert!(!config.enhanced_tools.contains_key("nghr_auth_ping"));

    // Base URL from hand-curated apis survives
    assert_eq!(
        config.apis.get("nghr").unwrap().base_url,
        "http://localhost:3004"
    );

    let _ = tokio::fs::remove_dir_all(&tmp).await;
}

#[tokio::test]
async fn real_configs_load_with_derived_tools() {
    // End-to-end against the repo's actual configs/ tree (skips when the
    // tree is absent, e.g. published crate).
    let root = std::path::Path::new("configs");
    if !root.exists() {
        eprintln!("skipping: configs/ not found");
        return;
    }

    let manager = ConfigDirectoryManager::new(root);
    let apis = manager.list_apis().await.expect("list");
    assert!(!apis.is_empty(), "expected at least one API config");

    for api in apis {
        let (config, openapi) = manager
            .load_api_config(&api)
            .await
            .unwrap_or_else(|e| panic!("config {} must load: {}", api, e));

        assert!(
            !config.enhanced_tools.is_empty(),
            "API {} must expose tools",
            api
        );
        assert!(
            !openapi.is_empty(),
            "API {} must have a discoverable spec file",
            api
        );

        // No $ref may leak into any derived tool schema.
        for (name, tool) in &config.enhanced_tools {
            for param in &tool.parameters {
                let s = serde_json::to_string(&param.schema).unwrap();
                assert!(
                    !s.contains("$ref"),
                    "{}.{} leaked a $ref: {}",
                    api,
                    name,
                    s
                );
            }
        }
    }
}
