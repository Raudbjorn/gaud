//! # kiro-gateway
//!
//! Rust client library for the Kiro API (Amazon Q / AWS CodeWhisperer).
//!
//! Provides both an Anthropic Messages API surface and raw Kiro API access.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use kiro_gateway::{KiroClient, KiroClientBuilder, Result};
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     // Build client from credentials file
//!     let client = KiroClientBuilder::new()
//!         .credentials_file("~/.kiro/credentials.json")
//!         .build()
//!         .await?;
//!
//!     // Send a message using the Anthropic Messages API
//!     let response = client.messages()
//!         .model("claude-sonnet-4.5")
//!         .max_tokens(1024)
//!         .user_message("Hello, Claude!")
//!         .send()
//!         .await?;
//!
//!     println!("{}", response.text());
//!     Ok(())
//! }
//! ```
//!
//! ## Features
//!
//! - `sqlite` - Enable loading credentials from kiro-cli SQLite database
//! - `keyring` - Enable system keyring token storage
//! - `full` - Enable all optional features

pub mod api;
pub mod auth;
pub mod client;
pub mod config;
pub mod convert;
pub mod error;
pub mod models;
pub mod storage;
pub mod transport;

// Re-exports for ergonomic usage
pub use client::{KiroClient, KiroClientBuilder};
pub use error::{Error, Result};
pub use models::request::{
    ContentBlock, Message, MessageContent, MessagesRequest, Role, SystemPrompt, ThinkingConfig,
    Tool, ToolChoice,
};
pub use models::response::{MessagesResponse, ResponseContentBlock, StopReason, Usage};
pub use models::stream::{ContentDelta, MessageDelta, StreamEvent};
pub use storage::TokenStorage;
