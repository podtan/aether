use aether_auth::oidc::UserClaims;
use aether_config::{AetherConfig, McpAuthConfig, OidcConfig};
use aether_mcp::middleware::{mcp_auth_middleware, AuthMiddlewareState};
use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::Response,
    routing::get,
    Router,
};
use serde_json::json;
use tower::ServiceExt;
use wiremock::{
    matchers::{header as wiremock_header, method, path},
    Mock, MockServer, ResponseTemplate,
};

// Simple handler that returns user info if authenticated
async fn protected_handler(req: Request<Body>) -> Response {
    if let Some(claims) = req.extensions().get::<UserClaims>() {
        Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(format!("Hello, {}!", claims.sub)))
            .unwrap()
    } else {
        Response::builder()
            .status(StatusCode::OK)
            .body(Body::from("Hello, anonymous!"))
            .unwrap()
    }
}

async fn setup_mock_oidc_server() -> MockServer {
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
        .mount(&mock_server)
        .await;

    mock_server
}

#[tokio::test]
async fn test_auth_disabled_allows_all_requests() {
    let config = McpAuthConfig {
        enabled: false,
        require_auth: false,
        public_health: true,
        issuer_url: None,
        audience: None,
    };

    let auth_state = AuthMiddlewareState::new(config, None)
        .await
        .expect("Should create auth state with disabled config");

    let app = Router::new()
        .route("/test", get(protected_handler))
        .layer(from_fn_with_state(auth_state, mcp_auth_middleware));

    // Request without token should succeed
    let request = Request::builder().uri("/test").body(Body::empty()).unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_auth_enabled_valid_token() {
    let mock_server = setup_mock_oidc_server().await;

    // Mock successful userinfo response
    Mock::given(method("GET"))
        .and(path("/userinfo"))
        .and(wiremock_header("authorization", "Bearer valid_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sub": "user123",
            "email": "user@example.com",
            "name": "Test User",
        })))
        .mount(&mock_server)
        .await;

    let config = McpAuthConfig {
        enabled: true,
        require_auth: true,
        public_health: true,
        issuer_url: None,
        audience: None,
    };

    let oidc_config = OidcConfig {
        provider: "test".to_string(),
        issuer_url: mock_server.uri(),
        client_id: "test_client".to_string(),
        client_secret: "test_secret".to_string(),
        redirect_url: "http://localhost/callback".to_string(),
        code_challenge_method: "S256".to_string(),
        scope: "openid email profile".to_string(),
    };

    let auth_state = AuthMiddlewareState::new(config, Some(&oidc_config))
        .await
        .expect("Should create auth state");

    let app = Router::new()
        .route("/test", get(protected_handler))
        .layer(from_fn_with_state(auth_state, mcp_auth_middleware));

    // Request with valid token should succeed
    let request = Request::builder()
        .uri("/test")
        .header(header::AUTHORIZATION, "Bearer valid_token")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_auth_enabled_invalid_token() {
    let mock_server = setup_mock_oidc_server().await;

    // Mock 401 response for invalid token
    Mock::given(method("GET"))
        .and(path("/userinfo"))
        .and(wiremock_header("authorization", "Bearer invalid_token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "invalid_token",
        })))
        .mount(&mock_server)
        .await;

    let config = McpAuthConfig {
        enabled: true,
        require_auth: true,
        public_health: true,
        issuer_url: None,
        audience: None,
    };

    let oidc_config = OidcConfig {
        provider: "test".to_string(),
        issuer_url: mock_server.uri(),
        client_id: "test_client".to_string(),
        client_secret: "test_secret".to_string(),
        redirect_url: "http://localhost/callback".to_string(),
        code_challenge_method: "S256".to_string(),
        scope: "openid email profile".to_string(),
    };

    let auth_state = AuthMiddlewareState::new(config, Some(&oidc_config))
        .await
        .expect("Should create auth state");

    let app = Router::new()
        .route("/test", get(protected_handler))
        .layer(from_fn_with_state(auth_state, mcp_auth_middleware));

    // Request with invalid token should fail
    let request = Request::builder()
        .uri("/test")
        .header(header::AUTHORIZATION, "Bearer invalid_token")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_required_missing_token() {
    let mock_server = setup_mock_oidc_server().await;

    let config = McpAuthConfig {
        enabled: true,
        require_auth: true,
        public_health: true,
        issuer_url: None,
        audience: None,
    };

    let oidc_config = OidcConfig {
        provider: "test".to_string(),
        issuer_url: mock_server.uri(),
        client_id: "test_client".to_string(),
        client_secret: "test_secret".to_string(),
        redirect_url: "http://localhost/callback".to_string(),
        code_challenge_method: "S256".to_string(),
        scope: "openid email profile".to_string(),
    };

    let auth_state = AuthMiddlewareState::new(config, Some(&oidc_config))
        .await
        .expect("Should create auth state");

    let app = Router::new()
        .route("/test", get(protected_handler))
        .layer(from_fn_with_state(auth_state, mcp_auth_middleware));

    // Request without token should fail when auth is required
    let request = Request::builder().uri("/test").body(Body::empty()).unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_optional_missing_token() {
    let mock_server = setup_mock_oidc_server().await;

    let config = McpAuthConfig {
        enabled: true,
        require_auth: false, // Auth is optional
        public_health: true,
        issuer_url: None,
        audience: None,
    };

    let oidc_config = OidcConfig {
        provider: "test".to_string(),
        issuer_url: mock_server.uri(),
        client_id: "test_client".to_string(),
        client_secret: "test_secret".to_string(),
        redirect_url: "http://localhost/callback".to_string(),
        code_challenge_method: "S256".to_string(),
        scope: "openid email profile".to_string(),
    };

    let auth_state = AuthMiddlewareState::new(config, Some(&oidc_config))
        .await
        .expect("Should create auth state");

    let app = Router::new()
        .route("/test", get(protected_handler))
        .layer(from_fn_with_state(auth_state, mcp_auth_middleware));

    // Request without token should succeed when auth is optional
    let request = Request::builder().uri("/test").body(Body::empty()).unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_invalid_bearer_format() {
    let mock_server = setup_mock_oidc_server().await;

    let config = McpAuthConfig {
        enabled: true,
        require_auth: true,
        public_health: true,
        issuer_url: None,
        audience: None,
    };

    let oidc_config = OidcConfig {
        provider: "test".to_string(),
        issuer_url: mock_server.uri(),
        client_id: "test_client".to_string(),
        client_secret: "test_secret".to_string(),
        redirect_url: "http://localhost/callback".to_string(),
        code_challenge_method: "S256".to_string(),
        scope: "openid email profile".to_string(),
    };

    let auth_state = AuthMiddlewareState::new(config, Some(&oidc_config))
        .await
        .expect("Should create auth state");

    let app = Router::new()
        .route("/test", get(protected_handler))
        .layer(from_fn_with_state(auth_state, mcp_auth_middleware));

    // Request with malformed Authorization header (missing "Bearer ")
    let request = Request::builder()
        .uri("/test")
        .header(header::AUTHORIZATION, "just_a_token")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_user_claims_attached_to_request() {
    let mock_server = setup_mock_oidc_server().await;

    // Mock successful userinfo response
    Mock::given(method("GET"))
        .and(path("/userinfo"))
        .and(wiremock_header("authorization", "Bearer valid_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sub": "user456",
            "email": "testuser@example.com",
            "name": "Integration Test User",
            "preferred_username": "testuser",
        })))
        .mount(&mock_server)
        .await;

    let config = McpAuthConfig {
        enabled: true,
        require_auth: true,
        public_health: true,
        issuer_url: None,
        audience: None,
    };

    let oidc_config = OidcConfig {
        provider: "test".to_string(),
        issuer_url: mock_server.uri(),
        client_id: "test_client".to_string(),
        client_secret: "test_secret".to_string(),
        redirect_url: "http://localhost/callback".to_string(),
        code_challenge_method: "S256".to_string(),
        scope: "openid email profile".to_string(),
    };

    let auth_state = AuthMiddlewareState::new(config, Some(&oidc_config))
        .await
        .expect("Should create auth state");

    let app = Router::new()
        .route("/test", get(protected_handler))
        .layer(from_fn_with_state(auth_state, mcp_auth_middleware));

    // Request with valid token
    let request = Request::builder()
        .uri("/test")
        .header(header::AUTHORIZATION, "Bearer valid_token")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Read body to verify user info was passed through
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert!(
        body_str.contains("user456"),
        "Response should contain user sub"
    );
}
