//! Regression tests for $ref resolution in config generation.
//!
//! Background (nghr issue d21d7308-6251-4792-966d-0cd43a1b9c5a): MCP tool
//! parameter schemas travel as standalone JSON documents with no
//! `components` section.  Any `#/components/...` pointer that survives config
//! generation breaks strict JSON-Schema consumers (llama-server rejects the
//! entire tools payload with HTTP 400).
//!
//! The historical tripwire shape — pdt's `tags` parameter — is
//! `tags: {type: array, items: {$ref: '#/components/schemas/AddTagRequest'}}`,
//! which the resolver must inline into a self-contained object schema.

use aether_config::generator::generate_config_from_openapi;
use aether_config::AetherConfig;
use serde_json::json;

/// Minimal OpenAPI spec where a requestBody property is an array whose
/// `items` is a `$ref` — the exact shape of pdt's AddTagRequest leak.
fn spec_with_items_ref() -> String {
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Test API", "version": "1.0.0"},
        "paths": {
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
                        "category": {"type": "string", "maxLength": 64},
                        "value": {"type": "string", "maxLength": 256}
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

/// OpenAPI spec exercising every schema keyword the resolver must recurse into.
fn spec_with_all_keywords() -> String {
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Test API", "version": "1.0.0"},
        "paths": {
            "/api/things": {
                "post": {
                    "operationId": "createThing",
                    "summary": "Create a thing",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {"$ref": "#/components/schemas/Everything"}
                            }
                        }
                    },
                    "responses": {"200": {"description": "OK"}}
                }
            }
        },
        "components": {
            "schemas": {
                "Nested": {
                    "type": "object",
                    "properties": {
                        "deep": {"$ref": "#/components/schemas/Deep"}
                    }
                },
                "Deep": {
                    "type": "string",
                    "enum": ["a", "b"]
                },
                "RefEnum": {
                    "type": "string",
                    "enum": ["Medium", "Low", "High"]
                },
                "Everything": {
                    "type": "object",
                    "required": ["title"],
                    "properties": {
                        "title": {"type": "string"},
                        "items_ref": {
                            "type": "array",
                            "items": {"$ref": "#/components/schemas/RefEnum"}
                        },
                        "nested_props": {"$ref": "#/components/schemas/Nested"},
                        "additional": {
                            "type": "object",
                            "additionalProperties": {"$ref": "#/components/schemas/RefEnum"}
                        },
                        "one_of": {
                            "oneOf": [
                                {"type": "null"},
                                {"$ref": "#/components/schemas/RefEnum"}
                            ]
                        },
                        "any_of": {
                            "anyOf": [
                                {"type": "string"},
                                {"$ref": "#/components/schemas/Nested"}
                            ]
                        },
                        "all_of": {
                            "allOf": [
                                {"type": "object"},
                                {"$ref": "#/components/schemas/Nested"}
                            ]
                        },
                        "not_ref": {
                            "type": "object",
                            "not": {"$ref": "#/components/schemas/Nested"}
                        },
                        "contains_ref": {
                            "type": "array",
                            "contains": {"$ref": "#/components/schemas/RefEnum"}
                        }
                    }
                }
            }
        }
    })
    .to_string()
}

/// OpenAPI spec with a self-referencing schema — must not infinite-loop.
fn spec_with_recursive_ref() -> String {
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Test API", "version": "1.0.0"},
        "paths": {
            "/api/things": {
                "post": {
                    "operationId": "createThing",
                    "summary": "Create a thing",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {"$ref": "#/components/schemas/Node"}
                            }
                        }
                    },
                    "responses": {"200": {"description": "OK"}}
                }
            }
        },
        "components": {
            "schemas": {
                "Node": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "children": {
                            "type": "array",
                            "items": {"$ref": "#/components/schemas/Node"}
                        }
                    }
                }
            }
        }
    })
    .to_string()
}

