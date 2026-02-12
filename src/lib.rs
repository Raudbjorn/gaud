// Enforce mutual exclusivity of cache features at compile time.
#[cfg(all(feature = "cache-persistent", feature = "cache-ephemeral"))]
compile_error!(
    "Features `cache-persistent` and `cache-ephemeral` are mutually exclusive. \
     Please enable only one."
);

pub mod api;
pub mod auth;
pub mod budget;
pub mod cache;
pub mod config;
pub mod db;
pub mod error;
pub mod oauth;
pub mod providers;
pub mod gemini;
pub mod web;

use crate::budget::BudgetTracker;
use crate::cache::SemanticCacheService;
use crate::config::Config;
use crate::db::Database;
use crate::oauth::OAuthManager;
use crate::providers::cost::CostCalculator;
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
    pub cost_calculator: Arc<CostCalculator>,
    pub cache: Option<Arc<SemanticCacheService>>,
    pub oauth_manager: Arc<OAuthManager>,
}
