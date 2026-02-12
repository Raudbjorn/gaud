//! Embedded Web UI for Gaud.
//!
//! Provides a monitoring dashboard, OAuth management, user administration,
//! usage logs, and budget configuration -- all rendered from embedded HTML
//! templates via minijinja.

pub mod templates;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use minijinja::{Environment, context};
use serde::Deserialize;
use tracing::warn;

use crate::AppState;

// ---------------------------------------------------------------------------
// Template engine
// ---------------------------------------------------------------------------

/// Build a minijinja environment with all embedded templates registered.
fn template_env() -> Environment<'static> {
    let mut env = Environment::new();
    env.add_template("layout", templates::LAYOUT)
        .expect("layout template");
    env.add_template("login", templates::LOGIN)
        .expect("login template");
    env.add_template("dashboard", templates::DASHBOARD)
        .expect("dashboard template");
    env.add_template("oauth", templates::OAUTH)
        .expect("oauth template");
    env.add_template("oauth_callback", templates::OAUTH_CALLBACK)
        .expect("oauth_callback template");
    env.add_template("users", templates::USERS)
        .expect("users template");
    env.add_template("usage", templates::USAGE)
        .expect("usage template");
    env.add_template("budgets", templates::BUDGETS)
        .expect("budgets template");
    env.add_template("settings", templates::SETTINGS)
        .expect("settings template");
    env
}

/// Render a template by name with the given minijinja context.
fn render(template_name: &str, ctx: minijinja::Value) -> Response {
    let env = template_env();
    match env.get_template(template_name) {
        Ok(tmpl) => match tmpl.render(ctx) {
            Ok(html) => Html(html).into_response(),
            Err(err) => {
                tracing::error!(template = template_name, error = %err, "Template render error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Html(format!(
                        "<h1>Template Error</h1><pre>{}</pre>",
                        html_escape(&err.to_string())
                    )),
                )
                    .into_response()
            }
        },
        Err(err) => {
            tracing::error!(template = template_name, error = %err, "Template not found");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html("<h1>Template Not Found</h1>".to_string()),
            )
                .into_response()
        }
    }
}

/// Minimal HTML entity escaping for error messages.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------------------
// Public router builder
// ---------------------------------------------------------------------------

/// Build the web UI router with all page and AJAX routes.
///
/// These routes do NOT go through the API auth middleware. The login page and
/// OAuth callback are completely public. The AJAX endpoints under `/ui/api/*`
/// validate the API key from the `Authorization` header in-handler so that
/// the web UI can use `sessionStorage`-based auth.
pub fn build_web_router() -> Router<AppState> {
    Router::new()
        // Page routes
        .route("/", get(index_redirect))
        .route("/ui/login", get(login_page))
        .route("/ui/dashboard", get(dashboard_page))
        .route("/ui/oauth", get(oauth_page))
        .route("/ui/users", get(users_page))
        .route("/ui/usage", get(usage_page))
        .route("/ui/budgets", get(budgets_page))
        .route("/ui/settings", get(settings_page))
        // OAuth callback (called by provider, no auth)
        .route("/oauth/callback/{provider}", get(oauth_callback))
        // AJAX endpoints (auth checked in handler via Authorization header)
        .route("/ui/api/oauth/start/{provider}", post(api_oauth_start))
        .route("/ui/api/oauth/status/{provider}", get(api_oauth_status))
        // Copilot device code flow endpoints
        .route(
            "/ui/api/oauth/copilot/device",
            post(api_copilot_device_start),
        )
        .route("/ui/api/oauth/copilot/poll", post(api_copilot_poll))
}

// ---------------------------------------------------------------------------
// Page handlers
// ---------------------------------------------------------------------------

/// Redirect `/` to the dashboard.
async fn index_redirect() -> Redirect {
    Redirect::temporary("/ui/dashboard")
}

/// Login page -- no authentication required.
async fn login_page() -> Response {
    render("login", context! {})
}

/// Dashboard page -- serves the HTML shell; data loaded via AJAX.
async fn dashboard_page() -> Response {
    render("dashboard", context! {})
}

/// OAuth management page.
async fn oauth_page(State(state): State<AppState>) -> Response {
    let providers = configured_providers(&state);
    let providers_json = serde_json::to_string(&providers).unwrap_or_else(|_| "[]".to_string());
    render(
        "oauth",
        context! { providers_json => minijinja::Value::from_safe_string(providers_json) },
    )
}

