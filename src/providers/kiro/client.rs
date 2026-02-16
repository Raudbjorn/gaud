use futures::Stream;
use futures::stream::StreamExt;
use serde_json::Value;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;
use uuid::Uuid;

use super::auth::KiroTokenProvider;
use crate::providers::ProviderError;

/// URL template for the Kiro API host.
const KIRO_API_HOST_TEMPLATE: &str = "https://q.{region}.amazonaws.com";

/// Origin query parameter sent with every Kiro API request.
const API_ORIGIN: &str = "AI_EDITOR";

/// AWS SDK for JS version emulated in the User-Agent.
const SDK_VERSION: &str = "1.0.27";

/// Node.js version emulated in the User-Agent.
const NODE_VERSION: &str = "22.21.1";

/// Kiro IDE version emulated in the User-Agent.
const IDE_VERSION: &str = "0.7.45";

pub struct KiroClient {
    http: reqwest::Client,
    auth: Arc<dyn KiroTokenProvider>,
    region: String,
    profile_arn: Option<String>,
    fingerprint: String,
}

impl KiroClient {
    pub fn new(
        auth: Arc<dyn KiroTokenProvider>,
        region: String,
        profile_arn: Option<String>,
        fingerprint: String,
    ) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();

        Self {
            http,
            auth,
            region,
            profile_arn,
            fingerprint,
        }
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn profile_arn(&self) -> Option<&str> {
        self.profile_arn.as_deref()
    }

    pub async fn access_token(&self) -> Result<String, ProviderError> {
        self.auth.get_token().await
    }

    pub fn generate_assistant_response_url(&self) -> String {
        let host = KIRO_API_HOST_TEMPLATE.replace("{region}", &self.region);
        match &self.profile_arn {
            Some(arn) => format!(
                "{host}/generateAssistantResponse?origin={API_ORIGIN}&profileArn={}",
                urlencoding::encode(arn)
            ),
            None => format!("{host}/generateAssistantResponse?origin={API_ORIGIN}"),
        }
    }

    fn headers(&self, token: &str) -> reqwest::header::HeaderMap {
        use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let ua = format!(
            "aws-sdk-js/{SDK_VERSION} ua/2.1 os/linux lang/js md/nodejs#{NODE_VERSION} \
             api/codewhispererstreaming#{SDK_VERSION} m/E KiroIDE-{IDE_VERSION}-{f}",
            f = self.fingerprint
        );
        headers.insert("user-agent", HeaderValue::from_str(&ua).unwrap());
        headers.insert(
            "x-amz-user-agent",
            HeaderValue::from_str(&format!(
                "aws-sdk-js/{SDK_VERSION} KiroIDE-{IDE_VERSION}-{f}",
                f = self.fingerprint
            ))
            .unwrap(),
        );
        headers.insert(
            "x-amzn-codewhisperer-optout",
            HeaderValue::from_static("true"),
        );
        headers.insert("x-amzn-kiro-agent-mode", HeaderValue::from_static("vibe"));
        headers.insert(
            HeaderName::from_static("amz-sdk-invocation-id"),
            HeaderValue::from_str(&Uuid::new_v4().to_string()).unwrap(),
        );
        headers.insert(
            HeaderName::from_static("amz-sdk-request"),
            HeaderValue::from_static("attempt=1; max=3"),
        );

        headers
    }

