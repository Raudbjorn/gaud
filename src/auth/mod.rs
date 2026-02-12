pub mod gemini;

pub mod keys;
pub mod middleware;
pub mod users;

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
