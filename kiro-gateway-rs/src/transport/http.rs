//! HTTP client with retry logic for the Kiro API.

use std::time::Duration;
use tracing::{debug, info, warn};

use crate::auth::KiroAuthManager;
use crate::config::{BASE_RETRY_DELAY, CONNECT_TIMEOUT, MAX_RETRIES, REQUEST_TIMEOUT};
use crate::error::{Error, Result};
use crate::transport::headers;

/// HTTP client for Kiro API with retry and refresh logic.
pub struct KiroHttpClient {
    client: reqwest::Client,
    auth: std::sync::Arc<KiroAuthManager>,
}

impl KiroHttpClient {
    /// Create a new HTTP client.
    pub fn new(auth: std::sync::Arc<KiroAuthManager>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("Failed to build HTTP client");

        Self { client, auth }
    }

    /// Create with a custom reqwest client.
    pub fn with_client(client: reqwest::Client, auth: std::sync::Arc<KiroAuthManager>) -> Self {
        Self { client, auth }
    }

    /// Send a POST request with automatic retry and token refresh.
    ///
    /// Retry strategy:
    /// - 403 Forbidden: Force token refresh, then retry
    /// - 429 Too Many Requests: Exponential backoff
    /// - 5xx Server Error: Exponential backoff
    pub async fn post_with_retry(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response> {
        let mut last_error = None;

        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                let delay = BASE_RETRY_DELAY * 2u32.pow(attempt - 1);
                debug!(attempt, delay_ms = delay.as_millis(), "Retrying request");
                tokio::time::sleep(delay).await;
            }

            let token = self.auth.get_access_token().await?;
            let fingerprint = self.auth.fingerprint();
            let hdrs = headers::kiro_api_headers(&token, fingerprint);

            match self
                .client
                .post(url)
                .headers(hdrs)
                .json(body)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status().as_u16();

                    if response.status().is_success() {
                        return Ok(response);
                    }

                    match status {
                        403 => {
                            warn!("Got 403 - refreshing token and retrying");
                            if let Err(e) = self.auth.force_refresh().await {
                                warn!("Token refresh failed: {}", e);
                            }
                            last_error = Some(Error::Api {
                                status,
                                message: "Forbidden - token may be expired".into(),
                            });
                        }
                        429 => {
                            let retry_after = response
                                .headers()
                                .get("retry-after")
                                .and_then(|v| v.to_str().ok())
                                .and_then(|v| v.parse::<u64>().ok())
                                .map(Duration::from_secs);

                            if let Some(delay) = retry_after {
                                info!(delay_secs = delay.as_secs(), "Rate limited, waiting");
                                tokio::time::sleep(delay).await;
                            }

                            last_error = Some(Error::RateLimited { retry_after });
                        }
                        500..=599 => {
                            let body_text = response.text().await.unwrap_or_default();
                            warn!(status, body = body_text.as_str(), "Server error, retrying");
                            last_error = Some(Error::Api {
                                status,
                                message: body_text,
                            });
                        }
                        _ => {
                            let body_text = response.text().await.unwrap_or_default();
                            return Err(Error::Api {
                                status,
                                message: body_text,
                            });
                        }
                    }
                }
                Err(e) => {
                    if e.is_timeout() {
                        warn!("Request timed out (attempt {})", attempt + 1);
                        last_error = Some(Error::Timeout);
                    } else {
                        warn!("Request failed (attempt {}): {}", attempt + 1, e);
                        last_error = Some(Error::Network(e));
                    }
                }
            }
        }

        Err(Error::RetriesExhausted {
            attempts: MAX_RETRIES,
            message: last_error
                .map(|e| e.to_string())
                .unwrap_or_else(|| "Unknown error".into()),
        })
    }

    /// Send a streaming POST request. Returns the response for stream processing.
    ///
    /// Uses Connection: close to prevent CLOSE_WAIT socket leak.
    /// Only retries on 403 (token refresh). Other errors are not retried for streaming.
    pub async fn post_streaming(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response> {
        for attempt in 0..2 {
            let token = self.auth.get_access_token().await?;
            let fingerprint = self.auth.fingerprint();
            let hdrs = headers::kiro_streaming_headers(&token, fingerprint);

            // Build a client without default timeout for streaming
            let stream_client = reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .build()
                .map_err(Error::Network)?;

            let response = stream_client
                .post(url)
                .headers(hdrs)
                .json(body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        Error::Timeout
                    } else {
                        Error::Network(e)
                    }
                })?;

            let status = response.status().as_u16();

            if response.status().is_success() {
                return Ok(response);
            }

            if status == 403 && attempt == 0 {
                warn!("Got 403 on stream - refreshing token and retrying");
                self.auth.force_refresh().await?;
                continue;
            }

            let body_text = response.text().await.unwrap_or_default();
            return Err(Error::Api {
                status,
                message: body_text,
            });
        }

        Err(Error::RetriesExhausted {
            attempts: 2,
            message: "Streaming request failed".into(),
        })
    }

    /// Send a GET request (for ListAvailableModels, etc.).
    pub async fn get(&self, url: &str) -> Result<reqwest::Response> {
        let token = self.auth.get_access_token().await?;
        let fingerprint = self.auth.fingerprint();
        let hdrs = headers::kiro_api_headers(&token, fingerprint);

        let response = self
            .client
            .get(url)
            .headers(hdrs)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    Error::Timeout
                } else {
                    Error::Network(e)
                }
            })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body_text = response.text().await.unwrap_or_default();
            return Err(Error::Api {
                status,
                message: body_text,
            });
        }

        Ok(response)
    }
}

impl std::fmt::Debug for KiroHttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KiroHttpClient")
            .field("auth", &self.auth)
            .finish()
    }
}