    pub async fn send_request(&self, body: &Value) -> Result<String, ProviderError> {
        let mut retry = true;
        loop {
            let token = self.auth.get_token().await?;
            let url = self.generate_assistant_response_url();
            let headers = self.headers(&token);

            let resp = self
                .http
                .post(&url)
                .headers(headers)
                .json(body)
                .send()
                .await
                .map_err(ProviderError::Http)?;

            let status = resp.status();

            if is_auth_error(status) && retry {
                warn!(
                    "Kiro API returned {}, attempting force refresh and retry",
                    status
                );
                self.auth.force_refresh().await?;
                retry = false;
                continue;
            }

            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(parse_rate_limit(resp.headers()));
            }

            if !status.is_success() {
                let body_text = resp.text().await.unwrap_or_default();
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    message: body_text,
                });
            }

            return resp.text().await.map_err(|e| {
                ProviderError::ResponseParsing(format!("Failed to read Kiro response body: {e}"))
            });
        }
    }

    pub async fn send_request_stream(
        &self,
        body: &Value,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, ProviderError>> + Send>>, ProviderError>
    {
        // Streaming retry is harder because we can't easily retry the whole stream creation
        // if it fails later, but we can retry the *initiation*.
        let mut retry = true;
        loop {
            let token = self.auth.get_token().await?;
            let url = self.generate_assistant_response_url();
            let mut headers = self.headers(&token);
            headers.insert("connection", "close".parse().unwrap());

            let resp = self
                .http
                .post(&url)
                .headers(headers)
                .json(body)
                .send()
                .await
                .map_err(ProviderError::Http)?;

            let status = resp.status();

            if is_auth_error(status) && retry {
                warn!(
                    "Kiro API (stream) returned {}, attempting force refresh and retry",
                    status
                );
                self.auth.force_refresh().await?;
                retry = false;
                continue;
            }

            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(parse_rate_limit(resp.headers()));
            }

            if !status.is_success() {
                let body_text = resp.text().await.unwrap_or_default();
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    message: body_text,
                });
            }

            let byte_stream = resp.bytes_stream();
            let line_stream = sse_line_stream(byte_stream);

            return Ok(Box::pin(line_stream));
        }
    }

    pub async fn health_check(&self) -> bool {
        self.auth.get_token().await.is_ok()
    }
}

/// Check if an HTTP status code indicates an auth error (401/403).
fn is_auth_error(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN
}

/// Parse a `ProviderError::RateLimited` from a 429 response's headers.
fn parse_rate_limit(headers: &reqwest::header::HeaderMap) -> ProviderError {
    let retry_after = headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs);
    ProviderError::RateLimited {
        retry_after_secs: retry_after.map(|d| d.as_secs()).unwrap_or(60),
        retry_after,
    }
}

/// Convert a raw byte stream into a stream of parsed SSE `data:` payloads.
///
/// Uses a line-buffered accumulator so that SSE lines split across TCP chunks
/// are correctly reassembled before parsing. Only complete lines prefixed with
/// `data: ` are yielded; `[DONE]` sentinels and non-data lines are filtered out.
fn sse_line_stream<S>(byte_stream: S) -> impl Stream<Item = Result<String, ProviderError>>
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    futures::stream::unfold(
        (byte_stream.boxed(), String::new()),
        |(mut stream, mut buffer)| async move {
            loop {
                // Drain complete lines from the buffer first.
                if let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim().to_string();
                    buffer.drain(..=newline_pos);

                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            continue;
                        }
                        return Some((Ok(data.to_string()), (stream, buffer)));
                    }
                    // Non-data line (empty keep-alive, event:, id:, etc.) — skip.
                    continue;
                }

                // Buffer has no complete line — read the next chunk.
                match stream.next().await {
                    Some(Ok(bytes)) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                    }
                    Some(Err(e)) => {
                        return Some((
                            Err(ProviderError::Stream(format!("Stream read error: {e}"))),
                            (stream, buffer),
                        ));
                    }
                    None => {
                        // Stream ended. Flush any trailing partial line.
                        let remaining = buffer.trim().to_string();
                        buffer.clear();
                        if let Some(data) = remaining.strip_prefix("data: ") {
                            if data != "[DONE]" && !data.is_empty() {
                                return Some((Ok(data.to_string()), (stream, buffer)));
                            }
                        }
                        return None;
                    }
                }
            }
        },
    )
}

/// Generate a machine fingerprint for User-Agent (SHA-256 of hostname-user).
pub fn machine_fingerprint() -> String {
    use sha2::{Digest, Sha256};
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let username = whoami::username_os()
        .map(|u| u.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let input = format!("{hostname}-{username}-kiro-gateway");
    let hash = Sha256::digest(input.as_bytes());
    hash.iter().map(|b| format!("{b:02x}")).collect::<String>()
}
