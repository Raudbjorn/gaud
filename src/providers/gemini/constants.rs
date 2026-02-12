//! Constants and configuration for the Cloud Code API.
//!
//! This module contains API endpoints, OAuth configuration, model detection
//! functions, and other constants used throughout the library.

use std::time::Duration;

// ============================================================================
// API Endpoints
// ============================================================================

/// Daily/experimental Cloud Code API endpoint.
pub const CLOUDCODE_ENDPOINT_DAILY: &str = "https://daily-cloudcode-pa.googleapis.com";

/// Production Cloud Code API endpoint.
pub const CLOUDCODE_ENDPOINT_PROD: &str = "https://cloudcode-pa.googleapis.com";

/// Cloud Code API endpoint fallback order (daily first, then prod).
///
/// The daily endpoint typically has newer features and better availability
/// for generateContent requests.
pub const CLOUDCODE_ENDPOINT_FALLBACKS: &[&str] =
    &[CLOUDCODE_ENDPOINT_DAILY, CLOUDCODE_ENDPOINT_PROD];

/// Endpoint order for loadCodeAssist API (prod first, then daily).
///
/// The prod endpoint works better for fresh/unprovisioned accounts
/// when discovering project IDs.
pub const LOAD_CODE_ASSIST_ENDPOINTS: &[&str] =
    &[CLOUDCODE_ENDPOINT_PROD, CLOUDCODE_ENDPOINT_DAILY];

// ============================================================================
// OAuth Configuration
// ============================================================================

/// OAuth 2.0 configuration for Google authentication.
///
/// This configuration uses the Antigravity app's OAuth credentials,
/// which are intentionally public (matching the desktop application).
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    /// OAuth client ID.
    pub client_id: &'static str,
    /// OAuth client secret.
    pub client_secret: &'static str,
    /// Authorization URL for initiating OAuth flow.
    pub auth_url: &'static str,
    /// Token URL for exchanging authorization code.
    pub token_url: &'static str,
    /// User info URL for fetching profile data.
    pub user_info_url: &'static str,
    /// Local callback port for OAuth redirect.
    pub callback_port: u16,
    /// OAuth scopes required for Cloud Code access.
    pub scopes: &'static [&'static str],
}

/// Default OAuth configuration for Google Cloud Code.
///
/// Uses the Antigravity app's OAuth credentials which enable access
/// to both Claude and Gemini models through the Cloud Code API.
pub const DEFAULT_OAUTH_CONFIG: OAuthConfig = OAuthConfig {
    client_id: "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com",
    client_secret: "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",
    auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
    token_url: "https://oauth2.googleapis.com/token",
    user_info_url: "https://www.googleapis.com/oauth2/v1/userinfo",
    callback_port: 51121,
    scopes: &[
        "https://www.googleapis.com/auth/cloud-platform",
        "https://www.googleapis.com/auth/userinfo.email",
        "https://www.googleapis.com/auth/userinfo.profile",
        "https://www.googleapis.com/auth/cclog",
        "https://www.googleapis.com/auth/experimentsandconfigs",
    ],
};

// ============================================================================
// Model Detection
// ============================================================================

/// Model family classification.
///
/// Used to determine format conversion and signature handling behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelFamily {
    /// Anthropic Claude models.
    Claude,
    /// Google Gemini models.
    Gemini,
    /// Unknown model family.
    Unknown,
}

impl std::fmt::Display for ModelFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelFamily::Claude => write!(f, "claude"),
            ModelFamily::Gemini => write!(f, "gemini"),
            ModelFamily::Unknown => write!(f, "unknown"),
        }
    }
}

/// Determine the model family from a model name.
///
/// Detection is case-insensitive and looks for "claude" or "gemini"
/// anywhere in the model name.
///
/// # Examples
///
/// ```
/// use gaud::providers::gemini::constants::{get_model_family, ModelFamily};
///
/// assert_eq!(get_model_family("claude-sonnet-4-5-thinking"), ModelFamily::Claude);
/// assert_eq!(get_model_family("gemini-3-flash"), ModelFamily::Gemini);
/// assert_eq!(get_model_family("gpt-4"), ModelFamily::Unknown);
/// ```
pub fn get_model_family(model: &str) -> ModelFamily {
    let lower = model.to_lowercase();
    if lower.contains("claude") {
        ModelFamily::Claude
    } else if lower.contains("gemini") {
        ModelFamily::Gemini
    } else {
        ModelFamily::Unknown
    }
}

