//! Authentication for the Kiro API.
//!
//! Handles token lifecycle: credential loading, refresh, caching.

pub mod aws_sso_oidc;
pub mod constants;
pub mod credentials;
pub mod kiro_desktop;
pub mod manager;

pub use manager::KiroAuthManager;
