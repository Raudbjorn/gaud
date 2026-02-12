//! Networking traits.

use crate::net::HttpClient;

/// Trait for creating HTTP clients.
///
/// This allows abstracting client creation logic (e.g., adding default headers,
/// configuring timeouts) from consumers.
pub trait HttpClientFactory: Send + Sync {
    /// Create a new HTTP client.
    fn create_client(&self) -> HttpClient;
}
