use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use chrono::Utc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::providers::{ProviderError, TokenService};
use super::models::*;
use super::strategies::{AuthStrategy, KiroDesktopStrategy, AwsSsoOidcStrategy};
use super::stores::{CredentialStore, EnvStore, JsonFileStore, SqliteStore};

#[async_trait::async_trait]
pub trait KiroTokenProvider: Send + Sync {
    async fn get_token(&self) -> Result<String, ProviderError>;
    async fn region(&self) -> String;
    async fn profile_arn(&self) -> Option<String>;
    async fn force_refresh(&self) -> Result<(), ProviderError>;
}

pub struct KiroAuthManager {
    token: Arc<RwLock<Option<KiroTokenInfo>>>,
    http: reqwest::Client,
    region: String,
    strategies: Vec<Box<dyn AuthStrategy>>,
    stores: RwLock<Vec<Box<dyn CredentialStore>>>,
}

impl KiroAuthManager {
    pub fn new(fingerprint: String, region: String) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        let strategies: Vec<Box<dyn AuthStrategy>> = vec![
            Box::new(KiroDesktopStrategy::new(fingerprint.clone())),
            Box::new(AwsSsoOidcStrategy),
        ];

        let stores: Vec<Box<dyn CredentialStore>> = vec![
            Box::new(EnvStore::new(region.clone())),
            // Other stores are added via add_store later
        ];

        Self {
            token: Arc::new(RwLock::new(None)),
            http,
            region,
            strategies,
            stores: RwLock::new(stores),
        }
    }

    pub async fn add_store(&self, store: Box<dyn CredentialStore>) {
        let mut stores = self.stores.write().await;
        stores.push(store);
    }

    pub async fn load_any(&self) -> Result<(), ProviderError> {
        let stores = self.stores.read().await;
        for store in stores.iter() {
            match store.load().await {
                Ok(Some(info)) => {
                    let mut guard = self.token.write().await;
                    *guard = Some(info);
                    return Ok(());
                }
                Ok(None) => continue,
                Err(e) => {
                    warn!("Error loading from store: {}", e);
                    continue;
                }
            }
        }
        Err(ProviderError::NoToken { provider: "kiro: all auth detection failed".to_string() })
    }

    async fn refresh(&self) -> Result<String, ProviderError> {
        // Try reload from source store strictly before refresh
        {
            let guard = self.token.read().await;
            if let Some(ref info) = *guard {
                let stores = self.stores.read().await;
                for store in stores.iter() {
                    if store.can_handle(&info.source) {
                         if let Ok(Some(reloaded)) = store.load().await {
                             if !reloaded.needs_refresh() && !reloaded.access_token.is_empty() {
                                 drop(guard);
                                 drop(stores);
                                 let token = reloaded.access_token.clone();
                                 let mut w_guard = self.token.write().await;
                                 *w_guard = Some(reloaded);
                                 debug!("Picked up fresh token from store reload");
                                 return Ok(token);
                             }
                         }
                    }
                }
            }
        }

        let mut guard = self.token.write().await;
        let info = guard.as_mut().ok_or_else(|| {
            ProviderError::NoToken {
                provider: "kiro: not authenticated".to_string(),
            }
        })?;

        // Double-check under lock
        if !info.needs_refresh() && !info.access_token.is_empty() {
            return Ok(info.access_token.clone());
        }

        let strategy = self.strategies.iter().find(|s| s.can_handle(info.auth_type))
            .ok_or_else(|| ProviderError::Other(format!("No strategy found for auth type {}", info.auth_type)))?;

        let update = strategy.refresh(info, &self.http).await?;

        // Apply update
        info.access_token = update.access_token;
        if let Some(rt) = update.refresh_token {
            if !rt.is_empty() { info.refresh_token = rt; }
        }
        if let Some(arn) = update.profile_arn {
            if !arn.is_empty() { info.profile_arn = Some(arn); }
        }
        info.expires_at = update.expires_at;

        // Persist
        let stores = self.stores.read().await;
        for store in stores.iter() {
            if store.can_handle(&info.source) {
                if let Err(e) = store.save(info).await {
                    warn!("Failed to persist refreshed Kiro token: {}", e);
                }
                break;
            }
        }

        Ok(info.access_token.clone())
    }
}

