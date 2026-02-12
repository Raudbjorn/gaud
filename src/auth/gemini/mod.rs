//! Authentication module for Google OAuth and token management.
//!
//! This module provides:
//!
//! - [`TokenInfo`] - Token storage with composite project ID format
//! - [`OAuthFlow`] - OAuth flow orchestrator for managing authentication
//! - [`OAuthFlowState`] - State for in-progress OAuth flows (PKCE)
//! - [`ProjectInfo`] - Project discovery results
//! - [`SubscriptionTier`] - Cloud Code subscription tier detection
//!
//! # OAuth Flow
//!
//! The authentication flow follows Google OAuth 2.0 with PKCE:
//!
//! 1. Generate PKCE verifier/challenge and state
//! 2. Build authorization URL and redirect user
//! 3. Exchange authorization code for tokens
//! 4. Store tokens for future requests
//! 5. Automatically refresh expired tokens
//!
//! # Example
//!
//! ```rust,ignore
//! use gaud::gemini::OAuthFlow;
//! use gaud::gemini::storage::MemoryTokenStorage;
//!
//! # async fn example() -> gaud::gemini::Result<()> {
//! let storage = MemoryTokenStorage::new();
//! let flow = OAuthFlow::new(storage);
//!
//! // Start OAuth flow
//! let (url, state) = flow.start_authorization_async().await?;
//! println!("Open: {}", url);
//!
//! // After user authorizes...
//! // let token = flow.exchange_code(&code, Some(&state.state)).await?;
//!
//! // Get access token (auto-refreshes if needed)
//! let token = flow.get_access_token().await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Project Discovery
//!
//! After authentication, discover the Cloud Code project:
//!
//! ```rust,ignore
//! use gaud::gemini::{discover_project, ProjectInfo};
//!
//! # async fn example() -> gaud::gemini::Result<()> {
//! let access_token = "ya29.xxx";
//! let project = discover_project(access_token, None).await?;
//! println!("Project: {}", project.project_id);
//! println!("Tier: {:?}", project.subscription_tier);
//! # Ok(())
//! # }
//! ```

mod flow;
pub mod oauth;
pub mod project;
mod token;

// Re-export main types at the auth level
pub use flow::OAuthFlow;
pub use oauth::{
    build_authorization_url, exchange_code, generate_pkce, generate_state, refresh_token,
    OAuthFlowState,
};
pub use project::{discover_project, onboard_user, ProjectInfo, SubscriptionTier};
pub use token::TokenInfo;
