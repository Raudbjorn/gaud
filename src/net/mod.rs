//! Shared networking infrastructure.

pub mod client;
pub mod sse;

pub mod traits;

pub use client::{HttpClient, HttpClientBuilder};
pub use traits::HttpClientFactory;
pub use sse::SseStream;
