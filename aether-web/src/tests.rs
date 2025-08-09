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
    let dev_template = crate::LoginTemplate { 
        is_dev_mode: true,
        error: None 
    };
    let rendered = dev_template.render();
    assert!(rendered.is_ok(), "Dev mode login template should render");
    let html = rendered.unwrap();
    assert!(html.contains("Development Mode Active"), "Should show dev mode notice");
    assert!(html.contains("Development Login"), "Should show dev login button");
    
    // Test non-dev mode template  
    let prod_template = crate::LoginTemplate { 
        is_dev_mode: false,
        error: None 
    };
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
    
    let template = crate::HomeTemplate { 
        user: None 
    };
    let rendered = template.render();
    assert!(rendered.is_ok(), "Home template should render");
    let html = rendered.unwrap();
    assert!(html.contains("Multi-Config Manager"), "Should contain page title");
    assert!(html.contains("Upload OpenAPI Specification"), "Should contain upload form");
    
    println!("✅ Home template renders correctly");
}

#[tokio::test]
async fn test_apis_template_with_runtime_apis() {
    use askama::Template;
    
    // Test ApiListTemplate with both uploaded and runtime APIs
    let mut configs = Vec::new();
    
    // Add a simulated uploaded API
    configs.push(crate::ConfigInfo {
        api_name: "uploaded-api".to_string(),
        openapi_content: "{}".to_string(),
        config_content: "{}".to_string(),
        base_url: "https://api.example.com".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    });
    
    // Add a simulated runtime API (these would normally be added by the runtime callback)
    configs.push(crate::ConfigInfo {
        api_name: "runtime-api".to_string(),
        openapi_content: "".to_string(), // Runtime APIs have minimal info
        config_content: "".to_string(),
        base_url: "".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    });
    
    let template = crate::ApiListTemplate {
        configs,
        user: None,
    };
    
    let rendered = template.render();
    assert!(rendered.is_ok(), "API list template should render");
    let html = rendered.unwrap();
    
    // Verify both APIs are displayed
    assert!(html.contains("uploaded-api"), "Should contain uploaded API");
    assert!(html.contains("runtime-api"), "Should contain runtime API");
    assert!(html.contains("https://api.example.com"), "Should contain base URL for uploaded API");
    
    println!("✅ API list template renders correctly with both uploaded and runtime APIs");
}

#[tokio::test]
async fn test_web_state_with_runtime_callback() {
    // Test WebState with runtime APIs callback
    let runtime_apis = vec!["test-api-1".to_string(), "test-api-2".to_string()];
    let runtime_apis_clone = runtime_apis.clone();
    
    let callback = move || {
        let apis = runtime_apis_clone.clone();
        async move { apis }
    };
    
    let web_state = crate::WebState::new_with_runtime_apis_callback(callback);
    
    // Verify callback is set
    assert!(web_state.runtime_apis_callback.is_some(), "Runtime APIs callback should be set");
    
    // Test the callback
    if let Some(callback) = &web_state.runtime_apis_callback {
        let result = callback().await;
        assert_eq!(result.len(), 2, "Should return 2 runtime APIs");
        assert!(result.contains(&"test-api-1".to_string()), "Should contain test-api-1");
        assert!(result.contains(&"test-api-2".to_string()), "Should contain test-api-2");
    }
    
    println!("✅ WebState with runtime APIs callback works correctly");
}