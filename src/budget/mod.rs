pub mod audit;
pub mod middleware;
pub mod tracker;

use serde::{Deserialize, Serialize};

pub use self::audit::spawn_audit_logger;
pub use self::middleware::budget_middleware;
pub use self::tracker::BudgetTracker;

/// A single usage event to be recorded asynchronously by the audit logger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub user_id: String,
    pub request_id: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost: f64,
    pub latency_ms: u64,
    pub status: String,
}
