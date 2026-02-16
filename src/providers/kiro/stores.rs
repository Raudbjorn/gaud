use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use tracing::debug;

use crate::providers::ProviderError;
use super::models::{CredentialSource, KiroTokenInfo};

#[async_trait]
pub trait CredentialStore: Send + Sync {
    /// Load credentials from this store
    async fn load(&self) -> Result<Option<KiroTokenInfo>, ProviderError>;

    /// Save updated credentials to this store
    async fn save(&self, info: &KiroTokenInfo) -> Result<(), ProviderError>;

    /// Check if this store handles the given source
    fn can_handle(&self, source: &CredentialSource) -> bool;
}

pub struct JsonFileStore {
    path: PathBuf,
}

impl JsonFileStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[async_trait]
impl CredentialStore for JsonFileStore {
    fn can_handle(&self, source: &CredentialSource) -> bool {
        match source {
            CredentialSource::JsonFile(p) => p == &self.path,
            _ => false,
        }
    }

    async fn load(&self) -> Result<Option<KiroTokenInfo>, ProviderError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            if !path.exists() {
                return Ok(None);
            }

            let content = std::fs::read_to_string(&path)
                .map_err(|e| ProviderError::Other(format!("Failed to read JSON creds {}: {e}", path.display())))?;
            let data: serde_json::Value = serde_json::from_str(&content)
                .map_err(|e| ProviderError::Other(format!("Failed to parse JSON creds {}: {e}", path.display())))?;

            let mut token = KiroTokenInfo::new(String::new(), CredentialSource::JsonFile(path.clone()));

            if let Some(v) = data.get("refreshToken").and_then(|v| v.as_str()) { token.refresh_token = v.to_string(); }
            if let Some(v) = data.get("accessToken").and_then(|v| v.as_str()) { token.access_token = v.to_string(); }
            if let Some(v) = data.get("region").and_then(|v| v.as_str()) { token.region = v.to_string(); }
            if let Some(v) = data.get("profileArn").and_then(|v| v.as_str()) { token.profile_arn = Some(v.to_string()); }
            if let Some(v) = data.get("clientId").and_then(|v| v.as_str()) { token.client_id = Some(v.to_string()); }
            if let Some(v) = data.get("clientSecret").and_then(|v| v.as_str()) { token.client_secret = Some(v.to_string()); }

            if let Some(hash) = data.get("clientIdHash").and_then(|v| v.as_str()) {
                super::auth::load_enterprise_device_registration(&mut token, hash);
            }

            if let Some(v) = data.get("expiresAt").and_then(|v| v.as_str()) {
                if let Ok(dt) = DateTime::parse_from_rfc3339(v) {
                    token.expires_at = dt.timestamp();
                } else if let Ok(dt) = v.replace('Z', "+00:00").parse::<DateTime<chrono::FixedOffset>>() {
                    token.expires_at = dt.timestamp();
                }
            }

            if token.refresh_token.is_empty() && token.access_token.is_empty() {
                return Ok(None);
            }

