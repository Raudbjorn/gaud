//! OAuth callback handler.
//!
//! Handles the OAuth redirect callback from providers (Claude, Gemini).
//! Receives GET requests with `?code=...&state=...`, validates state
//! against the `oauth_state` table, exchanges code for tokens, stores
//! them, and returns an HTML page showing success/failure.

use serde::Deserialize;
use tracing::warn;

use crate::auth::error::AuthError;

/// Query parameters from the OAuth callback.
#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

/// Result of processing an OAuth callback.
#[derive(Debug)]
pub struct CallbackResult {
    /// The authorization code from the provider.
    pub code: String,
    /// The validated state token.
    pub state: String,
    /// The PKCE code verifier retrieved from the database.
    pub code_verifier: String,
    /// The provider name (from the oauth_state row).
    pub provider: String,
}

/// Validate callback parameters and extract the code/state.
///
/// Returns an error if the callback contains an error from the provider,
/// or if the code is missing.
pub fn validate_callback_params(params: &CallbackParams) -> Result<(String, String), AuthError> {
    // Check for OAuth error from provider
    if let Some(ref error) = params.error {
        let desc = params
            .error_description
            .as_deref()
            .unwrap_or("Unknown error");
        warn!(error = %error, description = %desc, "OAuth error from provider");
        return Err(AuthError::ExchangeFailed(format!("{}: {}", error, desc)));
    }

    let code = params
        .code
        .as_ref()
        .ok_or(AuthError::ExchangeFailed(
            "Missing authorization code in callback".to_string(),
        ))?
        .clone();

    let state = params
        .state
        .as_ref()
        .ok_or(AuthError::InvalidState)?
        .clone();

    Ok((code, state))
}

/// Look up and validate the state token against the oauth_state table.
///
/// Retrieves the associated provider and code_verifier, and removes the
/// row (state tokens are single-use). Returns an error if the state is
/// not found or has expired.
pub fn validate_state_from_db(
    db: &crate::db::Database,
    state_token: &str,
) -> Result<(String, String), AuthError> {
    db.with_conn(|conn| {
        // Look up state and verify it hasn't expired
        let mut stmt = conn.prepare(
            "SELECT provider, code_verifier, expires_at FROM oauth_state WHERE state_token = ?1",
        )?;

        let result = stmt.query_row([state_token], |row| {
            let provider: String = row.get(0)?;
            let code_verifier: String = row.get(1)?;
            let expires_at: String = row.get(2)?;
            Ok((provider, code_verifier, expires_at))
        });

        match result {
            Ok((provider, code_verifier, expires_at)) => {
                // Delete the state token (single-use)
                conn.execute(
                    "DELETE FROM oauth_state WHERE state_token = ?1",
                    [state_token],
                )?;

                // Check expiry
                let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
                if expires_at < now {
                    return Ok(Err(AuthError::FlowExpired));
                }

                Ok(Ok((provider, code_verifier)))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Err(AuthError::InvalidState)),
            Err(e) => Err(e),
        }
    })
    .map_err(|e| AuthError::Storage(format!("Database error: {}", e)))?
}

/// Store a new OAuth state token in the database.
///
/// The state token expires after 15 minutes.
pub fn store_state_in_db(
    db: &crate::db::Database,
    state_token: &str,
    provider: &str,
    code_verifier: &str,
) -> Result<(), AuthError> {
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO oauth_state (state_token, provider, code_verifier, created_at, expires_at) \
             VALUES (?1, ?2, ?3, datetime('now'), datetime('now', '+15 minutes'))",
            rusqlite::params![state_token, provider, code_verifier],
        )?;
        Ok(())
    })
    .map_err(|e| AuthError::Storage(format!("Failed to store OAuth state: {}", e)))
}

/// Clean up expired state tokens from the database.
pub fn cleanup_expired_states(db: &crate::db::Database) -> Result<u64, AuthError> {
    db.with_conn(|conn| {
        let deleted = conn.execute(
            "DELETE FROM oauth_state WHERE expires_at < datetime('now')",
            [],
        )?;
        Ok(deleted as u64)
    })
    .map_err(|e| AuthError::Storage(format!("Failed to clean up expired states: {}", e)))
}

// =============================================================================
// HTML Response Generation
// =============================================================================

/// Generate a success HTML page.
pub fn success_html(provider_name: &str) -> String {
    let provider = html_escape(provider_name);
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{provider} Authentication Successful</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            min-height: 100vh;
            margin: 0;
            background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
            color: #e0e0e0;
        }}
        .container {{
            text-align: center;
            padding: 2rem;
            max-width: 400px;
        }}
        .success-icon {{
            font-size: 4rem;
            margin-bottom: 1rem;
        }}
        h1 {{
            color: #34d399;
            margin-bottom: 1rem;
        }}
        p {{
            color: #9ca3af;
            margin-bottom: 1.5rem;
        }}
        .close-hint {{
            font-size: 0.875rem;
            color: #6b7280;
        }}
    </style>
    <script>
        setTimeout(function() {{
            window.close();
        }}, 3000);
    </script>