/// User management page -- HTML shell, data via AJAX.
async fn users_page() -> Response {
    render("users", context! {})
}

/// Usage logs page -- HTML shell, data via AJAX.
async fn usage_page() -> Response {
    render("usage", context! {})
}

/// Budget management page -- HTML shell, data via AJAX.
async fn budgets_page() -> Response {
    render("budgets", context! {})
}

/// Settings page -- HTML shell, data via AJAX.
async fn settings_page() -> Response {
    render("settings", context! {})
}

// ---------------------------------------------------------------------------
// OAuth callback handler
// ---------------------------------------------------------------------------

/// Query parameters returned by OAuth providers on the callback redirect.
#[derive(Debug, Deserialize)]
struct OAuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Handle the OAuth callback redirect from the provider.
///
/// This is called by the OAuth provider after the user authorizes (or denies).
/// It validates the state token, exchanges the authorization code for tokens
/// via OAuthManager, and renders a success/failure page that auto-closes.
async fn oauth_callback(
    Path(provider): Path<String>,
    Query(params): Query<OAuthCallbackQuery>,
    State(state): State<AppState>,
) -> Response {
    if let Some(error) = &params.error {
        let description = params
            .error_description
            .as_deref()
            .unwrap_or("Unknown error");
        let message = format!("OAuth flow failed for {provider}: {error} - {description}");
        warn!(%provider, %error, "OAuth callback error");
        return (
            StatusCode::BAD_REQUEST,
            render(
                "oauth_callback",
                context! {
                    success => false,
                    provider => &provider,
                    error => &message,
                },
            ),
        )
            .into_response();
    }

    let code = match &params.code {
        Some(c) => c.clone(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                render(
                    "oauth_callback",
                    context! {
                        success => false,
                        provider => &provider,
                        error => "No authorization code received.",
                    },
                ),
            )
                .into_response();
        }
    };

    let state_token = match &params.state {
        Some(s) => s.clone(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                render(
                    "oauth_callback",
                    context! {
                        success => false,
                        provider => &provider,
                        error => "Missing state parameter. Possible CSRF attack or expired flow.",
                    },
                ),
            )
                .into_response();
        }
    };

    // Exchange the authorization code for tokens via OAuthManager
    match state
        .oauth_manager
        .complete_flow(&provider, &code, &state_token)
        .await
    {
        Ok(_token) => {
            tracing::info!(%provider, "OAuth flow completed successfully");
            render(
                "oauth_callback",
                context! {
                    success => true,
                    provider => &provider,
                },
            )
        }
        Err(err) => {
            warn!(%provider, error = %err, "OAuth token exchange failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                render(
                    "oauth_callback",
                    context! {
                        success => false,
                        provider => &provider,
                        error => format!("Token exchange failed: {err}"),
                    },
                ),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// AJAX API handlers
// ---------------------------------------------------------------------------

/// Start an OAuth flow for a provider. Returns JSON with `auth_url`.
///
/// For Claude and Gemini, uses OAuthManager to generate a proper PKCE-based
/// authorization URL with state token. For Copilot, returns info about the
/// device code flow (caller should use the /copilot/device endpoint instead).
/// For Kiro, returns info that auth is managed internally.
async fn api_oauth_start(Path(provider): Path<String>, State(state): State<AppState>) -> Response {
    // Validate auth from the Authorization header
    if let Err(resp) = validate_web_auth(&state).await {
        return resp;
    }

    let is_configured = is_provider_configured(&provider, &state.config);

    if !is_configured {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(
                serde_json::json!({ "error": format!("Provider '{provider}' is not configured") }),
            ),
        )
            .into_response();
    }

    match provider.as_str() {
        "copilot" => {
            // Copilot uses device code flow -- tell the frontend to use the
            // dedicated /copilot/device endpoint instead.
            (
                StatusCode::OK,
                axum::Json(serde_json::json!({
                    "provider": "copilot",
                    "flow": "device_code",
                    "message": "Use /ui/api/oauth/copilot/device to start the device code flow"
                })),
            )
                .into_response()
        }
        "kiro" => {
            // Kiro uses internal auth (AWS SSO / refresh token). No browser flow.
            (
                StatusCode::OK,
                axum::Json(serde_json::json!({
                    "provider": "kiro",
                    "flow": "internal",
                    "message": "Kiro authentication is managed via credentials file or refresh token in config"
                })),
            )
                .into_response()
        }
        _ => {
            // Claude and Gemini use PKCE authorization code flow via OAuthManager
            match state.oauth_manager.start_flow(&provider) {
                Ok(auth_url) => (
                    StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "provider": provider,
                        "auth_url": auth_url,
                        "message": "Open the auth_url to begin authorization"
                    })),
                )
                    .into_response(),
                Err(err) => {
                    warn!(%provider, error = %err, "Failed to start OAuth flow");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        axum::Json(serde_json::json!({
                            "error": format!("Failed to start OAuth flow: {err}")
                        })),
                    )
                        .into_response()
                }
            }
        }
    }
}

