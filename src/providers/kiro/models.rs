use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthType {
    /// Kiro Desktop Auth (refresh token flow)
    KiroDesktop,
    /// AWS SSO OIDC (clientId/clientSecret/refreshToken flow)
    AwsSsoOidc,
}

impl std::fmt::Display for AuthType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KiroDesktop => write!(f, "kiro_desktop"),
            Self::AwsSsoOidc => write!(f, "aws_sso_oidc"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CredentialSource {
    Environment,
    JsonFile(std::path::PathBuf),
    SqliteDb {
        path: std::path::PathBuf,
        key: String,
        reg_key: Option<String>,
    },
    Auto,
}

impl std::fmt::Display for CredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Environment => write!(f, "environment"),
            Self::JsonFile(p) => write!(f, "file:{}", p.display()),
            Self::SqliteDb { path, .. } => write!(f, "sqlite:{}", path.display()),
            Self::Auto => write!(f, "auto"),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct KiroTokenInfo {
    pub access_token: String,
    pub refresh_token: String,
    /// Seconds since the Unix epoch (i64).
    ///
    /// Compared against `chrono::Utc::now().timestamp()` in
    /// [`needs_refresh()`](Self::needs_refresh), so callers must store
    /// seconds — not milliseconds.
    pub expires_at: i64,
    pub region: String,
    pub profile_arn: Option<String>,
    pub auth_type: AuthType,
    pub source: CredentialSource,
    /// AWS SSO client registration
    pub client_id: Option<String>,
    #[serde(skip_serializing)]
    pub client_secret: Option<String>,
    pub sso_region: Option<String>,
    pub scopes: Option<Vec<String>>,
}

impl std::fmt::Debug for KiroTokenInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KiroTokenInfo")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("region", &self.region)
            .field("profile_arn", &self.profile_arn)
            .field("auth_type", &self.auth_type)
            .field("source", &self.source)
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("sso_region", &self.sso_region)
            .field("scopes", &self.scopes)
            .finish()
    }
}

impl KiroTokenInfo {
    pub fn new(refresh_token: String, source: CredentialSource) -> Self {
        Self {
            access_token: String::new(),
            refresh_token,
            expires_at: 0,
            region: "us-east-1".to_string(),
            profile_arn: None,
            auth_type: AuthType::KiroDesktop,
            source,
            client_id: None,
            client_secret: None,
            sso_region: None,
            scopes: None,
        }
    }

    pub fn needs_refresh(&self) -> bool {
        let now = chrono::Utc::now().timestamp();
        // Refresh 10 minutes before expiry
        self.expires_at < now + 600
    }

    pub fn detect_auth_type(&mut self) {
        if self.client_id.is_some() && self.client_secret.is_some() {
            self.auth_type = AuthType::AwsSsoOidc;
        } else {
            self.auth_type = AuthType::KiroDesktop;
        }
    }
}

#[derive(Clone)]
pub struct TokenUpdate {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: i64,
    pub profile_arn: Option<String>,
}

impl std::fmt::Debug for TokenUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenUpdate")
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expires_at", &self.expires_at)
            .field("profile_arn", &self.profile_arn)
            .finish()
    }
}
