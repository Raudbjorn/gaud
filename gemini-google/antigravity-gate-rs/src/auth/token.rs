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
//!
//! This allows storing all authentication state in a single token file
//! without requiring separate project ID storage.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::instrument;

/// Separator used in composite refresh token format.
const COMPOSITE_SEPARATOR: char = '|';

/// Safety margin for token expiry checks (60 seconds).
///
/// Tokens are considered expired this many seconds before their actual expiry
/// to account for clock skew and network latency.
const EXPIRY_SAFETY_MARGIN_SECS: i64 = 60;

/// OAuth token information with composite project ID storage.
///
/// Stores access token, refresh token (with optional embedded project IDs),
/// and expiration timestamp. The composite format allows encoding project
/// discovery results in the refresh token field for efficient storage.
///
/// # Example
///
/// ```
/// use antigravity_gate::auth::TokenInfo;
///
/// // Create a new token
/// let token = TokenInfo::new(
///     "access_token_here".to_string(),
///     "refresh_token_here".to_string(),
///     3600, // expires in 1 hour
/// );
///
/// // Add project IDs using composite format
/// let token = token.with_project_ids("proj-123", Some("managed-456"));
///
/// // Parse the project IDs back out
/// let (refresh, project_id, managed_id) = token.parse_refresh_parts();
/// assert_eq!(project_id.as_deref(), Some("proj-123"));
/// assert_eq!(managed_id.as_deref(), Some("managed-456"));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenInfo {
    /// Token type, typically "oauth" or "Bearer".
    pub token_type: String,

    /// OAuth access token for API requests.
    ///
    /// This token expires and must be refreshed using the refresh token.
    pub access_token: String,

    /// OAuth refresh token, potentially in composite format.
    ///
    /// May contain embedded project IDs: `refresh|project_id|managed_project_id`
    pub refresh_token: String,

    /// Unix timestamp when the access token expires.
    pub expires_at: i64,
}