/// Check if a model supports thinking/reasoning output.
///
/// Thinking models include:
/// - Claude models with "thinking" in the name
/// - Gemini models with "thinking" in the name
/// - Gemini version 3+ models (e.g., gemini-3-flash, gemini-3-pro-high)
///
/// # Examples
///
/// ```
/// use gaud::providers::gemini::constants::is_thinking_model;
///
/// assert!(is_thinking_model("claude-sonnet-4-5-thinking"));
/// assert!(is_thinking_model("gemini-3-flash"));
/// assert!(!is_thinking_model("claude-sonnet-4-5"));
/// assert!(!is_thinking_model("gemini-2.5-flash"));
/// ```
pub fn is_thinking_model(model: &str) -> bool {
    let lower = model.to_lowercase();

    // Claude thinking models have "thinking" in the name
    if lower.contains("claude") && lower.contains("thinking") {
        return true;
    }

    // Gemini thinking models: explicit "thinking" in name, OR gemini version 3+
    if lower.contains("gemini") {
        if lower.contains("thinking") {
            return true;
        }
        // Check for gemini-3 or higher (e.g., gemini-3, gemini-3.5, gemini-4, etc.)
        // Matches patterns like "gemini-3", "gemini-3-flash", "gemini-4-pro"
        if let Some(version_start) = lower.find("gemini-") {
            let after_prefix = &lower[version_start + 7..];
            // Extract the version number (first digit sequence)
            let version_str: String = after_prefix
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(version) = version_str.parse::<u32>() {
                if version >= 3 {
                    return true;
                }
            }
        }
    }

    false
}

// ============================================================================
// Project and API Constants
// ============================================================================

/// Default project ID used when project discovery fails.
///
/// This is a fallback project that may have limited quota.
pub const DEFAULT_PROJECT_ID: &str = "rising-fact-p41fc";

/// Maximum output tokens for Gemini models.
pub const GEMINI_MAX_OUTPUT_TOKENS: u32 = 16384;

/// Sentinel value to skip thought signature validation.
///
/// Used when Claude Code strips the `thoughtSignature` field from
/// Gemini responses. The proxy can inject this value to bypass
/// signature validation on subsequent requests.
///
/// See: <https://ai.google.dev/gemini-api/docs/thought-signatures>
pub const GEMINI_SKIP_SIGNATURE: &str = "skip_thought_signature_validator";

/// TTL for cached Gemini thought signatures (2 hours).
pub const SIGNATURE_CACHE_TTL_SECS: u64 = 7200;

/// Signature cache TTL as a Duration.
pub const SIGNATURE_CACHE_TTL: Duration = Duration::from_secs(SIGNATURE_CACHE_TTL_SECS);

/// Minimum valid thinking signature length.
///
/// Signatures shorter than this are likely invalid or corrupted.
pub const MIN_SIGNATURE_LENGTH: usize = 50;

// ============================================================================
// HTTP Headers
// ============================================================================

/// User-Agent header value for API requests.
pub const USER_AGENT: &str = crate::config::GAUD_USER_AGENT;

/// X-Goog-Api-Client header value.
pub const GOOG_API_CLIENT: &str = "google-cloud-sdk vscode_cloudshelleditor/0.1";

/// Client-Metadata header value (JSON).
pub const CLIENT_METADATA: &str =
    r#"{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}"#;

// ============================================================================
// Timeouts
// ============================================================================

/// Connection timeout for HTTP requests.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Request timeout for non-streaming requests.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Idle timeout for streaming responses.
///
/// If no data is received within this duration, the stream is considered stale.
pub const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

// ============================================================================
// Rate Limiting
// ============================================================================

/// Default cooldown after rate limit error.
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(10);

/// Maximum wait time before returning rate limit error to caller.
///
/// If a rate limit has a retry-after longer than this, fail immediately
/// rather than blocking the caller.
pub const MAX_WAIT_BEFORE_ERROR: Duration = Duration::from_secs(120);