            token.detect_auth_type();
            Ok(Some(token))
        })
        .await
        .map_err(|e| ProviderError::Other(format!("spawn_blocking join error: {e}")))?
    }

    async fn save(&self, info: &KiroTokenInfo) -> Result<(), ProviderError> {
        debug!(path = %self.path.display(), "Persisting updated token to JSON file");

        let path = self.path.clone();
        let info = info.clone();
        tokio::task::spawn_blocking(move || {
            let mut data: serde_json::Value = if path.exists() {
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
            } else {
                serde_json::json!({})
            };

            data["accessToken"] = serde_json::json!(info.access_token);
            data["refreshToken"] = serde_json::json!(info.refresh_token);
            data["expiresAt"] = serde_json::json!(DateTime::from_timestamp(info.expires_at, 0).unwrap_or_else(|| Utc::now()).to_rfc3339());

            let content = serde_json::to_string_pretty(&data).map_err(|e| ProviderError::Other(e.to_string()))?;
            std::fs::write(&path, content).map_err(|e| ProviderError::Other(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| ProviderError::Other(format!("spawn_blocking join error: {e}")))?
    }
}

pub struct SqliteStore {
    path: PathBuf,
}

impl SqliteStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[async_trait]
impl CredentialStore for SqliteStore {
    fn can_handle(&self, source: &CredentialSource) -> bool {
        match source {
            CredentialSource::SqliteDb { path, .. } => path == &self.path,
            _ => false,
        }
    }

    async fn load(&self) -> Result<Option<KiroTokenInfo>, ProviderError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            if !path.exists() {
                return Ok(None);
            }

            let conn = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| ProviderError::Other(format!("Failed to open SQLite {}: {e}", path.display())))?;

            let mut info = KiroTokenInfo::new(String::new(), CredentialSource::Auto);
            let mut found = false;

            let keys = [
                "kirocli:social:token",
                "kirocli:odic:token",
                "codewhisperer:odic:token",
                "auth_token", "aws_sso_token", "builder_id_token"
            ];

            for key in keys {
                let res: Result<String, _> = conn.query_row("SELECT value FROM auth_kv WHERE key = ?", [key], |r| r.get(0));
                if let Ok(json_str) = res {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&json_str) {
                        if let Some(v) = data.get("access_token").and_then(|v| v.as_str()) { info.access_token = v.to_string(); }
                        if let Some(v) = data.get("refresh_token").and_then(|v| v.as_str()) { info.refresh_token = v.to_string(); }
                        if let Some(v) = data.get("region").and_then(|v| v.as_str()) { info.sso_region = Some(v.to_string()); }
                        if let Some(v) = data.get("expires_at").and_then(|v| v.as_str()) {
                            let normalized = v.replace('Z', "+00:00");
                            if let Ok(dt) = DateTime::parse_from_rfc3339(&normalized) { info.expires_at = dt.timestamp(); }
                        }
                        info.source = CredentialSource::SqliteDb {
                            path: path.clone(),
                            key: key.to_string(),
                            reg_key: None,
                        };
                        found = true;
                        break;
                    }
                }
            }

            if !found {
                return Ok(None);
            }

            let reg_keys = [
                "kirocli:odic:device-registration",
                "codewhisperer:odic:device-registration",
                "auth_registration", "aws_sso_registration"
            ];
            for key in reg_keys {
                let res: Result<String, _> = conn.query_row("SELECT value FROM auth_kv WHERE key = ?", [key], |r| r.get(0));
                if let Ok(json_str) = res {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&json_str) {
                        if let Some(v) = data.get("client_id").and_then(|v| v.as_str()) { info.client_id = Some(v.to_string()); }
                        if let Some(v) = data.get("client_secret").and_then(|v| v.as_str()) { info.client_secret = Some(v.to_string()); }
                        if info.sso_region.is_none() {
                            if let Some(v) = data.get("region").and_then(|v| v.as_str()) { info.sso_region = Some(v.to_string()); }
                        }
                        if let CredentialSource::SqliteDb { ref mut reg_key, .. } = info.source {
                            *reg_key = Some(key.to_string());
                        }
                        break;
                    }
                }
            }

            info.detect_auth_type();
            Ok(Some(info))
        })
        .await
        .map_err(|e| ProviderError::Other(format!("spawn_blocking join error: {e}")))?
    }

    async fn save(&self, info: &KiroTokenInfo) -> Result<(), ProviderError> {
        let info = info.clone();
        tokio::task::spawn_blocking(move || {
            if let CredentialSource::SqliteDb { ref path, ref key, .. } = info.source {
                debug!(path = %path.display(), key = %key, "Persisting updated token to SQLite DB");
                let conn = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)
                    .map_err(|e| ProviderError::Other(format!("Failed to open SQLite for write: {e}")))?;

                let _ = conn.busy_timeout(std::time::Duration::from_secs(5));

                let mut data = serde_json::json!({
                    "access_token": info.access_token,
                    "refresh_token": info.refresh_token,
                    "expires_at": DateTime::from_timestamp(info.expires_at, 0).unwrap_or_else(|| Utc::now()).to_rfc3339(),
                });

                if let Some(ref r) = info.sso_region { data["region"] = serde_json::json!(r); }
                if let Some(ref s) = info.scopes { data["scopes"] = serde_json::json!(s); }

                let val_str = serde_json::to_string(&data).map_err(|e| ProviderError::Other(e.to_string()))?;

                conn.execute(
                    "UPDATE auth_kv SET value = ?1 WHERE key = ?2",
                    [val_str, key.clone()],
                ).map_err(|e| ProviderError::Other(format!("Failed to update SQLite: {e}")))?;

                Ok(())
            } else {
                Err(ProviderError::Other("Attempted to save non-SQLite credentials to SqliteStore".into()))
            }
        })
        .await
        .map_err(|e| ProviderError::Other(format!("spawn_blocking join error: {e}")))?
    }
}

#[derive(Default)]
pub struct EnvStore {
    region: String,
}

impl EnvStore {
    pub fn new(region: String) -> Self {
        Self { region }
    }
}

#[async_trait]
impl CredentialStore for EnvStore {
    fn can_handle(&self, source: &CredentialSource) -> bool {
        matches!(source, CredentialSource::Environment)
    }

    async fn load(&self) -> Result<Option<KiroTokenInfo>, ProviderError> {
        let rt = match std::env::var("GAUD_KIRO_REFRESH_TOKEN").ok().or_else(|| std::env::var("KIRO_REFRESH_TOKEN").ok()) {
            Some(t) => t,
            None => return Ok(None),
        };

        let mut token = KiroTokenInfo::new(rt, CredentialSource::Environment);
        token.region = std::env::var("GAUD_KIRO_REGION").ok().unwrap_or_else(|| self.region.clone());
        token.profile_arn = std::env::var("GAUD_KIRO_PROFILE_ARN").ok();
        token.detect_auth_type();

        Ok(Some(token))
    }

    async fn save(&self, _info: &KiroTokenInfo) -> Result<(), ProviderError> {
        // Environment variables are read-only for persistence
        Ok(())
    }
}