</head>
<body>
    <div class="container">
        <div class="success-icon">&#x2705;</div>
        <h1>Authentication Successful!</h1>
        <p>{provider} has been connected to gaud.</p>
        <p class="close-hint">This window will close automatically...</p>
    </div>
</body>
</html>"#
    )
}

/// Generate an error HTML page.
pub fn error_html(provider_name: &str, error: &str, description: &str) -> String {
    let provider = html_escape(provider_name);
    let error_code = html_escape(error);
    let desc = html_escape(description);
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{provider} Authentication Failed</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            min-height: 100vh;
            margin: 0;
            background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
            color: #e0e0e0;
        }}
        .container {{
            text-align: center;
            padding: 2rem;
            max-width: 500px;
        }}
        .error-icon {{
            font-size: 4rem;
            margin-bottom: 1rem;
        }}
        h1 {{
            color: #f87171;
            margin-bottom: 1rem;
        }}
        p {{
            color: #9ca3af;
            margin-bottom: 1rem;
        }}
        .error-details {{
            background: rgba(248, 113, 113, 0.1);
            border: 1px solid rgba(248, 113, 113, 0.3);
            border-radius: 8px;
            padding: 1rem;
            margin-top: 1rem;
            text-align: left;
        }}
        .error-code {{
            font-family: monospace;
            color: #f87171;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div class="error-icon">&#x274C;</div>
        <h1>Authentication Failed</h1>
        <p>Unable to connect {provider} to gaud.</p>
        <div class="error-details">
            <p><strong>Error:</strong> <span class="error-code">{error_code}</span></p>
            <p><strong>Details:</strong> {desc}</p>
        </div>
        <p>Please close this window and try again.</p>
    </div>
</body>
</html>"#
    )
}

/// Simple HTML escaping to prevent XSS.
fn html_escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_callback_params_success() {
        let params = CallbackParams {
            code: Some("test_code".to_string()),
            state: Some("test_state".to_string()),
            error: None,
            error_description: None,
        };
        let (code, state) = validate_callback_params(&params).unwrap();
        assert_eq!(code, "test_code");
        assert_eq!(state, "test_state");
    }

    #[test]
    fn test_validate_callback_params_error() {
        let params = CallbackParams {
            code: None,
            state: None,
            error: Some("access_denied".to_string()),
            error_description: Some("User denied access".to_string()),
        };
        let result = validate_callback_params(&params);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_callback_params_missing_code() {
        let params = CallbackParams {
            code: None,
            state: Some("test_state".to_string()),
            error: None,
            error_description: None,
        };
        let result = validate_callback_params(&params);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_callback_params_missing_state() {
        let params = CallbackParams {
            code: Some("test_code".to_string()),
            state: None,
            error: None,
            error_description: None,
        };
        let result = validate_callback_params(&params);
        assert!(matches!(result, Err(AuthError::InvalidState)));
    }

    #[test]
    fn test_success_html_generation() {
        let html = success_html("Claude");
        assert!(html.contains("Authentication Successful"));
        assert!(html.contains("Claude"));
        assert!(html.contains("gaud"));
        assert!(html.contains("window.close()"));
    }

    #[test]
    fn test_error_html_generation() {
        let html = error_html("Gemini", "invalid_grant", "Token was revoked");
        assert!(html.contains("Authentication Failed"));
        assert!(html.contains("Gemini"));
        assert!(html.contains("invalid_grant"));
        assert!(html.contains("Token was revoked"));
    }

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("hello"), "hello");
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a&b"), "a&amp;b");
        assert_eq!(html_escape("a\"b"), "a&quot;b");
        assert_eq!(html_escape("a'b"), "a&#39;b");
    }

    #[test]
    fn test_store_and_validate_state() {
        let db = crate::db::Database::open_in_memory().unwrap();

        // Store state
        store_state_in_db(&db, "test-state-123", "claude", "verifier-abc").unwrap();

        // Validate state
        let (provider, verifier) = validate_state_from_db(&db, "test-state-123").unwrap();
        assert_eq!(provider, "claude");
        assert_eq!(verifier, "verifier-abc");

        // State should be deleted after use (single-use)
        let result = validate_state_from_db(&db, "test-state-123");
        assert!(matches!(result, Err(AuthError::InvalidState)));
    }

    #[test]
    fn test_validate_nonexistent_state() {
        let db = crate::db::Database::open_in_memory().unwrap();
        let result = validate_state_from_db(&db, "nonexistent");
        assert!(matches!(result, Err(AuthError::InvalidState)));
    }

    #[test]
    fn test_cleanup_expired_states() {
        let db = crate::db::Database::open_in_memory().unwrap();

        // Insert an already-expired state
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO oauth_state (state_token, provider, code_verifier, created_at, expires_at) \
                 VALUES ('expired-state', 'claude', 'verifier', datetime('now', '-1 hour'), datetime('now', '-30 minutes'))",
                [],
            )?;
            Ok(())
        }).unwrap();

        let deleted = cleanup_expired_states(&db).unwrap();
        assert_eq!(deleted, 1);
    }
}