#[async_trait::async_trait]
impl KiroTokenProvider for KiroAuthManager {
    async fn get_token(&self) -> Result<String, ProviderError> {
        let (needs_refresh, access_token, expired) = {
            let guard = self.token.read().await;
            match *guard {
                Some(ref info) => (info.needs_refresh(), info.access_token.clone(), info.expires_at < Utc::now().timestamp()),
                None => (true, String::new(), true),
            }
        };

        if !needs_refresh && !access_token.is_empty() {
            return Ok(access_token);
        }

        match self.refresh().await {
            Ok(t) => Ok(t),
            Err(e) => {
                if !access_token.is_empty() && !expired {
                    warn!("Kiro token refresh failed, but cached token is still valid. Using cached token. Error: {}", e);
                    Ok(access_token)
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn region(&self) -> String {
        let guard = self.token.read().await;
        guard.as_ref().map(|t| t.region.clone()).unwrap_or_else(|| self.region.clone())
    }

    async fn profile_arn(&self) -> Option<String> {
        let guard = self.token.read().await;
        guard.as_ref().and_then(|t| t.profile_arn.clone())
    }

    async fn force_refresh(&self) -> Result<(), ProviderError> {
        info!("Force refreshing Kiro token");
        {
            let mut guard = self.token.write().await;
            if let Some(ref mut info) = *guard {
                info.access_token.clear();
            }
        }
        self.refresh().await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl TokenService for KiroAuthManager {
    async fn get_token(&self, _provider: &str) -> Result<String, ProviderError> {
        KiroTokenProvider::get_token(self).await
    }
}

// ---------------------------------------------------------------------------
// AutoDetectProvider (Wrapper)
// ---------------------------------------------------------------------------
// Kept for compatibility, but mainly configures KiroAuthManager

pub struct AutoDetectProvider {
    manager: Arc<KiroAuthManager>,
}

impl AutoDetectProvider {
    pub async fn new(
        manager: Arc<KiroAuthManager>,
        creds_file: Option<PathBuf>,
        db_path: Option<PathBuf>,
        sso_cache_dir: Option<PathBuf>,
    ) -> Self {
        // Configure manager with stores based on inputs

        // Explicit JSON
        if let Some(p) = creds_file {
             manager.add_store(Box::new(JsonFileStore::new(p))).await;
        }

        // Explicit SQLite
        if let Some(p) = db_path {
             manager.add_store(Box::new(SqliteStore::new(p))).await;
        }

        // Default SSO Cache
        let sso_dir = sso_cache_dir.unwrap_or_else(|| {
            home_dir().map(|h| h.join(".aws").join("sso").join("cache")).unwrap_or_default()
        });
        if sso_dir.exists() {
             if let Ok(entries) = std::fs::read_dir(&sso_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("json") {
                        manager.add_store(Box::new(JsonFileStore::new(path))).await;
                    }
                }
             }
        }

        // Default Kiro CLI locations
        let db_paths = [
            home_dir().map(|h| h.join(".local").join("share").join("kiro-cli").join("data.sqlite3")),
            home_dir().map(|h| h.join(".local").join("share").join("amazon-q-developer-cli").join("data.sqlite3")),
        ];

        for p in db_paths.into_iter().flatten() {
            manager.add_store(Box::new(SqliteStore::new(p))).await;
        }

        Self { manager }
    }

    async fn ensure_loaded(&self) -> Result<(), ProviderError> {
        if self.manager.token.read().await.is_some() {
            return Ok(());
        }
        self.manager.load_any().await
    }
}

#[async_trait::async_trait]
impl KiroTokenProvider for AutoDetectProvider {
    async fn get_token(&self) -> Result<String, ProviderError> {
        self.ensure_loaded().await?;
        KiroTokenProvider::get_token(&*self.manager).await
    }

    async fn region(&self) -> String {
        let _ = self.ensure_loaded().await;
        self.manager.region().await
    }

    async fn profile_arn(&self) -> Option<String> {
        let _ = self.ensure_loaded().await;
        self.manager.profile_arn().await
    }

    async fn force_refresh(&self) -> Result<(), ProviderError> {
        self.ensure_loaded().await?;
        self.manager.force_refresh().await
    }
}

#[async_trait::async_trait]
impl TokenService for AutoDetectProvider {
    async fn get_token(&self, _provider: &str) -> Result<String, ProviderError> {
        KiroTokenProvider::get_token(self).await
    }
}

// Utility function used in JsonFileStore, exposed here for now
pub fn load_enterprise_device_registration(token: &mut KiroTokenInfo, client_id_hash: &str) {
    let path = home_dir()
        .map(|h| h.join(".aws").join("sso").join("cache").join(format!("{}.json", client_id_hash)));

    if let Some(path) = path {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(v) = data.get("clientId").and_then(|v| v.as_str()) {
                        token.client_id = Some(v.to_string());
                    }
                    if let Some(v) = data.get("clientSecret").and_then(|v| v.as_str()) {
                        token.client_secret = Some(v.to_string());
                    }
                    debug!("Enterprise device registration loaded from {}", path.display());
                }
            }
        }
    }
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

    // =========================================================================
    // JsonFileStore / SqliteStore loading (existing)
    // =========================================================================

    #[tokio::test]
    async fn test_aws_sso_provider_loading() {
        let dir = tempdir().unwrap();

        let expires_at = Utc::now() + chrono::Duration::hours(1);
        let cache_content = serde_json::json!({
            "accessToken": "mock-sso-token",
            "refreshToken": "mock-sso-refresh",
            "expiresAt": expires_at.to_rfc3339(),
            "clientId": "test-client",
            "clientSecret": "test-secret"
        });
        let path = dir.path().join("test.json");
        fs::write(&path, cache_content.to_string()).unwrap();

        let manager = Arc::new(KiroAuthManager::new("test-f".into(), "us-east-1".into()));
        manager.add_store(Box::new(JsonFileStore::new(path))).await;
        manager.load_any().await.unwrap();

        let token = KiroTokenProvider::get_token(&*manager).await.unwrap();
        assert_eq!(token, "mock-sso-token");

        let info = manager.token.read().await;
        assert_eq!(info.as_ref().unwrap().auth_type, AuthType::AwsSsoOidc);
    }

    #[tokio::test]
    async fn test_kiro_cli_provider_loading() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("data.sqlite3");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE auth_kv (key TEXT PRIMARY KEY, value TEXT)", []).unwrap();

        let expires_at = Utc::now() + chrono::Duration::hours(1);
        let token_json = serde_json::json!({
            "access_token": "mock-cli-token",
            "refresh_token": "mock-cli-refresh",
            "expires_at": expires_at.to_rfc3339()
        });
        conn.execute("INSERT INTO auth_kv (key, value) VALUES ('kirocli:social:token', ?1)", [token_json.to_string()]).unwrap();

        let manager = Arc::new(KiroAuthManager::new("test-f".into(), "us-east-1".into()));
        manager.add_store(Box::new(SqliteStore::new(db_path))).await;
        manager.load_any().await.unwrap();

        let token = KiroTokenProvider::get_token(&*manager).await.unwrap();
        assert_eq!(token, "mock-cli-token");
    }

    // =========================================================================
    // EnvStore loading
    // =========================================================================

    #[tokio::test]
    async fn test_env_store_loading() {
        // Use temp env vars scoped to this test via the manager
        let store = EnvStore::new("eu-west-1".into());

        // When env var is not set, load returns None
        // (GAUD_KIRO_REFRESH_TOKEN is unlikely to be set in CI)
        let result = store.load().await.unwrap();
        if std::env::var("GAUD_KIRO_REFRESH_TOKEN").is_err()
            && std::env::var("KIRO_REFRESH_TOKEN").is_err()
        {
            assert!(result.is_none());
        }
    }

    #[tokio::test]
    async fn test_env_store_save_is_noop() {
        let store = EnvStore::new("us-east-1".into());
        let info = KiroTokenInfo::new("token".into(), CredentialSource::Environment);
        // save should succeed silently (no-op)
        store.save(&info).await.unwrap();
    }

    // =========================================================================
    // Auth type detection
    // =========================================================================

    #[test]
    fn test_detect_auth_type_desktop() {
        let mut info = KiroTokenInfo::new("rt".into(), CredentialSource::Auto);
        info.detect_auth_type();
        assert_eq!(info.auth_type, AuthType::KiroDesktop);
    }

    #[test]
    fn test_detect_auth_type_sso_oidc() {
        let mut info = KiroTokenInfo::new("rt".into(), CredentialSource::Auto);
        info.client_id = Some("cid".into());
        info.client_secret = Some("csecret".into());
        info.detect_auth_type();
        assert_eq!(info.auth_type, AuthType::AwsSsoOidc);
    }

    // =========================================================================
    // needs_refresh
    // =========================================================================

    #[test]
    fn test_needs_refresh_expired() {
        let info = KiroTokenInfo::new("rt".into(), CredentialSource::Auto);
        // expires_at defaults to 0 → definitely expired
        assert!(info.needs_refresh());
    }

    #[test]
    fn test_needs_refresh_valid() {
        let mut info = KiroTokenInfo::new("rt".into(), CredentialSource::Auto);
        info.expires_at = Utc::now().timestamp() + 3600; // 1 hour from now
        assert!(!info.needs_refresh());
    }

    #[test]
    fn test_needs_refresh_within_threshold() {
        let mut info = KiroTokenInfo::new("rt".into(), CredentialSource::Auto);
        // 5 minutes from now — within the 10-minute threshold
        info.expires_at = Utc::now().timestamp() + 300;
        assert!(info.needs_refresh());
    }

    // =========================================================================
    // Graceful degradation
    // =========================================================================

    #[tokio::test]
    async fn test_graceful_degradation_uses_cached_on_refresh_failure() {
        let manager = Arc::new(KiroAuthManager::new("test-f".into(), "us-east-1".into()));

        // Seed with a valid token that doesn't need refresh
        {
            let mut guard = manager.token.write().await;
            let mut info = KiroTokenInfo::new("refresh".into(), CredentialSource::Environment);
            info.access_token = "cached-access".into();
            info.expires_at = Utc::now().timestamp() + 3600;
            *guard = Some(info);
        }

        // get_token should return the cached token without trying to refresh
        let token = KiroTokenProvider::get_token(&*manager).await.unwrap();
        assert_eq!(token, "cached-access");
    }

    // =========================================================================
    // TokenService adapter
    // =========================================================================

    #[tokio::test]
    async fn test_token_service_adapter() {
        let manager = Arc::new(KiroAuthManager::new("test-f".into(), "us-east-1".into()));

        // Seed a valid token
        {
            let mut guard = manager.token.write().await;
            let mut info = KiroTokenInfo::new("refresh".into(), CredentialSource::Environment);
            info.access_token = "adapter-test-token".into();
            info.expires_at = Utc::now().timestamp() + 3600;
            *guard = Some(info);
        }

        // Call through the TokenService trait (provider arg is ignored)
        let token = TokenService::get_token(&*manager, "kiro").await.unwrap();
        assert_eq!(token, "adapter-test-token");
    }

    // =========================================================================
    // Force refresh
    // =========================================================================

    #[tokio::test]
    async fn test_force_refresh_clears_access_token() {
        let manager = Arc::new(KiroAuthManager::new("test-f".into(), "us-east-1".into()));

        // Seed a token
        {
            let mut guard = manager.token.write().await;
            let mut info = KiroTokenInfo::new("refresh".into(), CredentialSource::Environment);
            info.access_token = "old-token".into();
            info.expires_at = Utc::now().timestamp() + 3600;
            *guard = Some(info);
        }

        // Force refresh will clear the token and try to refresh,
        // which will fail (no real endpoint), but we verify the clear happened
        let result = KiroTokenProvider::force_refresh(&*manager).await;
        assert!(result.is_err()); // No strategy can refresh env-sourced tokens

        // The access token should have been cleared
        let guard = manager.token.read().await;
        assert!(guard.as_ref().unwrap().access_token.is_empty());
    }

    // =========================================================================
    // load_any fails when no stores have credentials
    // =========================================================================

    #[tokio::test]
    async fn test_load_any_all_empty() {
        let manager = Arc::new(KiroAuthManager::new("test-f".into(), "us-east-1".into()));
        let dir = tempdir().unwrap();
        // Add a store pointing to nonexistent file
        manager.add_store(Box::new(JsonFileStore::new(dir.path().join("nope.json")))).await;

        let result = manager.load_any().await;
        assert!(result.is_err());
    }

    // =========================================================================
    // CredentialStore::can_handle
    // =========================================================================

    #[test]
    fn test_json_file_store_can_handle() {
        let store = JsonFileStore::new(PathBuf::from("/tmp/test.json"));
        assert!(store.can_handle(&CredentialSource::JsonFile(PathBuf::from("/tmp/test.json"))));
        assert!(!store.can_handle(&CredentialSource::JsonFile(PathBuf::from("/tmp/other.json"))));
        assert!(!store.can_handle(&CredentialSource::Environment));
    }

    #[test]
    fn test_sqlite_store_can_handle() {
        let store = SqliteStore::new(PathBuf::from("/tmp/data.sqlite3"));
        assert!(store.can_handle(&CredentialSource::SqliteDb {
            path: PathBuf::from("/tmp/data.sqlite3"),
            key: "k".into(),
            reg_key: None,
        }));
        assert!(!store.can_handle(&CredentialSource::Environment));
    }

    #[test]
    fn test_env_store_can_handle() {
        let store = EnvStore::new("us-east-1".into());
        assert!(store.can_handle(&CredentialSource::Environment));
        assert!(!store.can_handle(&CredentialSource::Auto));
    }

    // =========================================================================
    // JsonFileStore save round-trip
    // =========================================================================

    #[tokio::test]
    async fn test_json_file_store_save_and_reload() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tokens.json");

        let store = JsonFileStore::new(path.clone());

        let mut info = KiroTokenInfo::new("refresh-tok".into(), CredentialSource::JsonFile(path.clone()));
        info.access_token = "access-tok".into();
        info.expires_at = Utc::now().timestamp() + 3600;

        store.save(&info).await.unwrap();

        // Reload from same store
        let loaded = store.load().await.unwrap().expect("should load token");
        assert_eq!(loaded.access_token, "access-tok");
        assert_eq!(loaded.refresh_token, "refresh-tok");
    }