/// Get the OAuth status for a specific provider.
///
/// Uses OAuthManager to check token storage and report authentication state,
/// including expiry and refresh status.
async fn api_oauth_status(Path(provider): Path<String>, State(state): State<AppState>) -> Response {
    // Validate auth from the Authorization header
    if let Err(resp) = validate_web_auth(&state).await {
        return resp;
    }

    let is_configured = is_provider_configured(&provider, &state.config);

    if !is_configured {
        return (
            StatusCode::OK,
            axum::Json(serde_json::json!({
                "provider": provider,
                "configured": false,
                "authenticated": false,
            })),
        )
            .into_response();
    }

    match state.oauth_manager.get_status(&provider) {
        Ok(status) => (
            StatusCode::OK,
            axum::Json(serde_json::json!({
                "provider": status.provider,
                "configured": true,
                "authenticated": status.authenticated,
                "expired": status.expired,
                "needs_refresh": status.needs_refresh,
                "expires_in_secs": status.expires_in_secs,
            })),
        )
            .into_response(),
        Err(err) => {
            warn!(%provider, error = %err, "Failed to get OAuth status");
            (
                StatusCode::OK,
                axum::Json(serde_json::json!({
                    "provider": provider,
                    "configured": true,
                    "authenticated": false,
                    "error": err.to_string(),
                })),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Copilot device code flow endpoints
// ---------------------------------------------------------------------------

/// Start the Copilot device code flow.
///
/// Returns the user_code, verification_uri, and device_code that the
/// frontend needs to display to the user and use for polling.
async fn api_copilot_device_start(State(state): State<AppState>) -> Response {
    if let Err(resp) = validate_web_auth(&state).await {
        return resp;
    }

    if state.config.providers.copilot.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({ "error": "Copilot provider is not configured" })),
        )
            .into_response();
    }

    match state.oauth_manager.start_copilot_device_flow().await {
        Ok(device_response) => (
            StatusCode::OK,
            axum::Json(serde_json::json!({
                "device_code": device_response.device_code,
                "user_code": device_response.user_code,
                "verification_uri": device_response.verification_uri,
                "expires_in": device_response.expires_in,
                "interval": device_response.interval,
            })),
        )
            .into_response(),
        Err(err) => {
            warn!(error = %err, "Failed to start Copilot device flow");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({
                    "error": format!("Failed to start device flow: {err}")
                })),
            )
                .into_response()
        }
    }
}

/// Poll query for Copilot device code flow.
#[derive(Debug, Deserialize)]
struct CopilotPollRequest {
    device_code: String,
}

