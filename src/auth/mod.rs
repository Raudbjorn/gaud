pub mod keys;
pub mod middleware;
pub mod users;

pub mod error;
pub mod oauth;
pub mod store;
pub mod tokens;
pub mod traits;

pub use traits::{AuthProvider, TokenProvider};

use serde::Serialize;

/// Authenticated user identity attached to request extensions by auth middleware.
#[derive(Debug, Clone, Serialize)]
pub struct AuthUser {
    pub user_id: String,
    pub name: String,
    pub role: String,
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}
