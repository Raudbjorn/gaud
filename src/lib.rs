pub mod api;
pub mod auth;
pub mod budget;
pub mod config;
pub mod db;
pub mod error;
pub mod oauth;
pub mod providers;
pub mod web;

use crate::budget::BudgetTracker;
use crate::config::Config;
use crate::db::Database;
use crate::oauth::OAuthManager;
use crate::providers::router::ProviderRouter;

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Shared application state accessible from all handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub config_path: PathBuf,
    pub db: Database,
    pub router: Arc<RwLock<ProviderRouter>>,
    pub budget: Arc<BudgetTracker>,
    pub audit_tx: tokio::sync::mpsc::UnboundedSender<budget::AuditEntry>,
    pub oauth_manager: Arc<OAuthManager>,
}
