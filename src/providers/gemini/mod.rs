//! # gemini
//!
//! OAuth-based Cloud Code API client for Claude and Gemini models.
//!
//! This library provides programmatic access to Google's Cloud Code API,
//! enabling use of Claude and Gemini models with Google OAuth credentials.
//!
//! ## Quick Start
//!
//! use std::sync::Arc;
//! use gaud::providers::gemini::CloudCodeClient;
//! use gaud::oauth::TokenProvider;
//!
//! # async fn example(token_provider: Arc<dyn TokenProvider>) -> gaud::providers::gemini::Result<()> {
//! // Create client with a token provider
//! let client = CloudCodeClient::new(token_provider);
//!
//! // Check if authenticated
//! if client.is_authenticated().await? {
//!     println!("Authenticated!");
//! }
//!
//! // Make a request
//! // client.generate_content(...);
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
// Implemented modules
pub mod client;
pub mod discovery;
pub mod provider;
pub mod thinking;
pub mod transport;

pub mod constants;
pub mod error;
pub mod models;

pub use models::{
    ContentBlock, ContentDelta, DocumentSource, ImageSource, Message, MessageContent, MessageDelta,
    MessagesRequest, MessagesResponse, PartialMessage, Role, StopReason, StreamError, StreamEvent,
    SystemPrompt, ThinkingConfig, Tool, ToolChoice, ToolResultContent, Usage,
};

// Re-export discovery types
pub use discovery::{ProjectInfo, SubscriptionTier, discover_project, onboard_user};

// Re-export thinking types at crate root
pub use thinking::{GLOBAL_SIGNATURE_CACHE, SignatureCache};

// Re-export client types at crate root
pub use client::{CloudCodeClient, CloudCodeClientBuilder};

// Re-export transport types (for advanced use cases)
pub use transport::{HttpClient, HttpClientBuilder, SseStream};

// Re-export provider
pub use provider::GeminiProvider;
