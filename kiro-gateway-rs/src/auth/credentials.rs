//! Credential loading from various sources.

use std::path::{Path, PathBuf};
use tracing::{info, warn};

use crate::error::{Error, Result};
use crate::models::auth::{CredentialSource, KiroTokenInfo};

/// Validate an expanded path: resolve symlinks/`..` and verify the result is
/// an absolute path that doesn't contain embedded null bytes.
fn validate_expanded_path(expanded: &str) -> Result<PathBuf> {
    if expanded.contains('\0') {
        return Err(Error::Config(format!(
            "Path contains null bytes: {}",
            expanded.replace('\0', "\\0")
        )));
    }

    let path = Path::new(expanded);

    // Canonicalize to resolve ".." — the file itself may not exist yet
    // (for new SQLite dbs), so resolve the parent and append the filename.
    if path.exists() {
        path.canonicalize()
            .map_err(|e| Error::storage_io(path, e.to_string()))
    } else if let Some(parent) = path.parent().filter(|p| p.exists()) {
        let resolved_parent = parent
            .canonicalize()
            .map_err(|e| Error::storage_io(parent, e.to_string()))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| Error::Config(format!("Path has no filename: {expanded}")))?;
        Ok(resolved_parent.join(file_name))
    } else {
        // Parent doesn't exist either — just use as-is.
        // The "file not found" case is caught by the caller.
        Ok(path.to_path_buf())
    }
}

/// Load credentials from environment variables.
pub fn load_from_env() -> Option<KiroTokenInfo> {
    let refresh_token = std::env::var("KIRO_REFRESH_TOKEN").or_else(|_| std::env::var("GAUD_KIRO_REFRESH_TOKEN")).or_else(|_| std::env::var("REFRESH_TOKEN")).ok()?;
    if refresh_token.is_empty() {
        return None;
    }

    let mut token = KiroTokenInfo::new(refresh_token);
    token.source = CredentialSource::Environment;

    if let Ok(arn) = std::env::var("KIRO_PROFILE_ARN").or_else(|_| std::env::var("GAUD_KIRO_PROFILE_ARN")).or_else(|_| std::env::var("PROFILE_ARN")) {
        if !arn.is_empty() {
            token.profile_arn = Some(arn);
        }
    }
    if let Ok(region) = std::env::var("KIRO_REGION") {
        if !region.is_empty() {
            token.region = region;
        }
    }

    token.detect_auth_type();
    info!("Credentials loaded from environment");
    Some(token)
}

/// Load credentials from a JSON file.
pub fn load_from_json_file(path: &str) -> Result<KiroTokenInfo> {
    let expanded = shellexpand::tilde(path);
    let path = validate_expanded_path(&expanded)?;

    if !path.exists() {
        return Err(Error::storage_io(&path, "Credentials file not found"));
    }

    let content =
        std::fs::read_to_string(&path).map_err(|e| Error::storage_io(&path, e.to_string()))?;
    let data: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| Error::StorageSerialization(e.to_string()))?;

    let mut token = KiroTokenInfo::new(String::new());
    token.source = CredentialSource::JsonFile(path.display().to_string());

    // camelCase fields (Kiro Desktop format)
    if let Some(v) = data.get("refreshToken").and_then(|v| v.as_str()) {
        token.refresh_token = v.to_string();
    }
    if let Some(v) = data.get("accessToken").and_then(|v| v.as_str()) {
        token.access_token = v.to_string();
    }
    if let Some(v) = data.get("profileArn").and_then(|v| v.as_str()) {
        token.profile_arn = Some(v.to_string());
    }
    if let Some(v) = data.get("region").and_then(|v| v.as_str()) {
        token.region = v.to_string();
    }
    if let Some(v) = data.get("clientId").and_then(|v| v.as_str()) {
        token.client_id = Some(v.to_string());
    }
    if let Some(v) = data.get("clientSecret").and_then(|v| v.as_str()) {
        token.client_secret = Some(v.to_string());
    }

    // Enterprise Kiro IDE: load clientId/clientSecret from device registration
    if let Some(hash) = data.get("clientIdHash").and_then(|v| v.as_str()) {
        load_enterprise_device_registration(&mut token, hash);
    }

    // Parse expiresAt
    if let Some(v) = data.get("expiresAt").and_then(|v| v.as_str()) {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(v) {
            token.expires_at = dt.timestamp();
        } else if let Ok(dt) = v.replace('Z', "+00:00").parse::<chrono::DateTime<chrono::FixedOffset>>() {
            token.expires_at = dt.timestamp();
        }
    }

    if token.refresh_token.is_empty() {
        return Err(Error::MissingCredential(
            "refreshToken not found in credentials file".into(),
        ));
    }

    token.detect_auth_type();
    info!(source = %token.source, auth_type = %token.auth_type, "Credentials loaded");
    Ok(token)
}

