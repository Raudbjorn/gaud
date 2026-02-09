//! Token information and composite format handling.
//!
//! This module provides the [`TokenInfo`] struct for storing OAuth tokens
//! along with project IDs in a composite format for efficient single-file storage.
//!
//! # Composite Token Format
//!
//! Project IDs are encoded in the refresh token using pipe separators:
//! ```text
//! refresh_token|project_id|managed_project_id
//! ```

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::instrument;

/// Separator used in composite refresh token format.
const COMPOSITE_SEPARATOR: char = '|';

/// Safety margin for token expiry checks (60 seconds).
const EXPIRY_SAFETY_MARGIN_SECS: i64 = 60;

/// Proactive refresh buffer (5 minutes / 300 seconds).
const REFRESH_BUFFER_SECS: i64 = 300;

/// OAuth token information with composite project ID storage.
///
/// Stores access token, refresh token (with optional embedded project IDs),
/// and expiration timestamp. The composite format allows encoding project
/// discovery results in the refresh token field for efficient storage.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenInfo {
    /// The OAuth access token for API requests.
    pub access_token: String,

    /// OAuth refresh token, potentially in composite format.
    ///
    /// May contain embedded project IDs: `refresh|project_id|managed_project_id`
    pub refresh_token: Option<String>,

    /// Unix timestamp when the access token expires, if known.
    pub expires_at: Option<i64>,

    /// Token type, typically "Bearer".
    #[serde(default = "default_token_type")]
    pub token_type: String,

    /// Provider identifier (e.g., "claude", "gemini", "copilot").
    pub provider: String,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

impl TokenInfo {
    /// Create a new TokenInfo with the given tokens and expiry duration.
    pub fn new(
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<i64>,
        provider: &str,
    ) -> Self {
        let expires_at = expires_in.map(|ei| chrono::Utc::now().timestamp() + ei);
        Self {
            access_token,
            refresh_token,
            expires_at,
            token_type: "Bearer".to_string(),
            provider: provider.to_string(),
        }
    }

    /// Create a TokenInfo with a specific expiration timestamp.
    pub fn with_expires_at(
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<i64>,
        provider: &str,
    ) -> Self {
        Self {
            access_token,
            refresh_token,
            expires_at,
            token_type: "Bearer".to_string(),
            provider: provider.to_string(),
        }
    }

