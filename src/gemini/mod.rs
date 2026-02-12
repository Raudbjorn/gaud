//! # gemini
//!
//! OAuth-based Cloud Code API client for Claude and Gemini models.
//!
//! This library provides programmatic access to Google's Cloud Code API,
//! enabling use of Claude and Gemini models with Google OAuth credentials.
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use gaud::gemini::{CloudCodeClient, FileTokenStorage};
//!
//! # async fn example() -> gaud::gemini::Result<()> {
//! // Create client with file-based token storage
//! let storage = FileTokenStorage::default_path()?;
//! let client = CloudCodeClient::builder()
//!     .with_storage(storage)
//!     .build()?;
//!
//! // Check if authenticated
//! if !client.is_authenticated().await? {
//!     // Start OAuth flow
//!     let auth_url = client.start_oauth_flow().await?;
//!     println!("Open this URL to authenticate: {}", auth_url);
//!     // ... user completes OAuth, provides code ...
//!     // client.complete_oauth_flow(code, state).await?;
//! }
//!
//! // Make a request
//! let response = client.messages()
//!     .model("claude-sonnet-4-5-thinking")
//!     .max_tokens(1024)
//!     .user_message("Hello, Claude!")
//!     .send()
//!     .await?;
//!
//! println!("{}", response.text());
//! # Ok(())
//! # }
//! ```
//!
//! ## Features
//!
//! - **Google OAuth 2.0**: Authenticate with your Google account
//! - **Dual Model Support**: Access both Claude and Gemini models
//! - **Thinking Models**: Full support for extended reasoning
//! - **Streaming**: Async streaming responses
//! - **Tool Use**: Function calling with automatic format conversion
//! - **Flexible Storage**: File, keyring, or custom token storage
//!
//! ## Feature Flags
//!
//! - `keyring`: Enable system keyring storage (macOS Keychain, Linux Secret Service)
//! - `cli`: Build the `ag` command-line tool
//! - `full`: Enable all features

#![warn(missing_docs)]
#![warn(rustdoc::missing_crate_level_docs)]
// Implemented modules
pub mod client;
pub mod convert;
pub mod storage;
pub mod transport;

pub mod constants;
pub mod error;
pub mod models;


pub use models::{
    ContentBlock, ContentDelta, DocumentSource, ImageSource, Message, MessageContent, MessageDelta,
    MessagesRequest, MessagesRequestBuilder, MessagesResponse, PartialMessage, Role, StopReason,
    StreamError, StreamEvent, SystemPrompt, ThinkingConfig, Tool, ToolChoice, ToolResultContent,
    Usage,
};

// Re-export auth types from crate::auth::gemini
pub use crate::auth::gemini::{
    build_authorization_url, discover_project, exchange_code, generate_pkce, generate_state,
    onboard_user, refresh_token, OAuthFlow, OAuthFlowState, ProjectInfo, SubscriptionTier,
    TokenInfo,
};

// Re-export storage types at crate root
pub use storage::{
    CallbackStorage, EnvSource, FileSource, FileTokenStorage, MemoryTokenStorage, TokenStorage,
};

#[cfg(feature = "system-keyring")]
pub use storage::KeyringTokenStorage;

// Re-export public conversion utilities at crate root
pub use convert::{convert_role, sanitize_schema, SignatureCache, GLOBAL_SIGNATURE_CACHE};

// Re-export client types at crate root
pub use client::{
    CloudCodeClient, CloudCodeClientBuilder, MessagesRequestBuilder as ClientMessagesRequestBuilder,
};

// Re-export transport types (for advanced use cases)
pub use transport::{HttpClient, HttpClientBuilder, SseStream};