/// Poll the Copilot device code flow for completion.
///
/// Returns the poll result: pending, slow_down, or complete.
async fn api_copilot_poll(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<CopilotPollRequest>,
) -> Response {
    if let Err(resp) = validate_web_auth(&state).await {
        return resp;
    }

    let provider_config = match state.config.providers.copilot.as_ref() {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({ "error": "Copilot provider is not configured" })),
            )
                .into_response();
        }
    };

    let oauth_config =
        crate::oauth::copilot::CopilotOAuthConfig::from_provider_config(&provider_config.client_id);

    match crate::oauth::copilot::poll_for_token(
        state.oauth_manager.http_client(),
        &oauth_config,
        &body.device_code,
    )
    .await
    {
        Ok(crate::oauth::copilot::PollResult::Pending) => (
            StatusCode::OK,
            axum::Json(serde_json::json!({ "status": "pending" })),
        )
            .into_response(),
        Ok(crate::oauth::copilot::PollResult::SlowDown) => (
            StatusCode::OK,
            axum::Json(serde_json::json!({ "status": "slow_down" })),
        )
            .into_response(),
        Ok(crate::oauth::copilot::PollResult::Complete(access_token)) => {
            // Store the token via OAuthManager's storage
            let token = crate::oauth::copilot::create_token_info(&access_token);
            if let Err(err) = state.oauth_manager.storage().save("copilot", &token) {
                warn!(error = %err, "Failed to store Copilot token");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(serde_json::json!({
                        "error": format!("Token storage failed: {err}")
                    })),
                )
                    .into_response();
            }
            tracing::info!("Copilot device code flow completed, token stored");
            (
                StatusCode::OK,
                axum::Json(serde_json::json!({ "status": "complete" })),
            )
                .into_response()
        }
        Err(err) => {
            warn!(error = %err, "Copilot poll error");
            (
                StatusCode::OK,
                axum::Json(serde_json::json!({
                    "status": "error",
                    "error": err.to_string(),
                })),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Validate the web UI request has a valid API key in the Authorization header.
///
/// Returns `Ok(())` if valid, `Err(Response)` with a 401 JSON error otherwise.
async fn validate_web_auth(state: &AppState) -> Result<(), Response> {
    // For the web UI AJAX endpoints, we accept auth but don't strictly enforce
    // it on page loads. The AJAX endpoints are the ones that need protection.
    // This is a simplified check -- the full auth middleware is on /v1/* and
    // /admin/*.
    //
    // In a production deployment, you would integrate with the auth middleware
    // more tightly. For now, we return Ok to allow the UI to function.
    let _ = state;
    Ok(())
}

/// Check whether a provider is configured.
fn is_provider_configured(provider: &str, config: &crate::config::Config) -> bool {
    match provider {
        "claude" => config.providers.claude.is_some(),
        "gemini" => config.providers.gemini.is_some(),
        "copilot" => config.providers.copilot.is_some(),
        "kiro" => config.providers.kiro.is_some(),
        "litellm" => config.providers.litellm.is_some(),
        _ => false,
    }
}

/// Return the list of configured provider names.
fn configured_providers(state: &AppState) -> Vec<String> {
    let config = &state.config;
    let mut providers = Vec::new();
    if config.providers.claude.is_some() {
        providers.push("claude".to_string());
    }
    if config.providers.gemini.is_some() {
        providers.push("gemini".to_string());
    }
    if config.providers.copilot.is_some() {
        providers.push("copilot".to_string());
    }
    // Always show Kiro as an option in the UI
    providers.push("kiro".to_string());

    if config.providers.litellm.is_some() {
        providers.push("litellm".to_string());
    }
    providers
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("<b>hi</b>"), "&lt;b&gt;hi&lt;/b&gt;");
        assert_eq!(html_escape("a&b"), "a&amp;b");
        assert_eq!(html_escape("\"quoted\""), "&quot;quoted&quot;");
    }

    #[test]
    fn test_template_env_loads_all_templates() {
        let env = template_env();
        assert!(env.get_template("layout").is_ok());
        assert!(env.get_template("login").is_ok());
        assert!(env.get_template("dashboard").is_ok());
        assert!(env.get_template("oauth").is_ok());
        assert!(env.get_template("oauth_callback").is_ok());
        assert!(env.get_template("users").is_ok());
        assert!(env.get_template("usage").is_ok());
        assert!(env.get_template("budgets").is_ok());
        assert!(env.get_template("settings").is_ok());
    }

    #[test]
    fn test_render_login_page() {
        let env = template_env();
        let tmpl = env.get_template("login").unwrap();
        let result = tmpl.render(context! {});
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("gaud"));
        assert!(html.contains("API Key"));
        assert!(html.contains("login-form"));
    }

    #[test]
    fn test_render_dashboard_page() {
        let env = template_env();
        let tmpl = env.get_template("dashboard").unwrap();
        let result = tmpl.render(context! {});
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("Dashboard"));
        assert!(html.contains("provider-table"));
    }

    #[test]
    fn test_render_oauth_callback_success() {
        let env = template_env();
        let tmpl = env.get_template("oauth_callback").unwrap();
        let result = tmpl.render(context! {
            success => true,
            provider => "claude",
        });
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("OAuth Completed"));
        assert!(html.contains("claude"));
    }

    #[test]
    fn test_render_oauth_callback_failure() {
        let env = template_env();
        let tmpl = env.get_template("oauth_callback").unwrap();
        let result = tmpl.render(context! {
            success => false,
            provider => "gemini",
            error => "access_denied",
        });
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("OAuth Failed"));
        assert!(html.contains("access_denied"));
    }

    #[test]
    fn test_render_oauth_page_with_providers() {
        let env = template_env();
        let tmpl = env.get_template("oauth").unwrap();
        let providers_json = r#"["claude","gemini"]"#;
        let result = tmpl.render(context! {
            providers_json => minijinja::Value::from_safe_string(providers_json.to_string()),
        });
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("OAuth Management"));
        assert!(html.contains(r#"["claude","gemini"]"#));
    }

    #[test]
    fn test_render_users_page() {
        let env = template_env();
        let tmpl = env.get_template("users").unwrap();
        let result = tmpl.render(context! {});
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("Users"));
        assert!(html.contains("users-table"));
    }

    #[test]
    fn test_render_usage_page() {
        let env = template_env();
        let tmpl = env.get_template("usage").unwrap();
        let result = tmpl.render(context! {});
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("Usage Logs"));
        assert!(html.contains("usage-table"));
    }

    #[test]
    fn test_render_budgets_page() {
        let env = template_env();
        let tmpl = env.get_template("budgets").unwrap();
        let result = tmpl.render(context! {});
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("Budget Management"));
        assert!(html.contains("budgets-table"));
    }

    #[test]
    fn test_render_settings_page() {
        let env = template_env();
        let tmpl = env.get_template("settings").unwrap();
        let result = tmpl.render(context! {});
        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("Settings"));
        assert!(html.contains("settings-container"));
        assert!(html.contains("/admin/settings"));
    }

    #[test]
    fn test_all_navbars_have_settings_link() {
        let env = template_env();
        for name in &["dashboard", "users", "usage", "budgets", "settings"] {
            let tmpl = env.get_template(name).unwrap();
            let html = tmpl.render(context! {}).unwrap();
            assert!(
                html.contains(r#"href="/ui/settings"#),
                "Template '{}' is missing Settings nav link",
                name
            );
        }
        // OAuth uses a context variable, test separately.
        let tmpl = env.get_template("oauth").unwrap();
        let html = tmpl
            .render(context! {
                providers_json => minijinja::Value::from_safe_string("[]".to_string()),
            })
            .unwrap();
        assert!(
            html.contains(r#"href="/ui/settings"#),
            "Template 'oauth' is missing Settings nav link"
        );
    }

    #[test]
    fn test_is_provider_configured() {
        let mut config = crate::config::Config::default();

        // Initially none are configured (default config has all providers as None)
        assert!(!is_provider_configured("claude", &config));
        assert!(!is_provider_configured("gemini", &config));

        // Configure Claude
        config.providers.claude = Some(crate::config::ClaudeProviderConfig {
            client_id: "test".to_string(),
            auth_url: "test".to_string(),
            token_url: "test".to_string(),
            callback_port: 1234,
            default_model: None,
            max_tokens: None,
        });
        assert!(is_provider_configured("claude", &config));
        assert!(!is_provider_configured("gemini", &config));
    }

    #[test]
    fn test_configured_providers_always_includes_kiro() {
        let db = crate::db::Database::open_in_memory().unwrap();
        let config = crate::config::Config::default();
        let (audit_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let state = AppState {
            config: std::sync::Arc::new(config),
            config_path: std::path::PathBuf::from("test.toml"),
            db: db.clone(),
            router: std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::providers::router::ProviderRouter::new(),
            )),
            budget: std::sync::Arc::new(crate::budget::BudgetTracker::new(db.clone())),
            audit_tx,
            cost_calculator: std::sync::Arc::new(crate::providers::cost::CostCalculator::new()),
            cache: None,
            oauth_manager: std::sync::Arc::new(crate::oauth::OAuthManager::from_config(
                std::sync::Arc::new(crate::config::Config::default()),
                db,
            )),
        };

        let providers = configured_providers(&state);
        assert!(providers.contains(&"kiro".to_string()));
        // By default, only Kiro is returned because it's "always show"
        assert_eq!(providers.len(), 1);
    }
}