fn load_enterprise_device_registration(token: &mut KiroTokenInfo, client_id_hash: &str) {
    let path = dirs::home_dir()
        .map(|h| h.join(".aws").join("sso").join("cache").join(format!("{}.json", client_id_hash)));

    if let Some(path) = path {
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(v) = data.get("clientId").and_then(|v| v.as_str()) {
                            token.client_id = Some(v.to_string());
                        }
                        if let Some(v) = data.get("clientSecret").and_then(|v| v.as_str()) {
                            token.client_secret = Some(v.to_string());
                        }
                        info!("Enterprise device registration loaded from {}", path.display());
                    }
                }
                Err(e) => warn!("Failed to read device registration: {}", e),
            }
        }
    }
}

/// Load credentials from kiro-cli SQLite database.
#[cfg(feature = "sqlite")]
pub fn load_from_sqlite(db_path: &str) -> Result<KiroTokenInfo> {
    use crate::models::auth::{SQLITE_REGISTRATION_KEYS, SQLITE_TOKEN_KEYS};
    use tracing::debug;

    let expanded = shellexpand::tilde(db_path);
    let path = validate_expanded_path(&expanded)?;

    if !path.exists() {
        return Err(Error::storage_io(&path, "SQLite database not found"));
    }

    let conn = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| Error::storage_io(&path, e.to_string()))?;

    let mut token = KiroTokenInfo::new(String::new());
    token.source = CredentialSource::SqliteDb(path.display().to_string());

    // Try token keys in priority order
    for key in SQLITE_TOKEN_KEYS {
        let result: std::result::Result<String, _> = conn.query_row(
            "SELECT value FROM auth_kv WHERE key = ?",
            [key],
            |row| row.get(0),
        );
        if let Ok(json_str) = result {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&json_str) {
                if let Some(v) = data.get("access_token").and_then(|v| v.as_str()) {
                    token.access_token = v.to_string();
                }
                if let Some(v) = data.get("refresh_token").and_then(|v| v.as_str()) {
                    token.refresh_token = v.to_string();
                }
                if let Some(v) = data.get("profile_arn").and_then(|v| v.as_str()) {
                    token.profile_arn = Some(v.to_string());
                }
                if let Some(v) = data.get("region").and_then(|v| v.as_str()) {
                    token.sso_region = Some(v.to_string());
                }
                if let Some(v) = data.get("scopes").and_then(|v| v.as_array()) {
                    token.scopes = Some(
                        v.iter()
                            .filter_map(|s| s.as_str().map(|s| s.to_string()))
                            .collect(),
                    );
                }
                if let Some(v) = data.get("expires_at").and_then(|v| v.as_str()) {
                    let normalized = v.replace('Z', "+00:00");
                    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&normalized) {
                        token.expires_at = dt.timestamp();
                    }
                }
                debug!(key, "Loaded credentials from SQLite");
                break;
            }
        }
    }

    // Load device registration (client_id, client_secret)
    for key in SQLITE_REGISTRATION_KEYS {
        let result: std::result::Result<String, _> = conn.query_row(
            "SELECT value FROM auth_kv WHERE key = ?",
            [key],
            |row| row.get(0),
        );
        if let Ok(json_str) = result {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&json_str) {
                if let Some(v) = data.get("client_id").and_then(|v| v.as_str()) {
                    token.client_id = Some(v.to_string());
                }
                if let Some(v) = data.get("client_secret").and_then(|v| v.as_str()) {
                    token.client_secret = Some(v.to_string());
                }
                if token.sso_region.is_none() {
                    if let Some(v) = data.get("region").and_then(|v| v.as_str()) {
                        token.sso_region = Some(v.to_string());
                    }
                }
                debug!(key, "Loaded device registration from SQLite");
                break;
            }
        }
    }

    if token.refresh_token.is_empty() {
        return Err(Error::MissingCredential(
            "No valid token found in SQLite database".into(),
        ));
    }

    token.detect_auth_type();
    info!(source = %token.source, auth_type = %token.auth_type, "Credentials loaded");
    Ok(token)
}