/// Maximum retries for transient errors.
pub const MAX_RETRIES: u32 = 5;

/// Maximum retries for empty API responses.
pub const MAX_EMPTY_RESPONSE_RETRIES: u32 = 2;

// ============================================================================
// API Paths
// ============================================================================

/// Path for generateContent API (non-streaming).
pub const API_PATH_GENERATE_CONTENT: &str = "/v1internal:generateContent";

/// Path for streamGenerateContent API (streaming).
pub const API_PATH_STREAM_GENERATE_CONTENT: &str = "/v1internal:streamGenerateContent?alt=sse";

/// Path for loadCodeAssist API (project discovery).
pub const API_PATH_LOAD_CODE_ASSIST: &str = "/v1internal/load_code_assist";

/// Path for onboardUser API.
pub const API_PATH_ONBOARD_USER: &str = "/v1internal/onboard_user";

/// Path for fetchAvailableModels API.
pub const API_PATH_FETCH_MODELS: &str = "/v1internal/fetch_available_models";

// ============================================================================
// System Instruction
// ============================================================================

/// System instruction for Antigravity identity.
///
/// This is injected into all requests to maintain compatibility with the
/// Cloud Code API's expected behavior.
pub const ANTIGRAVITY_SYSTEM_INSTRUCTION: &str = r#"You are Antigravity, a powerful agentic AI coding assistant designed by the Google Deepmind team working on Advanced Agentic Coding.You are pair programming with a USER to solve their coding task. The task may require creating a new codebase, modifying or debugging an existing codebase, or simply answering a question.**Absolute paths only****Proactiveness**"#;

// ============================================================================
// Known Models
// ============================================================================

/// Known Claude model identifiers.
pub const CLAUDE_MODELS: &[&str] = &[
    "claude-opus-4-5-thinking",
    "claude-sonnet-4-5-thinking",
    "claude-sonnet-4-5",
];

/// Known Gemini model identifiers.
pub const GEMINI_MODELS: &[&str] = &[
    "gemini-3-pro-high",
    "gemini-3-pro-low",
    "gemini-3-flash",
    "gemini-2.5-flash-lite",
];

