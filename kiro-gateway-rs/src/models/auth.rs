//! Authentication-related types.

use serde::{Deserialize, Serialize};

/// Type of authentication mechanism.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    /// Kiro IDE credentials (default).
    /// Uses `https://prod.{region}.auth.desktop.kiro.dev/refreshToken`
    #[default]
    KiroDesktop,
    /// AWS SSO OIDC credentials from kiro-cli.
    /// Uses `https://oidc.{region}.amazonaws.com/token`
    AwsSsoOidc,
}

impl std::fmt::Display for AuthType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KiroDesktop => write!(f, "Kiro Desktop"),
            Self::AwsSsoOidc => write!(f, "AWS SSO OIDC"),
        }
    }
}

/// Source of credentials.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CredentialSource {
    /// Direct / programmatic construction.
    #[default]
    Direct,
    /// Environment variable.
    Environment,
    /// JSON credentials file.
    JsonFile(String),
    /// SQLite database (kiro-cli).
    SqliteDb(String),
}

impl std::fmt::Display for CredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Direct => write!(f, "direct"),
            Self::Environment => write!(f, "environment"),
            Self::JsonFile(path) => write!(f, "json:{}", path),
            Self::SqliteDb(path) => write!(f, "sqlite:{}", path),
        }
    }
}

/// Kiro-specific token information.
///
/// Extends the generic `TokenInfo` with Kiro-specific fields like
/// `profile_arn`, `region`, and SSO OIDC client credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KiroTokenInfo {
    /// OAuth access token.
    pub access_token: String,
    /// OAuth refresh token.
    pub refresh_token: String,
    /// Unix timestamp when access token expires.
    pub expires_at: i64,
    /// AWS CodeWhisperer profile ARN.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_arn: Option<String>,
    /// AWS region for API calls.
    #[serde(default = "default_region")]
    pub region: String,
    /// SSO region (may differ from API region).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sso_region: Option<String>,
    /// OAuth client ID (for AWS SSO OIDC).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// OAuth client secret (for AWS SSO OIDC).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// OAuth scopes (for AWS SSO OIDC).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    /// Detected auth type.
    #[serde(skip)]
    pub auth_type: AuthType,
    /// Where credentials were loaded from.
    #[serde(skip)]
    pub source: CredentialSource,
}

fn default_region() -> String {
    crate::config::DEFAULT_REGION.to_string()
}

impl KiroTokenInfo {
    /// Create new KiroTokenInfo with minimal fields.
    pub fn new(refresh_token: String) -> Self {
        Self {
            access_token: String::new(),
            refresh_token,
            expires_at: 0,
            profile_arn: None,
            region: default_region(),
            sso_region: None,
            client_id: None,
            client_secret: None,
            scopes: None,
            auth_type: AuthType::KiroDesktop,
            source: CredentialSource::Direct,
        }
    }

    /// Check if the access token is expired (with safety margin).
    #[must_use]
    pub fn is_expired(&self) -> bool {
        let now = chrono::Utc::now().timestamp();
        self.expires_at <= now + 60
    }

    /// Check if the token needs proactive refresh (within threshold).
    #[must_use]
    pub fn needs_refresh(&self) -> bool {
        let now = chrono::Utc::now().timestamp();
        let threshold = crate::config::TOKEN_REFRESH_THRESHOLD.as_secs() as i64;
        self.expires_at <= now + threshold
    }

    /// Detect auth type from available credentials.
    pub fn detect_auth_type(&mut self) {
        if self.client_id.is_some() && self.client_secret.is_some() {
            self.auth_type = AuthType::AwsSsoOidc;
        } else {
            self.auth_type = AuthType::KiroDesktop;
        }
    }
}

/// Response from Kiro Desktop Auth refresh endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroDesktopRefreshResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default = "default_expires_in")]
    pub expires_in: i64,
    #[serde(default)]
    pub profile_arn: Option<String>,
}

/// Response from AWS SSO OIDC token endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AwsSsoOidcRefreshResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default = "default_expires_in")]
    pub expires_in: i64,
}

fn default_expires_in() -> i64 {
    3600
}

/// SQLite token keys searched in priority order.
pub const SQLITE_TOKEN_KEYS: &[&str] = &[
    "kirocli:social:token",
    "kirocli:odic:token",
    "codewhisperer:odic:token",
];

/// SQLite device registration keys for AWS SSO OIDC.
pub const SQLITE_REGISTRATION_KEYS: &[&str] = &[
    "kirocli:odic:device-registration",
    "codewhisperer:odic:device-registration",
];