    /// Check if the access token is expired or about to expire.
    ///
    /// Returns `true` if the token has expired or will expire within
    /// the safety margin (60 seconds), indicating a refresh is needed.
    /// Returns `false` if no expiry is set (token never expires).
    #[must_use]
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(exp) => {
                let now = chrono::Utc::now().timestamp();
                exp <= now + EXPIRY_SAFETY_MARGIN_SECS
            }
            None => false,
        }
    }

    /// Check if the token should be proactively refreshed.
    ///
    /// Returns `true` if the token will expire within 5 minutes.
    /// Returns `false` if no expiry is set.
    #[must_use]
    pub fn needs_refresh(&self) -> bool {
        match self.expires_at {
            Some(exp) => {
                let now = chrono::Utc::now().timestamp();
                exp <= now + REFRESH_BUFFER_SECS
            }
            None => false,
        }
    }

    /// Get the duration until the access token expires.
    ///
    /// Returns `Duration::ZERO` if the token has already expired or has no expiry set.
    pub fn time_until_expiry(&self) -> Duration {
        match self.expires_at {
            Some(exp) => {
                let now = chrono::Utc::now().timestamp();
                let remaining = exp - now;
                if remaining > 0 {
                    Duration::from_secs(remaining as u64)
                } else {
                    Duration::ZERO
                }
            }
            None => Duration::ZERO,
        }
    }

    /// Parse the composite refresh token into its parts.
    ///
    /// The composite format is: `refresh_token|project_id|managed_project_id`
    ///
    /// Returns a tuple of:
    /// - The base refresh token (always present, empty string if no refresh token)
    /// - Optional project ID
    /// - Optional managed project ID
    #[instrument(skip(self))]
    pub fn parse_refresh_parts(&self) -> (String, Option<String>, Option<String>) {
        let refresh = match &self.refresh_token {
            Some(rt) => rt.as_str(),
            None => return (String::new(), None, None),
        };
        let parts: Vec<&str> = refresh.split(COMPOSITE_SEPARATOR).collect();

        let base_refresh = parts[0].to_string();
        let project_id = parts
            .get(1)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let managed_project_id = parts
            .get(2)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        (base_refresh, project_id, managed_project_id)
    }

    /// Get the base refresh token without embedded project IDs.
    pub fn base_refresh_token(&self) -> String {
        self.parse_refresh_parts().0
    }

    /// Get the project ID if embedded in the composite token.
    pub fn project_id(&self) -> Option<String> {
        self.parse_refresh_parts().1
    }

    /// Get the managed project ID if embedded in the composite token.
    pub fn managed_project_id(&self) -> Option<String> {
        self.parse_refresh_parts().2
    }

    /// Create a new TokenInfo with project IDs embedded in the refresh token.
    pub fn with_project_ids(self, project_id: &str, managed_project_id: Option<&str>) -> Self {
        let base_refresh = self.base_refresh_token();

        let composite = match managed_project_id {
            Some(managed) => format!(
                "{}{}{}{}{}",
                base_refresh, COMPOSITE_SEPARATOR, project_id, COMPOSITE_SEPARATOR, managed
            ),
            None => format!("{}{}{}", base_refresh, COMPOSITE_SEPARATOR, project_id),
        };

        Self {
            refresh_token: Some(composite),
            ..self
        }
    }

    /// Update the access token and expiry while preserving other fields.
    pub fn with_new_access_token(self, access_token: String, expires_in: Option<i64>) -> Self {
        let expires_at = expires_in.map(|ei| chrono::Utc::now().timestamp() + ei);
        Self {
            access_token,
            expires_at,
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_token() {
        let token = TokenInfo::new(
            "access".to_string(),
            Some("refresh".to_string()),
            Some(3600),
            "claude",
        );
        assert_eq!(token.token_type, "Bearer");
        assert_eq!(token.access_token, "access");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh"));
        assert!(!token.is_expired());
        assert_eq!(token.provider, "claude");
    }

    #[test]
    fn test_is_expired() {
        let expired = TokenInfo::with_expires_at(
            "access".to_string(),
            Some("refresh".to_string()),
            Some(0),
            "claude",
        );
        assert!(expired.is_expired());

        // Token expiring within safety margin
        let soon = TokenInfo::with_expires_at(
            "access".to_string(),
            Some("refresh".to_string()),
            Some(chrono::Utc::now().timestamp() + 30),
            "claude",
        );
        assert!(soon.is_expired());

        // Token with plenty of time
        let fresh = TokenInfo::new(
            "access".to_string(),
            Some("refresh".to_string()),
            Some(3600),
            "claude",
        );
        assert!(!fresh.is_expired());
    }

    #[test]
    fn test_no_expiry_never_expires() {
        let token = TokenInfo::new("access".to_string(), Some("refresh".to_string()), None, "claude");
        assert!(!token.is_expired());
        assert!(!token.needs_refresh());
    }

    #[test]
    fn test_time_until_expiry() {
        let token = TokenInfo::new(
            "access".to_string(),
            Some("refresh".to_string()),
            Some(3600),
            "claude",
        );
        let remaining = token.time_until_expiry();
        assert!(remaining.as_secs() >= 3595);
        assert!(remaining.as_secs() <= 3600);

        let expired = TokenInfo::with_expires_at(
            "access".into(),
            Some("refresh".into()),
            Some(0),
            "claude",
        );
        assert_eq!(expired.time_until_expiry(), Duration::ZERO);
    }

    #[test]
    fn test_parse_refresh_parts_simple() {
        let token = TokenInfo::new("access".into(), Some("refresh_token".into()), Some(3600), "claude");
        let (base, project, managed) = token.parse_refresh_parts();
        assert_eq!(base, "refresh_token");
        assert!(project.is_none());
        assert!(managed.is_none());
    }

    #[test]
    fn test_parse_refresh_parts_with_project() {
        let token = TokenInfo::new("access".into(), Some("refresh|proj-123".into()), Some(3600), "claude");
        let (base, project, managed) = token.parse_refresh_parts();
        assert_eq!(base, "refresh");
        assert_eq!(project.as_deref(), Some("proj-123"));
        assert!(managed.is_none());
    }

    #[test]
    fn test_parse_refresh_parts_with_both() {
        let token = TokenInfo::new(
            "access".into(),
            Some("refresh|proj-123|managed-456".into()),
            Some(3600),
            "claude",
        );
        let (base, project, managed) = token.parse_refresh_parts();
        assert_eq!(base, "refresh");
        assert_eq!(project.as_deref(), Some("proj-123"));
        assert_eq!(managed.as_deref(), Some("managed-456"));
    }

    #[test]
    fn test_parse_refresh_parts_no_refresh_token() {
        let token = TokenInfo::new("access".into(), None, Some(3600), "claude");
        let (base, project, managed) = token.parse_refresh_parts();
        assert_eq!(base, "");
        assert!(project.is_none());
        assert!(managed.is_none());
    }

    #[test]
    fn test_with_project_ids() {
        let token = TokenInfo::new("access".into(), Some("refresh".into()), Some(3600), "claude");
        let token = token.with_project_ids("proj-123", Some("managed-456"));
        assert_eq!(
            token.refresh_token.as_deref(),
            Some("refresh|proj-123|managed-456")
        );
        assert_eq!(token.access_token, "access");
    }

    #[test]
    fn test_with_project_ids_no_managed() {
        let token = TokenInfo::new("access".into(), Some("refresh".into()), Some(3600), "claude");
        let token = token.with_project_ids("proj-123", None);
        assert_eq!(
            token.refresh_token.as_deref(),
            Some("refresh|proj-123")
        );
    }

    #[test]
    fn test_composite_round_trip() {
        let original = TokenInfo::new("access".into(), Some("refresh".into()), Some(3600), "claude");
        let with_ids = original.with_project_ids("proj-123", Some("managed-456"));

        let json = serde_json::to_string(&with_ids).unwrap();
        let restored: TokenInfo = serde_json::from_str(&json).unwrap();

        let (base, project, managed) = restored.parse_refresh_parts();
        assert_eq!(base, "refresh");
        assert_eq!(project.as_deref(), Some("proj-123"));
        assert_eq!(managed.as_deref(), Some("managed-456"));
    }

    #[test]
    fn test_with_new_access_token() {
        let token = TokenInfo::new(
            "old_access".into(),
            Some("refresh|proj|managed".into()),
            Some(3600),
            "claude",
        );
        let updated = token.with_new_access_token("new_access".into(), Some(7200));
        assert_eq!(updated.access_token, "new_access");
        assert_eq!(
            updated.refresh_token.as_deref(),
            Some("refresh|proj|managed")
        );
        assert!(!updated.is_expired());
    }

    #[test]
    fn test_needs_refresh() {
        let fresh = TokenInfo::new("access".into(), Some("refresh".into()), Some(3600), "claude");
        assert!(!fresh.needs_refresh());

        let soon = TokenInfo::with_expires_at(
            "access".into(),
            Some("refresh".into()),
            Some(chrono::Utc::now().timestamp() + 240),
            "claude",
        );
        assert!(soon.needs_refresh());

        let later = TokenInfo::with_expires_at(
            "access".into(),
            Some("refresh".into()),
            Some(chrono::Utc::now().timestamp() + 360),
            "claude",
        );
        assert!(!later.needs_refresh());

        let expired = TokenInfo::with_expires_at("access".into(), Some("refresh".into()), Some(0), "claude");
        assert!(expired.needs_refresh());
    }

    #[test]
    fn test_serialization() {
        let token = TokenInfo::new("access".into(), Some("refresh".into()), Some(3600), "claude");
        let json = serde_json::to_string_pretty(&token).unwrap();
        assert!(json.contains("\"access_token\""));
        assert!(json.contains("\"refresh_token\""));
        assert!(json.contains("\"expires_at\""));
        assert!(json.contains("\"provider\""));

        let restored: TokenInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.access_token, "access");
    }
}
