//! HTTP transport layer for Cloud Code API.
//!
//! This module provides the HTTP client wrapper and SSE stream parsing
//! for communicating with the Cloud Code API.
//!
//! ## Module Structure
//!
//! - [`http`]: HTTP client wrapper with request building and fallback
//! - [`sse`]: Server-Sent Events stream parser
//!
//! ## Example
//!
//! ```rust,ignore
//! use gaud::providers::gemini::transport::{HttpClient, SseStream};
//!
//! // Create HTTP client
//! let client = HttpClient::new();
//!
//! // Make a streaming request
//! let response = client.post_stream(&url, &headers, &body).await?;
//!
//! // Parse SSE events
//! let mut stream = SseStream::new(response);
//! while let Some(event) = stream.next().await {
//!     println!("Event: {:?}", event?);
//! }
//! ```

pub mod http;
pub mod sse;

pub use http::{HttpClient, HttpClientBuilder};
pub use sse::SseStream;
