//! Callback-based token storage for user-provided persistence.

use async_trait::async_trait;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::TokenStorage;
use crate::error::Result;
use crate::models::auth::KiroTokenInfo;

type LoadFn =
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<Option<KiroTokenInfo>>> + Send>> + Send + Sync;
type SaveFn =
    dyn Fn(String, KiroTokenInfo) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync;
type RemoveFn =
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync;

/// Storage backed by user-provided async callbacks.
pub struct CallbackStorage {
    load_fn: Arc<LoadFn>,
    save_fn: Arc<SaveFn>,
    remove_fn: Arc<RemoveFn>,
}

impl CallbackStorage {
    /// Create from async closures.
    pub fn new<L, S, R>(load: L, save: S, remove: R) -> Self
    where
        L: Fn(String) -> Pin<Box<dyn Future<Output = Result<Option<KiroTokenInfo>>> + Send>>
            + Send
            + Sync
            + 'static,
        S: Fn(String, KiroTokenInfo) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>
            + Send
            + Sync
            + 'static,
        R: Fn(String) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            load_fn: Arc::new(load),
            save_fn: Arc::new(save),
            remove_fn: Arc::new(remove),
        }
    }
}

#[async_trait]
impl TokenStorage for CallbackStorage {
    async fn load(&self, provider: &str) -> Result<Option<KiroTokenInfo>> {
        (self.load_fn)(provider.to_string()).await
    }

    async fn save(&self, provider: &str, token: &KiroTokenInfo) -> Result<()> {
        (self.save_fn)(provider.to_string(), token.clone()).await
    }

    async fn remove(&self, provider: &str) -> Result<()> {
        (self.remove_fn)(provider.to_string()).await
    }

    fn name(&self) -> &str {
        "callback"
    }
}