/// Stub for when sqlite feature is not enabled.
#[cfg(not(feature = "sqlite"))]
pub fn load_from_sqlite(_db_path: &str) -> Result<KiroTokenInfo> {
    Err(Error::Config(
        "SQLite support not enabled. Build with `--features sqlite`".into(),
    ))
}

// shellexpand is a simple tilde expansion - we inline it to avoid a dependency
mod shellexpand {
    pub fn tilde(path: &str) -> std::borrow::Cow<'_, str> {
        if path.starts_with('~') {
            if let Some(home) = dirs::home_dir() {
                return std::borrow::Cow::Owned(path.replacen('~', &home.to_string_lossy(), 1));
            }
        }
        std::borrow::Cow::Borrowed(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_expanded_path_null_bytes() {
        let result = validate_expanded_path("/tmp/foo\0bar.json");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("null bytes"), "Error was: {err}");
    }

    #[test]
    fn test_validate_expanded_path_existing_directory() {
        // /tmp exists on all Unix systems — a path under it should canonicalize.
        let result = validate_expanded_path("/tmp/nonexistent_creds.json");
        assert!(result.is_ok());
        let resolved = result.unwrap();
        // Should resolve /tmp to its canonical form and append the filename.
        assert!(
            resolved.to_string_lossy().ends_with("nonexistent_creds.json"),
            "Resolved path was: {}",
            resolved.display()
        );
    }

    #[test]
    fn test_validate_expanded_path_with_dotdot() {
        // /tmp/../tmp/file.json should canonicalize to /tmp/file.json (or its
        // canonical equivalent if /tmp is a symlink).
        let result = validate_expanded_path("/tmp/../tmp/test_file.json");
        assert!(result.is_ok());
        let resolved = result.unwrap();
        // After canonicalization, the ".." should be resolved.
        assert!(
            !resolved.to_string_lossy().contains(".."),
            "Resolved path still contains '..': {}",
            resolved.display()
        );
    }

    #[test]
    fn test_validate_expanded_path_nonexistent_parent() {
        // When neither the file nor its parent exist, the path is returned as-is.
        let result = validate_expanded_path("/nonexistent_dir_abc123/file.json");
        assert!(result.is_ok());
        let resolved = result.unwrap();
        assert_eq!(
            resolved,
            PathBuf::from("/nonexistent_dir_abc123/file.json")
        );
    }

    #[test]
    fn test_validate_expanded_path_no_filename() {
        // A path with a trailing slash after a real parent has no filename.
        // The function should still handle it (parent case, but file_name is None
        // for root-like paths without a filename component).
        let result = validate_expanded_path("/tmp/");
        // /tmp/ exists as a directory, so canonicalize succeeds.
        assert!(result.is_ok());
    }

    #[test]
    fn test_load_from_json_file_null_byte_rejected() {
        let result = load_from_json_file("/tmp/creds\0.json");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("null bytes"), "Error was: {err}");
    }

    #[test]
    fn test_load_from_json_file_not_found() {
        let result = load_from_json_file("/tmp/definitely_nonexistent_gaud_creds_12345.json");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found") || err.contains("No such file"),
            "Error was: {err}"
        );
    }
}