/// Get the default max tokens for a model.
///
/// Returns model-specific limits or a sensible default.
pub fn default_max_tokens(model: &str) -> u32 {
    let family = get_model_family(model);
    match family {
        ModelFamily::Gemini => GEMINI_MAX_OUTPUT_TOKENS,
        ModelFamily::Claude | ModelFamily::Unknown => 8192,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_model_family_claude() {
        assert_eq!(
            get_model_family("claude-sonnet-4-5-thinking"),
            ModelFamily::Claude
        );
        assert_eq!(
            get_model_family("claude-opus-4-5-thinking"),
            ModelFamily::Claude
        );
        assert_eq!(get_model_family("claude-sonnet-4-5"), ModelFamily::Claude);
        assert_eq!(get_model_family("CLAUDE-SONNET-4-5"), ModelFamily::Claude);
        assert_eq!(get_model_family("Claude-Haiku-3"), ModelFamily::Claude);
    }

    #[test]
    fn test_get_model_family_gemini() {
        assert_eq!(get_model_family("gemini-3-flash"), ModelFamily::Gemini);
        assert_eq!(get_model_family("gemini-3-pro-high"), ModelFamily::Gemini);
        assert_eq!(
            get_model_family("gemini-2.5-flash-lite"),
            ModelFamily::Gemini
        );
        assert_eq!(get_model_family("GEMINI-3-PRO"), ModelFamily::Gemini);
        assert_eq!(get_model_family("Gemini-2-Pro"), ModelFamily::Gemini);
    }

    #[test]
    fn test_get_model_family_unknown() {
        assert_eq!(get_model_family("gpt-4"), ModelFamily::Unknown);
        assert_eq!(get_model_family("llama-3"), ModelFamily::Unknown);
        assert_eq!(get_model_family(""), ModelFamily::Unknown);
        assert_eq!(get_model_family("some-random-model"), ModelFamily::Unknown);
    }

    #[test]
    fn test_is_thinking_model_claude() {
        // Claude thinking models
        assert!(is_thinking_model("claude-sonnet-4-5-thinking"));
        assert!(is_thinking_model("claude-opus-4-5-thinking"));
        assert!(is_thinking_model("CLAUDE-SONNET-4-5-THINKING"));

        // Claude non-thinking models
        assert!(!is_thinking_model("claude-sonnet-4-5"));
        assert!(!is_thinking_model("claude-haiku-3"));
    }

    #[test]
    fn test_is_thinking_model_gemini() {
        // Gemini 3+ models are thinking by default
        assert!(is_thinking_model("gemini-3-flash"));
        assert!(is_thinking_model("gemini-3-pro-high"));
        assert!(is_thinking_model("gemini-3-pro-low"));
        assert!(is_thinking_model("GEMINI-3-FLASH"));
        assert!(is_thinking_model("gemini-4-pro"));

        // Gemini explicit thinking
        assert!(is_thinking_model("gemini-2-thinking"));

        // Gemini < 3 are not thinking
        assert!(!is_thinking_model("gemini-2.5-flash-lite"));
        assert!(!is_thinking_model("gemini-2-pro"));
        assert!(!is_thinking_model("gemini-1.5-pro"));
    }

    #[test]
    fn test_is_thinking_model_unknown() {
        assert!(!is_thinking_model("gpt-4"));
        assert!(!is_thinking_model("llama-3"));
        assert!(!is_thinking_model(""));
    }

    #[test]
    fn test_model_family_display() {
        assert_eq!(ModelFamily::Claude.to_string(), "claude");
        assert_eq!(ModelFamily::Gemini.to_string(), "gemini");
        assert_eq!(ModelFamily::Unknown.to_string(), "unknown");
    }

    #[test]
    fn test_default_max_tokens() {
        assert_eq!(
            default_max_tokens("gemini-3-flash"),
            GEMINI_MAX_OUTPUT_TOKENS
        );
        assert_eq!(default_max_tokens("claude-sonnet-4-5"), 8192);
        assert_eq!(default_max_tokens("unknown-model"), 8192);
    }

    #[test]
    fn test_oauth_config() {
        assert!(!DEFAULT_OAUTH_CONFIG.client_id.is_empty());
        assert!(!DEFAULT_OAUTH_CONFIG.client_secret.is_empty());
        assert!(DEFAULT_OAUTH_CONFIG.auth_url.starts_with("https://"));
        assert!(DEFAULT_OAUTH_CONFIG.token_url.starts_with("https://"));
        // callback_port is a compile-time constant (51121), so just verify it's non-zero
        assert_ne!(DEFAULT_OAUTH_CONFIG.callback_port, 0);
        assert!(!DEFAULT_OAUTH_CONFIG.scopes.is_empty());
    }

    #[test]
    fn test_endpoints() {
        assert!(CLOUDCODE_ENDPOINT_DAILY.starts_with("https://"));
        assert!(CLOUDCODE_ENDPOINT_PROD.starts_with("https://"));
        assert_eq!(CLOUDCODE_ENDPOINT_FALLBACKS.len(), 2);
        assert_eq!(LOAD_CODE_ASSIST_ENDPOINTS.len(), 2);
        // Verify different ordering
        assert_eq!(CLOUDCODE_ENDPOINT_FALLBACKS[0], CLOUDCODE_ENDPOINT_DAILY);
        assert_eq!(LOAD_CODE_ASSIST_ENDPOINTS[0], CLOUDCODE_ENDPOINT_PROD);
    }

    #[test]
    fn test_timeouts() {
        assert!(CONNECT_TIMEOUT.as_secs() > 0);
        assert!(REQUEST_TIMEOUT.as_secs() > 0);
        assert!(STREAM_IDLE_TIMEOUT.as_secs() > 0);
        assert!(REQUEST_TIMEOUT > CONNECT_TIMEOUT);
    }

    #[test]
    fn test_signature_cache_ttl() {
        assert_eq!(SIGNATURE_CACHE_TTL, Duration::from_secs(7200));
        assert_eq!(SIGNATURE_CACHE_TTL_SECS, 7200);
    }
}
