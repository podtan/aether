use aether_auth::oidc::OidcValidator;
use wiremock::{
    matchers::{method, path, header},
    Mock, MockServer, ResponseTemplate,
};
use serde_json::json;

#[tokio::test]
async fn test_oidc_discovery() {
    let mock_server = MockServer::start().await;
    
    // Mock OIDC discovery endpoint
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": mock_server.uri(),
            "userinfo_endpoint": format!("{}/userinfo", mock_server.uri()),
            "jwks_uri": format!("{}/jwks", mock_server.uri()),
            "authorization_endpoint": format!("{}/authorize", mock_server.uri()),
            "token_endpoint": format!("{}/token", mock_server.uri()),
        })))
        .expect(1)
        .mount(&mock_server)
        .await;
    
    // Create validator - this should trigger discovery
    let result = OidcValidator::new(
        &mock_server.uri(),
        "test_client_id",
        "test_client_secret",
    ).await;
    
    assert!(result.is_ok(), "OIDC discovery should succeed");
    
    let validator = result.unwrap();
    assert!(validator.health_check().await.is_ok(), "Health check should pass after discovery");
}

#[tokio::test]
async fn test_validate_valid_token() {
    let mock_server = MockServer::start().await;
    
    // Mock OIDC discovery endpoint
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": mock_server.uri(),
            "userinfo_endpoint": format!("{}/userinfo", mock_server.uri()),
            "jwks_uri": format!("{}/jwks", mock_server.uri()),
            "authorization_endpoint": format!("{}/authorize", mock_server.uri()),
            "token_endpoint": format!("{}/token", mock_server.uri()),
        })))
        .expect(1)
        .mount(&mock_server)
        .await;
    
    // Mock successful userinfo response
    Mock::given(method("GET"))
        .and(path("/userinfo"))
        .and(header("authorization", "Bearer valid_token_123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sub": "user123",
            "email": "user@example.com",
            "name": "Test User",
            "preferred_username": "testuser",
        })))
        .mount(&mock_server)
        .await;
    
    let validator = OidcValidator::new(
        &mock_server.uri(),
        "test_client_id",
        "test_client_secret",
    ).await.expect("Discovery should succeed");
    
    // Validate token
    let result = validator.validate_bearer_token("valid_token_123").await;
    assert!(result.is_ok(), "Valid token should be accepted");
    
    let claims = result.unwrap();
    assert_eq!(claims.sub, "user123");
    assert_eq!(claims.email, "user@example.com");
    assert_eq!(claims.name, "Test User");
    assert_eq!(claims.preferred_username, Some("testuser".to_string()));
}

#[tokio::test]
async fn test_validate_invalid_token() {
    let mock_server = MockServer::start().await;
    
    // Mock OIDC discovery endpoint
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": mock_server.uri(),
            "userinfo_endpoint": format!("{}/userinfo", mock_server.uri()),
            "jwks_uri": format!("{}/jwks", mock_server.uri()),
            "authorization_endpoint": format!("{}/authorize", mock_server.uri()),
            "token_endpoint": format!("{}/token", mock_server.uri()),
        })))
        .expect(1)
        .mount(&mock_server)
        .await;
    
    // Mock 401 unauthorized response for invalid token
    Mock::given(method("GET"))
        .and(path("/userinfo"))
        .and(header("authorization", "Bearer invalid_token_xyz"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "invalid_token",
            "error_description": "Token is invalid or expired",
        })))
        .mount(&mock_server)
        .await;
    
    let validator = OidcValidator::new(
        &mock_server.uri(),
        "test_client_id",
        "test_client_secret",
    ).await.expect("Discovery should succeed");
    
    // Validate invalid token
    let result = validator.validate_bearer_token("invalid_token_xyz").await;
    assert!(result.is_err(), "Invalid token should be rejected");
}

#[tokio::test]
async fn test_validate_token_missing_sub_claim() {
    let mock_server = MockServer::start().await;
    
    // Mock OIDC discovery endpoint
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": mock_server.uri(),
            "userinfo_endpoint": format!("{}/userinfo", mock_server.uri()),
            "jwks_uri": format!("{}/jwks", mock_server.uri()),
            "authorization_endpoint": format!("{}/authorize", mock_server.uri()),
            "token_endpoint": format!("{}/token", mock_server.uri()),
        })))
        .expect(1)
        .mount(&mock_server)
        .await;
    
    // Mock userinfo response missing required 'sub' claim
    Mock::given(method("GET"))
        .and(path("/userinfo"))
        .and(header("authorization", "Bearer token_without_sub"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "email": "user@example.com",
            "name": "Test User",
        })))
        .mount(&mock_server)
        .await;
    
    let validator = OidcValidator::new(
        &mock_server.uri(),
        "test_client_id",
        "test_client_secret",
    ).await.expect("Discovery should succeed");
    
    // Validate token with missing sub claim
    let result = validator.validate_bearer_token("token_without_sub").await;
    assert!(result.is_err(), "Token without 'sub' claim should be rejected");
}

#[tokio::test]
async fn test_discovery_failure() {
    let mock_server = MockServer::start().await;
    
    // Mock discovery endpoint returning 404
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock_server)
        .await;
    
    // Attempt to create validator with failing discovery
    let result = OidcValidator::new(
        &mock_server.uri(),
        "test_client_id",
        "test_client_secret",
    ).await;
    
    assert!(result.is_err(), "Discovery failure should return error");
}

#[tokio::test]
async fn test_validate_token_with_optional_fields() {
    let mock_server = MockServer::start().await;
    
    // Mock OIDC discovery endpoint
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": mock_server.uri(),
            "userinfo_endpoint": format!("{}/userinfo", mock_server.uri()),
            "jwks_uri": format!("{}/jwks", mock_server.uri()),
            "authorization_endpoint": format!("{}/authorize", mock_server.uri()),
            "token_endpoint": format!("{}/token", mock_server.uri()),
        })))
        .expect(1)
        .mount(&mock_server)
        .await;
    
    // Mock userinfo with minimal claims (only sub required)
    Mock::given(method("GET"))
        .and(path("/userinfo"))
        .and(header("authorization", "Bearer minimal_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sub": "user456",
        })))
        .mount(&mock_server)
        .await;
    
    let validator = OidcValidator::new(
        &mock_server.uri(),
        "test_client_id",
        "test_client_secret",
    ).await.expect("Discovery should succeed");
    
    // Validate token with minimal claims
    let result = validator.validate_bearer_token("minimal_token").await;
    assert!(result.is_ok(), "Token with minimal claims should be accepted");
    
    let claims = result.unwrap();
    assert_eq!(claims.sub, "user456");
    assert_eq!(claims.email, "unknown@example.com"); // Default value
    assert_eq!(claims.name, "Unknown User"); // Default value
    assert_eq!(claims.preferred_username, None);
}
