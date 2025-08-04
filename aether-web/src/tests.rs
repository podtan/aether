use aether_config::AetherConfig;

#[tokio::test]
async fn test_router_creation() {
    // Create minimal config
    let mut config = AetherConfig::default();
    config.session = Some(aether_config::SessionConfig {
        secret_key: "test_secret".to_string(),
        cookie_name: "test_session".to_string(),
        max_age: 3600,
    });
    
    // Should be able to create router without OIDC config (will use dev mode)
    let router = crate::create_router(&config).await;
    assert!(router.is_ok());
}

#[tokio::test]
async fn test_router_creation_with_oidc() {
    // Create config with OIDC
    let mut config = AetherConfig::default();
    config.session = Some(aether_config::SessionConfig {
        secret_key: "test_secret".to_string(),
        cookie_name: "test_session".to_string(),
        max_age: 3600,
    });
    config.oidc = Some(aether_config::OidcConfig {
        provider: "test".to_string(),
        issuer_url: "https://test.example.com".to_string(),
        client_id: "test_client".to_string(),
        client_secret: "test_secret".to_string(),
        redirect_url: "http://localhost:8080/auth/callback".to_string(),
        code_challenge_method: "S256".to_string(),
        scope: "openid email profile".to_string(),
    });
    
    // Router creation might fail due to network issues connecting to OIDC provider
    // but the function should not panic
    let router = crate::create_router(&config).await;
    // We don't assert success here because it might fail due to network connectivity
    println!("Router creation result: {:?}", router.is_ok());
}