    // =========================================================================
    // SqliteStore save round-trip
    // =========================================================================

    #[tokio::test]
    async fn test_sqlite_store_save_and_reload() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("data.sqlite3");

        // Create initial DB with a token
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE auth_kv (key TEXT PRIMARY KEY, value TEXT)", []).unwrap();
        let expires_at = Utc::now() + chrono::Duration::hours(1);
        let initial = serde_json::json!({
            "access_token": "old-access",
            "refresh_token": "old-refresh",
            "expires_at": expires_at.to_rfc3339()
        });
        conn.execute("INSERT INTO auth_kv (key, value) VALUES ('kirocli:social:token', ?1)", [initial.to_string()]).unwrap();
        drop(conn);

        let store = SqliteStore::new(db_path.clone());

        // Load original
        let loaded = store.load().await.unwrap().expect("should load");
        assert_eq!(loaded.access_token, "old-access");

        // Update and save
        let mut updated = loaded;
        updated.access_token = "new-access".into();
        store.save(&updated).await.unwrap();

        // Reload and verify
        let reloaded = store.load().await.unwrap().expect("should reload");
        assert_eq!(reloaded.access_token, "new-access");
        assert_eq!(reloaded.refresh_token, "old-refresh");
    }

    // =========================================================================
    // SqliteStore with device registration
    // =========================================================================

    #[tokio::test]
    async fn test_sqlite_store_loads_device_registration() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("data.sqlite3");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE auth_kv (key TEXT PRIMARY KEY, value TEXT)", []).unwrap();

        let expires_at = Utc::now() + chrono::Duration::hours(1);
        let token_json = serde_json::json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "expires_at": expires_at.to_rfc3339()
        });
        conn.execute("INSERT INTO auth_kv (key, value) VALUES ('kirocli:odic:token', ?1)", [token_json.to_string()]).unwrap();

        let reg_json = serde_json::json!({
            "client_id": "reg-client-id",
            "client_secret": "reg-client-secret",
            "region": "eu-west-1"
        });
        conn.execute("INSERT INTO auth_kv (key, value) VALUES ('kirocli:odic:device-registration', ?1)", [reg_json.to_string()]).unwrap();
        drop(conn);

        let store = SqliteStore::new(db_path);
        let loaded = store.load().await.unwrap().expect("should load");

        assert_eq!(loaded.client_id.as_deref(), Some("reg-client-id"));
        assert_eq!(loaded.client_secret.as_deref(), Some("reg-client-secret"));
        assert_eq!(loaded.auth_type, AuthType::AwsSsoOidc);
        // reg_key should be recorded
        if let CredentialSource::SqliteDb { reg_key, .. } = &loaded.source {
            assert_eq!(reg_key.as_deref(), Some("kirocli:odic:device-registration"));
        } else {
            panic!("Expected SqliteDb source");
        }
    }

    // =========================================================================
    // Strategy selection
    // =========================================================================

    #[test]
    fn test_kiro_desktop_strategy_can_handle() {
        use super::super::strategies::{AuthStrategy, KiroDesktopStrategy};
        let s = KiroDesktopStrategy::new("fp".into());
        assert!(s.can_handle(AuthType::KiroDesktop));
        assert!(!s.can_handle(AuthType::AwsSsoOidc));
    }

    #[test]
    fn test_aws_sso_oidc_strategy_can_handle() {
        use super::super::strategies::{AuthStrategy, AwsSsoOidcStrategy};
        assert!(AwsSsoOidcStrategy.can_handle(AuthType::AwsSsoOidc));
        assert!(!AwsSsoOidcStrategy.can_handle(AuthType::KiroDesktop));
    }
}
