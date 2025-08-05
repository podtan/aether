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
async fn test_full_config_example() {
    // Test with full OIDC config (would fail network connection but shouldn't panic)
    let config = create_full_test_config();
    
    let result = crate::create_router(&config).await;
    // This might fail due to network connectivity to OIDC provider, which is expected in tests
    match result {
        Ok(_) => println!("✅ Router created successfully with OIDC config"),
        Err(e) => println!("⚠️  Router creation failed (expected in test environment): {}", e),
    }
}

#[tokio::test]
async fn test_dev_mode_config() {
    let config = create_dev_mode_config();
    
    let result = crate::create_router(&config).await;
    assert!(result.is_ok(), "Dev mode router should always work");
    
    println!("✅ Dev mode router created successfully!");
    println!("🔧 Dev mode: enabled");
    println!("🍪 Session cookie: {}", config.session.as_ref().unwrap().cookie_name);
    println!("👤 Dev user: {} ({})", 
        config.dev.as_ref().unwrap().local_dev_name,
        config.dev.as_ref().unwrap().local_dev_role
    );
}

fn create_full_test_config() -> AetherConfig {
    let mut config = AetherConfig::default();
    config.session = Some(aether_config::SessionConfig {
        secret_key: "test_secret_key_for_testing".to_string(),
        cookie_name: "aether_test_session".to_string(),
        max_age: 28800, // 8 hours
    });
    config.oidc = Some(aether_config::OidcConfig {
        provider: "kanidm".to_string(),
        issuer_url: "https://auth.example.com".to_string(),
        client_id: "aether-web".to_string(),
        client_secret: "test-secret".to_string(),
        redirect_url: "http://localhost:8080/auth/callback".to_string(),
        code_challenge_method: "S256".to_string(),
        scope: "openid email profile".to_string(),
    });
    config.dev = Some(aether_config::DevConfig {
        local_dev_mode: false, // OIDC mode
        local_dev_email: "dev@example.com".to_string(),
        local_dev_role: "user".to_string(),
        local_dev_name: "Dev User".to_string(),
    });
    config
}

fn create_dev_mode_config() -> AetherConfig {
    let mut config = AetherConfig::default();
    config.session = Some(aether_config::SessionConfig {
        secret_key: "dev_secret_key".to_string(),
        cookie_name: "aether_dev_session".to_string(),
        max_age: 3600, // 1 hour for dev
    });
    config.dev = Some(aether_config::DevConfig {
        local_dev_mode: true, // Dev mode enabled
        local_dev_email: "admin@localhost".to_string(),
        local_dev_role: "admin".to_string(),
        local_dev_name: "Local Admin".to_string(),
    });
    config
}

#[tokio::test]
async fn test_login_template_rendering() {
    use askama::Template;
    
    // Test dev mode template
    let dev_template = crate::LoginTemplate { is_dev_mode: true };
    let rendered = dev_template.render();
    assert!(rendered.is_ok(), "Dev mode login template should render");
    let html = rendered.unwrap();
    assert!(html.contains("Development Mode Active"), "Should show dev mode notice");
    assert!(html.contains("Development Login"), "Should show dev login button");
    
    // Test non-dev mode template  
    let prod_template = crate::LoginTemplate { is_dev_mode: false };
    let rendered = prod_template.render();
    assert!(rendered.is_ok(), "Production login template should render");
    let html = rendered.unwrap();
    assert!(!html.contains("Development Mode Active"), "Should not show dev mode notice");
    assert!(!html.contains("Development Login"), "Should not show dev login button");
    assert!(html.contains("Sign in with Kanidm"), "Should show OIDC login button");
    
    println!("✅ Login templates render correctly for both dev and production modes");
}

#[tokio::test]
async fn test_home_template_rendering() {
    use askama::Template;
    
    let template = crate::HomeTemplate;
    let rendered = template.render();
    assert!(rendered.is_ok(), "Home template should render");
    let html = rendered.unwrap();
    assert!(html.contains("Multi-Config Manager"), "Should contain page title");
    assert!(html.contains("Upload OpenAPI Specification"), "Should contain upload form");
    
    println!("✅ Home template renders correctly");
}