impl TokenInfo {
    /// Create a new TokenInfo with the given tokens and expiry duration.
    ///
    /// # Arguments
    ///
    /// * `access_token` - The OAuth access token for API requests
    /// * `refresh_token` - The OAuth refresh token for obtaining new access tokens
    /// * `expires_in` - Duration in seconds until the access token expires
    ///
    /// # Example
    ///
    /// ```
    /// use antigravity_gate::auth::TokenInfo;
    ///
    /// let token = TokenInfo::new(
    ///     "ya29.access".to_string(),
    ///     "1//refresh".to_string(),
    ///     3600,
    /// );
    /// assert!(!token.is_expired());
    /// ```
    pub fn new(access_token: String, refresh_token: String, expires_in: i64) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            token_type: "oauth".to_string(),
            access_token,
            refresh_token,
            expires_at: now + expires_in,
        }
    }

    /// Create a TokenInfo with a specific expiration timestamp.
    ///
    /// Useful for deserializing tokens from storage.
    pub fn with_expires_at(access_token: String, refresh_token: String, expires_at: i64) -> Self {
        Self {
            token_type: "oauth".to_string(),
            access_token,
            refresh_token,
            expires_at,
        }
    }

    /// Check if the access token is expired or about to expire.
    ///
    /// Returns `true` if the token has expired or will expire within
    /// the safety margin (60 seconds), indicating a refresh is needed.
    ///
    /// # Example
    ///
    /// ```
    /// use antigravity_gate::auth::TokenInfo;
    ///
    /// let expired = TokenInfo::with_expires_at(
    ///     "access".to_string(),
    ///     "refresh".to_string(),
    ///     0, // Unix epoch = definitely expired
    /// );
    /// assert!(expired.is_expired());
    /// ```
    pub fn is_expired(&self) -> bool {
        let now = chrono::Utc::now().timestamp();
        self.expires_at <= now + EXPIRY_SAFETY_MARGIN_SECS
    }

    /// Get the duration until the access token expires.
    ///
    /// Returns `Duration::ZERO` if the token has already expired.
    ///
    /// # Example
    ///
    /// ```
    /// use antigravity_gate::auth::TokenInfo;
    /// use std::time::Duration;
    ///
    /// let token = TokenInfo::new(
    ///     "access".to_string(),
    ///     "refresh".to_string(),
    ///     3600,
    /// );
    /// let remaining = token.time_until_expiry();
    /// assert!(remaining > Duration::from_secs(3500));
    /// ```
    pub fn time_until_expiry(&self) -> Duration {
        let now = chrono::Utc::now().timestamp();
        let remaining = self.expires_at - now;
        if remaining > 0 {
            Duration::from_secs(remaining as u64)
        } else {
            Duration::ZERO
        }
    }

    /// Parse the composite refresh token into its parts.
    ///
    /// The composite format is: `refresh_token|project_id|managed_project_id`
    ///
    /// Returns a tuple of:
    /// - The base refresh token (always present)
    /// - Optional project ID
    /// - Optional managed project ID
    ///
    /// # Example
    ///
    /// ```
    /// use antigravity_gate::auth::TokenInfo;
    ///
    /// // Simple token without project IDs
    /// let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
    /// let (base, project, managed) = token.parse_refresh_parts();
    /// assert_eq!(base, "refresh");
    /// assert!(project.is_none());
    /// assert!(managed.is_none());
    ///
    /// // Token with embedded project IDs
    /// let token = token.with_project_ids("proj-123", Some("managed-456"));
    /// let (base, project, managed) = token.parse_refresh_parts();
    /// assert_eq!(base, "refresh");
    /// assert_eq!(project.as_deref(), Some("proj-123"));
    /// assert_eq!(managed.as_deref(), Some("managed-456"));
    /// ```
    #[instrument(skip(self))]
    pub fn parse_refresh_parts(&self) -> (String, Option<String>, Option<String>) {
        let parts: Vec<&str> = self.refresh_token.split(COMPOSITE_SEPARATOR).collect();

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
    ///
    /// This is the token to use when refreshing access tokens.
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
    ///
    /// Encodes the project IDs in composite format for single-file storage.
    /// Preserves the original access token and expiry.
    ///
    /// # Arguments
    ///
    /// * `project_id` - The Cloud Code project ID
    /// * `managed_project_id` - Optional managed project ID
    ///
    /// # Example
    ///
    /// ```
    /// use antigravity_gate::auth::TokenInfo;
    ///
    /// let token = TokenInfo::new("access".into(), "refresh".into(), 3600)
    ///     .with_project_ids("proj-123", Some("managed-456"));
    ///
    /// assert!(token.refresh_token.contains("proj-123"));
    /// assert!(token.refresh_token.contains("managed-456"));
    /// ```
    pub fn with_project_ids(self, project_id: &str, managed_project_id: Option<&str>) -> Self {
        // Extract the base refresh token (in case it already has embedded IDs)
        let base_refresh = self.base_refresh_token();

        // Build composite token
        let composite = match managed_project_id {
            Some(managed) => format!(
                "{}{}{}{}{}",
                base_refresh, COMPOSITE_SEPARATOR, project_id, COMPOSITE_SEPARATOR, managed
            ),
            None => format!("{}{}{}", base_refresh, COMPOSITE_SEPARATOR, project_id),
        };

        Self {
            refresh_token: composite,
            ..self
        }
    }

    /// Update the access token and expiry while preserving other fields.
    ///
    /// Used when refreshing the access token while keeping the same
    /// refresh token and embedded project IDs.
    pub fn with_new_access_token(self, access_token: String, expires_in: i64) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            access_token,
            expires_at: now + expires_in,
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_token() {
        let token = TokenInfo::new("access".to_string(), "refresh".to_string(), 3600);

        assert_eq!(token.token_type, "oauth");
        assert_eq!(token.access_token, "access");
        assert_eq!(token.refresh_token, "refresh");
        assert!(!token.is_expired());
    }

    #[test]
    fn test_is_expired() {
        // Token expired in the past
        let expired = TokenInfo::with_expires_at("access".to_string(), "refresh".to_string(), 0);
        assert!(expired.is_expired());

        // Token expiring very soon (within safety margin)
        let soon = TokenInfo::with_expires_at(
            "access".to_string(),
            "refresh".to_string(),
            chrono::Utc::now().timestamp() + 30, // 30 seconds from now
        );
        assert!(soon.is_expired()); // Should be expired due to 60s safety margin

        // Token with plenty of time
        let fresh = TokenInfo::new("access".to_string(), "refresh".to_string(), 3600);
        assert!(!fresh.is_expired());
    }

    #[test]
    fn test_time_until_expiry() {
        let token = TokenInfo::new("access".to_string(), "refresh".to_string(), 3600);
        let remaining = token.time_until_expiry();

        // Should be close to 3600 seconds
        assert!(remaining.as_secs() >= 3595);
        assert!(remaining.as_secs() <= 3600);

        // Expired token returns zero
        let expired = TokenInfo::with_expires_at("access".into(), "refresh".into(), 0);
        assert_eq!(expired.time_until_expiry(), Duration::ZERO);
    }

    #[test]
    fn test_parse_refresh_parts_simple() {
        let token = TokenInfo::new("access".into(), "refresh_token".into(), 3600);
        let (base, project, managed) = token.parse_refresh_parts();

        assert_eq!(base, "refresh_token");
        assert!(project.is_none());
        assert!(managed.is_none());
    }

    #[test]
    fn test_parse_refresh_parts_with_project() {
        let token = TokenInfo::new("access".into(), "refresh|proj-123".into(), 3600);
        let (base, project, managed) = token.parse_refresh_parts();

        assert_eq!(base, "refresh");
        assert_eq!(project.as_deref(), Some("proj-123"));
        assert!(managed.is_none());
    }

    #[test]
    fn test_parse_refresh_parts_with_both() {
        let token = TokenInfo::new("access".into(), "refresh|proj-123|managed-456".into(), 3600);
        let (base, project, managed) = token.parse_refresh_parts();

        assert_eq!(base, "refresh");
        assert_eq!(project.as_deref(), Some("proj-123"));
        assert_eq!(managed.as_deref(), Some("managed-456"));
    }

    #[test]
    fn test_parse_refresh_parts_with_empty_parts() {
        // Empty project ID
        let token = TokenInfo::new("access".into(), "refresh||managed-456".into(), 3600);
        let (base, project, managed) = token.parse_refresh_parts();
        assert_eq!(base, "refresh");
        assert!(project.is_none());
        assert_eq!(managed.as_deref(), Some("managed-456"));

        // Empty managed project ID
        let token = TokenInfo::new("access".into(), "refresh|proj-123|".into(), 3600);
        let (base, project, managed) = token.parse_refresh_parts();
        assert_eq!(base, "refresh");
        assert_eq!(project.as_deref(), Some("proj-123"));
        assert!(managed.is_none());
    }

    #[test]
    fn test_with_project_ids() {
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        let token = token.with_project_ids("proj-123", Some("managed-456"));

        assert_eq!(token.refresh_token, "refresh|proj-123|managed-456");
        assert_eq!(token.access_token, "access"); // Preserved

        let (base, project, managed) = token.parse_refresh_parts();
        assert_eq!(base, "refresh");
        assert_eq!(project.as_deref(), Some("proj-123"));
        assert_eq!(managed.as_deref(), Some("managed-456"));
    }

    #[test]
    fn test_with_project_ids_no_managed() {
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        let token = token.with_project_ids("proj-123", None);

        assert_eq!(token.refresh_token, "refresh|proj-123");
    }

    #[test]
    fn test_with_project_ids_replaces_existing() {
        // Start with existing project IDs
        let token = TokenInfo::new("access".into(), "refresh|old-proj|old-managed".into(), 3600);
        let token = token.with_project_ids("new-proj", Some("new-managed"));

        let (base, project, managed) = token.parse_refresh_parts();
        assert_eq!(base, "refresh");
        assert_eq!(project.as_deref(), Some("new-proj"));
        assert_eq!(managed.as_deref(), Some("new-managed"));
    }

    #[test]
    fn test_composite_round_trip() {
        let original = TokenInfo::new("access".into(), "refresh".into(), 3600);
        let with_ids = original.with_project_ids("proj-123", Some("managed-456"));

        // Serialize to JSON
        let json = serde_json::to_string(&with_ids).unwrap();

        // Deserialize back
        let restored: TokenInfo = serde_json::from_str(&json).unwrap();

        // Verify round-trip
        let (base, project, managed) = restored.parse_refresh_parts();
        assert_eq!(base, "refresh");
        assert_eq!(project.as_deref(), Some("proj-123"));
        assert_eq!(managed.as_deref(), Some("managed-456"));
    }

    #[test]
    fn test_base_refresh_token() {
        let token = TokenInfo::new("access".into(), "refresh|proj|managed".into(), 3600);
        assert_eq!(token.base_refresh_token(), "refresh");
    }

    #[test]
    fn test_project_id_accessors() {
        let token = TokenInfo::new("access".into(), "refresh|proj-123|managed-456".into(), 3600);
        assert_eq!(token.project_id().as_deref(), Some("proj-123"));
        assert_eq!(token.managed_project_id().as_deref(), Some("managed-456"));
    }

    #[test]
    fn test_with_new_access_token() {
        let token = TokenInfo::new("old_access".into(), "refresh|proj|managed".into(), 3600);
        let updated = token.with_new_access_token("new_access".into(), 7200);

        assert_eq!(updated.access_token, "new_access");
        assert_eq!(updated.refresh_token, "refresh|proj|managed"); // Preserved
        assert!(!updated.is_expired());
    }

    #[test]
    fn test_serialization() {
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        let json = serde_json::to_string_pretty(&token).unwrap();

        assert!(json.contains("\"token_type\""));
        assert!(json.contains("\"access_token\""));
        assert!(json.contains("\"refresh_token\""));
        assert!(json.contains("\"expires_at\""));

        let restored: TokenInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.access_token, "access");
        assert_eq!(restored.refresh_token, "refresh");
    }

    #[test]
    fn test_equality() {
        let token1 = TokenInfo::with_expires_at("access".into(), "refresh".into(), 12345);
        let token2 = TokenInfo::with_expires_at("access".into(), "refresh".into(), 12345);
        let token3 = TokenInfo::with_expires_at("different".into(), "refresh".into(), 12345);

        assert_eq!(token1, token2);
        assert_ne!(token1, token3);
    }
}