/// Path/query parameters containing refs (formerly handled by the shallow
/// `get_effective_schema`, which never resolved nested refs).
fn spec_with_param_ref() -> String {
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Test API", "version": "1.0.0"},
        "paths": {
            "/api/things/{id}": {
                "get": {
                    "operationId": "getThing",
                    "summary": "Get a thing",
                    "parameters": [
                        {
                            "name": "id",
                            "in": "path",
                            "required": true,
                            "schema": {"type": "string"}
                        },
                        {
                            "name": "filter",
                            "in": "query",
                            "required": false,
                            "schema": {
                                "type": "array",
                                "items": {"$ref": "#/components/schemas/FilterValue"}
                            }
                        }
                    ],
                    "responses": {"200": {"description": "OK"}}
                }
            }
        },
        "components": {
            "schemas": {
                "FilterValue": {
                    "type": "string",
                    "enum": ["alpha", "beta"]
                }
            }
        }
    })
    .to_string()
}

/// Collect every `$ref` pointer remaining anywhere in a JSON value.
fn find_refs(value: &serde_json::Value) -> Vec<String> {
    let mut refs = Vec::new();
    fn walk(v: &serde_json::Value, acc: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, sub) in map {
                    if k == "$ref" {
                        if let Some(s) = sub.as_str() {
                            acc.push(s.to_string());
                        }
                    } else {
                        walk(sub, acc);
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for sub in items {
                    walk(sub, acc);
                }
            }
            _ => {}
        }
    }
    walk(value, &mut refs);
    refs
}

fn get_tool_param<'a>(config: &'a AetherConfig, tool: &str, param: &str) -> &'a serde_json::Value {
    &config
        .enhanced_tools
        .get(tool)
        .unwrap_or_else(|| panic!("tool {} not found", tool))
        .parameters
        .iter()
        .find(|p| p.name == param)
        .unwrap_or_else(|| panic!("param {} not found in {}", param, tool))
        .schema
}

#[tokio::test]
async fn items_nested_ref_is_inlined() {
    let config =
        generate_config_from_openapi(&spec_with_items_ref(), "test", "http://localhost:9999")
            .await
            .expect("generation must succeed");

    let tags = get_tool_param(&config, "test_createThing", "tags");
    assert_eq!(tags.get("type").and_then(|t| t.as_str()), Some("array"));

    // The items $ref must be replaced with the inlined AddTagRequest object.
    let items = tags.get("items").expect("items must survive resolution");
    assert_eq!(
        items.get("type").and_then(|t| t.as_str()),
        Some("object"),
        "items should be the inlined schema, got: {items}"
    );
    let props = items
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("inlined AddTagRequest must expose properties");
    assert!(props.contains_key("category"), "category must be inlined");
    assert!(props.contains_key("value"), "value must be inlined");

    assert!(
        find_refs(tags).is_empty(),
        "no $ref may remain: {:?}",
        find_refs(tags)
    );
}

#[tokio::test]
async fn all_schema_keywords_resolve_refs() {
    let config =
        generate_config_from_openapi(&spec_with_all_keywords(), "test", "http://localhost:9999")
            .await
            .expect("generation must succeed");

    let everything = get_tool_param(&config, "test_createThing", "title").get("type");
    assert_eq!(everything.and_then(|t| t.as_str()), Some("string"));

    // Every property of the "Everything" schema must be fully resolved.
    for prop in [
        "items_ref",
        "nested_props",
        "additional",
        "one_of",
        "any_of",
        "all_of",
        "not_ref",
        "contains_ref",
    ] {
        let schema = get_tool_param(&config, "test_createThing", prop);
        assert!(
            find_refs(schema).is_empty(),
            "property {} still contains $ref: {schema}",
            prop
        );
    }

    // Spot-check semantics: items_ref must inline the enum.
    let items_ref = get_tool_param(&config, "test_createThing", "items_ref");
    let enum_vals = items_ref
        .get("items")
        .and_then(|i| i.get("enum"))
        .and_then(|e| e.as_array())
        .expect("inlined enum must survive");
    assert_eq!(enum_vals.len(), 3);

    // oneOf nullable-enum shape must be preserved.
    let one_of = get_tool_param(&config, "test_createThing", "one_of");
    let variants = one_of
        .get("oneOf")
        .and_then(|o| o.as_array())
        .expect("oneOf must survive");
    assert_eq!(variants.len(), 2);
    assert!(variants[0].get("type").and_then(|t| t.as_str()) == Some("null"));
    assert!(variants[1].get("enum").is_some());
}

