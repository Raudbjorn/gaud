use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use crate::auth::users;
use crate::auth::AuthUser;
use crate::error::AppError;
use crate::AppState;

/// Axum middleware that extracts a Bearer token from the Authorization header,
/// validates it against the database, and injects an `AuthUser` into request
/// extensions.
///
/// Supports three modes:
/// 1. Auth disabled: injects a synthetic anonymous admin user.
/// 2. TLS client cert auth: checks a header set by a TLS-terminating proxy.
/// 3. Bearer token auth: validates an API key from the Authorization header.
pub async fn require_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    // 1. Auth disabled -> anonymous admin
    if !state.config.auth.enabled {
        let anon = AuthUser {
            user_id: "anonymous".to_string(),
            name: "anonymous".to_string(),
            role: "admin".to_string(),
        };
        request.extensions_mut().insert(anon);
        return Ok(next.run(request).await);
    }

    // 2. TLS client cert auth
    if state.config.auth.tls_client_cert.enabled {
        let header_name = state.config.auth.tls_client_cert.effective_header();
        let cert_cn = request
            .headers()
            .get(header_name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        if let Some(cn) = cert_cn {
            // Look up user by name
            match users::get_user_by_name(&state.db, &cn) {
                Ok(user) => {
                    let auth_user = AuthUser {
                        user_id: user.id.clone(),
                        name: user.name.clone(),
                        role: user.role.clone(),
                    };
                    tracing::debug!(
                        user_id = %auth_user.user_id,
                        name = %auth_user.name,
                        role = %auth_user.role,
                        "Authenticated via TLS client cert"
                    );
                    request.extensions_mut().insert(auth_user);
                    return Ok(next.run(request).await);
                }
                Err(_) => {
                    if state.config.auth.tls_client_cert.require_cert {
                        return Err(AppError::Unauthorized(format!(
                            "Unknown client certificate CN: {cn}"
                        )));
                    }
                    // Fall through to bearer token auth
                }
            }
        } else if state.config.auth.tls_client_cert.require_cert {
            return Err(AppError::Unauthorized(format!(
                "Client certificate required (header: {header_name})"
            )));
        }
    }

    // 3. Bearer token auth (existing logic)
    let token = extract_bearer_token(&request)?;
    let auth_user = users::validate_api_key(&state.db, &token)?;

    tracing::debug!(
        user_id = %auth_user.user_id,
        name = %auth_user.name,
        role = %auth_user.role,
        "Authenticated request"
    );

    request.extensions_mut().insert(auth_user);
    Ok(next.run(request).await)
}

/// Axum middleware that requires the authenticated user to have the admin role.
///
/// Must be applied _after_ `require_auth` so that `AuthUser` is present in
/// request extensions.
pub async fn require_admin(
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let auth_user = request
        .extensions()
        .get::<AuthUser>()
        .ok_or_else(|| {
            AppError::Internal("AuthUser missing from extensions -- is require_auth applied?".to_string())
        })?;

    if !auth_user.is_admin() {
        return Err(AppError::Forbidden(format!(
            "Admin role required, but user '{}' has role '{}'",
            auth_user.name, auth_user.role
        )));
    }

    Ok(next.run(request).await)
}

