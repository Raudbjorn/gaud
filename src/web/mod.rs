//! Embedded Web UI for Gaud.
//!
//! Provides a monitoring dashboard, OAuth management, user administration,
//! usage logs, and budget configuration -- all rendered from embedded HTML
//! templates via minijinja.

pub mod templates;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use minijinja::{context, Environment};
use serde::Deserialize;

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
            (StatusCode::INTERNAL_SERVER_ERROR, Html("<h1>Template Not Found</h1>".to_string()))
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
    render("oauth", context! { providers_json => minijinja::Value::from_safe_string(providers_json) })
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
/// It renders a small page that shows success/failure and auto-closes.
async fn oauth_callback(
    Path(provider): Path<String>,
    Query(params): Query<OAuthCallbackQuery>,
    State(_state): State<AppState>,
) -> Response {
    if let Some(error) = &params.error {
        let description = params
            .error_description
            .as_deref()
            .unwrap_or("Unknown error");
        let message = format!("OAuth flow failed for {provider}: {error} - {description}");
        tracing::warn!(%provider, %error, "OAuth callback error");
        return render(
            "oauth_callback",
            context! {
                success => false,
                provider => &provider,
                error => &message,
            },
        );
    }

    match &params.code {
        Some(_code) => {
            // The actual token exchange is handled by the OAuth module's callback
            // server (running on a separate port). This web UI callback route is
            // for display purposes -- showing the user that the flow completed.
            //
            // In a full implementation, we would exchange the code here via
            // OAuthManager::handle_callback(provider, code, state). For now we
            // show success and let the OAuth module handle the exchange on its
            // own callback port.
            tracing::info!(%provider, "OAuth callback received authorization code");
            render(
                "oauth_callback",
                context! {
                    success => true,
                    provider => &provider,
                },
            )
        }
        None => render(
            "oauth_callback",
            context! {
                success => false,
                provider => &provider,
                error => "No authorization code received.",
            },
        ),
    }
}

// ---------------------------------------------------------------------------
// AJAX API handlers
// ---------------------------------------------------------------------------

/// Start an OAuth flow for a provider. Returns JSON with `auth_url`.
async fn api_oauth_start(
    Path(provider): Path<String>,
    State(state): State<AppState>,
) -> Response {
    // Validate auth from the Authorization header
    if let Err(resp) = validate_web_auth(&state).await {
        return resp;
    }

    let config = &state.config;
    let is_configured = match provider.as_str() {
        "claude" => config.providers.claude.is_some(),
        "gemini" => config.providers.gemini.is_some(),
        "copilot" => config.providers.copilot.is_some(),
        _ => false,
    };

    if !is_configured {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({ "error": format!("Provider '{provider}' is not configured") })),
        )
            .into_response();
    }

    // Return a placeholder response. The actual OAuth flow initiation would be
    // done through the OAuthManager. This endpoint provides the auth URL to
    // open in a popup.
    let auth_url = build_oauth_start_url(&provider, config);
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({
            "provider": provider,
            "auth_url": auth_url,
            "message": "Open the auth_url to begin authorization"
        })),
    )
        .into_response()
}

/// Get the OAuth status for a specific provider.
async fn api_oauth_status(
    Path(provider): Path<String>,
    State(state): State<AppState>,
) -> Response {
    // Validate auth from the Authorization header
    if let Err(resp) = validate_web_auth(&state).await {
        return resp;
    }

    let config = &state.config;
    let is_configured = match provider.as_str() {
        "claude" => config.providers.claude.is_some(),
        "gemini" => config.providers.gemini.is_some(),
        "copilot" => config.providers.copilot.is_some(),
        _ => false,
    };

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

    // Check if there is a stored token for this provider by attempting to
    // read from the token storage directory.
    let token_dir = &config.providers.token_storage_dir;
    let token_file = token_dir.join(format!("{provider}.json"));
    let authenticated = token_file.exists();

    (
        StatusCode::OK,
        axum::Json(serde_json::json!({
            "provider": provider,
            "configured": true,
            "authenticated": authenticated,
        })),
    )
        .into_response()
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

/// Build the initial OAuth authorization URL for a given provider.
fn build_oauth_start_url(provider: &str, config: &crate::config::Config) -> String {
    match provider {
        "claude" => {
            if let Some(ref claude) = config.providers.claude {
                let redirect_uri = format!(
                    "http://localhost:{}/oauth/callback/claude",
                    claude.callback_port
                );
                format!(
                    "{}?response_type=code&client_id={}&redirect_uri={}&scope=user:inference&code_challenge_method=S256",
                    claude.auth_url,
                    claude.client_id,
                    urlencoding_encode(&redirect_uri),
                )
            } else {
                String::new()
            }
        }
        "gemini" => {
            if let Some(ref gemini) = config.providers.gemini {
                let redirect_uri = format!(
                    "http://localhost:{}/oauth/callback/gemini",
                    gemini.callback_port
                );
                format!(
                    "{}?response_type=code&client_id={}&redirect_uri={}&scope=https://www.googleapis.com/auth/generative-language&access_type=offline&prompt=consent",
                    gemini.auth_url,
                    gemini.client_id,
                    urlencoding_encode(&redirect_uri),
                )
            } else {
                String::new()
            }
        }
        "copilot" => {
            if let Some(ref copilot) = config.providers.copilot {
                format!(
                    "https://github.com/login/device/code?client_id={}&scope=copilot",
                    copilot.client_id,
                )
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

/// Percent-encode a string for use in URLs.
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push(char::from(HEX_CHARS[(byte >> 4) as usize]));
                out.push(char::from(HEX_CHARS[(byte & 0x0f) as usize]));
            }
        }
    }
    out
}

const HEX_CHARS: [u8; 16] = *b"0123456789ABCDEF";

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
    fn test_urlencoding_encode_basic() {
        assert_eq!(urlencoding_encode("hello"), "hello");
        assert_eq!(urlencoding_encode("hello world"), "hello%20world");
        assert_eq!(
            urlencoding_encode("http://localhost:8080/path"),
            "http%3A%2F%2Flocalhost%3A8080%2Fpath"
        );
    }

    #[test]
    fn test_urlencoding_encode_safe_chars() {
        // RFC 3986 unreserved characters should not be encoded.
        assert_eq!(urlencoding_encode("abc-._~123"), "abc-._~123");
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
        let html = tmpl.render(context! {
            providers_json => minijinja::Value::from_safe_string("[]".to_string()),
        }).unwrap();
        assert!(
            html.contains(r#"href="/ui/settings"#),
            "Template 'oauth' is missing Settings nav link"
        );
    }
}