#[tokio::test]
async fn self_referencing_schema_does_not_infinite_loop() {
    // Must terminate (depth guard) instead of blowing the stack.
    let result =
        generate_config_from_openapi(&spec_with_recursive_ref(), "test", "http://localhost:9999")
            .await;

    match result {
        Ok(config) => {
            // If generation succeeded despite recursion, no $ref may remain.
            for (name, tool) in &config.enhanced_tools {
                for param in &tool.parameters {
                    assert!(
                        find_refs(&param.schema).is_empty(),
                        "{}.{} still contains $ref",
                        name,
                        param.name
                    );
                }
            }
        }
        Err(e) => {
            // Failing loudly is acceptable — infinite recursion is not.
            let msg = format!("{}", e);
            assert!(
                msg.contains("$ref") || msg.contains("depth") || msg.contains("recursive"),
                "unexpected error: {}",
                msg
            );
        }
    }
}

#[tokio::test]
async fn query_parameter_ref_is_inlined() {
    let config =
        generate_config_from_openapi(&spec_with_param_ref(), "test", "http://localhost:9999")
            .await
            .expect("generation must succeed");

    let filter = get_tool_param(&config, "test_getThing", "filter");
    let items = filter.get("items").expect("items must survive");
    assert_eq!(
        items.get("type").and_then(|t| t.as_str()),
        Some("string"),
        "filter items should inline FilterValue: {items}"
    );
    let enum_vals = items
        .get("enum")
        .and_then(|e| e.as_array())
        .expect("enum must be inlined");
    assert_eq!(enum_vals.len(), 2);
    assert!(
        find_refs(filter).is_empty(),
        "no $ref may remain: {:?}",
        find_refs(filter)
    );
}

#[tokio::test]
async fn unresolvable_external_ref_fails_generation_loudly() {
    // External (non-local) refs cannot be resolved; the assertion must make
    // generation fail rather than silently shipping a broken tool schema.
    let spec = json!({
        "openapi": "3.0.3",
        "info": {"title": "Test API", "version": "1.0.0"},
        "paths": {
            "/api/things": {
                "post": {
                    "operationId": "createThing",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "properties": {
                                        "ext": {"$ref": "https://example.com/other.json#/Foo"}
                                    }
                                }
                            }
                        }
                    },
                    "responses": {"200": {"description": "OK"}}
                }
            }
        }
    })
    .to_string();

    let result = generate_config_from_openapi(&spec, "test", "http://localhost:9999").await;

    assert!(
        result.is_err(),
        "generation with unresolvable external $ref must fail, got: {:?}",
        result
            .as_ref()
            .map(|c| c.enhanced_tools.keys().collect::<Vec<_>>())
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("unresolvable $ref"),
        "error should mention unresolvable $ref: {}",
        msg
    );
}

#[tokio::test]
async fn real_pdt_spec_generates_clean_config() {
    // End-to-end check against the actual pdt OpenAPI spec that originally
    // leaked AddTagRequest.  Skipped silently when the file is absent (e.g.
    // when tests run from a published crate).
    let Ok(spec) = tokio::fs::read_to_string("configs/pdt/pdt-openapi.yaml").await else {
        eprintln!("skipping: configs/pdt/pdt-openapi.yaml not found");
        return;
    };

    let config = generate_config_from_openapi(&spec, "pdt", "http://localhost:8080")
        .await
        .expect("pdt spec must generate cleanly");

    for (name, tool) in &config.enhanced_tools {
        for param in &tool.parameters {
            let refs = find_refs(&param.schema);
            assert!(
                refs.is_empty(),
                "{}.{} still contains $ref: {:?}",
                name,
                param.name,
                refs
            );
        }
    }

    // The historical tripwire must now be an inlined object schema.
    let tags = get_tool_param(&config, "pdt_createAsset", "tags");
    assert_eq!(
        tags.get("items")
            .and_then(|i| i.get("type"))
            .and_then(|t| t.as_str()),
        Some("object"),
        "pdt_createAsset.tags.items must be the inlined AddTagRequest"
    );
}