/// Extract the Bearer token from the Authorization header.
fn extract_bearer_token(request: &Request) -> Result<String, AppError> {
    let header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .ok_or_else(|| AppError::Unauthorized("Missing Authorization header".to_string()))?;

    let value = header
        .to_str()
        .map_err(|_| AppError::Unauthorized("Invalid Authorization header encoding".to_string()))?;

    let token = value
        .strip_prefix("Bearer ")
        .ok_or_else(|| {
            AppError::Unauthorized("Authorization header must use Bearer scheme".to_string())
        })?
        .trim();

    if token.is_empty() {
        return Err(AppError::Unauthorized("Empty Bearer token".to_string()));
    }

    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request as HttpRequest, StatusCode};
    use axum::middleware;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    use crate::auth::users::{create_api_key, create_user};
    use crate::db::Database;

    /// Build a minimal AppState with an in-memory database for testing.
    fn test_state() -> AppState {
        let db = Database::open_in_memory().unwrap();
        let config = crate::config::Config::default();
        let config_arc = std::sync::Arc::new(config);
        let (audit_tx, _audit_rx) = tokio::sync::mpsc::unbounded_channel();
        let budget = crate::budget::BudgetTracker::new(db.clone());
        let router = crate::providers::router::ProviderRouter::new();
        let oauth_manager = crate::oauth::OAuthManager::from_config(config_arc.clone(), db.clone());

        AppState {
            config: config_arc,
            config_path: std::path::PathBuf::from("test.toml"),
            db,
            router: std::sync::Arc::new(tokio::sync::RwLock::new(router)),
            budget: std::sync::Arc::new(budget),
            audit_tx,
            oauth_manager: std::sync::Arc::new(oauth_manager),
        }
    }

    /// Dummy handler that returns the authenticated user's name.
    async fn whoami(request: Request) -> String {
        let user = request.extensions().get::<AuthUser>().unwrap();
        user.name.clone()
    }

    /// Dummy admin-only handler.
    async fn admin_only(request: Request) -> String {
        let user = request.extensions().get::<AuthUser>().unwrap();
        format!("admin: {}", user.name)
    }

    fn auth_router(state: AppState) -> Router {
        Router::new()
            .route("/whoami", get(whoami))
            .layer(middleware::from_fn_with_state(state.clone(), require_auth))
            .with_state(state)
    }

    fn admin_router(state: AppState) -> Router {
        Router::new()
            .route("/admin", get(admin_only))
            .layer(middleware::from_fn(require_admin))
            .layer(middleware::from_fn_with_state(state.clone(), require_auth))
            .with_state(state)
    }

    // -----------------------------------------------------------------------
    // Unit tests for extract_bearer_token (no AppState needed)
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_bearer_token_valid() {
        let req = HttpRequest::builder()
            .header(header::AUTHORIZATION, "Bearer sk-prx-abc123")
            .body(Body::empty())
            .unwrap();
        let token = extract_bearer_token(&req).unwrap();
        assert_eq!(token, "sk-prx-abc123");
    }

    #[test]
    fn test_extract_bearer_token_missing_header() {
        let req = HttpRequest::builder().body(Body::empty()).unwrap();
        let err = extract_bearer_token(&req).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    #[test]
    fn test_extract_bearer_token_wrong_scheme() {
        let req = HttpRequest::builder()
            .header(header::AUTHORIZATION, "Basic dXNlcjpwYXNz")
            .body(Body::empty())
            .unwrap();
        let err = extract_bearer_token(&req).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    #[test]
    fn test_extract_bearer_token_empty() {
        let req = HttpRequest::builder()
            .header(header::AUTHORIZATION, "Bearer ")
            .body(Body::empty())
            .unwrap();
        let err = extract_bearer_token(&req).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    #[test]
    fn test_extract_bearer_token_trims_whitespace() {
        let req = HttpRequest::builder()
            .header(header::AUTHORIZATION, "Bearer   sk-prx-abc123   ")
            .body(Body::empty())
            .unwrap();
        let token = extract_bearer_token(&req).unwrap();
        assert_eq!(token, "sk-prx-abc123");
    }

    // -----------------------------------------------------------------------
    // Integration tests for full middleware stack (require AppState)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_missing_auth_header() {
        let state = test_state();
        let app = auth_router(state);

        let req = HttpRequest::builder()
            .uri("/whoami")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_invalid_bearer_scheme() {
        let state = test_state();
        let app = auth_router(state);

        let req = HttpRequest::builder()
            .uri("/whoami")
            .header(header::AUTHORIZATION, "Basic dXNlcjpwYXNz")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_invalid_api_key() {
        let state = test_state();
        let app = auth_router(state);

        let req = HttpRequest::builder()
            .uri("/whoami")
            .header(header::AUTHORIZATION, "Bearer sk-prx-invalid00000000000000000000")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_valid_auth() {
        let state = test_state();
        let user = create_user(&state.db, "alice", "member").unwrap();
        let key = create_api_key(&state.db, &user.id, "test").unwrap();

        let app = auth_router(state);

        let req = HttpRequest::builder()
            .uri("/whoami")
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", key.plaintext),
            )
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(String::from_utf8(body.to_vec()).unwrap(), "alice");
    }

    #[tokio::test]
    async fn test_admin_middleware_allows_admin() {
        let state = test_state();
        let user = create_user(&state.db, "admin", "admin").unwrap();
        let key = create_api_key(&state.db, &user.id, "admin key").unwrap();

        let app = admin_router(state);

        let req = HttpRequest::builder()
            .uri("/admin")
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", key.plaintext),
            )
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_admin_middleware_rejects_member() {
        let state = test_state();
        let user = create_user(&state.db, "bob", "member").unwrap();
        let key = create_api_key(&state.db, &user.id, "member key").unwrap();

        let app = admin_router(state);

        let req = HttpRequest::builder()
            .uri("/admin")
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", key.plaintext),
            )
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_empty_bearer_token() {
        let state = test_state();
        let app = auth_router(state);

        let req = HttpRequest::builder()
            .uri("/whoami")
            .header(header::AUTHORIZATION, "Bearer ")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // -----------------------------------------------------------------------
    // Auth disabled tests
    // -----------------------------------------------------------------------

    /// Build a test state with auth disabled.
    fn test_state_auth_disabled() -> AppState {
        let mut state = test_state();
        let mut config = (*state.config).clone();
        config.auth.enabled = false;
        state.config = std::sync::Arc::new(config);
        state
    }

    /// Build a test state with TLS client cert auth enabled.
    fn test_state_tls_cert(require_cert: bool) -> AppState {
        let mut state = test_state();
        let mut config = (*state.config).clone();
        config.auth.tls_client_cert.enabled = true;
        config.auth.tls_client_cert.require_cert = require_cert;
        state.config = std::sync::Arc::new(config);
        state
    }

    #[tokio::test]
    async fn test_auth_disabled_allows_anonymous() {
        let state = test_state_auth_disabled();
        let app = auth_router(state);

        // Request with no auth header at all should succeed.
        let req = HttpRequest::builder()
            .uri("/whoami")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(String::from_utf8(body.to_vec()).unwrap(), "anonymous");
    }

    #[tokio::test]
    async fn test_auth_disabled_admin_access() {
        let state = test_state_auth_disabled();
        let app = admin_router(state);

        // Anonymous user should have admin role when auth is disabled.
        let req = HttpRequest::builder()
            .uri("/admin")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // -----------------------------------------------------------------------
    // TLS client cert auth tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_tls_cert_auth_known_user() {
        let state = test_state_tls_cert(false);
        let _user = create_user(&state.db, "alice", "member").unwrap();
        let app = auth_router(state);

        let req = HttpRequest::builder()
            .uri("/whoami")
            .header("X-Client-Cert-CN", "alice")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(String::from_utf8(body.to_vec()).unwrap(), "alice");
    }

    #[tokio::test]
    async fn test_tls_cert_auth_require_cert_missing_header() {
        let state = test_state_tls_cert(true);
        let app = auth_router(state);

        // No cert header and require_cert=true -> 401.
        let req = HttpRequest::builder()
            .uri("/whoami")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_tls_cert_auth_require_cert_unknown_cn() {
        let state = test_state_tls_cert(true);
        let app = auth_router(state);

        // Unknown CN with require_cert=true -> 401.
        let req = HttpRequest::builder()
            .uri("/whoami")
            .header("X-Client-Cert-CN", "unknown-user")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_tls_cert_auth_unknown_cn_falls_through_to_bearer() {
        let state = test_state_tls_cert(false);
        let user = create_user(&state.db, "bob", "member").unwrap();
        let key = create_api_key(&state.db, &user.id, "test").unwrap();
        let app = auth_router(state);

        // Unknown CN with require_cert=false -> falls through to bearer token.
        let req = HttpRequest::builder()
            .uri("/whoami")
            .header("X-Client-Cert-CN", "unknown-cert-user")
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", key.plaintext),
            )
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(String::from_utf8(body.to_vec()).unwrap(), "bob");
    }

    #[tokio::test]
    async fn test_tls_cert_auth_empty_header_falls_through() {
        let state = test_state_tls_cert(false);
        let user = create_user(&state.db, "charlie", "member").unwrap();
        let key = create_api_key(&state.db, &user.id, "test").unwrap();
        let app = auth_router(state);

        // Empty cert header with require_cert=false -> falls through to bearer.
        let req = HttpRequest::builder()
            .uri("/whoami")
            .header("X-Client-Cert-CN", "")
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", key.plaintext),
            )
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(String::from_utf8(body.to_vec()).unwrap(), "charlie");
    }
}
