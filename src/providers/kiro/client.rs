use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use futures::Stream;
use futures::stream::StreamExt;
use serde_json::Value;
use uuid::Uuid;
use tracing::warn;

use crate::providers::ProviderError;
use super::auth::KiroTokenProvider;

/// URL template for the Kiro API host.
const KIRO_API_HOST_TEMPLATE: &str = "https://q.{region}.amazonaws.com";

/// Origin query parameter sent with every Kiro API request.
const API_ORIGIN: &str = "AI_EDITOR";

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
            "aws-sdk-js/1.0.27 ua/2.1 os/linux lang/js md/nodejs#22.21.1 \
             api/codewhispererstreaming#1.0.27 m/E KiroIDE-0.7.45-{f}",
            f = self.fingerprint
        );
        headers.insert("user-agent", HeaderValue::from_str(&ua).unwrap());
        headers.insert(
            "x-amz-user-agent",
            HeaderValue::from_str(&format!("aws-sdk-js/1.0.27 KiroIDE-0.7.45-{f}", f = self.fingerprint)).unwrap(),
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
            
            if (status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN) && retry {
                warn!("Kiro API returned {}, attempting force refresh and retry", status);
                self.auth.force_refresh().await?;
                retry = false;
                continue;
            }

            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(Duration::from_secs);
                return Err(ProviderError::RateLimited {
                    retry_after_secs: retry_after.map(|d| d.as_secs()).unwrap_or(60),
                    retry_after,
                });
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

            if (status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN) && retry {
                warn!("Kiro API (stream) returned {}, attempting force refresh and retry", status);
                self.auth.force_refresh().await?;
                retry = false;
                continue;
            }

            if !status.is_success() {
                let body_text = resp.text().await.unwrap_or_default();
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    message: body_text,
                });
            }

            let byte_stream = resp.bytes_stream();
            let line_stream = byte_stream
                .map(|result| {
                    result.map_err(|e| ProviderError::Stream(format!("Stream read error: {e}")))
                })
                .flat_map(|result| {
                    let lines: Vec<Result<String, ProviderError>> = match result {
                        Ok(bytes) => {
                            let text = String::from_utf8_lossy(&bytes);
                            text.lines()
                                .filter_map(|line| {
                                    let line = line.trim();
                                    if let Some(data) = line.strip_prefix("data: ") {
                                        if data == "[DONE]" {
                                            None
                                        } else {
                                            Some(Ok(data.to_string()))
                                        }
                                    } else {
                                        None
                                    }
                                })
                                .collect()
                        }
                        Err(e) => vec![Err(e)],
                    };
                    futures::stream::iter(lines)
                });

            return Ok(Box::pin(line_stream));
        }
    }

    pub async fn health_check(&self) -> bool {
        self.auth.get_token().await.is_ok()
    }
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
