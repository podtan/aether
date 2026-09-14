//! Regression tests for `OAuthFacade::from_oidc_config` (issue 2bf2be1a).
//!
//! The production panic: a 200 JSON body that is NOT a valid OIDC discovery
//! document made `discovery["authorization_endpoint"].as_str().unwrap()`
//! panic on the startup thread and took the whole aether process down.
//! Every failure mode here must be an `Err` — never a panic.

use aether_config::OidcConfig;
use aether_mcp::transport::OAuthFacade;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn test_oidc_config(issuer_url: &str) -> OidcConfig {
    OidcConfig {
        provider: "test".to_string(),
        issuer_url: issuer_url.to_string(),
        client_id: "test-client".to_string(),
        client_secret: "test-secret".to_string(),
        redirect_url: "http://localhost:9001/callback".to_string(),
        code_challenge_method: "S256".to_string(),
        scope: "openid".to_string(),
    }
}

/// (a) A valid discovery document parses and both endpoints land on the facade.
#[tokio::test]
async fn facade_parses_valid_discovery_document() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issuer": "https://idp.example.com",
            "authorization_endpoint": "https://idp.example.com/oauth2/authorize",
            "token_endpoint": "https://idp.example.com/oauth2/token"
        })))
        .mount(&server)
        .await;

    let facade = OAuthFacade::from_oidc_config(&test_oidc_config(&server.uri()))
        .await
        .expect("a valid discovery document must parse");
    assert_eq!(
        facade.authorization_endpoint,
        "https://idp.example.com/oauth2/authorize"
    );
    assert_eq!(
        facade.token_endpoint,
        "https://idp.example.com/oauth2/token"
    );
}

/// (b) THE production panic case: HTTP 200 with JSON that is not an OIDC
/// document (wrong well-known path, non-OIDC service, proxy error body).
/// Must be an `Err` naming the missing field — never a panic.
#[tokio::test]
async fn facade_errors_on_json_without_oidc_fields() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "error": "not found",
            "message": "no such route"
        })))
        .mount(&server)
        .await;

    let err = OAuthFacade::from_oidc_config(&test_oidc_config(&server.uri()))
        .await
        .expect_err("JSON without OIDC fields must be an Err, never a panic");
    let msg = err.to_string();
    assert!(
        msg.contains("authorization_endpoint"),
        "error must name the missing field, got: {msg}"
    );
    assert!(
        msg.contains("issuer_url"),
        "error must point at the configured issuer, got: {msg}"
    );
}

/// (c) Unreachable issuer: connection refused fails fast as an `Err`
/// (the request timeout bounds the slow-blackhole case the same way).
#[tokio::test]
async fn facade_errors_on_unreachable_issuer() {
    // Bind then drop a listener to get a port that is guaranteed closed.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let err = OAuthFacade::from_oidc_config(&test_oidc_config(&format!("http://127.0.0.1:{port}")))
        .await
        .expect_err("unreachable issuer must be an Err, not a panic");
    assert!(
        err.to_string().contains("discovery"),
        "error should point at the discovery failure, got: {err}"
    );
}
