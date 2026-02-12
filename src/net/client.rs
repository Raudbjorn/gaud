//! Generic HTTP client.

use std::time::Duration;
use reqwest::{Client, ClientBuilder};

/// Default user agent for the application.
pub const USER_AGENT: &str = "gaud/0.1.0";

/// Default connection timeout.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default request timeout.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Generic HTTP client wrapper.
///
/// Provides a standard configuration (User-Agent, timeouts) for all providers.
#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: Client,
}

impl HttpClient {
    /// Create a new HTTP client with default settings.
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Create a new builder.
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::default()
    }

    /// Get the inner reqwest client.
    pub fn inner(&self) -> &Client {
        &self.inner
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for [`HttpClient`].
pub struct HttpClientBuilder {
    builder: ClientBuilder,
}

impl Default for HttpClientBuilder {
    fn default() -> Self {
        Self {
            builder: Client::builder()
                .user_agent(USER_AGENT)
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT),
        }
    }
}

impl HttpClientBuilder {
    /// Set a custom user agent.
    pub fn user_agent(mut self, ua: &str) -> Self {
        self.builder = self.builder.user_agent(ua);
        self
    }

    /// Set connection timeout.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.builder = self.builder.connect_timeout(timeout);
        self
    }

    /// Set request timeout.
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.builder = self.builder.timeout(timeout);
        self
    }

    /// Build the client.
    pub fn build(self) -> HttpClient {
        let inner = match self.builder.build() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to build HTTP client with custom config: {}; using defaults", e);
                Client::default()
            }
        };
        HttpClient { inner }
    }
}
